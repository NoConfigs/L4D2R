//! LSB-first bit streams (Source `bf_read` / `bf_write`) and Source LZSS.

#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pub pos: usize,
    end: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, end: data.len() * 8 }
    }
    /// Reader over `[start, end)` bits of `data`.
    pub fn range(data: &'a [u8], start: usize, end: usize) -> Self {
        BitReader { data, pos: start, end: end.min(data.len() * 8) }
    }
    pub fn left(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }
    pub fn end(&self) -> usize {
        self.end
    }
    pub fn bit(&mut self) -> Option<bool> {
        Some(self.u(1)? == 1)
    }
    /// Read `n` (<= 64) bits as an unsigned integer.
    pub fn u(&mut self, n: usize) -> Option<u64> {
        if n > 64 || self.left() < n {
            return None;
        }
        let mut v = 0u64;
        for i in 0..n {
            let p = self.pos + i;
            if self.data[p / 8] >> (p % 8) & 1 == 1 {
                v |= 1 << i;
            }
        }
        self.pos += n;
        Some(v)
    }
    pub fn u32(&mut self, n: usize) -> Option<u32> {
        self.u(n).map(|v| v as u32)
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        (self.left() >= n).then(|| self.pos += n)
    }
    pub fn bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.left() < n * 8 {
            return None;
        }
        if self.pos % 8 == 0 {
            let at = self.pos / 8;
            self.pos += n * 8;
            return Some(self.data[at..at + n].to_vec());
        }
        (0..n).map(|_| self.u(8).map(|b| b as u8)).collect()
    }
    /// Copy the next `n` bits into a new LSB-first buffer (exact length kept
    /// by the caller; trailing bits of the last byte are zero).
    pub fn bit_vec(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.left() < n {
            return None;
        }
        let mut w = BitWriter::new();
        let mut left = n;
        while left > 0 {
            let k = left.min(32);
            w.u(k, self.u(k)?);
            left -= k;
        }
        Some(w.data)
    }
    /// NUL-terminated string (at most `max` bytes before the NUL).
    pub fn string(&mut self, max: usize) -> Option<String> {
        let mut out = Vec::new();
        loop {
            let c = self.u(8)? as u8;
            if c == 0 {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            if out.len() >= max {
                return None;
            }
            out.push(c);
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct BitWriter {
    pub data: Vec<u8>,
    pub bits: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u(&mut self, n: usize, v: u64) {
        for i in 0..n {
            if self.bits % 8 == 0 {
                self.data.push(0);
            }
            if v >> i & 1 == 1 {
                let last = self.data.len() - 1;
                self.data[last] |= 1 << (self.bits % 8);
            }
            self.bits += 1;
        }
    }
    pub fn bit(&mut self, b: bool) {
        self.u(1, b as u64);
    }
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.u(8, x as u64);
        }
    }
    pub fn string(&mut self, s: &str) {
        self.bytes(s.as_bytes());
        self.u(8, 0);
    }
    /// Append another writer's bits.
    pub fn append(&mut self, other: &BitWriter) {
        let mut r = BitReader::range(&other.data, 0, other.bits);
        while r.left() > 0 {
            let n = r.left().min(32);
            let v = r.u(n).unwrap();
            self.u(n, v);
        }
    }
}

/// Source LZSS: "LZSS", u32 LE size, then 8-command-bit groups.
pub fn lzss_decompress(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 8 || &data[..4] != b"LZSS" {
        return None;
    }
    let size = u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    if size > 1 << 24 {
        return None;
    }
    let mut out = Vec::with_capacity(size);
    let (mut i, mut cmd, mut bit) = (8usize, 0u8, 0u8);
    loop {
        if bit == 0 {
            cmd = *data.get(i)?;
            i += 1;
        }
        bit = (bit + 1) & 7;
        if cmd & 1 == 1 {
            let (a, b) = (*data.get(i)? as usize, *data.get(i + 1)? as usize);
            i += 2;
            let pos = (a << 4) | (b >> 4);
            let count = (b & 15) + 1;
            if count == 1 {
                break;
            }
            let src = out.len().checked_sub(pos + 1)?;
            for k in 0..count {
                let byte = out[src + k];
                out.push(byte);
            }
        } else {
            out.push(*data.get(i)?);
            i += 1;
        }
        cmd >>= 1;
        if out.len() > size {
            return None;
        }
    }
    (out.len() == size).then_some(out)
}
