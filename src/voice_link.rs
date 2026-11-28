//! Voice chat plumbing for System Link (xbox_link.rs).
//!
//! The 360 announces `vaudio_speex` quality 5 in svc_VoiceInit: Speex
//! narrowband, 8 kHz mono, 20 ms (160-sample) frames. This module captures
//! the microphone while push-to-talk (K, Source's default +voicerecord) is
//! held, as 8 kHz frames, and plays received 8 kHz voice. The Speex codec
//! itself is not here yet (see vendor/l4d2r-xbox-link/PLAN.md): until a
//! `Codec` is installed, nothing is sent.
use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Speex narrowband frame: 20 ms at 8 kHz.
pub const FRAME: usize = 160;
const RATE: f32 = 8000.0;

/// A voice codec (Speex narrowband for the 360).
pub trait Codec: Send {
    fn encode(&mut self, frame: &[i16; FRAME]) -> Vec<u8>;
    fn decode(&mut self, data: &[u8]) -> Vec<i16>;
}

static CODEC: Mutex<Option<Box<dyn Codec>>> = Mutex::new(None);

/// Speex through `speex_helper.exe` (a 32-bit helper next to the game) running
/// the user's own Left 4 Dead 2 `bin/vaudio_speex.dll`, quality 5 as the 360
/// announces in svc_VoiceInit.
struct SpeexHelper {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
}

impl SpeexHelper {
    fn start(dll: &std::path::Path) -> Result<SpeexHelper, String> {
        use std::io::Read;
        let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("speex_helper.exe")))
            .filter(|p| p.exists())
            .ok_or("speex_helper.exe is not next to the game")?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg(dll).arg("5").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console window
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let mut n = [0u8; 4];
        stdout.read_exact(&mut n).map_err(|e| e.to_string())?;
        let mut name = vec![0u8; u32::from_le_bytes(n) as usize];
        stdout.read_exact(&mut name).map_err(|e| e.to_string())?;
        if name.is_empty() {
            let _ = child.kill();
            return Err(format!("{} did not load", dll.display()));
        }
        Ok(SpeexHelper { child, stdin, stdout })
    }
    fn call(&mut self, op: u8, body: &[u8]) -> Vec<u8> {
        use std::io::{Read, Write};
        let mut req = vec![op];
        req.extend_from_slice(&(body.len() as u32).to_le_bytes());
        req.extend_from_slice(body);
        if self.stdin.write_all(&req).and_then(|_| self.stdin.flush()).is_err() {
            return Vec::new();
        }
        let mut n = [0u8; 4];
        if self.stdout.read_exact(&mut n).is_err() {
            return Vec::new();
        }
        let mut out = vec![0u8; u32::from_le_bytes(n) as usize];
        if self.stdout.read_exact(&mut out).is_err() {
            return Vec::new();
        }
        out
    }
}

impl Drop for SpeexHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl Codec for SpeexHelper {
    fn encode(&mut self, frame: &[i16; FRAME]) -> Vec<u8> {
        let pcm: Vec<u8> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.call(b'E', &pcm)
    }
    fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        self.call(b'D', data).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }
}

/// Starts the Speex helper once (Ok if a codec is installed).
pub fn ensure_codec(dll: &std::path::Path) -> Result<(), String> {
    let mut guard = CODEC.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }
    let helper = SpeexHelper::start(dll)?;
    *guard = Some(Box::new(helper));
    Ok(())
}

/// Install the codec (none exists yet).
#[allow(dead_code)]
pub fn install_codec(c: Box<dyn Codec>) {
    *CODEC.lock().unwrap() = Some(c);
}

#[derive(Default)]
struct Shared {
    /// Captured 8 kHz mono samples.
    captured: VecDeque<i16>,
    /// Output samples at the device rate, mono.
    playback: VecDeque<f32>,
    output_rate: f32,
    error: Option<String>,
}

enum Ctl {
    Talk(bool),
}

struct Audio {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Ctl>,
}

static AUDIO: Mutex<Option<Audio>> = Mutex::new(None);

