//! Twelve people in one call (Gate C), through the media node as the
//! one-machine install runs it (every role in one process): twelve WebRTC
//! peers (str0m, with their own UDP sockets) join a room's call, each
//! sends audio and video, answers the node's offers over the data channel,
//! and receives the eleven others' audio and video. No decoding: the peers
//! send synthetic Opus and VP8 frames, which the node forwards as it would
//! a browser's.

mod common;

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

const PEOPLE: usize = 12;

struct Peer {
    rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    audio: Mid,
    video: Mid,
    channel: Option<ChannelId>,
    parts: Option<(String, Vec<String>)>,
    received: HashMap<Mid, usize>,
    audio_ticks: u64,
    video_ticks: u64,
    next_audio: Instant,
    next_video: Instant,
}

fn write_parts(rtc: &mut Rtc, channel: ChannelId, text: &str) {
    let id = format!(
        "{:x}",
        Instant::now().elapsed().as_nanos() ^ text.len() as u128
    );
    let chars: Vec<char> = text.chars().collect();
    let chunks: Vec<String> = chars
        .chunks(16 * 1024)
        .map(|c| c.iter().collect())
        .collect();
    for (i, data) in chunks.iter().enumerate() {
        if let Some(mut ch) = rtc.channel(channel) {
            let _ = ch.write(
                false,
                json!({"id": id, "part": i, "of": chunks.len(), "data": data})
                    .to_string()
                    .as_bytes(),
            );
        }
    }
}

impl Peer {
    fn pt(&mut self, mid: Mid, codec: Codec) -> Option<str0m::media::Pt> {
        self.rtc.writer(mid).and_then(|w| {
            w.payload_params()
                .find(|p| p.spec().codec == codec)
                .map(|p| p.pt())
        })
    }

    /// Sends what is due: 20 ms of audio, a video frame every 66 ms.
    fn send(&mut self, now: Instant) {
        if now >= self.next_audio {
            if let Some(pt) = self.pt(self.audio, Codec::Opus) {
                if let Some(w) = self.rtc.writer(self.audio) {
                    let _ = w.write(
                        pt,
                        now,
                        MediaTime::new(self.audio_ticks, Frequency::FORTY_EIGHT_KHZ),
                        vec![0x78u8; 80],
                    );
                }
            }
            self.audio_ticks += 960;
            self.next_audio += Duration::from_millis(20);
        }
        if now >= self.next_video {
            if let Some(pt) = self.pt(self.video, Codec::Vp8) {
                if let Some(w) = self.rtc.writer(self.video) {
                    let mut frame = vec![0u8; 1200];
                    frame[0] = 0x10;
                    let _ = w.write(
                        pt,
                        now,
                        MediaTime::new(self.video_ticks, Frequency::NINETY_KHZ),
                        frame,
                    );
                }
            }
            self.video_ticks += 6000;
            self.next_video += Duration::from_millis(66);
        }
    }

    fn drive(&mut self) {
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Transmit(t)) => {
                    let _ = self.socket.send_to(&t.contents, t.destination);
                }
                Ok(Output::Timeout(_)) | Err(_) => break,
                Ok(Output::Event(e)) => match e {
                    Event::ChannelOpen(id, _) => self.channel = Some(id),
                    Event::ChannelData(d) => self.part(&d.data),
                    Event::MediaData(d) => *self.received.entry(d.mid).or_default() += 1,
                    _ => {}
                },
            }
        }
        let mut buf = [0u8; 2000];
        while let Ok((n, source)) = self.socket.recv_from(&mut buf) {
            if let Ok(contents) = buf[..n].try_into() {
                let _ = self.rtc.handle_input(Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.local,
                        contents,
                    },
                ));
            }
        }
        let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
    }

    /// A part of the node's offer; the answer when the whole offer is in.
    fn part(&mut self, data: &[u8]) {
        let Ok(p) = serde_json::from_slice::<Value>(data) else {
            return;
        };
        let (Some(id), Some(i), Some(of), Some(text)) = (
            p["id"].as_str(),
            p["part"].as_u64(),
            p["of"].as_u64(),
            p["data"].as_str(),
        ) else {
            return;
        };
        if self
            .parts
            .as_ref()
            .is_none_or(|(have, got)| have != id || got.len() != of as usize)
        {
            self.parts = Some((id.to_string(), vec![String::new(); of as usize]));
        }
        let Some((_, got)) = self.parts.as_mut() else {
            return;
        };
        got[i as usize] = text.to_string();
        if got.iter().any(String::is_empty) {
            return;
        }
        let whole = got.concat();
        self.parts = None;
        let mut value: serde_json::Value = serde_json::from_str(&whole).unwrap_or_default();
        let n = value.as_object_mut().and_then(|o| o.remove("n"));
        if let Ok(offer) = serde_json::from_value::<SdpOffer>(value) {
            if let Ok(answer) = self.rtc.sdp_api().accept_offer(offer) {
                if let Some(channel) = self.channel {
                    // The offer's number goes back with the answer.
                    let mut answer = serde_json::to_value(&answer).unwrap();
                    answer["n"] = n.unwrap_or_default();
                    let text = answer.to_string();
                    write_parts(&mut self.rtc, channel, &text);
                }
            }
        }
    }
}

