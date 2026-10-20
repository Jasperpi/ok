//! The same calls as `sys_windows.rs`, for macOS and Linux. The rest of the
//! crate was written against Win32 handles and Win32 error numbers, so this
//! file speaks that language: an open file is renamed through the path it has
//! now, "delete on close" is an unlink while it is still open, and every
//! error is handed back as the Win32 number that means the same thing.
//!
//! Two things Windows has and these systems do not: a file cannot be found
//! again by its index, and there is no folder watch every system shares, so
//! the watch looks at the folder about once a second.

use std::ffi::{c_char, c_int, c_long, CString};
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[allow(unused_imports)]
pub(crate) use crate::calendar::{filetime_to_system, filetime_to_unix_ms, unix_ms_from_utc, unix_ms_now, utc_fields};
use crate::calendar::system_to_filetime;

pub(crate) const GENERIC_READ: u32 = 0x8000_0000;
pub(crate) const GENERIC_WRITE: u32 = 0x4000_0000;
pub(crate) const DELETE: u32 = 0x0001_0000;
pub(crate) const SYNCHRONIZE: u32 = 0x0010_0000;
pub(crate) const SHARE_READ: u32 = 1;
pub(crate) const SHARE_WRITE: u32 = 2;
pub(crate) const SHARE_DELETE: u32 = 4;
pub(crate) const SHARE_ALL: u32 = SHARE_READ | SHARE_WRITE | SHARE_DELETE;

pub(crate) const ATTR_READONLY: u32 = 0x1;
pub(crate) const ATTR_DIRECTORY: u32 = 0x10;
pub(crate) const ATTR_REPARSE: u32 = 0x400;
/// Never set here: these systems have no placeholder files.
pub(crate) const ATTR_PLACEHOLDER: u32 = 0x1000 | 0x0004_0000 | 0x0040_0000;

/// How long the folder watch waits between looks.
const WATCH_EVERY: Duration = Duration::from_millis(1000);
/// A folder with more entries than this is not watched at all.
const WATCH_MOST: usize = 20_000;

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
    fn rename(from: *const c_char, to: *const c_char) -> c_int;
    fn unlink(path: *const c_char) -> c_int;
    #[cfg(target_os = "linux")]
    fn renameat2(from_dir: c_int, from: *const c_char, to_dir: c_int, to: *const c_char, flags: u32) -> c_int;
    #[cfg(target_os = "macos")]
    fn renamex_np(from: *const c_char, to: *const c_char, flags: u32) -> c_int;
    #[cfg(target_os = "macos")]
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

/// The options Windows takes when opening a file, mapped onto what these
/// systems have. Access becomes read and write; sharing has no counterpart
/// and is dropped.
pub(crate) trait OpenOptionsExt {
    fn access_mode(&mut self, access: u32) -> &mut Self;
    fn share_mode(&mut self, share: u32) -> &mut Self;
}

impl OpenOptionsExt for OpenOptions {
    fn access_mode(&mut self, access: u32) -> &mut Self {
        if access & GENERIC_WRITE != 0 {
            self.write(true);
        }
        if access & GENERIC_READ != 0 || access & GENERIC_WRITE == 0 {
            self.read(true);
        }
        self
    }

    fn share_mode(&mut self, _share: u32) -> &mut Self {
        self
    }
}

/// A path as the UTF-16 units it is kept as in the journal, so a journal
/// reads the same whichever system wrote it.
pub(crate) fn wide_units(path: &Path) -> Vec<u16> {
    path.to_string_lossy().encode_utf16().collect()
}

/// The path kept as `units`.
pub(crate) fn wide_path(units: &[u16]) -> PathBuf {
    PathBuf::from(String::from_utf16_lossy(units))
}

/// What a listing says of a file, in the Windows terms the folder walk uses.
/// A link is a link: it is not followed.
pub(crate) trait MetadataExt {
    fn file_attributes(&self) -> u32;
    fn file_size(&self) -> u64;
    fn last_write_time(&self) -> u64;
    fn creation_time(&self) -> u64;
}

impl MetadataExt for fs::Metadata {
    fn file_attributes(&self) -> u32 {
        let mut attrs = 0;
        if self.is_dir() {
            attrs |= ATTR_DIRECTORY;
        }
        if self.file_type().is_symlink() {
            attrs |= ATTR_REPARSE;
        }
        if self.permissions().mode() & 0o200 == 0 {
            attrs |= ATTR_READONLY;
        }
        attrs
    }

    fn file_size(&self) -> u64 {
        self.len()
    }

    fn last_write_time(&self) -> u64 {
        self.modified().map(system_to_filetime).unwrap_or(0)
    }

