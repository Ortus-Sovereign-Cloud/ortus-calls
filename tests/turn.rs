//! The TURN relay: its codec (MD5, message types, integrity, addresses; no
//! input makes the parser panic) and a whole exchange over TCP with a
//! stand-in media node: credentials checked, the media node the only peer,
//! data both ways by Send/Data and ChannelData.

use std::net::SocketAddr;
use std::time::Duration;

use ortus_calls::turn::{self, Builder, Media};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn md5_matches_its_test_vectors() {
    assert_eq!("d41d8cd98f00b204e9800998ecf8427e", hex(&turn::md5(b"")));
    assert_eq!("900150983cd24fb0d6963f7d28e17f72", hex(&turn::md5(b"abc")));
    assert_eq!(
        "9e107d9d372bb6826bd81d3542a419d6",
        hex(&turn::md5(b"The quick brown fox jumps over the lazy dog"))
    );
    assert_eq!(
        "57edf4a22be3c955ac49da2e2107b67a",
        hex(&turn::md5(
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
        ))
    );
}

#[test]
fn messages_round_trip_and_tampering_is_seen() {
    for method in [
        turn::BINDING,
        turn::ALLOCATE,
        turn::REFRESH,
        turn::CHANNEL_BIND,
    ] {
        for class in [turn::REQUEST, turn::INDICATION, turn::SUCCESS, turn::ERROR] {
            assert_eq!(
                (method, class),
                turn::split_type(turn::message_type(method, class))
            );
        }
    }
    // A Binding success response is type 0x0101, an Allocate error 0x0113.
    assert_eq!(0x0101, turn::message_type(turn::BINDING, turn::SUCCESS));
    assert_eq!(0x0113, turn::message_type(turn::ALLOCATE, turn::ERROR));

    let txid = [7u8; 12];
    let peer: SocketAddr = "203.0.113.5:40000".parse().unwrap();
    let v6: SocketAddr = "[2001:db8::1]:3478".parse().unwrap();
    let raw = Builder::new(turn::ALLOCATE, turn::REQUEST, &txid)
        .attr(turn::USERNAME, b"1:s")
        .address(turn::XOR_PEER_ADDRESS, peer, &txid)
        .address(turn::XOR_RELAYED_ADDRESS, v6, &txid)
        .finish(Some(b"key"));
    let m = turn::parse(&raw).expect("parses");
    assert_eq!((turn::ALLOCATE, turn::REQUEST), (m.method, m.class));
    assert_eq!(
        Some(peer),
        turn::read_xor_address(m.get(turn::XOR_PEER_ADDRESS).unwrap(), &txid)
    );
    assert_eq!(
        Some(v6),
        turn::read_xor_address(m.get(turn::XOR_RELAYED_ADDRESS).unwrap(), &txid)
    );
    assert!(turn::integrity_ok(&m, b"key"));
    assert!(!turn::integrity_ok(&m, b"other key"));
    let mut tampered = raw.clone();
    tampered[25] ^= 1;
    assert!(!turn::integrity_ok(
        &turn::parse(&tampered).unwrap(),
        b"key"
    ));
}

#[test]
fn no_input_makes_the_parser_panic() {
    // Truncations and bit flips of a real message, then random bytes.
    let txid = [1u8; 12];
    let raw = Builder::new(turn::CREATE_PERMISSION, turn::REQUEST, &txid)
        .address(
            turn::XOR_PEER_ADDRESS,
            "203.0.113.5:40000".parse().unwrap(),
            &txid,
        )
        .attr(turn::NONCE, b"abc")
        .finish(Some(b"k"));
    for n in 0..=raw.len() {
        let _ = turn::parse(&raw[..n]);
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let mut m = raw.clone();
        let i = (next() as usize) % m.len();
        m[i] ^= (next() & 0xFF) as u8;
        if let Some(p) = turn::parse(&m) {
            for (kind, value, _) in &p.attrs {
                if *kind == turn::XOR_PEER_ADDRESS {
                    let _ = turn::read_xor_address(value, &p.txid);
                }
            }
            let _ = turn::integrity_ok(&p, b"k");
        }
        let len = (next() as usize) % 200;
        let random: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
        let _ = turn::parse(&random);
    }
}

/// Reads one frame back: a STUN message or ChannelData.
async fn read_frame(s: &mut TcpStream) -> Vec<u8> {
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.unwrap();
    let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
    let rest = if head[0] & 0xC0 == 0 {
        len + 16
    } else {
        len + (4 - len % 4) % 4
    };
    let mut buf = head.to_vec();
    buf.resize(4 + rest, 0);
    s.read_exact(&mut buf[4..]).await.unwrap();
    buf
}

async fn request(s: &mut TcpStream, raw: Vec<u8>) -> turn::Message {
    s.write_all(&raw).await.unwrap();
    turn::parse(
        &tokio::time::timeout(Duration::from_secs(5), read_frame(s))
            .await
            .unwrap(),
    )
    .unwrap()
}

fn error_code(m: &turn::Message) -> u16 {
    let v = m.get(turn::ERROR_CODE).unwrap();
    u16::from(v[2]) * 100 + u16::from(v[3])
}

