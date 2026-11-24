//! A TURN relay (RFC 8656, over TCP) for calls on networks that allow only
//! web ports: the browser reaches it as `turns:<name>:443` (rws hands that
//! name's TLS connections, decrypted, to `ORTUS_TURN_LISTEN`), allocates a
//! relayed address, and its media goes to the media node through it.
//!
//! It relays to the media node only, never anywhere else: a peer is the
//! media node's public address (the one its candidate names) or its local
//! one, and is sent to the local one (this machine cannot always reach its
//! own public address). Credentials are short-lived (TURN REST style):
//! username `<expiry>:<session>`, password base64(HMAC-SHA1(node token,
//! username)), handed out with the call join. One allocation per TCP
//! connection; connections are capped.
//!
//! Only what browsers use: Allocate (UDP), Refresh, CreatePermission,
//! ChannelBind, Send/Data indications and ChannelData. Messages are parsed
//! with bounds; anything else gets an error or is dropped.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const MAGIC: u32 = 0x2112_A442;
pub const REALM: &str = "ortus";
/// The largest message taken (a STUN message or ChannelData frame).
pub const MAX_MESSAGE: usize = 4096;
/// Connections at once.
pub const MAX_CONNECTIONS: usize = 1000;
/// Connections at once from one address (an IPv6 /64 is one): an office
/// behind one NAT shares it, one machine cannot take the whole relay.
pub const MAX_PER_ADDRESS: usize = 32;
/// How long TURN credentials handed to a call's session last.
pub const CREDENTIAL_SECS: u64 = 4 * 3600;
/// Allocation lifetimes: asked for, at most.
const DEFAULT_LIFETIME: u32 = 600;
const MAX_LIFETIME: u32 = 3600;
/// A connection that allocates nothing in this long is closed.
const FIRST_ALLOCATE: Duration = Duration::from_secs(30);

// Methods.
pub const BINDING: u16 = 0x001;
pub const ALLOCATE: u16 = 0x003;
pub const REFRESH: u16 = 0x004;
pub const SEND: u16 = 0x006;
pub const DATA: u16 = 0x007;
pub const CREATE_PERMISSION: u16 = 0x008;
pub const CHANNEL_BIND: u16 = 0x009;

// Classes.
pub const REQUEST: u8 = 0;
pub const INDICATION: u8 = 1;
pub const SUCCESS: u8 = 2;
pub const ERROR: u8 = 3;

// Attributes.
pub const USERNAME: u16 = 0x0006;
pub const MESSAGE_INTEGRITY: u16 = 0x0008;
pub const ERROR_CODE: u16 = 0x0009;
pub const CHANNEL_NUMBER: u16 = 0x000C;
pub const LIFETIME: u16 = 0x000D;
pub const XOR_PEER_ADDRESS: u16 = 0x0012;
pub const DATA_ATTR: u16 = 0x0013;
pub const ATTR_REALM: u16 = 0x0014;
pub const NONCE: u16 = 0x0015;
pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
pub const REQUESTED_TRANSPORT: u16 = 0x0019;
pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
pub const SOFTWARE: u16 = 0x8022;
pub const FINGERPRINT: u16 = 0x8028;

/// The message type of `method` in `class` (RFC 8489, 5).
pub fn message_type(method: u16, class: u8) -> u16 {
    let c = u16::from(class);
    (method & 0x000F)
        | ((method & 0x0070) << 1)
        | ((method & 0x0F80) << 2)
        | ((c & 1) << 4)
        | ((c & 2) << 7)
}

/// The method and class of a message type.
pub fn split_type(t: u16) -> (u16, u8) {
    let method = (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80);
    let class = (((t >> 4) & 1) | ((t >> 7) & 2)) as u8;
    (method, class)
}

/// A parsed STUN message: its type, transaction id, attributes (type,
/// value, offset of the attribute in `raw`) and the bytes themselves.
#[derive(Debug)]
pub struct Message {
    pub method: u16,
    pub class: u8,
    pub txid: [u8; 12],
    pub attrs: Vec<(u16, Vec<u8>, usize)>,
    pub raw: Vec<u8>,
}

impl Message {
    pub fn get(&self, t: u16) -> Option<&[u8]> {
        self.attrs.iter().find(|a| a.0 == t).map(|a| a.1.as_slice())
    }
}

