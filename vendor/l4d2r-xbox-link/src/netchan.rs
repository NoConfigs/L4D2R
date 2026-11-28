//! L4D2-era Source NetChannel as spoken by the Xbox 360 build.
//!
//! Header (little-endian): i32 sequence, i32 ack, u8 flags (pad bits << 5 |
//! RELIABLE 1 | CHOKED 16), u16 checksum = CRC32(bytes after it) low ^ high,
//! u8 reliable state, [u8 choke], then an LSB-first bit stream:
//! reliable subchannel data (3-bit subchannel, then per stream a presence
//! bit and a single block or a 256-byte-fragment range), then unreliable
//! messages (6-bit type each), then 1-bits of padding.
//! Single-block sizes are 17 bits on the lobby channel (27027) and 18 bits on
//! the game channel (27005/27015). Verified on ~81,700 captured packets.

use crate::bits::{lzss_decompress, BitReader, BitWriter};

pub const FLAG_RELIABLE: u8 = 1;
pub const FLAG_CHOKED: u8 = 16;
pub const CONNECTIONLESS: [u8; 4] = [0xff; 4];
pub const COMPRESSED: [u8; 4] = [0xfd, 0xff, 0xff, 0xff];
/// Split packets: -2, i32 split sequence, u16 (part << 8 | count), u16 split size, data.
pub const SPLIT: [u8; 4] = [0xfe, 0xff, 0xff, 0xff];
const FRAGMENT_SIZE: usize = 256;
/// Payload size bits for the matchmaking (lobby) channel.
pub const LOBBY_PAYLOAD_BITS: usize = 17;
/// Payload size bits for the game channel.
pub const GAME_PAYLOAD_BITS: usize = 18;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

pub fn checksum(data: &[u8]) -> u16 {
    let c = crc32(data);
    ((c & 0xFFFF) ^ (c >> 16)) as u16
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub sequence: u32,
    pub ack: u32,
    pub flags: u8,
    pub reliable_state: u8,
    pub payload_offset: usize,
}

pub fn parse_header(packet: &[u8]) -> Option<Header> {
    if packet.len() < 12 || packet[..4] == CONNECTIONLESS || packet[..4] == COMPRESSED {
        return None;
    }
    if checksum(&packet[11..]) != u16::from_le_bytes([packet[9], packet[10]]) {
        return None;
    }
    let flags = packet[8];
    let offset = if flags & FLAG_CHOKED != 0 { 13 } else { 12 };
    Some(Header {
        sequence: u32::from_le_bytes(packet[0..4].try_into().unwrap()),
        ack: u32::from_le_bytes(packet[4..8].try_into().unwrap()),
        flags,
        reliable_state: packet[11],
        payload_offset: offset.min(packet.len()),
    })
}

#[derive(Debug, Default)]
struct Fragments {
    total: usize,
    compressed: bool,
    buf: Vec<u8>,
    got: usize,
}

#[derive(Debug)]
struct Pending {
    data: Vec<u8>,
    sent_seq: u32,
    need_send: bool,
    /// Large transfers go out as 256-byte fragments, a few per packet, the
    /// next chunk once the previous one is acknowledged (bytes done, in flight).
    fragmented: bool,
    offset: usize,
    chunk: usize,
}

/// Reliable transfers above this size are sent as fragments (small ones stay
/// single blocks, byte-identical to the native client).
const SINGLE_BLOCK_LIMIT: usize = 1000;
/// Fragments per packet (4 x 256 bytes keeps datagrams under the split size).
const FRAGMENTS_PER_PACKET: usize = 4;
/// Datagrams above this are sent as split packets.
const SPLIT_SIZE: usize = 1188;

/// One received NetChannel packet.
#[derive(Debug)]
pub struct Received {
    pub header: Header,
    /// Completed stream-0 reliable transfers (each a run of messages).
    pub reliable: Vec<Vec<u8>>,
    /// Packet payload; unreliable messages live in bits `unreliable.0..unreliable.1`.
    pub payload: Vec<u8>,
    pub unreliable: (usize, usize),
}

