//! Read noise lookup data from the owned game; no Valve tables are bundled.
use std::{io, path::Path};
pub struct Noise {
    perm: [[u8; 256]; 3],
    values: [f32; 256],
}
impl Noise {
    pub fn load(root: &Path) -> io::Result<Self> {
        let signature = [66i32, 147, 106, 213, 89, 115, 239, 25]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        for path in [
            root.join("bin/engine.dll"),
            root.join("left4dead2/bin/client.dll"),
        ] {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let Some(at) = bytes.windows(signature.len()).position(|v| v == signature) else {
                continue;
            };
            let Some(data) = bytes.get(at..at + 5120) else {
                continue;
            };
            let mut perm = [[0u8; 256]; 3];
            let mut valid = true;
            for row in 0..3 {
                let mut seen = [false; 256];
                for i in 0..256 {
                    let start = row * 1024 + i * 4;
                    let n = i32::from_le_bytes(data[start..start + 4].try_into().unwrap());
                    if !(0..256).contains(&n) || seen[n as usize] {
                        valid = false;
                        break;
                    }
                    seen[n as usize] = true;
                    perm[row][i] = n as u8;
                }
            }
            let values = std::array::from_fn(|i| {
                f32::from_le_bytes(data[4096 + i * 4..4100 + i * 4].try_into().unwrap())
            });
            if valid
                && values
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v))
            {
                return Ok(Self { perm, values });
            }
        }
        Err(io::Error::other("Native particle noise data unavailable"))
    }
    pub fn sample(&self, point: [f32; 3]) -> f32 {
        if point.iter().any(|v| !v.is_finite()) {
            return 0.;
        }
        // Scalar equivalent of Source's 8-bit-fraction SIMD lattice indexing.
        let encoded = point.map(|v| (v + 32768.).to_bits() & 0xffff);
        let cell = encoded.map(|v| (v >> 8) as usize);
        let fraction = encoded.map(|v| (v & 255) as f32 / 256.);
        let value = |x: usize, y: usize, z: usize| {
            let a = self.perm[0][x & 255] as usize;
            let b = self.perm[1][(y + a) & 255] as usize;
            self.values[self.perm[2][(z + b) & 255] as usize]
        };
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let plane = |z| {
            lerp(
                lerp(
                    value(cell[0], cell[1], z),
                    value(cell[0] + 1, cell[1], z),
                    fraction[0],
                ),
                lerp(
                    value(cell[0], cell[1] + 1, z),
                    value(cell[0] + 1, cell[1] + 1, z),
                    fraction[0],
                ),
                fraction[1],
            )
        };
        2. * (lerp(plane(cell[2]), plane(cell[2] + 1), fraction[2]) - 0.5)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_noise_tables_validate_and_reproduce_wrapping_and_fraction_quantization() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let noise = Noise::load(root).unwrap();
        let a = noise.sample([5.125, 9.5, 3.]);
        assert_eq!(a, noise.sample([261.125, 9.5, 3.]));
        assert_eq!(a, noise.sample([5.12501, 9.5, 3.]));
        assert!((-1. ..=1.).contains(&a));
        assert_ne!(a, noise.sample([5.25, 9.5, 3.]));
    }
}
