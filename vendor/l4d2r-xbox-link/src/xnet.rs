//! XNet System Link wire codecs, ported from tools/xbox360_net (validated on a
//! real Xbox 360 on 2026-11-27): discovery broadcasts, the DH key exchange,
//! protected control records and protected UDP/VDP datagrams, plus the
//! 0.0.0.1 Ethernet/IPv4/UDP 3074 framing.
//!
//! Keys are always supplied by the caller (derived from the user's own game
//! executable); none are embedded here.

use crate::crypto::{dh_public, dh_shared, equal, hmac_sha1, Block};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Layout(&'static str),
    Auth(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Authentication key, 3DES/DES cipher key and IV seed for one direction
/// (or for discovery broadcasts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub auth: [u8; 16],
    pub cipher: [u8; 24],
    pub iv_seed: [u8; 8],
}

fn arr<const N: usize>(s: &[u8]) -> [u8; N] {
    s[..N].try_into().unwrap()
}

/// Broadcast keys from the title's LAN key (XEX optional header 0x40404) and
/// the platform key.
pub fn broadcast_keys(lan_key: &[u8; 16], platform_key: &[u8; 16]) -> Keys {
    let mut stream = Vec::new();
    for i in 0..3u8 {
        let mut msg = vec![i];
        msg.extend_from_slice(lan_key);
        stream.extend_from_slice(&hmac_sha1(platform_key, &msg));
    }
    Keys { auth: arr(&stream[..16]), cipher: arr(&stream[16..40]), iv_seed: arr(&stream[40..48]) }
}

/// Read the title LAN key from a XEX2 image.
pub fn xex_lan_key(xex: &[u8]) -> Result<[u8; 16]> {
    if xex.len() < 24 || &xex[..4] != b"XEX2" {
        return Err(Error::Layout("not a XEX2 executable"));
    }
    let count = u32::from_be_bytes(arr(&xex[20..24])) as usize;
    if count > 4096 || 24 + count * 8 > xex.len() {
        return Err(Error::Layout("bad XEX optional header table"));
    }
    let mut found = None;
    for i in 0..count {
        let at = 24 + i * 8;
        let kind = u32::from_be_bytes(arr(&xex[at..at + 4]));
        let off = u32::from_be_bytes(arr(&xex[at + 4..at + 8])) as usize;
        if kind == 0x40404 {
            if off + 16 > xex.len() || found.is_some() {
                return Err(Error::Layout("bad or duplicate LAN key header"));
            }
            found = Some(arr(&xex[off..off + 16]));
        }
    }
    found.ok_or(Error::Layout("XEX has no LAN key"))
}

pub fn iv(seed: &[u8; 8], sequence: u32) -> [u8; 8] {
    let a = u32::from_le_bytes(arr(&seed[..4])) as u64 * sequence as u64;
    let b = u32::from_le_bytes(arr(&seed[4..])) as u64 * sequence as u64;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(((a & 0xFFFF_FFFF) ^ (b >> 32)) as u32).to_le_bytes());
    out[4..].copy_from_slice(&(((b & 0xFFFF_FFFF) ^ (a >> 32)) as u32).to_le_bytes());
    out
}

fn tag(key: &[u8; 16], descriptor: &[u8], ciphertext: &[u8], tail: &[u8], sequence: u32) -> [u8; 10] {
    let mut msg = tail.to_vec();
    msg.extend_from_slice(&sequence.to_le_bytes());
    msg.extend_from_slice(descriptor);
    while msg.len() % 4 != 0 {
        msg.push(0);
    }
    msg.extend_from_slice(ciphertext);
    arr(&hmac_sha1(key, &msg))
}

/// Port encoding shared by broadcasts and data packets.
fn port_header(source: u16, destination: u16) -> (u8, Vec<u8>) {
    if source == 1000 && destination == 1000 {
        (0, vec![])
    } else if (1000..=1255).contains(&source) && (1000..=1255).contains(&destination) {
        (1, vec![(source - 1000) as u8, (destination - 1000) as u8])
    } else {
        let mut h = source.to_be_bytes().to_vec();
        h.extend_from_slice(&destination.to_be_bytes());
        (2, h)
    }
}

fn ports_from(encoding: u8, header: &[u8]) -> (u16, u16) {
    match encoding {
        0 => (1000, 1000),
        1 => (1000 + header[0] as u16, 1000 + header[1] as u16),
        _ => (u16::from_be_bytes([header[0], header[1]]), u16::from_be_bytes([header[2], header[3]])),
    }
}

