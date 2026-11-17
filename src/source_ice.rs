//! Thin-ICE decoding of locally owned NUC scripts. Algorithm: Matthew Kwan's public-domain ICE.
//! Retail loader/key verified in server.dll FUN_10221e10/FUN_1020cf90; final partial block stays plaintext.
use std::sync::OnceLock;
fn mult(mut a: u32, mut b: u32, m: u32) -> u32 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a <<= 1;
        b >>= 1;
        if a >= 256 {
            a ^= m;
        }
    }
    r
}
fn exp(b: u32, m: u32) -> u32 {
    let x = mult(b, b, m);
    let x = mult(b, x, m);
    mult(b, mult(x, x, m), m)
}
fn boxes() -> &'static [[u32; 1024]; 4] {
    static BOX: OnceLock<[[u32; 1024]; 4]> = OnceLock::new();
    BOX.get_or_init(|| {
        let mods = [
            [333, 313, 505, 369],
            [379, 375, 319, 391],
            [361, 445, 451, 397],
            [397, 425, 395, 505],
        ];
        let xor = [
            [0x83, 0x85, 0x9b, 0xcd],
            [0xcc, 0xa7, 0xad, 0x41],
            [0x4b, 0x2e, 0xd4, 0x33],
            [0xea, 0xcb, 0x2e, 4],
        ];
        let perm = [
            1, 0x80, 0x400, 0x2000, 0x80000, 0x200000, 0x1000000, 0x40000000, 8, 0x20, 0x100,
            0x4000, 0x10000, 0x800000, 0x4000000, 0x20000000, 4, 0x10, 0x200, 0x8000, 0x20000,
            0x400000, 0x8000000, 0x10000000, 2, 0x40, 0x800, 0x1000, 0x40000, 0x100000, 0x2000000,
            0x80000000,
        ];
        let mut out = [[0; 1024]; 4];
        for i in 0..1024 {
            let col = (i >> 1) & 255;
            let row = (i & 1) | ((i & 512) >> 8);
            for k in 0..4 {
                let x = exp(col as u32 ^ xor[k][row], mods[k][row]) << (24 - k * 8);
                for (b, p) in perm.iter().enumerate() {
                    if x & (1 << b) != 0 {
                        out[k][i] |= *p;
                    }
                }
            }
        }
        out
    })
}
fn schedule(key: &[u8; 8]) -> [[u32; 3]; 8] {
    let mut kb = [0u16; 4];
    for i in 0..4 {
        kb[3 - i] = u16::from_be_bytes([key[i * 2], key[i * 2 + 1]]);
    }
    let mut s = [[0; 3]; 8];
    for (i, kr) in [0, 1, 2, 3, 2, 1, 3, 0].into_iter().enumerate() {
        for j in 0..15 {
            for k in 0..4 {
                let v = &mut kb[(kr + k) & 3];
                let bit = *v & 1;
                s[i][j % 3] = (s[i][j % 3] << 1) | bit as u32;
                *v = (*v >> 1) | ((bit ^ 1) << 15);
            }
        }
    }
    s
}
fn f(p: u32, s: [u32; 3]) -> u32 {
    let tl = ((p >> 16) & 1023) | (((p >> 14) | (p << 18)) & 0xffc00);
    let tr = (p & 1023) | ((p << 2) & 0xffc00);
    let salt = s[2] & (tl ^ tr);
    let a = (salt ^ tl) ^ s[0];
    let b = (salt ^ tr) ^ s[1];
    let bx = boxes();
    bx[0][(a >> 10) as usize]
        | bx[1][(a & 1023) as usize]
        | bx[2][(b >> 10) as usize]
        | bx[3][(b & 1023) as usize]
}
pub fn decode(data: &[u8]) -> Vec<u8> {
    let s = schedule(b"SDhfi878");
    let mut out = data.to_vec();
    for block in out.chunks_exact_mut(8) {
        let mut l = u32::from_be_bytes(block[..4].try_into().unwrap());
        let mut r = u32::from_be_bytes(block[4..].try_into().unwrap());
        for i in (1..8).step_by(2).rev() {
            l ^= f(r, s[i]);
            r ^= f(l, s[i - 1]);
        }
        block[..4].copy_from_slice(&r.to_be_bytes());
        block[4..].copy_from_slice(&l.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn owned_quiet_script_block_and_plaintext_tail() {
        assert_eq!(
            super::decode(&[98, 193, 67, 205, 142, 31, 253, 228, 1, 2, 3]),
            b"Msg(\"Qui\x01\x02\x03"
        );
    }
}
