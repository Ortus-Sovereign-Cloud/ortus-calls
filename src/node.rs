//! The media node (the "media" role): WebRTC with str0m on one UDP port,
//! forwarding each person's audio, video and screen to the others in the
//! same call (a selective forwarding unit). Media is encrypted in transit
//! (DTLS-SRTP) and never decoded here.
//!
//! Settings: `ORTUS_MEDIA_LISTEN` (UDP, default 0.0.0.0:40000),
//! The same port on TCP takes ICE over TCP (RFC 4571 framing) for
//! networks that block UDP: the relay path, with no separate TURN server.
//! `ORTUS_MEDIA_PUBLIC` (the address browsers reach, ip:port; default the
//! listening address, or this machine's address when listening on all),
//! `ORTUS_MEDIA_SIGNAL_LISTEN` (where web nodes hand over offers, default
//! 127.0.0.1:0) and `ORTUS_MEDIA_SIGNAL_URL` (how they reach it, default
//! http:// the signal address). Web nodes find nodes in `calls_nodes`; a
//! random token, kept sealed there, signs their requests.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::sync::Mutex;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use ortus_core::instance::Instance;
use serde_json::{json, Value};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::{ChannelData, ChannelId};
use str0m::media::{Direction, KeyframeRequest, KeyframeRequestKind, MediaData, MediaKind, Mid};
use str0m::net::{Protocol, Receive, TcpType};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::store;

/// What the signalling side asks of the media loop.
pub enum Command {
    Join {
        call: String,
        session: String,
        rtc: Box<Rtc>,
    },
    Kick {
        session: String,
    },
}

/// The role task: binds the media port, registers the node, serves
/// signalling until told to stop.
pub fn run(
    instance: Arc<Instance>,
    mut stop: ortus_core::app::Stop,
) -> ortus_core::storage::BoxFuture<'static, Result<(), String>> {
    Box::pin(async move {
        let listen = std::env::var("ORTUS_MEDIA_LISTEN").unwrap_or_else(|_| "0.0.0.0:40000".into());
        let socket = UdpSocket::bind(&listen).map_err(|e| format!("media on {listen}: {e}"))?;
        let local = socket.local_addr().map_err(|e| e.to_string())?;
        let public = public_address(local)?;
        // ICE over TCP on the same port (when it is free).
        let tcp = TcpListener::bind(local)
            .map_err(|e| eprintln!("ortus: calls: no ICE over TCP on {local}: {e}"))
            .ok();
        let only_tcp = std::env::var("ORTUS_MEDIA_ONLY_TCP").is_ok_and(|v| v == "1");
        let candidates = Candidates {
            public,
            udp: !only_tcp,
            tcp: tcp.is_some(),
        };
        let signal = tokio::net::TcpListener::bind(
            std::env::var("ORTUS_MEDIA_SIGNAL_LISTEN").unwrap_or_else(|_| "127.0.0.1:0".into()),
        )
        .await
        .map_err(|e| format!("media signalling: {e}"))?;
        let signal_url = std::env::var("ORTUS_MEDIA_SIGNAL_URL").unwrap_or_else(|_| {
            format!(
                "http://{}",
                signal
                    .local_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_default()
            )
        });
        let token = ortus_core::auth::random_hex(24);
        let id = store::new_id("n");
        let (tx, rx) = std::sync::mpsc::sync_channel::<Command>(256);
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let loop_running = Arc::clone(&running);
        std::thread::Builder::new()
            .name("ortus-media".into())
            .spawn(move || media_loop(socket, tcp, public, rx, loop_running))
            .map_err(|e| e.to_string())?;
        println!("ortus: calls: media on {local} (public {public}), signalling {signal_url}");
        let s = instance.store.as_ref();
        let sealed = instance.keys.seal(token.as_bytes());
        s.execute(
            "INSERT INTO calls_nodes (id, signal, token_sealed, seen_at) VALUES ($1, $2, $3, $4)",
            &[
                id.as_str().into(),
                signal_url.as_str().into(),
                sealed.into(),
                store::now().into(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
        let mut beat = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                _ = beat.tick() => {
                    let _ = s.execute("UPDATE calls_nodes SET seen_at = $1 WHERE id = $2", &[store::now().into(), id.as_str().into()]).await;
                }
                accepted = signal.accept() => {
                    if let Ok((conn, _)) = accepted {
                        let tx = tx.clone();
                        let token = token.clone();
                        tokio::spawn(async move { let _ = signalling(conn, &token, candidates, tx).await; });
                    }
                }
            }
        }
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = s
            .execute(
                "DELETE FROM calls_nodes WHERE id = $1",
                &[id.as_str().into()],
            )
            .await;
        Ok(())
    })
}

