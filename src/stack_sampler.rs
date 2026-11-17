//! Opt-in in-process sampling profiler (`--profile-sample <warmups>`).
//!
//! After the given number of render warmups (2 = after one in-window map
//! change) plus ten seconds, a background thread repeatedly suspends the
//! main and render threads, walks their stacks with the Windows x64 unwinder
//! and, at the end, resolves the addresses through dbghelp. Reports go to
//! local-research/sample-<thread>.txt (inclusive and self sample counts per
//! function). Diagnostic only; ordinary play never starts the thread.
#![allow(clippy::upper_case_acronyms)]
use std::sync::atomic::{AtomicU32, Ordering};

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static RENDER_THREAD: AtomicU32 = AtomicU32::new(0);
static WARMUPS: AtomicU32 = AtomicU32::new(0);

const SAMPLES: usize = 3000;
const DEPTH: usize = 96;

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    pub const THREAD_ALL_ACCESS: u32 = 0x1F03FF;
    pub const CONTEXT_FULL: u32 = 0x10000B;
    #[repr(C, align(16))]
    pub struct Context(pub [u8; 1232]);
    impl Context {
        pub fn rip(&self) -> u64 {
            u64::from_le_bytes(self.0[0xF8..0x100].try_into().unwrap())
        }
        pub fn rsp(&self) -> u64 {
            u64::from_le_bytes(self.0[0x98..0xA0].try_into().unwrap())
        }
        pub fn set_rip(&mut self, v: u64) {
            self.0[0xF8..0x100].copy_from_slice(&v.to_le_bytes());
        }
        pub fn set_rsp(&mut self, v: u64) {
            self.0[0x98..0xA0].copy_from_slice(&v.to_le_bytes());
        }
    }
    #[repr(C)]
    pub struct ThreadEntry32 {
        pub size: u32,
        pub usage: u32,
        pub thread_id: u32,
        pub owner_process_id: u32,
        pub base_priority: i32,
        pub delta_priority: i32,
        pub flags: u32,
    }
    #[repr(C)]
    pub struct SymbolInfoW {
        pub size_of_struct: u32,
        pub type_index: u32,
        pub reserved: [u64; 2],
        pub index: u32,
        pub size: u32,
        pub mod_base: u64,
        pub flags: u32,
        pub value: u64,
        pub address: u64,
        pub register: u32,
        pub scope: u32,
        pub tag: u32,
        pub name_len: u32,
        pub max_name_len: u32,
        pub name: [u16; 1],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetCurrentProcessId() -> u32;
        pub fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        pub fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        pub fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
        pub fn GetThreadDescription(thread: *mut c_void, description: *mut *mut u16) -> i32;
        pub fn LocalFree(memory: *mut c_void) -> *mut c_void;
        pub fn GetLastError() -> u32;
        pub fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        pub fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
        pub fn GetCurrentProcess() -> *mut c_void;
        pub fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
        pub fn SuspendThread(thread: *mut c_void) -> u32;
        pub fn ResumeThread(thread: *mut c_void) -> u32;
        pub fn GetThreadContext(thread: *mut c_void, context: *mut Context) -> i32;
        pub fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
        pub fn RtlVirtualUnwind(
            handler_type: u32,
            base: u64,
            pc: u64,
            entry: *mut c_void,
            context: *mut Context,
            handler_data: *mut *mut c_void,
            establisher: *mut u64,
            pointers: *mut c_void,
        ) -> *mut c_void;
    }
    #[link(name = "dbghelp")]
    unsafe extern "system" {
        pub fn SymInitializeW(process: *mut c_void, path: *const u16, invade: i32) -> i32;
        pub fn SymSetOptions(options: u32) -> u32;
        pub fn SymLoadModuleExW(
            process: *mut c_void,
            file: *mut c_void,
            image: *const u16,
            module: *const u16,
            base: u64,
            size: u32,
            data: *mut c_void,
            flags: u32,
        ) -> u64;
        pub fn SymFromAddrW(
            process: *mut c_void,
            address: u64,
            displacement: *mut u64,
            symbol: *mut SymbolInfoW,
        ) -> i32;
    }
}

/// Records the calling thread as the main (false) or render (true) thread.
pub fn register(render: bool) {
    #[cfg(windows)]
    {
        let id = unsafe { sys::GetCurrentThreadId() };
        if render {
            RENDER_THREAD.store(id, Ordering::Relaxed);
        } else {
            MAIN_THREAD.store(id, Ordering::Relaxed);
        }
    }
    let _ = render;
}
pub fn warmup_finished() {
    WARMUPS.fetch_add(1, Ordering::Relaxed);
}
pub fn enabled() -> bool {
    std::env::args().any(|a| a == "--profile-sample")
}
pub fn install(app: &mut bevy::prelude::App) {
    use bevy::prelude::*;
    if !enabled() {
        return;
    }
    app.add_systems(First, |_: &mut World| register(false));
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render.add_systems(bevy::render::Render, (|_: &mut World| register(true)).in_set(bevy::render::RenderSystems::Cleanup));
    }
    let warmups = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|a| a[0] == "--profile-sample")
        .and_then(|a| a[1].parse::<u32>().ok())
        .unwrap_or(1);
    std::thread::spawn(move || run(warmups));
}

