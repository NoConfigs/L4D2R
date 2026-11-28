//! OS glue: secure random bytes and raw Ethernet I/O through Npcap
//! (wpcap.dll loaded at run time, so builds need no SDK).

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(alg: *mut c_void, buf: *mut u8, len: u32, flags: u32) -> i32;
    }
    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    pub fn random(buf: &mut [u8]) {
        let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, 2) };
        assert!(status == 0, "BCryptGenRandom failed: {status:#x}");
    }

    pub fn load(path: &str) -> Option<*mut c_void> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        // LOAD_WITH_ALTERED_SEARCH_PATH: find Packet.dll next to wpcap.dll.
        let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), 0x8) };
        (!h.is_null()).then_some(h)
    }

    pub fn symbol(module: *mut c_void, name: &str) -> Option<*mut c_void> {
        let c: Vec<u8> = name.bytes().chain(Some(0)).collect();
        let p = unsafe { GetProcAddress(module, c.as_ptr()) };
        (!p.is_null()).then_some(p)
    }
}

/// Fill `buf` from the OS cryptographic RNG.
pub fn random_bytes(buf: &mut [u8]) {
    #[cfg(windows)]
    win::random(buf);
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).expect("/dev/urandom");
    }
}

pub fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    random_bytes(&mut out);
    out
}

/// Non-zero 24-bit SPI.
pub fn random_spi() -> u32 {
    loop {
        let b = random_array::<3>();
        let v = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        if v != 0 {
            return v;
        }
    }
}

#[cfg(windows)]
pub use pcap::Pcap;

#[cfg(windows)]
mod pcap {
    use super::win;
    use std::ffi::{c_char, c_int, c_uint, c_void};

    #[repr(C)]
    struct PktHdr {
        sec: i32,
        usec: i32,
        caplen: u32,
        len: u32,
    }
    #[repr(C)]
    struct BpfProgram {
        len: c_uint,
        insns: *mut c_void,
    }

    type OpenLive = unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> *mut c_void;
    type Compile = unsafe extern "C" fn(*mut c_void, *mut BpfProgram, *const c_char, c_int, c_uint) -> c_int;
    type SetFilter = unsafe extern "C" fn(*mut c_void, *mut BpfProgram) -> c_int;
    type NextEx = unsafe extern "C" fn(*mut c_void, *mut *mut PktHdr, *mut *const u8) -> c_int;
    type SendPacket = unsafe extern "C" fn(*mut c_void, *const u8, c_int) -> c_int;
    type GetErr = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type Close = unsafe extern "C" fn(*mut c_void);

    /// One open Npcap adapter, filtered to UDP 3074.
    pub struct Pcap {
        handle: *mut c_void,
        next_ex: NextEx,
        send: SendPacket,
        geterr: GetErr,
        close: Close,
    }

    // The handle is only used from the thread that owns the Pcap.
    unsafe impl Send for Pcap {}

    fn err(geterr: GetErr, h: *mut c_void) -> String {
        unsafe { std::ffi::CStr::from_ptr(geterr(h)).to_string_lossy().into_owned() }
    }

    impl Pcap {
        /// `device` is `\Device\NPF_{GUID}`; `timeout_ms` bounds each `recv`.
        pub fn open(device: &str, timeout_ms: i32) -> Result<Pcap, String> {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            let dll = format!(r"{root}\System32\Npcap\wpcap.dll");
            let module = win::load(&dll).ok_or_else(|| format!("Npcap not installed ({dll})"))?;
            let sym = |n: &str| win::symbol(module, n).ok_or_else(|| format!("wpcap.dll lacks {n}"));
            unsafe {
                let open: OpenLive = std::mem::transmute(sym("pcap_open_live")?);
                let compile: Compile = std::mem::transmute(sym("pcap_compile")?);
                let setfilter: SetFilter = std::mem::transmute(sym("pcap_setfilter")?);
                let geterr: GetErr = std::mem::transmute(sym("pcap_geterr")?);
                let mut errbuf = [0 as c_char; 512];
                let name: Vec<u8> = device.bytes().chain(Some(0)).collect();
                let handle = open(name.as_ptr() as *const c_char, 65535, 1, timeout_ms, errbuf.as_mut_ptr());
                if handle.is_null() {
                    return Err(std::ffi::CStr::from_ptr(errbuf.as_ptr()).to_string_lossy().into_owned());
                }
                let mut prog = BpfProgram { len: 0, insns: std::ptr::null_mut() };
                let filter = b"udp port 3074\0";
                if compile(handle, &mut prog, filter.as_ptr() as *const c_char, 1, 0) != 0
                    || setfilter(handle, &mut prog) != 0
                {
                    return Err(err(geterr, handle));
                }
                Ok(Pcap {
                    handle,
                    next_ex: std::mem::transmute(sym("pcap_next_ex")?),
                    send: std::mem::transmute(sym("pcap_sendpacket")?),
                    geterr,
                    close: std::mem::transmute(sym("pcap_close")?),
                })
            }
        }

        pub fn send(&mut self, frame: &[u8]) -> Result<(), String> {
            if unsafe { (self.send)(self.handle, frame.as_ptr(), frame.len() as c_int) } != 0 {
                return Err(err(self.geterr, self.handle));
            }
            Ok(())
        }

        /// Next captured frame, or `None` after the read timeout.
        pub fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
            let mut hdr: *mut PktHdr = std::ptr::null_mut();
            let mut data: *const u8 = std::ptr::null();
            match unsafe { (self.next_ex)(self.handle, &mut hdr, &mut data) } {
                1 => Ok(Some(unsafe { std::slice::from_raw_parts(data, (*hdr).caplen as usize).to_vec() })),
                0 => Ok(None),
                _ => Err(err(self.geterr, self.handle)),
            }
        }
    }

    impl Drop for Pcap {
        fn drop(&mut self) {
            unsafe { (self.close)(self.handle) }
        }
    }
}
