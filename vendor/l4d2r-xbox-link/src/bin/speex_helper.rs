//! 32-bit helper that runs the Speex voice codec from the user's own Left 4
//! Dead 2 install (`bin/vaudio_speex.dll`, Source's IVoiceCodec), so the 64-bit
//! game can use the exact codec the Xbox 360 announces (`vaudio_speex`, q 5).
//!
//! Build: cargo build --release --target i686-pc-windows-msvc --bin speex_helper
//! Run:   speex_helper <path to vaudio_speex.dll> [quality]
//!
//! Protocol on stdin/stdout, little-endian:
//!   request  = op (u8: b'E' encode, b'D' decode, b'R' reset) + u32 length + bytes
//!              (E: 16-bit mono 8 kHz PCM; D: compressed bytes; R: no bytes)
//!   response = u32 length + bytes (E: compressed; D: PCM; R: empty)
//! On start it writes one response: the codec's name if Init succeeded, else empty.

#[cfg(all(windows, target_arch = "x86"))]
mod codec {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryExA(name: *const u8, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

    type CreateInterface = unsafe extern "C" fn(*const u8, *mut i32) -> *mut c_void;
    type InitFn = unsafe extern "thiscall" fn(*mut c_void, i32) -> bool;
    type CompressFn = unsafe extern "thiscall" fn(*mut c_void, *const u8, i32, *mut u8, i32, bool) -> i32;
    type DecompressFn = unsafe extern "thiscall" fn(*mut c_void, *const u8, i32, *mut u8, i32) -> i32;
    type ResetFn = unsafe extern "thiscall" fn(*mut c_void) -> bool;

    /// Source IVoiceCodec vtable: [0] destructor, [1] Init, [2] Release,
    /// [3] Compress, [4] Decompress, [5] ResetState.
    pub struct Codec {
        this: *mut c_void,
        vtable: *const usize,
    }

    impl Codec {
        pub fn load(path: &str, quality: i32) -> Option<Codec> {
            // LOAD_WITH_ALTERED_SEARCH_PATH needs an absolute path with backslashes.
            let mut p = path.replace('/', "\\").into_bytes();
            p.push(0);
            unsafe {
                let module = LoadLibraryExA(p.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
                if module.is_null() {
                    return None;
                }
                let f = GetProcAddress(module, b"CreateInterface\0".as_ptr());
                if f.is_null() {
                    return None;
                }
                let create: CreateInterface = std::mem::transmute(f);
                let mut rc = 0i32;
                let this = create(b"vaudio_speex\0".as_ptr(), &mut rc);
                if this.is_null() {
                    return None;
                }
                let vtable = *(this as *const *const usize);
                let c = Codec { this, vtable };
                let init: InitFn = std::mem::transmute(*c.vtable.add(1));
                init(c.this, quality).then_some(c)
            }
        }
        pub fn compress(&self, pcm: &[u8], last: bool) -> Vec<u8> {
            let mut out = vec![0u8; 2048];
            unsafe {
                let f: CompressFn = std::mem::transmute(*self.vtable.add(3));
                let n = f(self.this, pcm.as_ptr(), (pcm.len() / 2) as i32, out.as_mut_ptr(), out.len() as i32, last);
                out.truncate(n.max(0) as usize);
            }
            out
        }
        pub fn decompress(&self, data: &[u8]) -> Vec<u8> {
            let mut out = vec![0u8; 32768];
            unsafe {
                let f: DecompressFn = std::mem::transmute(*self.vtable.add(4));
                // Returns samples written.
                let n = f(self.this, data.as_ptr(), data.len() as i32, out.as_mut_ptr(), out.len() as i32);
                out.truncate((n.max(0) as usize * 2).min(out.len()));
            }
            out
        }
        pub fn reset(&self) {
            unsafe {
                let f: ResetFn = std::mem::transmute(*self.vtable.add(5));
                f(self.this);
            }
        }
    }
}

#[cfg(all(windows, target_arch = "x86"))]
fn main() {
    use std::io::{Read, Write};
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_default();
    let quality: i32 = args.get(2).and_then(|q| q.parse().ok()).unwrap_or(5);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let (mut input, mut output) = (stdin.lock(), stdout.lock());
    let reply = |out: &mut std::io::StdoutLock, bytes: &[u8]| {
        let _ = out.write_all(&(bytes.len() as u32).to_le_bytes());
        let _ = out.write_all(bytes);
        let _ = out.flush();
    };
    let Some(codec) = codec::Codec::load(&path, quality) else {
        reply(&mut output, b"");
        return;
    };
    reply(&mut output, b"vaudio_speex");
    loop {
        let mut head = [0u8; 5];
        if input.read_exact(&mut head).is_err() {
            return;
        }
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut body = vec![0u8; len.min(1 << 20)];
        if input.read_exact(&mut body).is_err() {
            return;
        }
        match head[0] {
            b'E' => reply(&mut output, &codec.compress(&body, false)),
            b'D' => reply(&mut output, &codec.decompress(&body)),
            b'R' => {
                codec.reset();
                reply(&mut output, b"");
            }
            _ => reply(&mut output, b""),
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86")))]
fn main() {
    eprintln!("speex_helper must be built for i686-pc-windows-msvc (it loads the 32-bit vaudio_speex.dll)");
}
