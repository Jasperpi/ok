//! The machine behind [`Host`] on macOS and Linux: the clock, the folders
//! each system keeps for documents and data, the clipboard through the
//! system's own tools, and the browser.

use std::ffi::{c_char, c_int, c_long};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ok_gfx::AlphaGlyph;
use ok_host::{FaceCell, Host, LocalTime, Waker};

#[repr(C)]
struct Tm {
    sec: c_int,
    min: c_int,
    hour: c_int,
    mday: c_int,
    mon: c_int,
    year: c_int,
    wday: c_int,
    yday: c_int,
    isdst: c_int,
    gmtoff: c_long,
    zone: *const c_char,
}

extern "C" {
    fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
}

pub(crate) struct MiniHost {
    start: Instant,
    woken: Arc<AtomicBool>,
    name: String,
    /// What was last copied, for when the system has no clipboard tool.
    copied: Option<String>,
}

impl MiniHost {
    pub(crate) fn new(name: &str, woken: Arc<AtomicBool>) -> MiniHost {
        MiniHost { start: Instant::now(), woken, name: name.to_string(), copied: None }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// An XDG base folder: the variable if it names an absolute path, or the
/// usual place under the home folder.
#[cfg(not(target_os = "macos"))]
fn xdg(variable: &str, usual: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or_else(|| home().join(usual))
}

/// The Documents folder `user-dirs.dirs` names, as `xdg-user-dir` would.
#[cfg(not(target_os = "macos"))]
fn xdg_documents() -> Option<PathBuf> {
    let dirs = std::fs::read_to_string(xdg("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs")).ok()?;
    let line = dirs.lines().find_map(|line| line.trim().strip_prefix("XDG_DOCUMENTS_DIR="))?;
    let value = line.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    // A Documents folder set to the home folder itself would put the
    // writing folder loose in it; that is not what the setting means.
    (path.is_absolute() && path != home()).then_some(path)
}

/// Runs a clipboard tool and returns what it printed.
fn read_from(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs a clipboard tool with `text` on its input. True if it took it.
fn write_to(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    wrote && child.wait().is_ok_and(|status| status.success())
}

/// The clipboard tools to try, readers and writers, in order.
#[cfg(target_os = "macos")]
const CLIPBOARD: &[(&str, &[&str], &str, &[&str])] = &[("pbpaste", &[], "pbcopy", &[])];
#[cfg(not(target_os = "macos"))]
const CLIPBOARD: &[(&str, &[&str], &str, &[&str])] = &[
    ("wl-paste", &["--no-newline"], "wl-copy", &[]),
    ("xclip", &["-selection", "clipboard", "-o"], "xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--output"], "xsel", &["--clipboard", "--input"]),
];

/// Whether a Linux clipboard tool fits the session: the Wayland one only
/// under Wayland, the X ones only where there is an X display.
fn fits(program: &str) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    if program.starts_with("wl-") {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
    } else {
        std::env::var_os("DISPLAY").is_some()
    }
}

impl Host for MiniHost {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn local_time(&self) -> LocalTime {
        let unix = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => since.as_secs() as i64,
            Err(before) => -(before.duration().as_secs() as i64),
        };
        let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 1, mon: 0, year: 70, wday: 4, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
        // SAFETY: both pointers are valid for the call; localtime_r is thread-safe.
        unsafe { localtime_r(&unix, &mut tm) };
        LocalTime {
            year: tm.year + 1900,
            month: (tm.mon + 1) as u8,
            day: tm.mday as u8,
            hour: tm.hour as u8,
            minute: tm.min as u8,
            second: tm.sec.min(59) as u8,
            // The system counts from Sunday; the interface counts from Monday.
            weekday: ((tm.wday + 6) % 7) as u8,
            unix,
        }
    }

    fn waker(&self) -> Waker {
        let woken = Arc::clone(&self.woken);
        Arc::new(move || woken.store(true, Ordering::Release))
    }

    fn clipboard_get(&mut self) -> Option<String> {
        for (reader, args, _, _) in CLIPBOARD {
            if fits(reader) {
                if let Some(text) = read_from(reader, args) {
                    return Some(text);
                }
            }
        }
        self.copied.clone()
    }

    fn clipboard_set(&mut self, text: &str) {
        self.copied = Some(text.to_string());
        for (_, _, writer, args) in CLIPBOARD {
            if fits(writer) && write_to(writer, args, text) {
                return;
            }
        }
    }

    fn documents_dir(&self) -> PathBuf {
        #[cfg(not(target_os = "macos"))]
        if let Some(documents) = xdg_documents() {
            return documents;
        }
        home().join("Documents")
    }

    #[cfg(target_os = "macos")]
    fn local_data_dir(&self) -> PathBuf {
        home().join("Library").join("Application Support").join(&self.name)
    }

    #[cfg(not(target_os = "macos"))]
    fn local_data_dir(&self) -> PathBuf {
        xdg("XDG_DATA_HOME", ".local/share").join(&self.name)
    }

    #[cfg(target_os = "macos")]
    fn roaming_data_dir(&self) -> PathBuf {
        self.local_data_dir()
    }

    #[cfg(not(target_os = "macos"))]
    fn roaming_data_dir(&self) -> PathBuf {
        xdg("XDG_CONFIG_HOME", ".config").join(&self.name)
    }

    /// The system's own faces are a Windows feature for now: the three
    /// faces inside the program are what there is here.
    fn system_faces(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn load_face_file(&mut self, _path: &Path) -> Option<String> {
        None
    }

    fn face_cell(&mut self, _family: &str, _height: u32) -> Option<FaceCell> {
        None
    }

    fn rasterize(&mut self, _family: &str, _height: u32, _ch: char) -> Option<AlphaGlyph> {
        None
    }

    fn open_in_browser(&mut self, url: &str) {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return;
        }
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = Command::new(opener).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }

    fn reduced_motion(&self) -> bool {
        false
    }
}