/// Starts the audio thread once (streams are created and owned there).
fn audio() -> std::sync::MutexGuard<'static, Option<Audio>> {
    let mut guard = AUDIO.lock().unwrap();
    if guard.is_none() {
        let shared = Arc::new(Mutex::new(Shared { output_rate: 48000.0, ..Default::default() }));
        let (tx, rx) = channel::<Ctl>();
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("voice".into()).spawn(move || {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            let host = cpal::default_host();
            // Output: always on (silence when nothing is queued).
            let _output = host.default_output_device().and_then(|dev| {
                let cfg = dev.default_output_config().ok()?;
                let channels = cfg.channels() as usize;
                s.lock().unwrap().output_rate = cfg.sample_rate() as f32;
                let s2 = s.clone();
                let stream = dev
                    .build_output_stream::<f32, _, _>(
                        &cfg.config(),
                        move |out, _| {
                            let mut g = s2.lock().unwrap();
                            for frame in out.chunks_mut(channels) {
                                let v = g.playback.pop_front().unwrap_or(0.0);
                                frame.iter_mut().for_each(|x| *x = v);
                            }
                        },
                        |e| eprintln!("voice output: {e}"),
                        None,
                    )
                    .ok()?;
                stream.play().ok()?;
                Some(stream)
            });
            let mut input: Option<cpal::Stream> = None;
            while let Ok(c) = rx.recv() {
                match c {
                    Ctl::Talk(true) if input.is_none() => {
                        let Some(dev) = host.default_input_device() else {
                            s.lock().unwrap().error = Some("no microphone".into());
                            continue;
                        };
                        let Ok(cfg) = dev.default_input_config() else { continue };
                        let channels = cfg.channels() as usize;
                        let step = cfg.sample_rate() as f32 / RATE;
                        let s2 = s.clone();
                        let mut phase = 0.0f32;
                        let stream = dev.build_input_stream::<f32, _, _>(
                            &cfg.config(),
                            move |data, _| {
                                let mut g = s2.lock().unwrap();
                                // Mono, then decimate to 8 kHz.
                                for frame in data.chunks(channels) {
                                    phase += 1.0;
                                    if phase >= step {
                                        phase -= step;
                                        let v = frame.iter().sum::<f32>() / channels as f32;
                                        g.captured.push_back((v.clamp(-1.0, 1.0) * 32767.0) as i16);
                                    }
                                }
                                while g.captured.len() > FRAME * 50 {
                                    g.captured.pop_front();
                                }
                            },
                            |e| eprintln!("voice input: {e}"),
                            None,
                        );
                        match stream {
                            Ok(st) => {
                                let _ = st.play();
                                input = Some(st);
                            }
                            Err(e) => s.lock().unwrap().error = Some(e.to_string()),
                        }
                    }
                    Ctl::Talk(false) => {
                        input = None;
                        s.lock().unwrap().captured.clear();
                    }
                    Ctl::Talk(true) => {}
                }
            }
        });
        *guard = Some(Audio { shared, tx });
    }
    guard
}

/// Whole 20 ms frames captured since the last call.
pub fn take_frames() -> Vec<[i16; FRAME]> {
    let guard = audio();
    let Some(a) = guard.as_ref() else { return Vec::new() };
    let mut g = a.shared.lock().unwrap();
    let mut out = Vec::new();
    while g.captured.len() >= FRAME {
        let mut f = [0i16; FRAME];
        for x in f.iter_mut() {
            *x = g.captured.pop_front().unwrap_or(0);
        }
        out.push(f);
    }
    out
}

/// Queue 8 kHz voice for playback (resampled to the output device).
pub fn play(pcm: &[i16]) {
    let guard = audio();
    let Some(a) = guard.as_ref() else { return };
    let mut g = a.shared.lock().unwrap();
    let step = RATE / g.output_rate.max(1.0);
    let mut t = 0.0f32;
    while (t as usize) < pcm.len() {
        let i = t as usize;
        let next = pcm.get(i + 1).copied().unwrap_or(pcm[i]) as f32;
        let v = pcm[i] as f32 + (next - pcm[i] as f32) * t.fract();
        g.playback.push_back(v / 32768.0);
        t += step;
    }
    while g.playback.len() > 48000 * 2 {
        g.playback.pop_front();
    }
}

/// Encode with the installed codec (None without one).
pub fn encode(frame: &[i16; FRAME]) -> Option<Vec<u8>> {
    CODEC.lock().unwrap().as_mut().map(|c| c.encode(frame))
}

/// Decode with the installed codec (None without one).
#[allow(dead_code)]
pub fn decode(data: &[u8]) -> Option<Vec<i16>> {
    CODEC.lock().unwrap().as_mut().map(|c| c.decode(data))
}

/// Push-to-talk (K) while in a System Link game: captures 20 ms frames and
/// encodes them once a codec is installed.
fn push_to_talk(keys: Res<ButtonInput<KeyCode>>, mut talking: Local<bool>, mut warned: Local<bool>) {
    if !crate::xbox_link::in_system_link() {
        return;
    }
    let held = keys.pressed(KeyCode::KeyK);
    if held != *talking {
        *talking = held;
        if let Some(a) = audio().as_ref() {
            let _ = a.tx.send(Ctl::Talk(held));
        }
    }
    if !*talking {
        return;
    }
    for frame in take_frames() {
        match encode(&frame) {
            Some(bytes) if !bytes.is_empty() => crate::xbox_link::send_voice(bytes),
            Some(_) => {}
            None if !*warned => {
                *warned = true;
                crate::xbox_link::log("voice: microphone works, but the Speex codec did not start, so voice is not sent");
            }
            None => {}
        }
    }
}

pub fn install(app: &mut App) {
    app.add_systems(Update, push_to_talk);
}