/// Parses one whole STUN message; `None` for anything malformed.
pub fn parse(raw: &[u8]) -> Option<Message> {
    if raw.len() < 20 || raw.len() > MAX_MESSAGE || raw[0] & 0xC0 != 0 {
        return None;
    }
    let t = u16::from_be_bytes([raw[0], raw[1]]);
    let len = usize::from(u16::from_be_bytes([raw[2], raw[3]]));
    if u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]) != MAGIC
        || len + 20 != raw.len()
        || len % 4 != 0
    {
        return None;
    }
    let mut txid = [0u8; 12];
    txid.copy_from_slice(&raw[8..20]);
    let mut attrs = Vec::new();
    let mut at = 20;
    while at + 4 <= raw.len() {
        let kind = u16::from_be_bytes([raw[at], raw[at + 1]]);
        let n = usize::from(u16::from_be_bytes([raw[at + 2], raw[at + 3]]));
        let end = at + 4 + n;
        if end > raw.len() || attrs.len() >= 64 {
            return None;
        }
        attrs.push((kind, raw[at + 4..end].to_vec(), at));
        at = end + (4 - n % 4) % 4;
    }
    if at != raw.len() {
        return None;
    }
    let (method, class) = split_type(t);
    Some(Message {
        method,
        class,
        txid,
        attrs,
        raw: raw.to_vec(),
    })
}

/// Builds a message; `key` adds MESSAGE-INTEGRITY, then FINGERPRINT.
pub struct Builder {
    buf: Vec<u8>,
}

impl Builder {
    pub fn new(method: u16, class: u8, txid: &[u8; 12]) -> Self {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&message_type(method, class).to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(txid);
        Builder { buf }
    }

    pub fn attr(mut self, kind: u16, value: &[u8]) -> Self {
        self.buf.extend_from_slice(&kind.to_be_bytes());
        self.buf
            .extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(value);
        self.buf
            .resize(self.buf.len() + (4 - value.len() % 4) % 4, 0);
        self
    }

    pub fn address(self, kind: u16, addr: SocketAddr, txid: &[u8; 12]) -> Self {
        self.attr(kind, &xor_address(addr, txid))
    }

    fn set_length(&mut self, extra: usize) {
        let len = (self.buf.len() - 20 + extra) as u16;
        self.buf[2..4].copy_from_slice(&len.to_be_bytes());
    }

    pub fn finish(mut self, key: Option<&[u8]>) -> Vec<u8> {
        if let Some(key) = key {
            self.set_length(24);
            let mac = hmac_sha1(key, &self.buf);
            self = self.attr(MESSAGE_INTEGRITY, &mac);
        }
        self.set_length(8);
        let crc = crc32fast::hash(&self.buf) ^ 0x5354_554E;
        self = self.attr(FINGERPRINT, &crc.to_be_bytes());
        self.set_length(0);
        self.buf
    }
}

pub fn hmac_sha1(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, key);
    aws_lc_rs::hmac::sign(&key, data).as_ref().to_vec()
}

/// Whether `m` carries a MESSAGE-INTEGRITY made with `key`.
pub fn integrity_ok(m: &Message, key: &[u8]) -> bool {
    let Some((_, value, offset)) = m.attrs.iter().find(|a| a.0 == MESSAGE_INTEGRITY) else {
        return false;
    };
    if value.len() != 20 {
        return false;
    }
    let mut covered = m.raw[..*offset].to_vec();
    let len = (*offset - 20 + 24) as u16;
    covered[2..4].copy_from_slice(&len.to_be_bytes());
    let mac = hmac_sha1(key, &covered);
    aws_lc_rs::constant_time::verify_slices_are_equal(&mac, value).is_ok()
}

/// XOR-encodes an address (RFC 8489, 14.2).
pub fn xor_address(addr: SocketAddr, txid: &[u8; 12]) -> Vec<u8> {
    let port = addr.port() ^ (MAGIC >> 16) as u16;
    let mut out = vec![0];
    let mut mask = MAGIC.to_be_bytes().to_vec();
    mask.extend_from_slice(txid);
    match addr.ip() {
        IpAddr::V4(v4) => {
            out.push(1);
            out.extend_from_slice(&port.to_be_bytes());
            out.extend(v4.octets().iter().zip(&mask).map(|(a, b)| a ^ b));
        }
        IpAddr::V6(v6) => {
            out.push(2);
            out.extend_from_slice(&port.to_be_bytes());
            out.extend(v6.octets().iter().zip(&mask).map(|(a, b)| a ^ b));
        }
    }
    out
}