/// The address browsers send media to.
fn public_address(local: SocketAddr) -> Result<SocketAddr, String> {
    if let Ok(p) = std::env::var("ORTUS_MEDIA_PUBLIC") {
        return p.parse().map_err(|e| format!("ORTUS_MEDIA_PUBLIC: {e}"));
    }
    if !local.ip().is_unspecified() {
        return Ok(local);
    }
    // This machine's outward address (no packet is sent).
    let probe = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    probe.connect("192.0.2.1:9").map_err(|e| e.to_string())?;
    Ok(SocketAddr::new(
        probe.local_addr().map_err(|e| e.to_string())?.ip(),
        local.port(),
    ))
}

const MAX_REQUEST: usize = 64 * 1024;

/// One signalling request: `POST /offer {call, session, offer}` gives the
/// answer; `POST /kick {session}` drops a session.
/// Where browsers reach this node: its address, over UDP and TCP.
#[derive(Clone, Copy)]
struct Candidates {
    public: SocketAddr,
    udp: bool,
    tcp: bool,
}

async fn signalling(
    mut conn: tokio::net::TcpStream,
    token: &str,
    candidates: Candidates,
    tx: SyncSender<Command>,
) -> Result<(), String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(10), conn.read(&mut chunk))
            .await
            .map_err(|_| "slow")?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_REQUEST {
            return Err("too big".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut length = 0usize;
    let mut auth = String::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = v.trim().parse().unwrap_or(0),
                "authorization" => auth = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if length > MAX_REQUEST {
        return Err("too big".into());
    }
    while buf.len() < head_end + length {
        let n = tokio::time::timeout(Duration::from_secs(10), conn.read(&mut chunk))
            .await
            .map_err(|_| "slow")?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body: Value =
        serde_json::from_slice(&buf[head_end..head_end + length]).unwrap_or(Value::Null);
    let expected = format!("Bearer {token}");
    let authorised = auth.len() == expected.len()
        && auth
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |d, (a, b)| d | (a ^ b))
            == 0;
    let (status, out) = if !authorised {
        (401, json!({"error": "not signed"}))
    } else if request_line.starts_with("POST /offer ") {
        match offer(&body, candidates, &tx) {
            Ok(answer) => (200, json!({"answer": answer})),
            Err(e) => (400, json!({"error": e})),
        }
    } else if request_line.starts_with("POST /kick ") {
        let session = body["session"].as_str().unwrap_or_default().to_string();
        let _ = tx.try_send(Command::Kick { session });
        (200, json!({}))
    } else {
        (404, json!({"error": "no such thing"}))
    };
    let text = out.to_string();
    let reply = format!("HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", if status == 200 { "OK" } else { "Error" }, text.len());
    conn.write_all(reply.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn offer(
    body: &Value,
    candidates: Candidates,
    tx: &SyncSender<Command>,
) -> Result<SdpAnswer, String> {
    let call = body["call"].as_str().ok_or("no call")?.to_string();
    let session = body["session"].as_str().ok_or("no session")?.to_string();
    let offer: SdpOffer = serde_json::from_value(body["offer"].clone())
        .map_err(|e| format!("an unreadable offer: {e}"))?;
    let mut rtc = Rtc::builder().build(Instant::now());
    if candidates.udp {
        rtc.add_local_candidate(
            Candidate::host(candidates.public, "udp").map_err(|e| e.to_string())?,
        );
    }
    if candidates.tcp {
        let tcp = Candidate::builder()
            .tcp()
            .host(candidates.public)
            .tcptype(TcpType::Passive)
            .build()
            .map_err(|e| e.to_string())?;
        rtc.add_local_candidate(tcp);
    }
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| e.to_string())?;
    tx.try_send(Command::Join {
        call,
        session,
        rtc: Box::new(rtc),
    })
    .map_err(|_| "the media node is busy")?;
    Ok(answer)
}

// ---- the media loop (after str0m's chat example) ---------------------------

struct TrackIn {
    origin: u64,
    mid: Mid,
    kind: MediaKind,
}

struct TrackInEntry {
    id: Arc<TrackIn>,
    last_keyframe_request: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrackOutState {
    ToOpen,
    Negotiating(Mid),
    Open(Mid),
    ToStop(Mid),
    NegotiatingStop(Mid),
}

struct TrackOut {
    track_in: Weak<TrackIn>,
    origin_session: String,
    state: TrackOutState,
}

impl TrackOut {
    fn mid(&self) -> Option<Mid> {
        match self.state {
            TrackOutState::ToOpen => None,
            TrackOutState::Negotiating(m)
            | TrackOutState::Open(m)
            | TrackOutState::ToStop(m)
            | TrackOutState::NegotiatingStop(m) => Some(m),
        }
    }
}

struct Client {
    id: u64,
    call: String,
    session: String,
    rtc: Rtc,
    pending: Option<SdpPendingOffer>,
    /// The offer waiting for its answer, and when it was last sent (sent
    /// again if no answer comes).
    sent: Option<(String, Instant)>,
    /// Parts of a message from the browser, by message id.
    parts: Option<(String, usize, Vec<String>)>,
    channel: Option<ChannelId>,
    tracks_in: Vec<TrackInEntry>,
    tracks_out: Vec<TrackOut>,
    /// Since when its connection has been lost (it ends after `GRACE`).
    lost: Option<Instant>,
    /// Offers made to the browser (the last one's number).
    offers: u64,
}

/// How long a lost connection may take to come back.
const GRACE: Duration = Duration::from_secs(20);

/// Session descriptions go over the data channel in parts of at most this
/// many bytes (large messages were seen to go missing under load).
const PART: usize = 16 * 1024;

/// Writes a message in parts: {"id", "part", "of", "data"}.
fn write_parts(channel: &mut str0m::channel::Channel<'_>, text: &str) {
    let id = ortus_core::auth::random_hex(6);
    let chars: Vec<char> = text.chars().collect();
    let chunks: Vec<String> = chars.chunks(PART).map(|c| c.iter().collect()).collect();
    let of = chunks.len().max(1);
    for (i, data) in chunks.iter().enumerate() {
        let part = json!({"id": id, "part": i, "of": of, "data": data}).to_string();
        let _ = channel.write(false, part.as_bytes());
    }
}

#[allow(clippy::large_enum_variant)]
enum Propagated {
    Noop,
    Timeout(Instant),
    TrackOpen(u64, Weak<TrackIn>),
    MediaData(u64, MediaData),
    KeyframeRequest(u64, KeyframeRequest, u64, Mid),
}

/// A packet from the network: UDP, or one RFC 4571 frame of a TCP connection.
struct Packet {
    proto: Protocol,
    source: SocketAddr,
    destination: SocketAddr,
    data: Vec<u8>,
}

/// Where the loop sends: the UDP socket, and the open TCP connections by
/// their remote address.
/// One ICE-over-TCP connection: frames to write go through a bounded
/// queue to its own writer thread (a slow reader never stalls the media
/// loop; frames that do not fit are dropped, as UDP would). `live` turns
/// true once a call's connection takes its packets; one that is not live
/// within `PROVE` is closed.
struct Conn {
    out: SyncSender<Vec<u8>>,
    live: Arc<std::sync::atomic::AtomicBool>,
}

type Conns = Arc<Mutex<HashMap<SocketAddr, Conn>>>;

struct Wire {
    udp: UdpSocket,
    tcp: Conns,
}

impl Wire {
    /// A call's connection took a packet from this TCP peer.
    fn proven(&self, peer: SocketAddr) {
        if let Some(c) = self
            .tcp
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&peer)
        {
            c.live.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn send(&self, proto: Protocol, destination: SocketAddr, data: &[u8]) {
        match proto {
            Protocol::Udp => {
                let _ = self.udp.send_to(data, destination);
            }
            Protocol::Tcp => {
                let conns = self.tcp.lock().unwrap_or_else(|e| e.into_inner());
                let Some(conn) = conns.get(&destination) else {
                    return;
                };
                let Ok(len) = u16::try_from(data.len()) else {
                    return;
                };
                let mut framed = Vec::with_capacity(data.len() + 2);
                framed.extend_from_slice(&len.to_be_bytes());
                framed.extend_from_slice(data);
                let _ = conn.out.try_send(framed);
            }
            _ => {}
        }
    }
}

const MAX_TCP: usize = 500;
/// TCP connections from one address (an office behind one NAT shares it;
/// a connection that starts no call goes after `PROVE`).
const MAX_TCP_PER_IP: usize = 64;
/// How long a TCP connection has to start a call's connection.
const PROVE: Duration = Duration::from_secs(10);
/// Frames queued for one TCP connection.
const TCP_QUEUE: usize = 512;

/// Reads UDP packets into the loop's channel.
/// Packets are given to str0m as sent to `public`, the address its host
/// candidate names (a socket bound to 0.0.0.0, or behind NAT, has another
/// local address, and str0m drops checks to an address it does not know).
fn udp_reader(
    socket: UdpSocket,
    public: SocketAddr,
    out: SyncSender<Packet>,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    let destination = public;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buf = vec![0u8; 2000];
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        if let Ok((n, source)) = socket.recv_from(&mut buf) {
            let _ = out.try_send(Packet {
                proto: Protocol::Udp,
                source,
                destination,
                data: buf[..n].to_vec(),
            });
        }
    }
}

/// Takes TCP connections (ICE over TCP) and reads their frames into the
/// loop's channel.
fn tcp_acceptor(
    listener: TcpListener,
    public: SocketAddr,
    out: SyncSender<Packet>,
    conns: Conns,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    let destination = public;
    let _ = listener.set_nonblocking(true);
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        match listener.accept() {
            Ok((conn, peer)) => {
                {
                    let all = conns.lock().unwrap_or_else(|e| e.into_inner());
                    let same = all.keys().filter(|a| a.ip() == peer.ip()).count();
                    if all.len() >= MAX_TCP || same >= MAX_TCP_PER_IP {
                        continue;
                    }
                }
                let _ = conn.set_nonblocking(false);
                let _ = conn.set_nodelay(true);
                let _ = conn.set_write_timeout(Some(Duration::from_secs(5)));
                let Ok(mut writer) = conn.try_clone() else {
                    continue;
                };
                let (tx, frames) = std::sync::mpsc::sync_channel::<Vec<u8>>(TCP_QUEUE);
                let live = Arc::new(std::sync::atomic::AtomicBool::new(false));
                conns.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    peer,
                    Conn {
                        out: tx,
                        live: Arc::clone(&live),
                    },
                );
                let _ = std::thread::Builder::new()
                    .name("ortus-media-tcp-write".into())
                    .spawn(move || {
                        // Ends when the connection's entry (the sender) goes.
                        while let Ok(frame) = frames.recv() {
                            if writer.write_all(&frame).is_err() {
                                let _ = writer.shutdown(std::net::Shutdown::Both);
                                break;
                            }
                        }
                    });
                let out = out.clone();
                let conns = Arc::clone(&conns);
                let running = Arc::clone(&running);
                let _ = std::thread::Builder::new()
                    .name("ortus-media-tcp".into())
                    .spawn(move || {
                        let mut conn = conn;
                        let opened = Instant::now();
                        let mut len = [0u8; 2];
                        while running.load(std::sync::atomic::Ordering::SeqCst) {
                            let proven = live.load(std::sync::atomic::Ordering::SeqCst);
                            if !proven && opened.elapsed() > PROVE {
                                break;
                            }
                            let wait = if proven {
                                Duration::from_secs(60)
                            } else {
                                PROVE
                            };
                            let _ = conn.set_read_timeout(Some(wait));
                            if conn.read_exact(&mut len).is_err() {
                                break;
                            }
                            let size = u16::from_be_bytes(len) as usize;
                            let mut data = vec![0u8; size];
                            if conn.read_exact(&mut data).is_err() {
                                break;
                            }
                            if size == 0 {
                                continue;
                            }
                            let _ = out.try_send(Packet {
                                proto: Protocol::Tcp,
                                source: peer,
                                destination,
                                data,
                            });
                        }
                        let _ = conn.shutdown(std::net::Shutdown::Both);
                        conns
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&peer);
                    });
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn media_loop(
    udp: UdpSocket,
    tcp: Option<TcpListener>,
    public: SocketAddr,
    rx: Receiver<Command>,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut clients: Vec<Client> = Vec::new();
    let mut queue: VecDeque<(String, Propagated)> = VecDeque::new();
    let mut next_id = 0u64;
    let (net_tx, net_rx) = std::sync::mpsc::sync_channel::<Packet>(8192);
    let conns: Conns = Arc::new(Mutex::new(HashMap::new()));
    let Ok(reader) = udp.try_clone() else { return };
    {
        let (out, running) = (net_tx.clone(), Arc::clone(&running));
        let _ = std::thread::Builder::new()
            .name("ortus-media-udp".into())
            .spawn(move || udp_reader(reader, public, out, running));
    }
    if let Some(listener) = tcp {
        let (out, conns, running) = (net_tx.clone(), Arc::clone(&conns), Arc::clone(&running));
        let _ = std::thread::Builder::new()
            .name("ortus-media-tcp-accept".into())
            .spawn(move || tcp_acceptor(listener, public, out, conns, running));
    }
    drop(net_tx);
    let wire = Wire { udp, tcp: conns };
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        for c in clients.iter_mut() {
            if c.lost.is_some_and(|t| t.elapsed() > GRACE) {
                c.rtc.disconnect();
            }
        }
        clients.retain(|c| c.rtc.is_alive());
        loop {
            match rx.try_recv() {
                Ok(Command::Join { call, session, rtc }) => {
                    // A session that offers again starts afresh.
                    clients.retain(|c| c.session != session);
                    next_id += 1;
                    let mut client = Client {
                        id: next_id,
                        call: call.clone(),
                        session,
                        rtc: *rtc,
                        pending: None,
                        sent: None,
                        parts: None,
                        channel: None,
                        tracks_in: Vec::new(),
                        tracks_out: Vec::new(),
                        lost: None,
                        offers: 0,
                    };
                    for other in clients.iter().filter(|c| c.call == call) {
                        for t in &other.tracks_in {
                            client.tracks_out.push(TrackOut {
                                track_in: Arc::downgrade(&t.id),
                                origin_session: other.session.clone(),
                                state: TrackOutState::ToOpen,
                            });
                        }
                    }
                    clients.push(client);
                }
                Ok(Command::Kick { session }) => {
                    for c in clients.iter_mut().filter(|c| c.session == session) {
                        c.rtc.disconnect();
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let mut timeout = Instant::now() + Duration::from_millis(100);
        for client in clients.iter_mut() {
            loop {
                if !client.rtc.is_alive() {
                    break;
                }
                match client.poll_output(&wire) {
                    Propagated::Timeout(t) => {
                        timeout = timeout.min(t);
                        break;
                    }
                    Propagated::Noop => {}
                    p => queue.push_back((client.call.clone(), p)),
                }
            }
        }
        // Everything queued is forwarded, then every packet waiting is
        // read: neither starves the other with many people sending.
        let forwarded = !queue.is_empty();
        while let Some((call, p)) = queue.pop_front() {
            propagate(&call, &p, &mut clients);
        }
        let wait = if forwarded {
            Duration::ZERO
        } else {
            timeout
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1))
        };
        let mut first = if wait.is_zero() {
            net_rx.try_recv().ok()
        } else {
            net_rx.recv_timeout(wait).ok()
        };
        let mut read = 0;
        while let Some(packet) = first.take() {
            if let Ok(contents) = packet.data.as_slice().try_into() {
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: packet.proto,
                        source: packet.source,
                        destination: packet.destination,
                        contents,
                    },
                );
                if let Some(c) = clients.iter_mut().find(|c| c.rtc.accepts(&input)) {
                    if packet.proto == Protocol::Tcp {
                        wire.proven(packet.source);
                    }
                    c.input(input);
                    // Its output at once: str0m's queues are small, and a
                    // burst of input left unpolled overflows them.
                    while c.rtc.is_alive() {
                        match c.poll_output(&wire) {
                            Propagated::Timeout(_) => break,
                            Propagated::Noop => {}
                            p => queue.push_back((c.call.clone(), p)),
                        }
                    }
                }
            }
            read += 1;
            if read < 1000 {
                first = net_rx.try_recv().ok();
            }
        }
        let now = Instant::now();
        for c in clients.iter_mut() {
            c.input(Input::Timeout(now));
        }
    }
}

fn propagate(call: &str, p: &Propagated, clients: &mut [Client]) {
    let origin = match p {
        Propagated::TrackOpen(o, _)
        | Propagated::MediaData(o, _)
        | Propagated::KeyframeRequest(o, _, _, _) => *o,
        _ => return,
    };
    let origin_session = clients
        .iter()
        .find(|c| c.id == origin)
        .map(|c| c.session.clone())
        .unwrap_or_default();
    for c in clients
        .iter_mut()
        .filter(|c| c.call == call && c.id != origin)
    {
        match p {
            Propagated::TrackOpen(_, t) => c.tracks_out.push(TrackOut {
                track_in: t.clone(),
                origin_session: origin_session.clone(),
                state: TrackOutState::ToOpen,
            }),
            Propagated::MediaData(_, data) => c.media_out(origin, data),
            Propagated::KeyframeRequest(_, req, to, mid) if c.id == *to => c.keyframe(*req, *mid),
            _ => {}
        }
    }
}

impl Client {
    fn input(&mut self, input: Input) {
        // A packet that cannot be taken is dropped (DTLS and SCTP send it
        // again); the connection ends only when str0m ends it.
        if self.rtc.is_alive() {
            let _ = self.rtc.handle_input(input);
        }
    }

    fn poll_output(&mut self, wire: &Wire) -> Propagated {
        self.resend();
        if self.negotiate() {
            return Propagated::Noop;
        }
        match self.rtc.poll_output() {
            Ok(Output::Transmit(t)) => {
                wire.send(t.proto, t.destination, &t.contents);
                Propagated::Noop
            }
            Ok(Output::Timeout(t)) => Propagated::Timeout(t),
            Ok(Output::Event(e)) => match e {
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    // A network change (Wi-Fi to mobile) often recovers;
                    // the browser offers again on the same session if not.
                    self.lost.get_or_insert_with(Instant::now);
                    Propagated::Noop
                }
                Event::IceConnectionStateChange(_) => {
                    self.lost = None;
                    Propagated::Noop
                }
                Event::MediaAdded(m) => {
                    let entry = TrackInEntry {
                        id: Arc::new(TrackIn {
                            origin: self.id,
                            mid: m.mid,
                            kind: m.kind,
                        }),
                        last_keyframe_request: None,
                    };
                    let weak = Arc::downgrade(&entry.id);
                    self.tracks_in.push(entry);
                    Propagated::TrackOpen(self.id, weak)
                }
                Event::MediaData(data) => {
                    if !data.contiguous {
                        self.request_keyframe(data.mid);
                    }
                    Propagated::MediaData(self.id, data)
                }
                Event::KeyframeRequest(req) => {
                    let Some(out) = self.tracks_out.iter().find(|t| t.mid() == Some(req.mid))
                    else {
                        return Propagated::Noop;
                    };
                    let Some(t) = out.track_in.upgrade() else {
                        return Propagated::Noop;
                    };
                    Propagated::KeyframeRequest(self.id, req, t.origin, t.mid)
                }
                Event::ChannelOpen(id, _) => {
                    self.channel = Some(id);
                    Propagated::Noop
                }
                Event::ChannelData(d) => {
                    self.channel_data(d);
                    Propagated::Noop
                }
                _ => Propagated::Noop,
            },
            Err(_) => {
                self.rtc.disconnect();
                Propagated::Noop
            }
        }
    }

    fn request_keyframe(&mut self, mid: Mid) {
        let Some(entry) = self.tracks_in.iter_mut().find(|t| t.id.mid == mid) else {
            return;
        };
        if entry
            .last_keyframe_request
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        if let Some(mut w) = self.rtc.writer(mid) {
            let _ = w.request_keyframe(None, KeyframeRequestKind::Fir);
        }
        entry.last_keyframe_request = Some(Instant::now());
    }

    fn keyframe(&mut self, req: KeyframeRequest, mid: Mid) {
        if let Some(mut w) = self.rtc.writer(mid) {
            let _ = w.request_keyframe(req.rid, req.kind);
        }
    }

    /// Offers the tracks others opened (and stops those they closed), over
    /// the data channel, one negotiation at a time.
    fn negotiate(&mut self) -> bool {
        if self.channel.is_none() || self.pending.is_some() {
            return false;
        }
        for t in &mut self.tracks_out {
            if let TrackOutState::Open(m) = t.state {
                if t.track_in.upgrade().is_none() {
                    t.state = TrackOutState::ToStop(m);
                }
            }
        }
        let mut change = self.rtc.sdp_api();
        for t in &mut self.tracks_out {
            match t.state {
                TrackOutState::ToOpen => {
                    if let Some(i) = t.track_in.upgrade() {
                        // The browser sees who it is from (the stream) and
                        // which of their tracks (its own mid).
                        let mid = change.add_media(
                            i.kind,
                            Direction::SendOnly,
                            Some(t.origin_session.clone()),
                            Some(i.mid.to_string()),
                            None,
                        );
                        t.state = TrackOutState::Negotiating(mid);
                    }
                }
                TrackOutState::ToStop(m) => {
                    change.stop_media(m);
                    t.state = TrackOutState::NegotiatingStop(m);
                }
                _ => {}
            }
        }
        if !change.has_changes() {
            return false;
        }
        let Some((offer, pending)) = change.apply() else {
            return false;
        };
        let Some(mut channel) = self.channel.and_then(|id| self.rtc.channel(id)) else {
            return false;
        };
        // Numbered: the browser's answer names it, so an answer to an
        // offer sent again late is told from the answer to this one.
        self.offers += 1;
        let mut tagged = serde_json::to_value(&offer).unwrap_or_default();
        tagged["n"] = self.offers.into();
        let text = tagged.to_string();
        write_parts(&mut channel, &text);
        self.sent = Some((text, Instant::now()));
        self.pending = Some(pending);
        true
    }

    /// An offer with no answer for a while is sent again.
    fn resend(&mut self) {
        let Some((text, at)) = self.sent.as_ref() else {
            return;
        };
        if at.elapsed() < Duration::from_secs(8) {
            return;
        }
        let text = text.clone();
        if let Some(mut channel) = self.channel.and_then(|id| self.rtc.channel(id)) {
            write_parts(&mut channel, &text);
        }
        self.sent = Some((text, Instant::now()));
    }

    /// A part of a message from the browser; the whole message when the
    /// last part has come.
    fn channel_data(&mut self, d: ChannelData) {
        let Ok(part) = serde_json::from_slice::<Value>(&d.data) else {
            return;
        };
        let (Some(id), Some(i), Some(of), Some(data)) = (
            part["id"].as_str(),
            part["part"].as_u64(),
            part["of"].as_u64(),
            part["data"].as_str(),
        ) else {
            return;
        };
        let (i, of) = (i as usize, of as usize);
        if of == 0 || of > 64 || i >= of {
            return;
        }
        if self
            .parts
            .as_ref()
            .is_none_or(|(have, n, _)| have != id || *n != of)
        {
            self.parts = Some((id.to_string(), of, vec![String::new(); of]));
        }
        let Some((_, _, got)) = self.parts.as_mut() else {
            return;
        };
        got[i] = data.to_string();
        if got.iter().any(String::is_empty) {
            return;
        }
        let whole = got.concat();
        self.parts = None;
        self.message(whole.as_bytes());
    }

    fn message(&mut self, data: &[u8]) {
        let Ok(mut value) = serde_json::from_slice::<Value>(data) else {
            return;
        };
        let n = value
            .as_object_mut()
            .and_then(|o| o.remove("n"))
            .and_then(|n| n.as_u64());
        if let Ok(offer) = serde_json::from_value::<SdpOffer>(value.clone()) {
            // The browser added or removed its own tracks (a screen).
            let Ok(answer) = self.rtc.sdp_api().accept_offer(offer) else {
                self.rtc.disconnect();
                return;
            };
            for t in &mut self.tracks_out {
                match t.state {
                    TrackOutState::Negotiating(_) => t.state = TrackOutState::ToOpen,
                    TrackOutState::NegotiatingStop(m) => t.state = TrackOutState::ToStop(m),
                    _ => {}
                }
            }
            if let Some(mut channel) = self.channel.and_then(|id| self.rtc.channel(id)) {
                write_parts(
                    &mut channel,
                    &serde_json::to_string(&answer).unwrap_or_default(),
                );
            }
        } else if let Ok(answer) = serde_json::from_value::<SdpAnswer>(value) {
            if n != Some(self.offers) {
                // An answer to an offer that has been answered already.
                return;
            }
            if let Some(pending) = self.pending.take() {
                self.sent = None;
                if self.rtc.sdp_api().accept_answer(pending, answer).is_err() {
                    self.rtc.disconnect();
                    return;
                }
                for t in &mut self.tracks_out {
                    if let TrackOutState::Negotiating(m) = t.state {
                        t.state = TrackOutState::Open(m);
                    }
                }
                self.tracks_out
                    .retain(|t| !matches!(t.state, TrackOutState::NegotiatingStop(_)));
            }
        }
    }

    fn media_out(&mut self, origin: u64, data: &MediaData) {
        let Some(mid) = self
            .tracks_out
            .iter()
            .find(|o| {
                o.track_in
                    .upgrade()
                    .is_some_and(|i| i.origin == origin && i.mid == data.mid)
            })
            .and_then(TrackOut::mid)
        else {
            return;
        };
        let Some(writer) = self.rtc.writer(mid) else {
            return;
        };
        let Some(pt) = writer.match_params(data.params) else {
            return;
        };
        if writer
            .write(pt, data.network_time, data.time, data.data.clone())
            .is_err()
        {
            self.rtc.disconnect();
        }
    }
}
