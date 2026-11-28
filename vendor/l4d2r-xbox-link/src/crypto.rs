//! Small self-contained primitives XNet needs: SHA-1, HMAC-SHA1, DES / 3DES
//! (EDE, CBC) and the 768-bit Oakley group 1 Diffie-Hellman.
//! Performance is irrelevant at System Link packet rates; clarity wins.

use num_bigint::BigUint;

// ---------------------------------------------------------------- SHA-1 ---

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(data);
    let mut outer = Vec::with_capacity(84);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// Constant-time-ish comparison (good enough for a LAN game transport).
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------------ DES ---

const IP: [u8; 64] = [58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61, 53, 45, 37, 29,
    21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7];
const FP: [u8; 64] = [40, 8, 48, 16, 56, 24, 64, 32, 39, 7, 47, 15, 55, 23, 63, 31, 38, 6, 46, 14, 54, 22, 62, 30,
    37, 5, 45, 13, 53, 21, 61, 29, 36, 4, 44, 12, 52, 20, 60, 28, 35, 3, 43, 11, 51, 19, 59, 27, 34, 2, 42, 10, 50,
    18, 58, 26, 33, 1, 41, 9, 49, 17, 57, 25];
const E: [u8; 48] = [32, 1, 2, 3, 4, 5, 4, 5, 6, 7, 8, 9, 8, 9, 10, 11, 12, 13, 12, 13, 14, 15, 16, 17, 16, 17, 18,
    19, 20, 21, 20, 21, 22, 23, 24, 25, 24, 25, 26, 27, 28, 29, 28, 29, 30, 31, 32, 1];
const P: [u8; 32] = [16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25];
const PC1: [u8; 56] = [57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3,
    60, 52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21, 13, 5, 28,
    20, 12, 4];
const PC2: [u8; 48] = [14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41,
    52, 31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32];
const SHIFTS: [u32; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];
const S: [[u8; 64]; 8] = [
    [14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12, 11, 9, 5, 3, 8, 4, 1,
        14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9, 1, 7, 5, 11, 3, 14, 10, 0, 6, 13],
    [15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1, 10, 6, 9, 11, 5, 0,
        14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15, 4, 2, 11, 6, 7, 12, 0, 5, 14, 9],
    [10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5, 14, 12, 11, 15, 1, 13,
        6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6, 9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12],
    [7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2, 12, 1, 10, 14, 9, 10,
        6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1, 13, 8, 9, 4, 5, 11, 12, 7, 2, 14],
    [2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15, 10, 3, 9, 8, 6, 4,
        2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14, 2, 13, 6, 15, 0, 9, 10, 4, 5, 3],
    [12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13, 14, 0, 11, 3, 8, 9,
        14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5, 15, 10, 11, 14, 1, 7, 6, 0, 8, 13],
    [4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5, 12, 2, 15, 8, 6, 1,
        4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4, 10, 7, 9, 5, 0, 15, 14, 2, 3, 12],
    [13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6, 11, 0, 14, 9, 2, 7,
        11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10, 8, 13, 15, 12, 9, 0, 3, 5, 6, 11],
];

/// Permute `input` (an `width`-bit value, bit 1 = MSB) by a 1-indexed table.
fn permute(input: u64, width: u32, table: &[u8]) -> u64 {
    table.iter().fold(0u64, |acc, &pos| (acc << 1) | ((input >> (width - pos as u32)) & 1))
}

#[derive(Clone)]
pub struct Des {
    subkeys: [u64; 16],
}

impl Des {
    pub fn new(key: &[u8]) -> Self {
        let key = u64::from_be_bytes(key[..8].try_into().unwrap());
        let cd = permute(key, 64, &PC1);
        let (mut c, mut d) = ((cd >> 28) as u32 & 0x0FFF_FFFF, cd as u32 & 0x0FFF_FFFF);
        let mut subkeys = [0u64; 16];
        for (i, shift) in SHIFTS.iter().enumerate() {
            c = ((c << shift) | (c >> (28 - shift))) & 0x0FFF_FFFF;
            d = ((d << shift) | (d >> (28 - shift))) & 0x0FFF_FFFF;
            subkeys[i] = permute(((c as u64) << 28) | d as u64, 56, &PC2);
        }
        Des { subkeys }
    }