pub enum Incoming {
    Packet(Received),
    Connectionless(Vec<u8>),
    Invalid,
}

#[derive(Debug)]
pub struct Channel {
    pub payload_bits: usize,
    pub out_sequence: u32,
    pub in_sequence: u32,
    pub in_reliable_state: u8,
    pub out_reliable_state: u8,
    pub packets_received: u64,
    pub reliable_received: u64,
    /// Our reliable transfers the other side acknowledged, and resends.
    pub reliable_acked: u64,
    pub reliable_resent: u64,
    pending: Option<Pending>,
    queue: BitWriter,
    fragments: [Option<Fragments>; 2],
    splits: std::collections::HashMap<i32, Vec<Option<Vec<u8>>>>,
}

impl Channel {
    pub fn new(payload_bits: usize) -> Self {
        Channel {
            payload_bits,
            out_sequence: 0,
            in_sequence: 0,
            in_reliable_state: 0,
            out_reliable_state: 0,
            packets_received: 0,
            reliable_received: 0,
            reliable_acked: 0,
            reliable_resent: 0,
            pending: None,
            queue: BitWriter::new(),
            fragments: [None, None],
            splits: Default::default(),
        }
    }
    pub fn lobby() -> Self {
        Self::new(LOBBY_PAYLOAD_BITS)
    }
    pub fn game() -> Self {
        Self::new(GAME_PAYLOAD_BITS)
    }

    /// True while reliable data is queued or awaiting acknowledgement.
    pub fn reliable_outstanding(&self) -> bool {
        self.pending.is_some() || self.queue.bits > 0
    }

    pub fn receive(&mut self, packet: &[u8]) -> Incoming {
        if packet.starts_with(&SPLIT) {
            if packet.len() < 12 {
                return Incoming::Invalid;
            }
            let seq = i32::from_le_bytes(packet[4..8].try_into().unwrap());
            let id = u16::from_le_bytes([packet[8], packet[9]]);
            let (part, count) = ((id >> 8) as usize, (id & 0xff) as usize);
            if count == 0 || part >= count || count > 64 {
                return Incoming::Invalid;
            }
            let parts = self.splits.entry(seq).or_insert_with(|| vec![None; count]);
            if parts.len() != count {
                *parts = vec![None; count];
            }
            parts[part] = Some(packet[12..].to_vec());
            if parts.iter().all(|p| p.is_some()) {
                let whole: Vec<u8> = self.splits.remove(&seq).unwrap().into_iter().flatten().flatten().collect();
                self.splits.retain(|s, _| s.wrapping_sub(seq) > 0); // drop older incomplete sets
                return self.receive(&whole);
            }
            return Incoming::Invalid;
        }
        if packet.starts_with(&CONNECTIONLESS) {
            return Incoming::Connectionless(packet[4..].to_vec());
        }
        let owned;
        let packet = if packet.starts_with(&COMPRESSED) {
            match lzss_decompress(&packet[4..]) {
                Some(p) => {
                    owned = p;
                    &owned[..]
                }
                None => return Incoming::Invalid,
            }
        } else {
            packet
        };
        let Some(header) = parse_header(packet) else { return Incoming::Invalid };
        if header.sequence <= self.in_sequence {
            return Incoming::Invalid;
        }
        self.in_sequence = header.sequence;
        self.packets_received += 1;

        // Acknowledgement of our outstanding reliable transfer.
        if let Some(p) = &mut self.pending {
            if p.sent_seq != 0 {
                if header.reliable_state & 1 == self.out_reliable_state & 1 {
                    if p.fragmented && p.offset + p.chunk < p.data.len() {
                        // Next chunk of a fragmented transfer.
                        p.offset += p.chunk;
                        p.chunk = 0;
                        p.need_send = true;
                        p.sent_seq = 0;
                        self.out_reliable_state ^= 1;
                    } else {
                        self.pending = None;
                        self.reliable_acked += 1;
                    }
                } else if header.ack >= p.sent_seq {
                    p.need_send = true; // lost: resend
                    self.reliable_resent += 1;
                }
            }
        }

        let payload = packet[header.payload_offset..].to_vec();
        let pad = (header.flags >> 5) as usize;
        let end = (payload.len() * 8).saturating_sub(pad);
        let mut r = BitReader::range(&payload, 0, end);
        let mut reliable = Vec::new();
        if header.flags & FLAG_RELIABLE != 0 {
            match self.read_reliable(&mut r, &mut reliable) {
                Some(sub) => {
                    self.in_reliable_state ^= 1 << sub;
                    self.reliable_received += 1;
                }
                None => return Incoming::Invalid,
            }
        }
        let start = r.pos;
        Incoming::Packet(Received { header, reliable, unreliable: (start, end), payload })
    }