    fn creation_time(&self) -> u64 {
        self.created().map(system_to_filetime).unwrap_or_else(|_| self.last_write_time())
    }
}

/// The errno behind a failed call, as the Win32 number for the same failure.
fn last_error() -> u32 {
    win32(io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

/// The Win32 number for an errno. Numbers with no counterpart pass through
/// unchanged, which also keeps the Win32 numbers tests inject meaning what
/// they meant.
fn win32(errno: i32) -> u32 {
    #[cfg(target_os = "macos")]
    const NAME_TOO_LONG: i32 = 63;
    #[cfg(not(target_os = "macos"))]
    const NAME_TOO_LONG: i32 = 36;
    match errno {
        1 | 13 | 21 => 5,  // EPERM, EACCES, EISDIR: access denied
        16 | 26 => 32,     // EBUSY, ETXTBSY: in use
        17 => 80,          // EEXIST: the name is taken
        18 => 17,          // EXDEV: not the same device
        20 => 3,           // ENOTDIR: a folder on the way is missing
        22 => 87,          // EINVAL
        28 | 69 | 122 => 112, // ENOSPC, EDQUOT: disk full
        30 => 19,          // EROFS: write protected
        e if e == NAME_TOO_LONG => 206,
        e => e.max(0) as u32,
    }
}

/// The Win32 error number inside an `io::Error`, or 0 when it has none.
pub(crate) fn code(error: &io::Error) -> u32 {
    error.raw_os_error().map(win32).unwrap_or(0)
}

fn c_path(path: &Path) -> Result<CString, u32> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| 123)
}

/// What one look at a file tells us. Times are FILETIME ticks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Info {
    pub attrs: u32,
    pub created: u64,
    pub modified: u64,
    pub size: u64,
    pub links: u32,
    pub index: u64,
}

/// Size, times and inode through an open file.
pub(crate) fn info(file: &File) -> io::Result<Info> {
    let meta = file.metadata()?;
    let mut attrs = 0;
    if meta.is_dir() {
        attrs |= ATTR_DIRECTORY;
    }
    if meta.permissions().mode() & 0o200 == 0 {
        attrs |= ATTR_READONLY;
    }
    let modified = meta.modified().map(system_to_filetime).unwrap_or(0);
    Ok(Info {
        attrs,
        created: meta.created().map(system_to_filetime).unwrap_or(modified),
        modified,
        size: meta.len(),
        links: meta.nlink() as u32,
        index: meta.ino(),
    })
}

/// Opens a file or folder for looking only.
pub(crate) fn open_for_stat(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// [`info`] by path.
pub(crate) fn stat(path: &Path) -> io::Result<Info> {
    info(&open_for_stat(path)?)
}

/// Renames `from` to `to`, refusing with 80 if `to` is there and `replace`
/// is false.
pub(crate) fn rename_path(from: &Path, to: &Path, replace: bool) -> Result<(), u32> {
    let (a, b) = (c_path(from)?, c_path(to)?);
    if replace {
        // SAFETY: both are nul-terminated paths that outlive the call.
        return if unsafe { rename(a.as_ptr(), b.as_ptr()) } == 0 { Ok(()) } else { Err(last_error()) };
    }
    #[cfg(target_os = "linux")]
    // SAFETY: as above; -100 is AT_FDCWD and 1 is RENAME_NOREPLACE.
    let done = unsafe { renameat2(-100, a.as_ptr(), -100, b.as_ptr(), 1) };
    #[cfg(target_os = "macos")]
    // SAFETY: as above; 4 is RENAME_EXCL.
    let done = unsafe { renamex_np(a.as_ptr(), b.as_ptr(), 4) };
    if done == 0 {
        return Ok(());
    }
    let error = last_error();
    // A file system that cannot refuse in one step: look, then rename.
    if matches!(error, 87 | 38 | 45 | 95 | 102) {
        if fs::symlink_metadata(to).is_ok() {
            return Err(80);
        }
        // SAFETY: as above.
        return if unsafe { rename(a.as_ptr(), b.as_ptr()) } == 0 { Ok(()) } else { Err(last_error()) };
    }
    Err(error)
}

/// Renames the file behind `file` to `to`, found through the name it has
/// now. `posix` makes no difference here: every rename on these systems is
/// the one-step kind. Returns the Win32 error number on failure.
pub(crate) fn rename_handle(file: &File, to: &Path, replace: bool, _posix: bool) -> Result<(), u32> {
    let from = final_path(file).ok_or(2u32)?;
    rename_path(&from, to, replace)
}

/// Gives up the file behind `file`: its name goes now and its bytes when the
/// last handle closes.
pub(crate) fn delete_on_close(file: &File) -> bool {
    let Some(path) = final_path(file) else { return false };
    let Ok(name) = c_path(&path) else { return false };
    // SAFETY: a nul-terminated path that outlives the call.
    unsafe { unlink(name.as_ptr()) == 0 }
}

/// What the volume under a folder can do.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Volume {
    /// A file can be replaced by a rename while other programs have it open,
    /// in one step. Without it a save here is not atomic.
    pub atomic_replace: bool,
    /// Files can be found again by their index after being renamed.
    pub open_by_id: bool,
    /// The file system's name. Empty if unknown.
    pub file_system: String,
}

