//! Sound containers (RIFF WAV, MP3) and soundscripts (`scripts/game_sounds_manifest.txt` + the files it lists).
//! Only headers/metadata are parsed here; decoding lives in the engine's audio module.

use crate::kv::{Kv, KvValue};
use anyhow::{anyhow, bail, Result};

/// WAVE format tags seen in Source content.
pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_ADPCM: u16 = 2;
pub const WAVE_FORMAT_MP3: u16 = 0x55;

#[derive(Clone, Debug, Default)]
pub struct WavInfo {
    pub format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// Byte range of the `data` chunk payload.
    pub data: std::ops::Range<usize>,
    /// Sample frames (from `fact` when present, else derived from PCM data size).
    pub frames: Option<u32>,
    /// First cue point (Source uses it as the loop start), in sample frames.
    pub loop_start: Option<u32>,
    /// Loop end from the `smpl` chunk, if any.
    pub loop_end: Option<u32>,
    /// ADPCM coefficient table and samples-per-block from the extended `fmt ` chunk.
    pub adpcm: Option<(u16, Vec<(i16, i16)>)>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl WavInfo {
    pub fn parse(b: &[u8]) -> Result<WavInfo> {
        if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
            bail!("not a RIFF/WAVE file");
        }
        let mut w = WavInfo::default();
        let (mut have_fmt, mut have_data) = (false, false);
        let mut p = 12usize;
        while p + 8 <= b.len() {
            let id = &b[p..p + 4];
            let len = u32le(b, p + 4) as usize;
            let body = p + 8;
            // Some retail files have a truncated last chunk; clamp instead of failing.
            let end = body.saturating_add(len).min(b.len());
            let c = &b[body..end];
            match id {
                b"fmt " => {
                    if c.len() < 16 {
                        bail!("short fmt chunk");
                    }
                    w.format = u16le(c, 0);
                    w.channels = u16le(c, 2);
                    w.sample_rate = u32le(c, 4);
                    w.block_align = u16le(c, 12);
                    w.bits_per_sample = u16le(c, 14);
                    if w.format == WAVE_FORMAT_ADPCM && c.len() >= 22 {
                        let spb = u16le(c, 18);
                        let n = u16le(c, 20) as usize;
                        let coefs = (0..n)
                            .take_while(|i| 22 + i * 4 + 4 <= c.len())
                            .map(|i| (u16le(c, 22 + i * 4) as i16, u16le(c, 24 + i * 4) as i16))
                            .collect();
                        w.adpcm = Some((spb, coefs));
                    }
                    have_fmt = true;
                }
                b"data" => {
                    w.data = body..end;
                    have_data = true;
                }
                b"fact" if c.len() >= 4 => w.frames = Some(u32le(c, 0)),
                b"cue " if c.len() >= 4 && u32le(c, 0) > 0 && c.len() >= 4 + 24 => {
                    // cue point: id, position, fccChunk, chunkStart, blockStart, sampleOffset
                    w.loop_start = Some(u32le(c, 4 + 20));
                }
                b"smpl" if c.len() >= 36 && u32le(c, 28) > 0 && c.len() >= 36 + 24 => {
                    // first sample loop: id, type, start, end, fraction, playcount
                    w.loop_end = Some(u32le(c, 36 + 12));
                }
                _ => {}
            }
            p = body + len + (len & 1);
        }
        if !have_fmt || !have_data {
            bail!("missing fmt or data chunk");
        }
        if w.frames.is_none() && w.format == WAVE_FORMAT_PCM && w.block_align > 0 {
            w.frames = Some((w.data.len() / w.block_align as usize) as u32);
        }
        Ok(w)
    }
}

/// MPEG audio frame header (first valid frame after any ID3v2 tag).
#[derive(Clone, Copy, Debug)]
pub struct Mp3Info {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_kbps: u32,
    pub first_frame: usize,
}