    fn read_reliable(&mut self, r: &mut BitReader, done: &mut Vec<Vec<u8>>) -> Option<u8> {
        let sub = r.u(3)? as u8;
        for stream in 0..2 {
            if !r.bit()? {
                continue;
            }
            if !r.bit()? {
                // Single block.
                let compressed = r.bit()?;
                if compressed {
                    r.u(26)?;
                }
                let size = r.u(self.payload_bits)? as usize;
                let mut data = r.bytes(size)?;
                if compressed {
                    data = lzss_decompress(&data)?;
                }
                if stream == 0 {
                    done.push(data);
                }
            } else {
                let start = r.u(18)? as usize;
                let count = r.u(3)? as usize;
                let offset = start * FRAGMENT_SIZE;
                if offset == 0 {
                    if r.bit()? {
                        r.u(32)?;
                        r.string(260)?;
                    }
                    let compressed = r.bit()?;
                    if compressed {
                        r.u(26)?;
                    }
                    let total = r.u(26)? as usize;
                    if total > 1 << 24 {
                        return None;
                    }
                    self.fragments[stream] = Some(Fragments { total, compressed, buf: vec![0; total], got: 0 });
                }
                let Some(f) = self.fragments[stream].as_mut() else {
                    // Missed the first fragment: cannot place this data.
                    return None;
                };
                let length = (count * FRAGMENT_SIZE).min(f.total.saturating_sub(offset));
                let chunk = r.bytes(length)?;
                f.buf[offset..offset + length].copy_from_slice(&chunk);
                f.got += length;
                if f.got >= f.total {
                    let f = self.fragments[stream].take().unwrap();
                    let data = if f.compressed { lzss_decompress(&f.buf)? } else { f.buf };
                    if stream == 0 {
                        done.push(data);
                    }
                }
            }
        }
        Some(sub)
    }

    /// Queue reliable messages (sent in order, one transfer at a time).
    pub fn queue_reliable(&mut self, messages: &BitWriter) {
        self.queue.append(messages);
    }