// ------------------------------------------------------------ broadcast ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broadcast {
    pub sequence: u16,
    pub source_port: u16,
    pub destination_port: u16,
    pub body: Vec<u8>,
    /// Title id, version and XNet version (12 bytes, little-endian u32s).
    pub metadata: [u8; 12],
}

pub fn encode_broadcast(keys: &Keys, b: &Broadcast) -> Vec<u8> {
    let (encoding, header) = port_header(b.source_port, b.destination_port);
    let reserved = (8 - b.body.len() % 8) % 8;
    let embedded = reserved.min(header.len());
    let mut plain = b.body.clone();
    plain.resize(b.body.len() + reserved - embedded, 0);
    plain.extend_from_slice(&header[..embedded]);
    let full = b.sequence as u32 | ((b.sequence as u32) << 16);
    let ct = Block::new(&keys.cipher).cbc_encrypt(iv(&keys.iv_seed, full), &plain);
    let mut tail = header[embedded..].to_vec();
    tail.extend_from_slice(&b.metadata);
    tail.push((ct.len() / 8) as u8);
    let descriptor = [7 + encoding | (reserved as u8) << 5, 0, 0, 0];
    let mut wire = descriptor.to_vec();
    wire.extend_from_slice(&ct);
    wire.extend_from_slice(&tail);
    wire.extend_from_slice(&b.sequence.to_le_bytes());
    wire.extend_from_slice(&tag(&keys.auth, &descriptor, &ct, &tail, full));
    wire
}

pub fn decode_broadcast(keys: &Keys, wire: &[u8]) -> Result<Broadcast> {
    if wire.len() < 29 || wire[1..4] != [0, 0, 0] {
        return Err(Error::Layout("not a discovery broadcast"));
    }
    let kind = wire[0] & 15;
    let reserved = (wire[0] >> 5) as usize;
    if !(7..=9).contains(&kind) || wire[0] & 16 != 0 {
        return Err(Error::Layout("not a discovery broadcast"));
    }
    let encoding = (kind - 1) % 3;
    let ports = 2 * encoding as usize;
    let blocks = wire[wire.len() - 13] as usize;
    if wire.len() != 4 + blocks * 8 + ports - reserved.min(ports) + 25 {
        return Err(Error::Layout("discovery length mismatch"));
    }
    let size = blocks * 8;
    if size < reserved {
        return Err(Error::Layout("ciphertext smaller than padding"));
    }
    let sequence = u16::from_le_bytes([wire[wire.len() - 12], wire[wire.len() - 11]]);
    let full = sequence as u32 | ((sequence as u32) << 16);
    let ct = &wire[4..4 + size];
    let tail = &wire[4 + size..wire.len() - 12];
    if !equal(&tag(&keys.auth, &wire[..4], ct, tail, full), &wire[wire.len() - 10..]) {
        return Err(Error::Auth("discovery authentication failed"));
    }
    let plain = Block::new(&keys.cipher).cbc_decrypt(iv(&keys.iv_seed, full), ct);
    let embedded = reserved.min(ports);
    let body_size = plain.len() - reserved;
    if plain[body_size..plain.len() - embedded].iter().any(|&x| x != 0) {
        return Err(Error::Layout("nonzero discovery padding"));
    }
    let mut header = plain[plain.len() - embedded..].to_vec();
    header.extend_from_slice(&tail[..ports - embedded]);
    let (source_port, destination_port) = ports_from(encoding, &header);
    Ok(Broadcast {
        sequence,
        source_port,
        destination_port,
        body: plain[..body_size].to_vec(),
        metadata: arr(&tail[ports - embedded..tail.len() - 1]),
    })
}

// ------------------------------------------------------- key exchange ---

pub fn registration_keys(broadcast_auth: &[u8; 16], session_id: &[u8; 8], session_key: &[u8; 16]) -> (
    [u8; 16],
    [u8; 24],
) {
    let mut a = session_id.to_vec();
    a.extend_from_slice(session_key);
    let mut counter = session_id.to_vec();
    counter[0] = counter[0].wrapping_add(1);
    counter.extend_from_slice(session_key);
    let first = hmac_sha1(broadcast_auth, &a);
    let second = hmac_sha1(broadcast_auth, &counter);
    let mut cipher = first[16..].to_vec();
    cipher.extend_from_slice(&second);
    (arr(&first[..16]), arr(&cipher))
}

