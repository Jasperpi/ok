//! Win32 declarations the review tests share, and the few helpers built on
//! them. The shell keeps its window handle to itself, so a test that wants
//! to look at the window from outside finds it by its class name.
//!
//! Nothing here shows a window, takes the focus, writes the clipboard, makes
//! a sound or moves the pointer. Every call either reads, or acts on the
//! test's own cloaked window.

#![allow(dead_code, non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::path::PathBuf;

use ok_win::Options;

pub type HWND = isize;

pub const WM_QUIT: u32 = 0x0012;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_QUERYENDSESSION: u32 = 0x0011;
pub const WM_ENDSESSION: u32 = 0x0016;
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
pub const WM_IME_CHAR: u32 = 0x0286;
pub const QS_ALLINPUT: u32 = 0x04FF;
pub const MWMO_INPUTAVAILABLE: u32 = 0x0004;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_MENU: i32 = 0x12;
pub const VK_LCONTROL: i32 = 0xA2;
pub const VK_LMENU: i32 = 0xA4;
pub const VK_CAPITAL: i32 = 0x14;
pub const VK_NUMLOCK: i32 = 0x90;
pub const VK_SCROLL: i32 = 0x91;
pub const WH_GETMESSAGE: i32 = 3;
pub const PM_REMOVE: usize = 1;
pub const SW_SHOWNOACTIVATE: i32 = 4;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
pub const DWMWA_CLOAKED: u32 = 14;
pub const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
pub const DUPLICATE_SAME_ACCESS: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: usize,
    pub lParam: isize,
    pub time: u32,
    pub pt: POINT,
}

pub type TimerProc = unsafe extern "system" fn(HWND, u32, usize, u32);
pub type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

#[link(name = "user32")]
unsafe extern "system" {
    pub fn FindWindowW(class: *const u16, name: *const u16) -> HWND;
    pub fn WindowFromPoint(point: POINT) -> HWND;
    pub fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> i32;
    pub fn GetForegroundWindow() -> HWND;
    pub fn IsWindowVisible(hwnd: HWND) -> i32;
    pub fn GetKeyState(key: i32) -> i16;
    pub fn GetAsyncKeyState(key: i32) -> i16;
    pub fn GetKeyboardState(keys: *mut u8) -> i32;
    pub fn SetKeyboardState(keys: *const u8) -> i32;
    pub fn GetClipboardSequenceNumber() -> u32;
    pub fn SendMessageW(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> isize;
    pub fn PostMessageW(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> i32;
    pub fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        owner: HWND,
        menu: isize,
        instance: isize,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> i32;
    pub fn ShowWindow(hwnd: HWND, how: i32) -> i32;
    pub fn SetTimer(hwnd: HWND, id: usize, ms: u32, proc: Option<TimerProc>) -> usize;
    pub fn KillTimer(hwnd: HWND, id: usize) -> i32;
    pub fn SetWindowsHookExW(kind: i32, proc: Option<HookProc>, module: isize, thread: u32) -> isize;
    pub fn UnhookWindowsHookEx(hook: isize) -> i32;
    pub fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    pub fn PeekMessageW(message: *mut MSG, hwnd: HWND, first: u32, last: u32, how: u32) -> i32;
    pub fn TranslateMessage(message: *const MSG) -> i32;
    pub fn DispatchMessageW(message: *const MSG) -> isize;
    pub fn MsgWaitForMultipleObjectsEx(count: u32, handles: *const isize, ms: u32, wake: u32, flags: u32) -> u32;
    pub fn PostQuitMessage(code: i32);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetCurrentThread() -> isize;
    pub fn GetCurrentProcess() -> isize;
    pub fn DuplicateHandle(from_process: isize, handle: isize, to_process: isize, out: *mut isize, access: u32, inherit: i32, options: u32) -> i32;
    pub fn CloseHandle(handle: isize) -> i32;
    pub fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    pub fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    pub fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Options for one review test. The name goes into the application id, and
/// so into the window's class, which is how [`window_of`] finds exactly this
/// test's window while others run beside it.
pub fn options(name: &str) -> Options {
    Options { app_id: format!("JasperMeller.ok.review.{name}"), name: String::from("ok-review"), icons: Vec::new(), open: None }
}

/// The cloaked window of the self-test started with [`options`]`(name)`.
pub fn window_of(name: &str) -> HWND {
    unsafe { FindWindowW(wide(&format!("JasperMeller.ok.review.{name}.selftest")).as_ptr(), std::ptr::null()) }
}

/// An empty folder for one review test's files.
pub fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ok-win-review").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a test folder can be made");
    dir
}

/// What the compositor says about a window's cloak: 0 is not cloaked, 1 by
/// the application, 2 by the shell, 4 inherited from the window that owns
/// it. `None` when the compositor cannot be asked.
pub fn cloak_of(hwnd: HWND) -> Option<u32> {
    type GetAttribute = unsafe extern "system" fn(HWND, u32, *mut c_void, u32) -> i32;
    unsafe {
        let module = LoadLibraryExW(wide("dwmapi.dll").as_ptr(), 0, LOAD_LIBRARY_SEARCH_SYSTEM32);
        if module == 0 {
            return None;
        }
        let address = GetProcAddress(module, c"DwmGetWindowAttribute".as_ptr().cast());
        if address.is_null() {
            return None;
        }
        let get: GetAttribute = std::mem::transmute(address);
        let mut value = 0u32;
        (get(hwnd, DWMWA_CLOAKED, (&mut value as *mut u32).cast(), 4) >= 0).then_some(value)
    }
}

/// A handle to the calling thread that another thread can measure it with.
pub fn this_thread() -> isize {
    let mut handle = 0isize;
    unsafe { DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &mut handle, 0, 0, DUPLICATE_SAME_ACCESS) };
    handle
}

/// Processor cycles a thread has been charged so far. Unlike processor
/// time, which Windows counts in 15.6 ms steps, this counts every cycle the
/// thread ran, so a thread that woke for a few microseconds shows up.
pub fn cycles_of(thread: isize) -> u64 {
    let mut cycles = 0u64;
    unsafe { QueryThreadCycleTime(thread, &mut cycles) };
    cycles
}

/// How many cycles this machine charges for a millisecond of work, found by
/// working for a while and dividing.
pub fn cycles_per_ms() -> f64 {
    let thread = this_thread();
    let (from, started) = (cycles_of(thread), std::time::Instant::now());
    let mut spin = 0u64;
    while started.elapsed().as_millis() < 60 {
        spin = std::hint::black_box(spin.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
    let per_ms = (cycles_of(thread) - from) as f64 / (started.elapsed().as_secs_f64() * 1000.0);
    unsafe { CloseHandle(thread) };
    per_ms
}