pub fn read_xor_address(value: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 8 {
        return None;
    }
    let port = u16::from_be_bytes([value[2], value[3]]) ^ (MAGIC >> 16) as u16;
    let mut mask = MAGIC.to_be_bytes().to_vec();
    mask.extend_from_slice(txid);
    match (value[1], value.len()) {
        (1, 8) => {
            let o: Vec<u8> = value[4..8].iter().zip(&mask).map(|(a, b)| a ^ b).collect();
            Some(SocketAddr::new(
                IpAddr::from([o[0], o[1], o[2], o[3]]),
                port,
            ))
        }
        (2, 20) => {
            let o: Vec<u8> = value[4..20].iter().zip(&mask).map(|(a, b)| a ^ b).collect();
            let a: [u8; 16] = o.try_into().ok()?;
            Some(SocketAddr::new(IpAddr::from(a), port))
        }
        _ => None,
    }
}

/// The password for `username` (TURN REST style).
pub fn password(secret: &str, username: &str) -> String {
    base64_encode(&hmac_sha1(secret.as_bytes(), username.as_bytes()))
}

/// Short-lived credentials for a session: (username, password).
pub fn credentials(secret: &str, session: &str, now_secs: u64, ttl_secs: u64) -> (String, String) {
    let username = format!("{}:{}", now_secs + ttl_secs, session);
    let password = password(secret, &username);
    (username, password)
}

fn base64_encode(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// MD5 (RFC 1321), only for TURN's long-term credential key
/// MD5(username:realm:password); nothing secret rests on it.
pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64)
        .map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32)
        .collect();
    let (mut a0, mut b0, mut c0, mut d0) = (
        0x6745_2301u32,
        0xefcd_ab89u32,
        0x98ba_dcfeu32,
        0x1032_5476u32,
    );
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((input.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Where the relay may send: the media node, by its public address (as
/// its candidate names it) and its local one (where packets really go).
#[derive(Clone, Copy, Debug)]
pub struct Media {
    pub public: SocketAddr,
    pub local: SocketAddr,
}

impl Media {
    /// The address to send to for `peer`, if the relay may.
    pub fn send_to(&self, peer: SocketAddr) -> Option<SocketAddr> {
        (peer == self.public || peer == self.local).then_some(self.local)
    }
}

/// Runs the relay on `listen` until `stop`.
pub async fn serve(
    listen: SocketAddr,
    secret: String,
    media: Media,
    mut stop: ortus_core::app::Stop,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("TURN on {listen}: {e}"))?;
    println!("ortus: calls: TURN relay on {listen}");
    let open = Arc::new(AtomicUsize::new(0));
    let from: Arc<std::sync::Mutex<std::collections::HashMap<IpAddr, usize>>> = Arc::default();
    let secret = Arc::new(secret);
    loop {
        tokio::select! {
            _ = stop.changed() => return Ok(()),
            accepted = listener.accept() => {
                let Ok((stream, client)) = accepted else { continue };
                if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue;
                }
                let network = network_of(client.ip());
                {
                    let mut from = from.lock().expect("the address counts");
                    let n = from.entry(network).or_default();
                    if *n >= MAX_PER_ADDRESS {
                        continue;
                    }
                    *n += 1;
                }
                open.fetch_add(1, Ordering::SeqCst);
                let (open, from, secret) = (Arc::clone(&open), Arc::clone(&from), Arc::clone(&secret));
                tokio::spawn(async move {
                    let _ = connection(stream, client, &secret, media).await;
                    open.fetch_sub(1, Ordering::SeqCst);
                    let mut from = from.lock().expect("the address counts");
                    if let Some(n) = from.get_mut(&network) {
                        *n -= 1;
                        if *n == 0 {
                            from.remove(&network);
                        }
                    }
                });
            }
        }
    }
}

/// The address connections are counted by: an IPv4 address, or an IPv6
/// /64 (one machine or home holds a whole /64).
pub fn network_of(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut o = v6.octets();
                o[8..].fill(0);
                IpAddr::V6(o.into())
            }
        },
    }
}

struct Allocation {
    udp: tokio::net::UdpSocket,
    expires: Instant,
    permitted: bool,
    /// Channel number bound to the media node, if any.
    channel: Option<u16>,
}

/// Reads one frame (a STUN message or ChannelData) from the stream.
async fn frame<R: AsyncReadExt + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut head = [0u8; 4];
    r.read_exact(&mut head).await?;
    let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
    let rest = if head[0] & 0xC0 == 0 {
        len + 16
    } else if (0x40..0x80).contains(&head[0]) {
        len + (4 - len % 4) % 4
    } else {
        return Err(std::io::ErrorKind::InvalidData.into());
    };
    if rest + 4 > MAX_MESSAGE {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut buf = head.to_vec();
    buf.resize(4 + rest, 0);
    r.read_exact(&mut buf[4..]).await?;
    Ok(buf)
}