/// Directional keys: (initiator -> responder, responder -> initiator).
pub fn traffic_keys(reg_auth: &[u8; 16], shared: &[u8; 96], ic: &[u8; 8], rc: &[u8; 8]) -> (Keys, Keys) {
    let mut s = Vec::with_capacity(100);
    for i in 0..5u8 {
        let mut msg = vec![shared[0].wrapping_add(i)];
        msg.extend_from_slice(&shared[1..]);
        msg.extend_from_slice(ic);
        msg.extend_from_slice(rc);
        s.extend_from_slice(&hmac_sha1(reg_auth, &msg));
    }
    (
        Keys { auth: arr(&s[..16]), cipher: arr(&s[16..40]), iv_seed: arr(&s[80..88]) },
        Keys { auth: arr(&s[40..56]), cipher: arr(&s[56..80]), iv_seed: arr(&s[88..96]) },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub response: bool,
    pub session_id: [u8; 8],
    pub initiator_spi: u32,
    pub responder_spi: u32,
    pub initiator_challenge: [u8; 8],
    pub responder_challenge: [u8; 8],
    pub freshness: u64,
    pub address_iv: [u8; 8],
    pub initiator_address: [u8; 36],
    pub responder_address: [u8; 36],
    pub public: [u8; 96],
    pub reserved: u16,
    pub hops: u16,
}

impl Exchange {
    pub fn matches_request(&self, req: &Exchange) -> bool {
        !req.response
            && self.response
            && req.session_id == self.session_id
            && req.initiator_spi == self.initiator_spi
            && req.initiator_challenge == self.initiator_challenge
            && req.initiator_address[10..16] == self.initiator_address[10..16]
            && req.responder_address[10..16] == self.responder_address[10..16]
    }
}

pub struct Registration {
    pub auth: [u8; 16],
    pub cipher: [u8; 24],
}

pub fn encode_exchange(reg: &Registration, x: &Exchange) -> Vec<u8> {
    let mut r = vec![0u8; 252];
    r[0..2].copy_from_slice(&(if x.response { 0x0158u16 } else { 0x0058 }).to_be_bytes());
    r[2..4].copy_from_slice(&128u16.to_le_bytes());
    r[4..6].copy_from_slice(&x.reserved.to_be_bytes());
    r[6..8].copy_from_slice(&x.hops.to_le_bytes());
    r[8..16].copy_from_slice(&x.session_id);
    r[16..20].copy_from_slice(&x.initiator_spi.to_be_bytes());
    r[20..24].copy_from_slice(&x.responder_spi.to_be_bytes());
    r[24..32].copy_from_slice(&x.initiator_challenge);
    r[32..40].copy_from_slice(&x.responder_challenge);
    r[40..48].copy_from_slice(&x.freshness.to_le_bytes());
    r[48..56].copy_from_slice(&x.address_iv);
    let mut addrs = x.initiator_address.to_vec();
    addrs.extend_from_slice(&x.responder_address);
    r[56..128].copy_from_slice(&Block::new(&reg.cipher).cbc_encrypt(x.address_iv, &addrs));
    r[128..132].copy_from_slice(&[0x80, 0x58, 0x64, 0]);
    r[132..228].copy_from_slice(&x.public);
    r[228..232].copy_from_slice(&[0x81, 0x58, 0x18, 0]);
    let mac = hmac_sha1(&reg.auth, &r[..228]);
    r[232..].copy_from_slice(&mac);
    let mut wire = vec![0u8; 4];
    wire.extend_from_slice(&r);
    wire
}

pub fn is_exchange(wire: &[u8]) -> bool {
    wire.len() == 256 && wire[..4] == [0, 0, 0, 0]
}

/// Session id carried in an exchange record (to pick registration keys).
pub fn exchange_session_id(wire: &[u8]) -> Option<[u8; 8]> {
    is_exchange(wire).then(|| arr(&wire[12..20]))
}

pub fn decode_exchange(reg: &Registration, wire: &[u8]) -> Result<Exchange> {
    if !is_exchange(wire) {
        return Err(Error::Layout("not an exchange record"));
    }
    let r = &wire[4..];
    let kind = u16::from_be_bytes([r[0], r[1]]);
    if !(kind == 0x0058 || kind == 0x0158) || r[2..4] != [0x80, 0] || r[128..132] != [0x80, 0x58, 0x64, 0]
        || r[228..232] != [0x81, 0x58, 0x18, 0]
    {
        return Err(Error::Layout("bad exchange syntax"));
    }
    if !equal(&hmac_sha1(&reg.auth, &r[..228]), &r[232..]) {
        return Err(Error::Auth("exchange authentication failed"));
    }
    let addrs = Block::new(&reg.cipher).cbc_decrypt(arr(&r[48..56]), &r[56..128]);
    Ok(Exchange {
        response: kind == 0x0158,
        session_id: arr(&r[8..16]),
        initiator_spi: u32::from_be_bytes(arr(&r[16..20])),
        responder_spi: u32::from_be_bytes(arr(&r[20..24])),
        initiator_challenge: arr(&r[24..32]),
        responder_challenge: arr(&r[32..40]),
        freshness: u64::from_le_bytes(arr(&r[40..48])),
        address_iv: arr(&r[48..56]),
        initiator_address: arr(&addrs[..36]),
        responder_address: arr(&addrs[36..]),
        public: arr(&r[132..228]),
        reserved: u16::from_be_bytes([r[4], r[5]]),
        hops: u16::from_le_bytes([r[6], r[7]]),
    })
}

pub fn public_value(exponent: &[u8; 32]) -> [u8; 96] {
    dh_public(exponent)
}
pub fn shared_value(exponent: &[u8; 32], peer: &[u8]) -> Result<[u8; 96]> {
    dh_shared(exponent, peer).ok_or(Error::Auth("invalid peer DH value"))
}

// ------------------------------------------------------ protected packets ---

pub const KEEPALIVE: u16 = 0x0153;
pub const TEARDOWN: u16 = 0x0053;

fn des8(keys: &Keys) -> Block {
    Block::new(&keys.cipher[..8])
}

pub fn encode_control(keys: &Keys, spi: u32, sequence: u32, kind: u16, body: &[u8]) -> Vec<u8> {
    let mut plain = kind.to_be_bytes().to_vec();
    plain.extend_from_slice(&((body.len() + 4) as u16).to_le_bytes());
    plain.extend_from_slice(body);
    let reserved = (8 - plain.len() % 8) % 8;
    plain.resize(plain.len() + reserved, 0);
    let mut descriptor = vec![(reserved as u8) << 5];
    descriptor.extend_from_slice(&spi.to_be_bytes()[1..]);
    let ct = des8(keys).cbc_encrypt(iv(&keys.iv_seed, sequence), &plain);
    let mut wire = descriptor.clone();
    wire.extend_from_slice(&ct);
    wire.extend_from_slice(&(sequence as u16).to_le_bytes());
    wire.extend_from_slice(&tag(&keys.auth, &descriptor, &ct, &[], sequence));
    wire
}

/// Packet carried inside an association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Control record (keepalive, teardown, or unknown) with its body.
    Control { kind: u16, body: Vec<u8> },
    /// Protected datagram. `vdp` packets may carry a clear (unencrypted) part.
    Data { vdp: bool, source_port: u16, destination_port: u16, encrypted: Vec<u8>, clear: Vec<u8> },
}

