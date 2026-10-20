//! The machine, as the interface is allowed to see it.

use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ok_gfx::AlphaGlyph;
use ok_host::{FaceCell, Host, LocalTime, Waker};

use crate::fonts::Faces;
use crate::win32::*;

/// The message a waker posts to the window. It carries nothing: the flag in
/// [`WakeLine`] is what says a tick is owed.
pub(crate) const WM_WAKE: u32 = WM_APP + 1;

/// What every waker made for one window shares with it: where to post, and
/// whether a wake is already on its way.
///
/// The flag is what makes many calls cost one tick. A call that finds it set
/// posts nothing; the window clears it before it sends the tick, so news that
/// arrives while that tick is being handled posts again.
#[derive(Default)]
pub(crate) struct WakeLine {
    hwnd: AtomicIsize,
    owed: AtomicBool,
}

impl WakeLine {
    /// Any thread: asks the window for a tick. Before the window exists the
    /// request is kept and sent when it does.
    pub(crate) fn wake(&self) {
        if !self.owed.swap(true, Ordering::AcqRel) {
            self.post();
        }
    }

    /// The window exists: wakes go to it from now on.
    pub(crate) fn attach(&self, hwnd: HWND) {
        self.hwnd.store(hwnd, Ordering::Release);
        if self.owed.load(Ordering::Acquire) {
            self.post();
        }
    }

    /// The window is gone. A waker kept by a thread that outlives it does
    /// nothing from here on, and can never reach a later window that
    /// Windows gives the same handle.
    pub(crate) fn detach(&self) {
        self.hwnd.store(0, Ordering::Release);
    }

    /// The window's side: the wake has arrived and its tick is about to be
    /// sent. Returns false when no wake was owed.
    pub(crate) fn take(&self) -> bool {
        self.owed.swap(false, Ordering::AcqRel)
    }

    fn post(&self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd == 0 {
            return;
        }
        // A full message queue refuses the post. The wake is then no longer
        // on its way, so the next call must try again.
        if unsafe { PostMessageW(hwnd, WM_WAKE, 0, 0) } == 0 {
            self.owed.store(false, Ordering::Release);
        }
    }
}

/// The three folders the interface keeps its files in.
#[derive(Clone, Debug)]
pub(crate) struct Folders {
    pub documents: PathBuf,
    pub local: PathBuf,
    pub roaming: PathBuf,
}

impl Folders {
    /// The user's real folders. `name` is the application's own folder under
    /// the two data folders.
    pub(crate) fn of_user(name: &str) -> Folders {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let or_env = |id: &GUID, var: &str, fallback: PathBuf| {
            known_folder(id).or_else(|| std::env::var_os(var).map(PathBuf::from)).unwrap_or(fallback)
        };
        Folders {
            documents: known_folder(&FOLDERID_Documents).unwrap_or_else(|| home.join("Documents")),
            local: or_env(&FOLDERID_LocalAppData, "LOCALAPPDATA", home.join("AppData").join("Local")).join(name),
            roaming: or_env(&FOLDERID_RoamingAppData, "APPDATA", home.join("AppData").join("Roaming")).join(name),
        }
    }

    /// Stand-ins under one directory, for a self-test.
    pub(crate) fn under(dir: &Path) -> Folders {
        Folders { documents: dir.join("Documents"), local: dir.join("Local"), roaming: dir.join("Roaming") }
    }
}

/// What a self-test gets in place of the real clipboard and browser, so a
/// test can never touch what the person at the computer is doing.
#[derive(Default)]
pub(crate) struct StandIns {
    pub clipboard: Option<String>,
    pub opened: Vec<String>,
}

pub(crate) struct WinHost {
    /// The window that owns clipboard operations and browser launches. Zero
    /// until the window exists.
    pub hwnd: HWND,
    pub folders: Folders,
    pub stand_ins: Option<StandIns>,
    /// Shared with the window and with every waker handed out.
    pub wake: Arc<WakeLine>,
    faces: Faces,
}

impl WinHost {
    pub(crate) fn new(folders: Folders, quiet: bool) -> WinHost {
        WinHost { hwnd: 0, folders, stand_ins: quiet.then(StandIns::default), wake: Arc::new(WakeLine::default()), faces: Faces::new() }
    }
}

/// Milliseconds on the performance counter, which never goes backwards.
pub(crate) fn now_ms() -> u64 {
    (qpc().max(0) as u128 * 1000 / qpc_frequency() as u128) as u64
}

/// True for an address that is safe to hand to the shell: `http` or `https`,
/// and nothing in it that could end the address early and start an argument
/// to the browser. The text comes out of the user's documents, so it is
/// treated as hostile.
pub(crate) fn is_web_address(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        return false;
    };
    !rest.is_empty() && url.len() <= 2048 && !url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"')
}