#[test]
fn twelve_people_in_one_call() {
    std::env::set_var("ORTUS_MEDIA_LISTEN", "127.0.0.1:0");
    let (name, instance, _keep) = common::installations().into_iter().next().unwrap();
    for app in instance.apps.clone() {
        app.provide(&instance, &instance.services);
    }
    let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
    let (stop_tx, stop) = tokio::sync::watch::channel(false);
    let node = ortus_core::runtime::io().spawn(ortus_calls::node::run(Arc::clone(&instance), stop));
    for _ in 0..50 {
        if ortus_core::runtime::block_on(ortus_calls::store::pick_node(instance.store.as_ref()))
            .unwrap()
            .is_some()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let room = "all-hands";
    let mut peers = Vec::new();
    for n in 0..PEOPLE {
        let email = format!("p{n}@ministry.example");
        let token = signed_in(&instance, &email, Role::Member);
        let id = ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            &email,
        ))
        .unwrap()
        .unwrap()
        .id;
        common::put_in_room(&id, room);
        let joined: Value = serde_json::from_slice(
            client
                .post(&format!("/api/v1/calls/rooms/{room}/join"))
                .header("Authorization", &format!("Bearer {token}"))
                .header("X-Requested-With", "ortus")
                .send()
                .body_bytes(),
        )
        .unwrap();
        let session = joined["session"].as_str().expect("joined").to_string();

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let local = socket.local_addr().unwrap();
        let mut rtc = Rtc::builder().build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        let mut change = rtc.sdp_api();
        let audio = change.add_media(
            MediaKind::Audio,
            Direction::SendOnly,
            Some(format!("p{n}")),
            None,
            None,
        );
        let video = change.add_media(
            MediaKind::Video,
            Direction::SendOnly,
            Some(format!("p{n}")),
            None,
            None,
        );
        change.add_channel("signalling".into());
        let (offer, pending) = change.apply().unwrap();
        let r = client
            .post(&format!("/api/v1/calls/sessions/{session}/offer"))
            .header("Authorization", &format!("Bearer {token}"))
            .header("X-Requested-With", "ortus")
            .header("Content-Type", "application/json")
            .body_bytes(json!({"offer": offer}).to_string().into_bytes())
            .send();
        let answered: Value = serde_json::from_slice(r.body_bytes()).unwrap();
        let answer: SdpAnswer =
            serde_json::from_value(answered["answer"].clone()).expect("an answer");
        rtc.sdp_api().accept_answer(pending, answer).unwrap();
        let now = Instant::now();
        peers.push(Peer {
            rtc,
            socket,
            local,
            audio,
            video,
            channel: None,
            parts: None,
            received: HashMap::new(),
            audio_ticks: 0,
            video_ticks: 0,
            next_audio: now,
            next_video: now,
        });
        // The earlier peers keep going while others join.
        for p in peers.iter_mut() {
            p.drive();
        }
    }

    // Everyone talks and shows video until every track flows: up to a
    // minute, as twelve peers in one thread lag on a busy machine (it ends
    // as soon as all of them receive everything).
    let until = Instant::now() + Duration::from_secs(60);
    while Instant::now() < until {
        let now = Instant::now();
        for p in peers.iter_mut() {
            p.send(now);
            p.drive();
        }
        let done = peers
            .iter()
            .all(|p| p.received.values().filter(|n| **n >= 20).count() >= 2 * (PEOPLE - 1));
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    for (n, p) in peers.iter().enumerate() {
        let flowing = p.received.values().filter(|c| **c >= 20).count();
        assert!(
            flowing >= 2 * (PEOPLE - 1),
            "{name}: person {n} receives {flowing} of {} tracks: {:?}",
            2 * (PEOPLE - 1),
            p.received.values().collect::<Vec<_>>()
        );
    }
    stop_tx.send(true).unwrap();
    let _ = ortus_core::runtime::block_on(node);
}