/// The low 16 sequence bits of a protected packet.
pub fn sequence_low(wire: &[u8]) -> Option<u16> {
    (wire.len() >= 16).then(|| u16::from_le_bytes([wire[wire.len() - 12], wire[wire.len() - 11]]))
}

/// SPI addressed by a protected packet (the receiver's SPI).
pub fn packet_spi(wire: &[u8]) -> Option<u32> {
    (wire.len() >= 4).then(|| u32::from_be_bytes([0, wire[1], wire[2], wire[3]]))
}

pub fn encode_data(keys: &Keys, spi: u32, sequence: u32, p: &Packet) -> Vec<u8> {
    let Packet::Data { vdp, source_port, destination_port, encrypted, clear } = p else {
        panic!("encode_data needs a data packet");
    };
    let (encoding, header) = port_header(*source_port, *destination_port);
    let base = if !vdp { 4 } else if encrypted.is_empty() { 10 } else if clear.is_empty() { 13 } else { 7 };
    let kind = base + encoding;
    let reserved = (8 - encrypted.len() % 8) % 8;
    let embedded = reserved.min(header.len());
    let mut plain = encrypted.clone();
    plain.resize(encrypted.len() + reserved - embedded, 0);
    plain.extend_from_slice(&header[..embedded]);
    let ct = des8(keys).cbc_encrypt(iv(&keys.iv_seed, sequence), &plain);
    let mut tail = header[embedded..].to_vec();
    tail.extend_from_slice(clear);
    if (7..=9).contains(&kind) {
        tail.push((ct.len() / 8) as u8);
    }
    let mut descriptor = vec![kind | (reserved as u8) << 5];
    descriptor.extend_from_slice(&spi.to_be_bytes()[1..]);
    let mut wire = descriptor.clone();
    wire.extend_from_slice(&ct);
    wire.extend_from_slice(&tail);
    wire.extend_from_slice(&(sequence as u16).to_le_bytes());
    wire.extend_from_slice(&tag(&keys.auth, &descriptor, &ct, &tail, sequence));
    wire
}

