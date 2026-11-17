//! Valve's `CUniformRandomStream` (vstdlib) and the `SharedRandom*` helpers the server uses for predicted randomness
//! (`FUN_101bcad0` in server.dll: CRC32 over `[prediction seed][additional seed][name]`, then `SetSeed` + `RandomFloat`).
//! The stream algorithm is the Numerical-Recipes `ran1` variant shipped in vstdlib; verified function by function against
//! vstdlib.dll (`SetSeed` 0x1000c450, `GenerateRandomNumber` 0xc4c0, `RandomFloat` 0xc720, `RandomInt` 0xc7d0): docs/re/random.md.

const NTAB: usize = 32;
const IA: i32 = 16807;
const IM: i32 = 2147483647;
const IQ: i32 = 127773;
const IR: i32 = 2836;
const NDIV: i32 = 1 + (IM - 1) / NTAB as i32;
const MAX_RANDOM_RANGE: u32 = 0x7FFF_FFFF;
/// The literal vstdlib uses (`4.656613e-10`), which differs in the last bits from `1.0 / IM`.
const AM: f32 = 4.656613e-10;
const EPS: f32 = 1.2e-7;
const RNMX: f32 = 1.0 - EPS;

#[derive(Clone, Debug)]
pub struct UniformRandomStream {
    idum: i32,
    iy: i32,
    iv: [i32; NTAB],
}

impl Default for UniformRandomStream {
    fn default() -> Self {
        let mut s = UniformRandomStream {
            idum: 0,
            iy: 0,
            iv: [0; NTAB],
        };
        s.set_seed(0);
        s
    }
}

impl UniformRandomStream {
    pub fn new(seed: i32) -> Self {
        let mut s = UniformRandomStream {
            idum: 0,
            iy: 0,
            iv: [0; NTAB],
        };
        s.set_seed(seed);
        s
    }

    pub fn set_seed(&mut self, seed: i32) {
        self.idum = if seed < 0 { seed } else { -seed };
        self.iy = 0;
        self.iv = [0; NTAB];
    }

    pub fn generate_random_number(&mut self) -> i32 {
        if self.idum <= 0 || self.iy == 0 {
            self.idum = if -self.idum < 1 { 1 } else { -self.idum };
            for j in (0..NTAB + 8).rev() {
                let k = self.idum / IQ;
                self.idum = IA * (self.idum - k * IQ) - IR * k;
                if self.idum < 0 {
                    self.idum += IM;
                }
                if j < NTAB {
                    self.iv[j] = self.idum;
                }
            }
            self.iy = self.iv[0];
        }
        let k = self.idum / IQ;
        self.idum = IA * (self.idum - k * IQ) - IR * k;
        if self.idum < 0 {
            self.idum += IM;
        }
        let mut j = (self.iy / NDIV) as usize;
        if j >= NTAB {
            j = (j % NTAB) as usize;
        }
        self.iy = self.iv[j];
        self.iv[j] = self.idum;
        self.iy
    }

    pub fn random_float(&mut self, low: f32, high: f32) -> f32 {
        let mut fl = AM * self.generate_random_number() as f32;
        if fl > RNMX {
            fl = RNMX;
        }
        fl * (high - low) + low
    }

    pub fn random_int(&mut self, low: i32, high: i32) -> i32 {
        let x = (high as i64 - low as i64 + 1) as u32;
        if x <= 1 || MAX_RANDOM_RANGE < x - 1 {
            return low;
        }
        let max_acceptable = MAX_RANDOM_RANGE - ((MAX_RANDOM_RANGE + 1) % x);
        let mut n;
        loop {
            n = self.generate_random_number() as u32;
            if n <= max_acceptable {
                break;
            }
        }
        low + (n % x) as i32
    }
}

/// Standard CRC-32 (IEEE), as Valve's `CRC32_*`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// `SeedFileLineHash` as used by `SharedRandomFloat` in server.dll (`FUN_101bcad0`).
pub fn seed_file_line_hash(prediction_seed: i32, name: &str, additional_seed: i32) -> i32 {
    let mut buf = Vec::with_capacity(8 + name.len());
    buf.extend_from_slice(&prediction_seed.to_le_bytes());
    buf.extend_from_slice(&additional_seed.to_le_bytes());
    buf.extend_from_slice(name.as_bytes());
    crc32(&buf) as i32
}

/// The server's global shared stream (`DAT_106d3b98`) plus the per-command prediction seed (`DAT_105fe10c`).
#[derive(Clone, Debug, Default)]
pub struct SharedRandom {
    pub stream: UniformRandomStream,
    pub prediction_seed: i32,
}

impl SharedRandom {
    pub fn float(&mut self, name: &str, min: f32, max: f32, additional_seed: i32) -> f32 {
        let seed = seed_file_line_hash(self.prediction_seed, name, additional_seed);
        self.stream.set_seed(seed);
        self.stream.random_float(min, max)
    }

    pub fn int(&mut self, name: &str, min: i32, max: i32, additional_seed: i32) -> i32 {
        let seed = seed_file_line_hash(self.prediction_seed, name, additional_seed);
        self.stream.set_seed(seed);
        self.stream.random_int(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn stream_is_deterministic_and_in_range() {
        let mut a = UniformRandomStream::new(1234);
        let mut b = UniformRandomStream::new(1234);
        for _ in 0..1000 {
            let (x, y) = (a.random_float(-1.0, 1.0), b.random_float(-1.0, 1.0));
            assert_eq!(x, y);
            assert!((-1.0..1.0).contains(&x));
        }
        let mut c = UniformRandomStream::new(1234);
        assert!((0..100).all(|_| (3..=7).contains(&c.random_int(3, 7))));
    }
}