/// Every rename here replaces in one step; nothing is found by index.
pub(crate) fn volume(folder: &Path) -> Volume {
    Volume { atomic_replace: fs::metadata(folder).is_ok(), open_by_id: false, file_system: String::new() }
}

/// Never found: these systems cannot open a file by its inode.
pub(crate) fn open_by_id(_hint: &File, _index: u64) -> Option<File> {
    None
}

/// The present full path of an open file, or `None` once it has no name.
#[cfg(target_os = "linux")]
pub(crate) fn final_path(file: &File) -> Option<PathBuf> {
    let path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
    if path.as_os_str().as_bytes().ends_with(b" (deleted)") || !path.is_absolute() {
        return None;
    }
    Some(path)
}

/// The present full path of an open file, or `None` once it has no name.
#[cfg(target_os = "macos")]
pub(crate) fn final_path(file: &File) -> Option<PathBuf> {
    const F_GETPATH: c_int = 50;
    let mut buffer = vec![0u8; 1024];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN (1024) bytes into the buffer.
    if unsafe { fcntl(file.as_raw_fd(), F_GETPATH, buffer.as_mut_ptr()) } == -1 {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0)?;
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..end]));
    // A file that has been unlinked keeps its last path here; it is gone if
    // that path is now something else or nothing.
    let here = fs::metadata(&path).ok()?;
    let mine = file.metadata().ok()?;
    (here.ino() == mine.ino() && here.dev() == mine.dev()).then_some(path)
}

/// A standing request to be told when anything under a folder changes. It
/// looks: the names, sizes and times of everything under the folder are
/// summed up, and a different sum is a change.
pub(crate) struct Watch {
    folder: PathBuf,
    /// The last sum, when the next look is due, and whether a look has been
    /// made since the watch began.
    seen: Mutex<(u64, Instant, bool)>,
}

impl Watch {
    pub(crate) fn new(folder: &Path) -> Option<Watch> {
        let seen = look(folder)?;
        Some(Watch { folder: folder.to_path_buf(), seen: Mutex::new((seen, Instant::now() + WATCH_EVERY, false)) })
    }

    /// How long until the next look.
    fn until_due(&self) -> Duration {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).1.saturating_duration_since(Instant::now())
    }

    /// The watch never dies.
    pub(crate) fn again(&self) -> bool {
        true
    }

    /// True when the folder is not as it was last time. The first look
    /// always counts: the sum the watch began with may be older than what
    /// this program wrote since, and a file that has gone again would match
    /// it. A folder that cannot be summed up now (it is gone, or a sub-folder
    /// cannot be read) counts as changed, once.
    fn changed(&self) -> bool {
        let now = look(&self.folder).unwrap_or(0);
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let changed = now != seen.0 || !seen.2;
        *seen = (now, Instant::now() + WATCH_EVERY, true);
        changed
    }
}

/// The sum of a folder: every entry's name, size and time, and the time of
/// every folder in it.
fn look(folder: &Path) -> Option<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut stack = vec![folder.to_path_buf()];
    let mut count = 0usize;
    while let Some(dir) = stack.pop() {
        // A folder's own time moves on with every name made or removed in
        // it, so a folder that has come back to how it looked before still
        // sums up differently.
        fs::metadata(&dir).ok()?.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).hash(&mut hasher);
        let mut entries: Vec<_> = fs::read_dir(&dir).ok()?.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            count += 1;
            if count > WATCH_MOST {
                return None;
            }
            let Ok(meta) = entry.metadata() else { continue };
            entry.file_name().hash(&mut hasher);
            meta.len().hash(&mut hasher);
            meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).hash(&mut hasher);
            meta.permissions().mode().hash(&mut hasher);
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Some(hasher.finish())
}

/// An auto-reset event: one thread sets it, another wakes from its wait.
pub(crate) struct Event {
    set: Mutex<bool>,
    woken: Condvar,
}