    fn feistel(r: u32, k: u64) -> u32 {
        let x = permute(r as u64, 32, &E) ^ k;
        let mut out = 0u32;
        for (i, sbox) in S.iter().enumerate() {
            let six = ((x >> (42 - 6 * i)) & 0x3F) as usize;
            let row = ((six & 0x20) >> 4) | (six & 1);
            let col = (six >> 1) & 0xF;
            out = (out << 4) | sbox[row * 16 + col] as u32;
        }
        permute(out as u64, 32, &P) as u32
    }

    fn crypt(&self, block: u64, decrypt: bool) -> u64 {
        let ip = permute(block, 64, &IP);
        let (mut l, mut r) = ((ip >> 32) as u32, ip as u32);
        for i in 0..16 {
            let k = if decrypt { self.subkeys[15 - i] } else { self.subkeys[i] };
            let next = l ^ Self::feistel(r, k);
            l = r;
            r = next;
        }
        permute(((r as u64) << 32) | l as u64, 64, &FP)
    }

    pub fn encrypt_block(&self, b: u64) -> u64 {
        self.crypt(b, false)
    }
    pub fn decrypt_block(&self, b: u64) -> u64 {
        self.crypt(b, true)
    }
}

/// DES (8-byte key) or 3DES-EDE (24-byte key) block cipher.
#[derive(Clone)]
pub enum Block {
    Single(Des),
    Triple(Des, Des, Des),
}

impl Block {
    pub fn new(key: &[u8]) -> Self {
        match key.len() {
            8 => Block::Single(Des::new(key)),
            24 => Block::Triple(Des::new(&key[..8]), Des::new(&key[8..16]), Des::new(&key[16..24])),
            n => panic!("unsupported DES key length {n}"),
        }
    }
    fn enc(&self, b: u64) -> u64 {
        match self {
            Block::Single(d) => d.encrypt_block(b),
            Block::Triple(a, b2, c) => c.encrypt_block(b2.decrypt_block(a.encrypt_block(b))),
        }
    }
    fn dec(&self, b: u64) -> u64 {
        match self {
            Block::Single(d) => d.decrypt_block(b),
            Block::Triple(a, b2, c) => a.decrypt_block(b2.encrypt_block(c.decrypt_block(b))),
        }
    }

    /// CBC encrypt; `data` must be a multiple of 8 bytes.
    pub fn cbc_encrypt(&self, iv: [u8; 8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() % 8 == 0, "CBC input must be block aligned");
        let mut prev = u64::from_be_bytes(iv);
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks(8) {
            prev = self.enc(u64::from_be_bytes(chunk.try_into().unwrap()) ^ prev);
            out.extend_from_slice(&prev.to_be_bytes());
        }
        out
    }

    pub fn cbc_decrypt(&self, iv: [u8; 8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() % 8 == 0, "CBC input must be block aligned");
        let mut prev = u64::from_be_bytes(iv);
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks(8) {
            let c = u64::from_be_bytes(chunk.try_into().unwrap());
            out.extend_from_slice(&(self.dec(c) ^ prev).to_be_bytes());
            prev = c;
        }
        out
    }
}

// ------------------------------------------------------- Diffie-Hellman ---

const PRIME_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A63A3620FFFFFFFFFFFFFFFF";

fn prime() -> BigUint {
    BigUint::parse_bytes(PRIME_HEX.as_bytes(), 16).unwrap()
}

fn to_le96(v: &BigUint) -> [u8; 96] {
    let mut out = [0u8; 96];
    let bytes = v.to_bytes_le();
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

/// g^exponent mod p as XNet's 96-byte little-endian public value.
pub fn dh_public(exponent: &[u8; 32]) -> [u8; 96] {
    let e = BigUint::from_bytes_be(exponent);
    to_le96(&BigUint::from(2u32).modpow(&e, &prime()))
}

/// Shared secret with peer validation (range + order-q subgroup).
pub fn dh_shared(exponent: &[u8; 32], peer: &[u8]) -> Option<[u8; 96]> {
    if peer.len() != 96 {
        return None;
    }
    let p = prime();
    let v = BigUint::from_bytes_le(peer);
    let two = BigUint::from(2u32);
    if v < two || v > &p - &two || v.modpow(&((&p - 1u32) >> 1), &p) != BigUint::from(1u32) {
        return None;
    }
    Some(to_le96(&v.modpow(&BigUint::from_bytes_be(exponent), &p)))
}