fn error(m: &Message, code: u16, reason: &str, nonce: &str, key: Option<&[u8]>) -> Vec<u8> {
    let mut value = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
    value.extend_from_slice(reason.as_bytes());
    let mut b = Builder::new(m.method, ERROR, &m.txid).attr(ERROR_CODE, &value);
    if code == 401 || code == 438 {
        b = b
            .attr(ATTR_REALM, REALM.as_bytes())
            .attr(NONCE, nonce.as_bytes());
    }
    b.finish(key)
}

/// The long-term key for a request's USERNAME, if its credentials hold.
fn key_for(m: &Message, secret: &str, now_secs: u64) -> Option<Vec<u8>> {
    let username = std::str::from_utf8(m.get(USERNAME)?).ok()?;
    let expiry: u64 = username.split(':').next()?.parse().ok()?;
    if expiry < now_secs {
        return None;
    }
    let pass = password(secret, username);
    let key = md5(format!("{username}:{REALM}:{pass}").as_bytes()).to_vec();
    integrity_ok(m, &key).then_some(key)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn connection(
    stream: tokio::net::TcpStream,
    client: SocketAddr,
    secret: &str,
    media: Media,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let nonce = ortus_core::auth::random_hex(16);
    let mut alloc: Option<Allocation> = None;
    let mut key: Option<Vec<u8>> = None;
    let started = Instant::now();
    let mut udp_buf = vec![0u8; 2048];
    loop {
        // Until it allocates, a connection has a short life; then its
        // allocation's.
        let deadline = alloc
            .as_ref()
            .map(|a| a.expires)
            .unwrap_or(started + FIRST_ALLOCATE);
        let from_peer = async {
            match &alloc {
                Some(a) => a.udp.recv_from(&mut udp_buf).await.map(Some),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.into()) => return Ok(()),
            got = frame(&mut reader) => {
                let raw = got?;
                if raw[0] & 0xC0 != 0 {
                    // ChannelData: to the media node, on the bound channel.
                    let ch = u16::from_be_bytes([raw[0], raw[1]]);
                    let len = usize::from(u16::from_be_bytes([raw[2], raw[3]]));
                    if let Some(a) = alloc.as_ref().filter(|a| a.channel == Some(ch)) {
                        let _ = a.udp.send_to(&raw[4..4 + len], media.local).await;
                    }
                    continue;
                }
                let Some(m) = parse(&raw) else { return Ok(()) };
                let reply = handle(&m, client, secret, &nonce, media, &mut alloc, &mut key).await;
                if let Some(reply) = reply {
                    writer.write_all(&reply).await?;
                }
            }
            got = from_peer => {
                let Ok(Some((n, from))) = got else { continue };
                // Only the media node answers through the relay.
                if from != media.local {
                    continue;
                }
                let Some(a) = alloc.as_ref().filter(|a| a.permitted) else { continue };
                let data = &udp_buf[..n];
                let out = match a.channel {
                    Some(ch) => {
                        let mut f = Vec::with_capacity(n + 8);
                        f.extend_from_slice(&ch.to_be_bytes());
                        f.extend_from_slice(&(n as u16).to_be_bytes());
                        f.extend_from_slice(data);
                        f.resize(f.len() + (4 - n % 4) % 4, 0);
                        f
                    }
                    None => {
                        let txid: [u8; 12] = rand_txid();
                        Builder::new(DATA, INDICATION, &txid)
                            .address(XOR_PEER_ADDRESS, media.public, &txid)
                            .attr(DATA_ATTR, data)
                            .finish(None)
                    }
                };
                writer.write_all(&out).await?;
            }
        }
    }
}

fn rand_txid() -> [u8; 12] {
    let hex = ortus_core::auth::random_hex(12);
    let mut out = [0u8; 12];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    out
}

async fn handle(
    m: &Message,
    client: SocketAddr,
    secret: &str,
    nonce: &str,
    media: Media,
    alloc: &mut Option<Allocation>,
    key: &mut Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    if m.class == INDICATION {
        // Send: data to the media node.
        if m.method == SEND {
            let peer = m
                .get(XOR_PEER_ADDRESS)
                .and_then(|v| read_xor_address(v, &m.txid))?;
            let data = m.get(DATA_ATTR)?;
            let a = alloc.as_ref().filter(|a| a.permitted)?;
            let to = media.send_to(peer)?;
            let _ = a.udp.send_to(data, to).await;
        }
        return None;
    }
    if m.class != REQUEST {
        return None;
    }
    if m.method == BINDING {
        return Some(
            Builder::new(BINDING, SUCCESS, &m.txid)
                .address(XOR_MAPPED_ADDRESS, client, &m.txid)
                .finish(None),
        );
    }
    // Every other request is authenticated.
    if m.get(MESSAGE_INTEGRITY).is_none() || m.get(USERNAME).is_none() {
        return Some(error(m, 401, "Unauthorized", nonce, None));
    }
    if m.get(NONCE) != Some(nonce.as_bytes()) {
        return Some(error(m, 438, "Stale Nonce", nonce, None));
    }
    let Some(k) = key_for(m, secret, now_secs()) else {
        return Some(error(m, 401, "Unauthorized", nonce, None));
    };
    *key = Some(k.clone());
    let k = Some(k.as_slice());
    let lifetime = |m: &Message| {
        m.get(LIFETIME)
            .and_then(|v| v.try_into().ok())
            .map(u32::from_be_bytes)
            .unwrap_or(DEFAULT_LIFETIME)
            .min(MAX_LIFETIME)
    };
    match m.method {
        ALLOCATE => {
            if alloc.is_some() {
                return Some(error(m, 437, "Allocation Mismatch", nonce, k));
            }
            if m.get(REQUESTED_TRANSPORT).map(|v| v.first().copied()) != Some(Some(17)) {
                return Some(error(m, 442, "Unsupported Transport Protocol", nonce, k));
            }
            let bind_ip = if media.local.ip().is_unspecified() {
                IpAddr::from([127, 0, 0, 1])
            } else {
                media.local.ip()
            };
            let Ok(udp) = tokio::net::UdpSocket::bind(SocketAddr::new(bind_ip, 0)).await else {
                return Some(error(m, 508, "Insufficient Capacity", nonce, k));
            };
            let relayed = udp.local_addr().ok()?;
            let secs = lifetime(m).max(1);
            *alloc = Some(Allocation {
                udp,
                expires: Instant::now() + Duration::from_secs(u64::from(secs)),
                permitted: false,
                channel: None,
            });
            Some(
                Builder::new(ALLOCATE, SUCCESS, &m.txid)
                    .address(XOR_RELAYED_ADDRESS, relayed, &m.txid)
                    .address(XOR_MAPPED_ADDRESS, client, &m.txid)
                    .attr(LIFETIME, &secs.to_be_bytes())
                    .attr(SOFTWARE, b"Ortus")
                    .finish(k),
            )
        }
        REFRESH => {
            let Some(a) = alloc.as_mut() else {
                return Some(error(m, 437, "Allocation Mismatch", nonce, k));
            };
            let secs = lifetime(m);
            if secs == 0 {
                *alloc = None;
            } else {
                a.expires = Instant::now() + Duration::from_secs(u64::from(secs));
            }
            Some(
                Builder::new(REFRESH, SUCCESS, &m.txid)
                    .attr(LIFETIME, &secs.to_be_bytes())
                    .finish(k),
            )
        }
        CREATE_PERMISSION | CHANNEL_BIND => {
            let Some(a) = alloc.as_mut() else {
                return Some(error(m, 437, "Allocation Mismatch", nonce, k));
            };
            let peers: Vec<SocketAddr> = m
                .attrs
                .iter()
                .filter(|x| x.0 == XOR_PEER_ADDRESS)
                .filter_map(|x| read_xor_address(&x.1, &m.txid))
                .collect();
            // Only the media node: a permission for any other address (an
            // IP's) is refused, so this is never an open relay.
            if peers.is_empty()
                || peers.iter().any(|p| {
                    media.send_to(*p).is_none()
                        && p.ip() != media.public.ip()
                        && p.ip() != media.local.ip()
                })
            {
                return Some(error(m, 403, "Forbidden", nonce, k));
            }
            a.permitted = true;
            if m.method == CHANNEL_BIND {
                let ch = m
                    .get(CHANNEL_NUMBER)
                    .and_then(|v| v.get(..2))
                    .map(|v| u16::from_be_bytes([v[0], v[1]]));
                match ch {
                    Some(ch)
                        if (0x4000..0x8000).contains(&ch)
                            && peers.iter().all(|p| media.send_to(*p).is_some()) =>
                    {
                        a.channel = Some(ch)
                    }
                    _ => return Some(error(m, 400, "Bad Request", nonce, k)),
                }
            }
            Some(Builder::new(m.method, SUCCESS, &m.txid).finish(k))
        }
        _ => Some(error(m, 400, "Bad Request", nonce, k)),
    }
}

/// The allocation's relayed address, for tests.
#[doc(hidden)]
pub fn relayed_of(m: &Message) -> Option<SocketAddr> {
    m.get(XOR_RELAYED_ADDRESS)
        .and_then(|v| read_xor_address(v, &m.txid))
}