impl Mp3Info {
    pub fn parse(b: &[u8]) -> Result<Mp3Info> {
        let mut p = 0;
        if b.len() >= 10 && &b[0..3] == b"ID3" {
            let sz = ((b[6] as usize & 0x7f) << 21)
                | ((b[7] as usize & 0x7f) << 14)
                | ((b[8] as usize & 0x7f) << 7)
                | (b[9] as usize & 0x7f);
            p = 10 + sz;
        }
        while p + 4 <= b.len() {
            if b[p] == 0xff && b[p + 1] & 0xe0 == 0xe0 {
                let ver = (b[p + 1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
                let layer = (b[p + 1] >> 1) & 3; // 1 = Layer III
                let br_i = (b[p + 2] >> 4) as usize;
                let sr_i = ((b[p + 2] >> 2) & 3) as usize;
                if ver != 1 && layer == 1 && br_i != 0 && br_i != 15 && sr_i != 3 {
                    const SR: [u32; 3] = [44100, 48000, 32000];
                    const BR1: [u32; 15] = [
                        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
                    ];
                    const BR2: [u32; 15] =
                        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
                    let sample_rate = match ver {
                        3 => SR[sr_i],
                        2 => SR[sr_i] / 2,
                        _ => SR[sr_i] / 4,
                    };
                    let bitrate_kbps = if ver == 3 { BR1[br_i] } else { BR2[br_i] };
                    let channels = if b[p + 3] >> 6 == 3 { 1 } else { 2 };
                    return Ok(Mp3Info {
                        sample_rate,
                        channels,
                        bitrate_kbps,
                        first_frame: p,
                    });
                }
            }
            p += 1;
        }
        Err(anyhow!("no MPEG layer III frame found"))
    }
}

/// Characters Source allows in front of a wave path to select channel/spatialization behaviour
/// (`*` stream, `#` music/dry, `@` omni, `)` spatial stereo, `^` distance variant, ...).
pub const SOUND_CHARS: &str = "*#@><^)}$!?&~`+%";

/// Strip the leading sound characters from a wave path.
pub fn strip_sound_chars(wave: &str) -> &str {
    wave.trim_start_matches(|c| SOUND_CHARS.contains(c))
}

/// One soundscript entry (e.g. `"Weapon_Pistol.Single"`). Values are kept as written; ranges like
/// `"95,105"` and named levels like `SNDLVL_NORM` are resolved by the audio engine.
#[derive(Clone, Debug, Default)]
pub struct SoundEntry {
    pub name: String,
    pub channel: Option<String>,
    pub volume: Option<String>,
    pub pitch: Option<String>,
    pub soundlevel: Option<String>,
    /// `wave` (one) or `rndwave` (several) paths relative to `sound/`, with sound characters intact.
    pub waves: Vec<String>,
    /// Every key/value of the entry as parsed (for less common keys such as `gamedata`).
    pub raw: Kv,
}

/// Parse one soundscript file into its entries.
pub fn parse_soundscript(src: &str) -> Vec<SoundEntry> {
    Kv::parse(src)
        .children()
        .iter()
        .filter(|e| matches!(e.value, KvValue::Obj(_)))
        .map(|e| {
            let mut waves = Vec::new();
            for c in e.children() {
                if c.key.eq_ignore_ascii_case("wave") {
                    waves.extend(c.as_str().map(str::to_string));
                } else if c.key.eq_ignore_ascii_case("rndwave") {
                    waves.extend(
                        c.children()
                            .iter()
                            .filter_map(|w| w.as_str().map(str::to_string)),
                    );
                }
            }
            SoundEntry {
                name: e.key.clone(),
                channel: e.str("channel").map(str::to_string),
                volume: e.str("volume").map(str::to_string),
                pitch: e.str("pitch").map(str::to_string),
                soundlevel: e
                    .str("soundlevel")
                    .or_else(|| e.str("attenuation"))
                    .map(str::to_string),
                waves,
                raw: e.clone(),
            }
        })
        .collect()
}

/// Files listed by `scripts/game_sounds_manifest.txt` (`precache_file`, `preload_file`, `declare_file`), in order.
pub fn manifest_files(src: &str) -> Vec<String> {
    let kv = Kv::parse(src);
    let Some(m) = kv.children().first() else {
        return Vec::new();
    };
    m.children()
        .iter()
        .filter_map(|c| {
            c.as_str()
                .map(|s| s.replace('\\', "/").to_ascii_lowercase())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundscript_entry() {
        let s = parse_soundscript(
            "\"Foo.Bar\" { \"channel\" \"CHAN_WEAPON\" \"pitch\" \"95,105\" \"soundlevel\" \"SNDLVL_GUNFIRE\" \
             \"rndwave\" { \"wave\" \")weapons/a.wav\" \"wave\" \")weapons/b.wav\" } }",
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].waves, vec![")weapons/a.wav", ")weapons/b.wav"]);
        assert_eq!(strip_sound_chars(&s[0].waves[0]), "weapons/a.wav");
        assert_eq!(s[0].soundlevel.as_deref(), Some("SNDLVL_GUNFIRE"));
    }

    #[test]
    fn minimal_wav() {
        let mut b = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        b.extend_from_slice(&[1, 0, 1, 0, 0x44, 0xac, 0, 0, 0x88, 0x58, 1, 0, 2, 0, 16, 0]);
        b.extend_from_slice(b"data\x04\0\0\0\x01\x02\x03\x04");
        let w = WavInfo::parse(&b).unwrap();
        assert_eq!(
            (w.format, w.channels, w.sample_rate, w.frames),
            (1, 1, 44100, Some(2))
        );
    }
}