    /// Build the next datagram, carrying pending reliable data and `unreliable`.
    pub fn send(&mut self, unreliable: &BitWriter) -> Vec<u8> {
        if self.pending.is_none() && self.queue.bits > 0 {
            let q = std::mem::take(&mut self.queue);
            self.out_reliable_state ^= 1;
            let fragmented = q.data.len() > SINGLE_BLOCK_LIMIT;
            self.pending = Some(Pending { data: q.data, sent_seq: 0, need_send: true, fragmented, offset: 0, chunk: 0 });
        }
        self.out_sequence += 1;
        let mut flags = 0u8;
        let mut w = BitWriter::new();
        if let Some(p) = &mut self.pending {
            if p.need_send && p.fragmented {
                flags |= FLAG_RELIABLE;
                let total = p.data.len();
                let frags_left = (total - p.offset).div_ceil(FRAGMENT_SIZE);
                let count = frags_left.min(FRAGMENTS_PER_PACKET);
                let len = (count * FRAGMENT_SIZE).min(total - p.offset);
                w.u(3, 0); // subchannel 0
                w.bit(true); // stream 0 present
                w.bit(true); // fragments
                w.u(18, (p.offset / FRAGMENT_SIZE) as u64);
                w.u(3, count as u64);
                if p.offset == 0 {
                    w.bit(false); // not a file
                    w.bit(false); // not compressed
                    w.u(26, total as u64);
                }
                w.bytes(&p.data[p.offset..p.offset + len]);
                w.bit(false); // no file stream
                p.chunk = len;
                p.sent_seq = self.out_sequence;
                p.need_send = false;
            } else if p.need_send {
                assert!(p.data.len() < 1 << self.payload_bits, "reliable transfer too large");
                flags |= FLAG_RELIABLE;
                w.u(3, 0); // subchannel 0
                w.bit(true); // stream 0 present
                w.bit(false); // single block
                w.bit(false); // not compressed
                w.u(self.payload_bits, p.data.len() as u64);
                w.bytes(&p.data);
                w.bit(false); // no file stream
                p.sent_seq = self.out_sequence;
                p.need_send = false;
            }
        }
        w.append(unreliable);
        while (12 * 8 + w.bits + 7) / 8 < 16 {
            w.u(6, 0); // NOP
        }
        let pad = (8 - w.bits % 8) % 8;
        w.u(pad, (1 << pad) - 1);
        flags |= (pad as u8) << 5;
        let mut p = Vec::with_capacity(12 + w.data.len());
        p.extend_from_slice(&self.out_sequence.to_le_bytes());
        p.extend_from_slice(&self.in_sequence.to_le_bytes());
        p.push(flags);
        p.extend_from_slice(&[0, 0]);
        p.push(self.in_reliable_state);
        p.extend_from_slice(&w.data);
        let sum = checksum(&p[11..]);
        p[9..11].copy_from_slice(&sum.to_le_bytes());
        p
    }

    /// As `send`, split into SPLIT packets when the datagram is too large for
    /// one VDP packet (-2, i32 split sequence, u16 part << 8 | count, u16 size, data).
    pub fn send_split(&mut self, unreliable: &BitWriter) -> Vec<Vec<u8>> {
        let p = self.send(unreliable);
        if p.len() <= SPLIT_SIZE {
            return vec![p];
        }
        let parts: Vec<&[u8]> = p.chunks(SPLIT_SIZE).collect();
        let seq = self.out_sequence as i32;
        parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let mut v = SPLIT.to_vec();
                v.extend_from_slice(&seq.to_le_bytes());
                v.extend_from_slice(&(((i as u16) << 8) | parts.len() as u16).to_le_bytes());
                v.extend_from_slice(&(SPLIT_SIZE as u16).to_le_bytes());
                v.extend_from_slice(part);
                v
            })
            .collect()
    }

    /// An idle datagram (NOPs only), byte-identical to the native client's.
    pub fn heartbeat(&mut self) -> Vec<u8> {
        self.send(&BitWriter::new())
    }
}

/// Printable runs of 4+ chars at any bit alignment. Diagnostic only.
pub fn bit_strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for shift in 0..8 {
        let mut run = String::new();
        for i in 0..data.len() {
            let b = ((data[i] as u16 >> shift) | ((*data.get(i + 1).unwrap_or(&0) as u16) << (8 - shift))) as u8;
            if (0x20..0x7f).contains(&b) {
                run.push(b as char);
            } else {
                if run.len() >= 4 {
                    out.push(std::mem::take(&mut run));
                }
                run.clear();
            }
        }
        if run.len() >= 4 {
            out.push(run);
        }
    }
    out
}