#[tokio::test]
async fn a_browser_reaches_the_media_node_through_the_relay_and_nothing_else() {
    // The media node: a UDP socket answering "pong:" + what it got.
    let node = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let local = node.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = node.recv_from(&mut buf).await {
            let mut reply = b"pong:".to_vec();
            reply.extend_from_slice(&buf[..n]);
            let _ = node.send_to(&reply, from).await;
        }
    });
    // Its public address is one this machine may not reach (no hairpin):
    // the relay must send to the local one.
    let public: SocketAddr = "203.0.113.5:40000".parse().unwrap();
    let media = Media { public, local };
    let listen = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (_stop_tx, stop) = tokio::sync::watch::channel(false);
    tokio::spawn(turn::serve(listen, "node-token".into(), media, stop));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut s = TcpStream::connect(listen).await.unwrap();

    // Without credentials: 401 with a realm and nonce.
    let txid = [2u8; 12];
    let r = request(
        &mut s,
        Builder::new(turn::ALLOCATE, turn::REQUEST, &txid)
            .attr(turn::REQUESTED_TRANSPORT, &[17, 0, 0, 0])
            .finish(None),
    )
    .await;
    assert_eq!((turn::ERROR, 401), (r.class, error_code(&r)));
    let nonce = r.get(turn::NONCE).unwrap().to_vec();
    assert_eq!(Some(turn::REALM.as_bytes()), r.get(turn::ATTR_REALM));

    let allocate = |user: &str, pass: &str, txid: [u8; 12]| {
        let key = turn::md5(format!("{user}:{}:{pass}", turn::REALM).as_bytes());
        Builder::new(turn::ALLOCATE, turn::REQUEST, &txid)
            .attr(turn::REQUESTED_TRANSPORT, &[17, 0, 0, 0])
            .attr(turn::USERNAME, user.as_bytes())
            .attr(turn::ATTR_REALM, turn::REALM.as_bytes())
            .attr(turn::NONCE, &nonce)
            .finish(Some(&key))
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Expired, or the wrong password: refused.
    let (old_user, old_pass) = turn::credentials("node-token", "s1", now - 7200, 3600);
    let r = request(&mut s, allocate(&old_user, &old_pass, [3; 12])).await;
    assert_eq!(401, error_code(&r), "expired");
    let (user, pass) = turn::credentials("node-token", "s1", now, 3600);
    let r = request(&mut s, allocate(&user, "guessed", [4; 12])).await;
    assert_eq!(401, error_code(&r), "wrong password");

    // The right ones: an allocation, signed with the same key.
    let key = turn::md5(format!("{user}:{}:{pass}", turn::REALM).as_bytes());
    let r = request(&mut s, allocate(&user, &pass, [5; 12])).await;
    assert_eq!(turn::SUCCESS, r.class, "{:?}", r.get(turn::ERROR_CODE));
    assert!(turn::integrity_ok(&r, &key));
    assert!(turn::relayed_of(&r).is_some());

    let signed = |method: u16, txid: [u8; 12], peer: SocketAddr, channel: Option<u16>| {
        let mut b =
            Builder::new(method, turn::REQUEST, &txid).address(turn::XOR_PEER_ADDRESS, peer, &txid);
        if let Some(ch) = channel {
            b = b.attr(turn::CHANNEL_NUMBER, &[(ch >> 8) as u8, ch as u8, 0, 0]);
        }
        b.attr(turn::USERNAME, user.as_bytes())
            .attr(turn::ATTR_REALM, turn::REALM.as_bytes())
            .attr(turn::NONCE, &nonce)
            .finish(Some(&key))
    };
    // Any address but the media node's: forbidden (not an open relay).
    let r = request(
        &mut s,
        signed(
            turn::CREATE_PERMISSION,
            [6; 12],
            "198.51.100.9:53".parse().unwrap(),
            None,
        ),
    )
    .await;
    assert_eq!(403, error_code(&r));
    let r = request(
        &mut s,
        signed(turn::CREATE_PERMISSION, [7; 12], public, None),
    )
    .await;
    assert_eq!(turn::SUCCESS, r.class);

    // A Send indication to the public address reaches the node (at its
    // local address); its answer comes back as a Data indication.
    let txid = [8u8; 12];
    s.write_all(
        &Builder::new(turn::SEND, turn::INDICATION, &txid)
            .address(turn::XOR_PEER_ADDRESS, public, &txid)
            .attr(turn::DATA_ATTR, b"hello")
            .finish(None),
    )
    .await
    .unwrap();
    let data = turn::parse(
        &tokio::time::timeout(Duration::from_secs(5), read_frame(&mut s))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!((turn::DATA, turn::INDICATION), (data.method, data.class));
    assert_eq!(
        Some(public),
        turn::read_xor_address(data.get(turn::XOR_PEER_ADDRESS).unwrap(), &data.txid)
    );
    assert_eq!(Some(&b"pong:hello"[..]), data.get(turn::DATA_ATTR));

    // A channel: ChannelData both ways.
    let r = request(
        &mut s,
        signed(turn::CHANNEL_BIND, [9; 12], public, Some(0x4001)),
    )
    .await;
    assert_eq!(turn::SUCCESS, r.class);
    let mut frame = vec![0x40, 0x01, 0, 3];
    frame.extend_from_slice(b"abc");
    frame.push(0);
    s.write_all(&frame).await.unwrap();
    let back = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut s))
        .await
        .unwrap();
    assert_eq!(&[0x40, 0x01, 0, 8], &back[..4]);
    assert_eq!(b"pong:abc", &back[4..12]);
}