/// What the clipboard holds for `text`, read back the way a paste reads it.
fn through_clipboard(text: &str) -> String {
    let units = clipboard_units(text);
    let units = &units[..units.len().saturating_sub(1)];
    String::from_utf16_lossy(units).replace("\r\n", "\n").replace('\r', "\n")
}

impl Host for WinHost {
    fn now_ms(&self) -> u64 {
        now_ms()
    }

    fn local_time(&self) -> LocalTime {
        let mut t = SYSTEMTIME::default();
        unsafe { GetLocalTime(&mut t) };
        let unix = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since) => since.as_secs() as i64,
            Err(before) => -(before.duration().as_secs() as i64),
        };
        LocalTime {
            year: t.wYear as i32,
            month: t.wMonth as u8,
            day: t.wDay as u8,
            hour: t.wHour as u8,
            minute: t.wMinute as u8,
            second: t.wSecond as u8,
            // Windows counts from Sunday; the interface counts from Monday.
            weekday: ((t.wDayOfWeek + 6) % 7) as u8,
            unix,
        }
    }

    fn waker(&self) -> Waker {
        let line = Arc::clone(&self.wake);
        Arc::new(move || line.wake())
    }

    fn clipboard_get(&mut self) -> Option<String> {
        match &self.stand_ins {
            Some(s) => s.clipboard.clone(),
            None => clipboard_get_text(self.hwnd),
        }
    }

    fn clipboard_set(&mut self, text: &str) {
        match &mut self.stand_ins {
            Some(s) => s.clipboard = Some(through_clipboard(text)),
            None => {
                clipboard_set_text(self.hwnd, text);
            }
        }
    }

    fn documents_dir(&self) -> PathBuf {
        self.folders.documents.clone()
    }

    fn local_data_dir(&self) -> PathBuf {
        self.folders.local.clone()
    }

    fn roaming_data_dir(&self) -> PathBuf {
        self.folders.roaming.clone()
    }

    fn system_faces(&mut self) -> Vec<String> {
        self.faces.system_faces()
    }

    fn load_face_file(&mut self, path: &Path) -> Option<String> {
        self.faces.load_file(path)
    }

    fn face_cell(&mut self, family: &str, height: u32) -> Option<FaceCell> {
        self.faces.cell(family, height)
    }

    fn rasterize(&mut self, family: &str, height: u32, ch: char) -> Option<AlphaGlyph> {
        self.faces.rasterize(family, height, ch)
    }

    fn open_in_browser(&mut self, url: &str) {
        if !is_web_address(url) {
            return;
        }
        match &mut self.stand_ins {
            Some(s) => s.opened.push(url.to_string()),
            None => unsafe {
                ShellExecuteW(self.hwnd, wide("open").as_ptr(), wide(url).as_ptr(), null(), null(), SW_SHOWNORMAL);
            },
        }
    }

    fn reduced_motion(&self) -> bool {
        let mut animations: BOOL = TRUE;
        unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut animations as *mut BOOL as *mut _, 0) };
        animations == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host whose folders are never touched by these tests.
    fn quiet_host() -> WinHost {
        WinHost::new(Folders::under(Path::new("unused")), true)
    }

    #[test]
    fn many_wakes_before_the_window_exists_are_kept_as_one() {
        let host = quiet_host();
        let wake = host.waker();
        assert!(!host.wake.take(), "nothing is owed until something asks");
        for _ in 0..10 {
            wake();
        }
        assert!(host.wake.take(), "the request waits for the window");
        assert!(!host.wake.take(), "ten calls were one request");
        // The waker is its own thing: it can leave for another thread and
        // be called after the host that made it has gone.
        let far = host.waker();
        drop(host);
        std::thread::spawn(move || far()).join().expect("a waker with no window does nothing");
        wake();
    }

    #[test]
    fn the_clock_does_not_go_backwards() {
        let host = quiet_host();
        let a = host.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let b = host.now_ms();
        assert!(b >= a + 20 && b < a + 5_000, "{a} then {b}");
    }

    #[test]
    fn local_time_is_a_real_date_and_monday_is_zero() {
        let t = quiet_host().local_time();
        assert!(t.year >= 2025 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 61 && t.weekday < 7);
        assert!(t.unix > 1_700_000_000);
        // Zeller's congruence gives the weekday of any date; check ours against it.
        let (mut y, mut m) = (t.year, t.month as i32);
        if m < 3 {
            m += 12;
            y -= 1;
        }
        let saturday_based = (t.day as i32 + 13 * (m + 1) / 5 + y % 100 + (y % 100) / 4 + (y / 100) / 4 + 5 * (y / 100)) % 7;
        assert_eq!(t.weekday as i32, (saturday_based + 5) % 7);
    }

    #[test]
    fn real_folders_end_in_the_application_name() {
        let f = Folders::of_user("ok");
        assert!(f.documents.is_absolute() && f.documents.is_dir(), "{:?}", f.documents);
        assert!(f.local.ends_with("ok") && f.local.parent().is_some_and(|p| p.is_dir()), "{:?}", f.local);
        assert!(f.roaming.ends_with("ok") && f.roaming.parent().is_some_and(|p| p.is_dir()), "{:?}", f.roaming);
        assert_ne!(f.local, f.roaming);
    }

    #[test]
    fn only_plain_web_addresses_reach_the_browser() {
        for good in ["http://example.com", "https://example.com/a?b=c#d", "HTTPS://Example.com/%20x"] {
            assert!(is_web_address(good), "{good}");
        }
        for bad in [
            "",
            "https://",
            "example.com",
            "file:///C:/Windows/notepad.exe",
            "C:\\Windows\\notepad.exe",
            "calc.exe",
            "javascript:alert(1)",
            "https://example.com/ --flag",
            "https://example.com/\" --flag",
            "https://example.com/\nmore",
            "https://exam\u{00A0}ple.com",
            " https://example.com",
            "htt",
            "héé",
        ] {
            assert!(!is_web_address(bad), "{bad:?}");
        }
        assert!(!is_web_address(&format!("https://example.com/{}", "a".repeat(3000))));
    }

    #[test]
    fn a_quiet_host_never_reaches_the_real_clipboard_or_browser() {
        let mut host = quiet_host();
        assert_eq!(host.clipboard_get(), None);
        let before = unsafe { GetClipboardSequenceNumber() };
        host.clipboard_set("one\ntwo\r\nthree \u{1F600}\n");
        assert_eq!(host.clipboard_get().as_deref(), Some("one\ntwo\nthree \u{1F600}\n"));
        assert_eq!(unsafe { GetClipboardSequenceNumber() }, before, "the real clipboard was written");
        host.open_in_browser("https://example.com/");
        host.open_in_browser("calc.exe");
        assert_eq!(host.stand_ins.as_ref().map(|s| s.opened.clone()), Some(vec!["https://example.com/".to_string()]));
    }

    #[test]
    fn line_ends_are_windows_going_out_and_plain_coming_in() {
        let units = clipboard_units("a\nb\r\nc");
        assert_eq!(String::from_utf16_lossy(&units), "a\r\nb\r\nc\0");
        assert_eq!(through_clipboard("a\nb\r\nc\rd"), "a\nb\nc\nd");
    }

    #[test]
    fn the_host_lists_faces_and_measures_them() {
        let mut host = quiet_host();
        let faces = host.system_faces();
        assert!(faces.iter().any(|f| f == "Consolas"), "{faces:?}");
        assert!(faces.iter().any(|f| f == "Courier New"), "{faces:?}");
        assert!(!faces.iter().any(|f| f == "Arial" || f.starts_with('@')), "{faces:?}");
        assert_eq!(host.face_cell("Courier New", 22), Some(FaceCell { width: 12, height: 22, ascent: 17 }));
        assert!(host.rasterize("Courier New", 22, 'Q').is_some_and(|g| g.width > 0));
    }

    /// Makes a copy of Consolas under another family name, so the face is
    /// certainly not installed, and loads it from the file.
    #[test]
    fn a_face_file_is_loaded_for_this_process_and_named() {
        let Ok(mut bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") else { return };
        let be = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect() };
        let (from, to) = (be("Consolas"), be("Okprobe1"));
        let mut i = 0;
        while i + from.len() <= bytes.len() {
            if bytes[i..i + from.len()] == from[..] {
                bytes[i..i + from.len()].copy_from_slice(&to);
                i += from.len();
            } else {
                i += 1;
            }
        }
        let dir = crate::scratch("face-file");
        let path = dir.join("okprobe1.ttf");
        std::fs::write(&path, &bytes).expect("scratch font written");

        let mut host = quiet_host();
        assert_eq!(host.face_cell("Okprobe1", 22), None, "not installed");
        assert_eq!(host.load_face_file(&path).as_deref(), Some("Okprobe1"));
        assert_eq!(host.face_cell("Okprobe1", 22), Some(FaceCell { width: 10, height: 22, ascent: 17 }));
        assert!(host.rasterize("Okprobe1", 22, 'g').is_some_and(|g| g.width > 0));
        assert!(host.system_faces().iter().any(|f| f == "Okprobe1"));
        assert_eq!(host.load_face_file(&path).as_deref(), Some("Okprobe1"), "loading twice is harmless");

        assert_eq!(host.load_face_file(Path::new(r"C:\no\such\face.ttf")), None);
        let junk = dir.join("junk.ttf");
        std::fs::write(&junk, b"this is not a font").expect("junk written");
        assert_eq!(host.load_face_file(&junk), None);
        drop(host);

        // Gone again once the host that loaded it is dropped.
        let mut after = quiet_host();
        assert_eq!(after.face_cell("Okprobe1", 22), None);
    }
}