pub fn encode_packet(keys: &Keys, spi: u32, sequence: u32, p: &Packet) -> Vec<u8> {
    match p {
        Packet::Control { kind, body } => encode_control(keys, spi, sequence, *kind, body),
        Packet::Data { .. } => encode_data(keys, spi, sequence, p),
    }
}

/// Decode a protected packet with a fully reconstructed sequence number.
pub fn decode_packet(keys: &Keys, spi: u32, sequence: u32, wire: &[u8]) -> Result<Packet> {
    if wire.len() < 16 || wire[1..4] != spi.to_be_bytes()[1..] || sequence_low(wire) != Some(sequence as u16) {
        return Err(Error::Layout("bad protected envelope"));
    }
    let kind = wire[0] & 15;
    let reserved = (wire[0] >> 5) as usize;
    if wire[0] & 16 != 0 {
        return Err(Error::Layout("unsupported padding flag"));
    }
    if kind == 0 {
        // Control record: whole body encrypted, no tail.
        if (wire.len() - 16) % 8 != 0 {
            return Err(Error::Layout("unsupported control layout"));
        }
        let ct = &wire[4..wire.len() - 12];
        if !equal(&tag(&keys.auth, &wire[..4], ct, &[], sequence), &wire[wire.len() - 10..]) {
            return Err(Error::Auth("control authentication failed"));
        }
        let plain = des8(keys).cbc_decrypt(iv(&keys.iv_seed, sequence), ct);
        let plain = &plain[..plain.len() - reserved.min(plain.len())];
        if plain.len() < 4 || u16::from_le_bytes([plain[2], plain[3]]) as usize != plain.len() {
            return Err(Error::Layout("bad control length"));
        }
        return Ok(Packet::Control { kind: u16::from_be_bytes([plain[0], plain[1]]), body: plain[4..].to_vec() });
    }
    if !(4..=15).contains(&kind) {
        return Err(Error::Layout("unsupported data kind"));
    }
    let encoding = (kind - 1) % 3;
    let header_size = 2 * encoding as usize;
    let embedded = reserved.min(header_size);
    let clear_header = header_size - embedded;
    let counted = (7..=9).contains(&kind);
    if wire.len() < 16 + clear_header + counted as usize {
        return Err(Error::Layout("truncated port header"));
    }
    let cipher_size = if counted {
        wire[wire.len() - 13] as usize * 8
    } else if (10..=12).contains(&kind) {
        if reserved != 0 {
            return Err(Error::Layout("clear VDP with padding"));
        }
        0
    } else {
        (wire.len() - 16 - clear_header) & !7
    };
    if cipher_size < reserved || cipher_size > wire.len() - 16 {
        return Err(Error::Layout("bad ciphertext length"));
    }
    let ct = &wire[4..4 + cipher_size];
    let tail = &wire[4 + cipher_size..wire.len() - 12];
    if tail.len() < clear_header + counted as usize {
        return Err(Error::Layout("truncated tail"));
    }
    if !equal(&tag(&keys.auth, &wire[..4], ct, tail, sequence), &wire[wire.len() - 10..]) {
        return Err(Error::Auth("datagram authentication failed"));
    }
    let plain = des8(keys).cbc_decrypt(iv(&keys.iv_seed, sequence), ct);
    let body_size = plain.len() - reserved;
    if plain[body_size..plain.len() - embedded].iter().any(|&x| x != 0) {
        return Err(Error::Layout("nonzero padding"));
    }
    let mut header = plain[plain.len() - embedded..].to_vec();
    header.extend_from_slice(&tail[..clear_header]);
    let (source_port, destination_port) = ports_from(encoding, &header);
    let clear = tail[clear_header..tail.len() - counted as usize].to_vec();
    if (kind <= 6 || kind >= 13) && !clear.is_empty() {
        return Err(Error::Layout("unexpected clear body"));
    }
    Ok(Packet::Data { vdp: kind > 6, source_port, destination_port, encrypted: plain[..body_size].to_vec(), clear })
}