#[cfg(windows)]
fn run(warmups: u32) {
    while WARMUPS.load(Ordering::Relaxed) < warmups {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::thread::sleep(std::time::Duration::from_secs(10));
    println!("Stack sampler: sampling");
    let threads = all_threads();
    // Preallocated: nothing may allocate while a target thread is suspended
    // (it may hold the heap lock).
    let mut stacks: Vec<Vec<u64>> = threads.iter().map(|_| Vec::with_capacity(SAMPLES * (DEPTH + 1))).collect();
    let handles: Vec<_> = threads
        .iter()
        .map(|(_, id)| unsafe { sys::OpenThread(sys::THREAD_ALL_ACCESS, 0, *id) })
        .collect();
    let mut context = Box::new(sys::Context([0; 1232]));
    for _ in 0..SAMPLES {
        for (t, handle) in handles.iter().enumerate() {
            if handle.is_null() {
                continue;
            }
            unsafe {
                if sys::SuspendThread(*handle) == u32::MAX {
                    continue;
                }
                context.0.fill(0);
                context.0[0x30..0x34].copy_from_slice(&sys::CONTEXT_FULL.to_le_bytes());
                if sys::GetThreadContext(*handle, &mut *context) != 0 {
                    let out = &mut stacks[t];
                    let start = out.len();
                    for _ in 0..DEPTH {
                        let pc = context.rip();
                        if pc == 0 {
                            break;
                        }
                        out.push(pc);
                        let mut base = 0u64;
                        let entry = sys::RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                        if entry.is_null() {
                            let rsp = context.rsp();
                            context.set_rip(*(rsp as *const u64));
                            context.set_rsp(rsp + 8);
                        } else {
                            let mut data = std::ptr::null_mut();
                            let mut frame = 0u64;
                            sys::RtlVirtualUnwind(0, base, pc, entry, &mut *context, &mut data, &mut frame, std::ptr::null_mut());
                        }
                    }
                    let depth = out.len() - start;
                    out.push(depth as u64 | 1 << 63);
                }
                sys::ResumeThread(*handle);
            }
        }
        std::thread::sleep(std::time::Duration::from_micros(700));
    }
    println!("Stack sampler: resolving");
    unsafe {
        sys::SymSetOptions(0x2 | 0x4 | 0x80000000); // UNDNAME | DEFERRED_LOADS | DEBUG
        // Search the executable's own directory for its PDB.
        let search: Vec<u16> = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_string_lossy().into_owned()))
            .unwrap_or_default()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let ok = sys::SymInitializeW(sys::GetCurrentProcess(), search.as_ptr(), 1);
        let _ = ok;
        // The executable itself is not always enumerated; register it.
        let module = sys::GetModuleHandleW(std::ptr::null());
        let mut path = [0u16; 1024];
        let len = sys::GetModuleFileNameW(module, path.as_mut_ptr(), 1024);
        let loaded = sys::SymLoadModuleExW(sys::GetCurrentProcess(), std::ptr::null_mut(), path.as_ptr(), std::ptr::null(), module as u64, 0, std::ptr::null_mut(), 0);
        let _ = (len, loaded);
    }
    let mut names: std::collections::HashMap<u64, String> = Default::default();
    let mut resolve = |pc: u64| -> String {
        names
            .entry(pc)
            .or_insert_with(|| unsafe {
                let mut buffer = vec![0u64; (std::mem::size_of::<sys::SymbolInfoW>() + 2048 * 2) / 8 + 1];
                let symbol = buffer.as_mut_ptr() as *mut sys::SymbolInfoW;
                (*symbol).size_of_struct = std::mem::size_of::<sys::SymbolInfoW>() as u32;
                (*symbol).max_name_len = 2000;
                let mut displacement = 0;
                if sys::SymFromAddrW(sys::GetCurrentProcess(), pc, &mut displacement, symbol) != 0 {
                    let len = (*symbol).name_len as usize;
                    let name = std::slice::from_raw_parts((*symbol).name.as_ptr(), len);
                    String::from_utf16_lossy(name)
                } else {
                    format!("0x{pc:x}")
                }
            })
            .clone()
    };
    let mut busy_inclusive: std::collections::HashMap<String, usize> = Default::default();
    // Folded busy stacks ("root;...;leaf count"), for flame graphs/callers.
    let mut folded: std::collections::HashMap<String, usize> = Default::default();
    let mut busy_own: std::collections::HashMap<String, usize> = Default::default();
    let mut summary = String::from("thread\tsamples\tbusy%\n");
    for (t, (label, _)) in threads.iter().enumerate() {
        let data = &stacks[t];
        let mut busy = 0usize;
        let mut inclusive: std::collections::HashMap<String, usize> = Default::default();
        let mut own: std::collections::HashMap<String, usize> = Default::default();
        let mut frames = vec![];
        let mut count = 0;
        for &v in data {
            if v & 1 << 63 != 0 {
                count += 1;
                let mut seen = std::collections::HashSet::new();
                let names: Vec<String> = frames
                    .iter()
                    .enumerate()
                    // Return addresses point after the call; step back one byte.
                    .map(|(i, pc)| resolve(if i == 0 { *pc } else { pc - 1 }))
                    .collect();
                let idle = names.first().is_none_or(|n| {
                    n.contains("Wait") || n.contains("GetMessage") || n.contains("RemoveIoCompletion") || n.contains("DelayExecution")
                });
                if !idle {
                    busy += 1;
                }
                if !idle {
                    let mut key = label.split(" #").next().unwrap_or("").to_string();
                    for name in names.iter().rev() {
                        key.push(';');
                        key.push_str(name);
                    }
                    *folded.entry(key).or_default() += 1;
                }
                for (i, name) in names.iter().enumerate() {
                    if i == 0 {
                        *own.entry(name.clone()).or_default() += 1;
                        if !idle {
                            *busy_own.entry(name.clone()).or_default() += 1;
                        }
                    }
                    if seen.insert(name.clone()) {
                        *inclusive.entry(name.clone()).or_default() += 1;
                        if !idle {
                            *busy_inclusive.entry(name.clone()).or_default() += 1;
                        }
                    }
                }
                frames.clear();
            } else {
                frames.push(v);
            }
        }
        let mut text = format!("{label} thread: {count} samples\n\n== inclusive ==\n");
        let mut rows: Vec<_> = inclusive.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, n) in rows.iter().take(400) {
            text += &format!("{:6.2}% {n:6} {name}\n", *n as f64 * 100. / count.max(1) as f64);
        }
        text += "\n== self ==\n";
        let mut rows: Vec<_> = own.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, n) in rows.iter().take(200) {
            text += &format!("{:6.2}% {n:6} {name}\n", *n as f64 * 100. / count.max(1) as f64);
        }
        summary += &format!("{label}\t{count}\t{:.1}\n", busy as f64 * 100. / count.max(1) as f64);
        if label == "main" || label == "render" {
            let _ = std::fs::write(format!("local-research/sample-{label}.txt"), text);
        }
    }
    // Busy samples of every thread, as a share of one thread's wall time
    // (100% = one core busy for the whole sampling window).
    let mut text = format!("{summary}\n== busy inclusive (all threads, % of one core) ==\n");
    let mut rows: Vec<_> = busy_inclusive.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in rows.iter().take(600) {
        text += &format!("{:7.2}% {n:6} {name}\n", *n as f64 * 100. / SAMPLES as f64);
    }
    text += "\n== busy self ==\n";
    let mut rows: Vec<_> = busy_own.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in rows.iter().take(300) {
        text += &format!("{:7.2}% {n:6} {name}\n", *n as f64 * 100. / SAMPLES as f64);
    }
    let _ = std::fs::write("local-research/sample-all.txt", text);
    let mut rows: Vec<_> = folded.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let folded: String = rows.iter().map(|(k, n)| format!("{k} {n}
")).collect();
    let _ = std::fs::write("local-research/sample-folded.txt", folded);
    println!("Stack sampler: done");
}
/// Every thread of this process except the sampler, named by its thread
/// description (Bevy task pools name theirs) or main/render.
#[cfg(windows)]
fn all_threads() -> Vec<(String, u32)> {
    let me = unsafe { sys::GetCurrentThreadId() };
    let pid = unsafe { sys::GetCurrentProcessId() };
    let main = MAIN_THREAD.load(Ordering::Relaxed);
    let render = RENDER_THREAD.load(Ordering::Relaxed);
    let mut out = vec![];
    unsafe {
        let snapshot = sys::CreateToolhelp32Snapshot(0x4, 0);
        let mut entry = sys::ThreadEntry32 {
            size: std::mem::size_of::<sys::ThreadEntry32>() as u32,
            usage: 0,
            thread_id: 0,
            owner_process_id: 0,
            base_priority: 0,
            delta_priority: 0,
            flags: 0,
        };
        let mut ok = sys::Thread32First(snapshot, &mut entry);
        while ok != 0 {
            if entry.owner_process_id == pid && entry.thread_id != me {
                let id = entry.thread_id;
                let label = if id == main {
                    "main".to_string()
                } else if id == render {
                    "render".to_string()
                } else {
                    let handle = sys::OpenThread(sys::THREAD_ALL_ACCESS, 0, id);
                    let mut text = std::ptr::null_mut();
                    let mut name = String::new();
                    if !handle.is_null() && sys::GetThreadDescription(handle, &mut text) >= 0 && !text.is_null() {
                        let mut len = 0;
                        while *text.add(len) != 0 {
                            len += 1;
                        }
                        name = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
                        sys::LocalFree(text as _);
                    }
                    if !handle.is_null() {
                        sys::CloseHandle(handle);
                    }
                    format!("{} #{id}", if name.is_empty() { "thread" } else { &name })
                };
                out.push((label, id));
            }
            ok = sys::Thread32Next(snapshot, &mut entry);
        }
        sys::CloseHandle(snapshot);
    }
    out
}
#[cfg(not(windows))]
fn run(_: u32) {}