impl Event {
    pub(crate) fn new() -> Option<Event> {
        Some(Event { set: Mutex::new(false), woken: Condvar::new() })
    }

    pub(crate) fn set(&self) {
        *self.set.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.woken.notify_all();
    }

    /// Waits up to `ms` for the event, clearing it. True if it was set.
    fn take(&self, ms: Duration) -> bool {
        let deadline = Instant::now() + ms;
        let mut set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        while !*set {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            set = self.woken.wait_timeout(set, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        *set = false;
        true
    }
}

/// Which of the two things a wait was ended by.
#[derive(PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub(crate) enum Woke {
    Event,
    Watch,
    Timeout,
    Failed,
}

/// Waits for the event, the watch (if there is one), or `ms` milliseconds
/// (`None` waits without end).
pub(crate) fn wait(event: &Event, watch: Option<&Watch>, ms: Option<u32>) -> Woke {
    // The watch looks on its own schedule, so an event that keeps being set
    // can never keep a change from being seen.
    let deadline = ms.map(|ms| Instant::now() + Duration::from_millis(ms as u64));
    loop {
        if let Some(watch) = watch {
            if watch.until_due().is_zero() && watch.changed() {
                return Woke::Watch;
            }
        }
        let left = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        let step = match (watch, left) {
            (Some(watch), Some(left)) => left.min(watch.until_due()),
            (Some(watch), None) => watch.until_due(),
            (None, Some(left)) => left,
            (None, None) => Duration::from_secs(3600),
        };
        if event.take(step) {
            return Woke::Event;
        }
        if deadline.is_some_and(|at| Instant::now() >= at) {
            return Woke::Timeout;
        }
    }
}

/// The local calendar fields of a UTC instant: year, month, day, hour,
/// minute, second. Falls back to UTC if the system will not say.
pub(crate) fn local_fields(unix_ms: u64) -> (u16, u8, u8, u8, u8, u8) {
    let secs = (unix_ms / 1000) as i64;
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
    // SAFETY: both pointers are valid for the call; localtime_r is thread-safe.
    if unsafe { localtime_r(&secs, &mut tm) }.is_null() {
        return utc_fields(unix_ms);
    }
    ((tm.year + 1900) as u16, (tm.mon + 1) as u8, tm.mday as u8, tm.hour as u8, tm.min as u8, tm.sec as u8)
}

/// Opens a folder for listing, used as the hint for [`open_by_id`].
pub(crate) fn open_dir(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// The root of the volume a path is on: the mount point of a removable or
/// network volume where these systems put them, or `/`. It is a place that
/// still exists after the folder itself has been renamed away.
pub(crate) fn volume_root(path: &Path) -> PathBuf {
    let parts: Vec<Component> = path.components().collect();
    let names: Vec<&str> = parts.iter().filter_map(|part| part.as_os_str().to_str()).collect();
    let depth = match names.as_slice() {
        ["/", "Volumes", _, ..] => 3,
        ["/", "media", _, _, ..] | ["/", "run", "media", _, ..] => if names[1] == "run" { 5 } else { 4 },
        ["/", "mnt", _, ..] => 3,
        _ => 1,
    };
    parts.iter().take(depth.min(parts.len())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_root_is_the_mount_point_or_slash() {
        assert_eq!(volume_root(Path::new("/Users/x/Documents/ok")), PathBuf::from("/"));
        assert_eq!(volume_root(Path::new("/Volumes/Stick/ok")), PathBuf::from("/Volumes/Stick"));
        assert_eq!(volume_root(Path::new("/media/jasper/Stick/ok")), PathBuf::from("/media/jasper/Stick"));
        assert_eq!(volume_root(Path::new("/run/media/jasper/Stick/ok")), PathBuf::from("/run/media/jasper/Stick"));
    }

    #[test]
    fn errors_read_as_their_win32_numbers() {
        assert_eq!(win32(2), 2);
        assert_eq!(win32(13), 5);
        assert_eq!(win32(17), 80);
        assert_eq!(win32(28), 112);
    }

    #[test]
    fn a_rename_that_must_not_replace_does_not() {
        let dir = std::env::temp_dir().join(format!("ok-sys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        assert_eq!(rename_path(&a, &b, false), Err(80));
        let file = File::open(&a).unwrap();
        assert_eq!(final_path(&file).map(|p| fs::canonicalize(p).unwrap()), Some(fs::canonicalize(&a).unwrap()));
        rename_handle(&file, &dir.join("c"), false, true).unwrap();
        assert_eq!(fs::read_to_string(dir.join("c")).unwrap(), "a");
        assert!(delete_on_close(&file));
        assert!(!dir.join("c").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