// ------------------------------------------------------- dh helpers glue ---

pub fn random_exponent(rng: &mut impl FnMut() -> u64) -> [u8; 32] {
    let mut e = [0u8; 32];
    for chunk in e.chunks_mut(8) {
        chunk.copy_from_slice(&rng().to_be_bytes());
    }
    e[0] |= 0x80;
    e
}

// --------------------------------------------------------- Ethernet I/O ---

pub const BROADCAST_MAC: [u8; 6] = [0xff; 6];
const SYSTEM_LINK_IP: [u8; 4] = [0, 0, 0, 1];

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// A System Link frame: 0.0.0.1 -> 0.0.0.1 (unicast) or 255.255.255.255
/// (broadcast), UDP 3074 -> 3074, IPv4 id 0, TTL 128.
pub fn ethernet_frame(payload: &[u8], source: [u8; 6], destination: [u8; 6]) -> Vec<u8> {
    let dst_ip = if destination == BROADCAST_MAC { [255; 4] } else { SYSTEM_LINK_IP };
    let udp_len = 8 + payload.len();
    let mut ip = vec![0x45, 0];
    ip.extend_from_slice(&((20 + udp_len) as u16).to_be_bytes());
    ip.extend_from_slice(&[0, 0, 0, 0, 128, 17, 0, 0]);
    ip.extend_from_slice(&SYSTEM_LINK_IP);
    ip.extend_from_slice(&dst_ip);
    let c = checksum(&ip);
    ip[10..12].copy_from_slice(&c.to_be_bytes());
    let mut udp = 3074u16.to_be_bytes().to_vec();
    udp.extend_from_slice(&3074u16.to_be_bytes());
    udp.extend_from_slice(&(udp_len as u16).to_be_bytes());
    udp.extend_from_slice(&[0, 0]);
    udp.extend_from_slice(payload);
    let mut pseudo = SYSTEM_LINK_IP.to_vec();
    pseudo.extend_from_slice(&dst_ip);
    pseudo.extend_from_slice(&[0, 17]);
    pseudo.extend_from_slice(&(udp_len as u16).to_be_bytes());
    pseudo.extend_from_slice(&udp);
    let mut uc = checksum(&pseudo);
    if uc == 0 {
        uc = 0xFFFF;
    }
    udp[6..8].copy_from_slice(&uc.to_be_bytes());
    let mut frame = destination.to_vec();
    frame.extend_from_slice(&source);
    frame.extend_from_slice(&[0x08, 0x00]);
    frame.extend_from_slice(&ip);
    frame.extend_from_slice(&udp);
    frame
}

/// A received System Link frame (UDP 3074 -> 3074 over IPv4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub payload: &'a [u8],
}

pub fn parse_frame(frame: &[u8]) -> Option<Frame<'_>> {
    let mut kind = u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]);
    let mut off = 14;
    while kind == 0x8100 || kind == 0x88A8 {
        kind = u16::from_be_bytes([*frame.get(off + 2)?, *frame.get(off + 3)?]);
        off += 4;
    }
    let ip = frame.get(off..)?;
    if kind != 0x0800 || ip.len() < 20 || ip[0] >> 4 != 4 || ip[9] != 17 {
        return None;
    }
    let ihl = (ip[0] & 15) as usize * 4;
    let total = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if ihl < 20 || total > ip.len() || total < ihl + 8 || u16::from_be_bytes([ip[6], ip[7]]) & 0x3FFF != 0 {
        return None;
    }
    let udp = &ip[ihl..total];
    let (sport, dport) = (u16::from_be_bytes([udp[0], udp[1]]), u16::from_be_bytes([udp[2], udp[3]]));
    let len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if sport != 3074 || dport != 3074 || len < 8 || len > udp.len() {
        return None;
    }
    Some(Frame { destination: arr(&frame[..6]), source: arr(&frame[6..12]), payload: &udp[8..len] })
}
