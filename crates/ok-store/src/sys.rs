//! The kernel32 calls std does not reach, declared by hand, and the thin safe
//! wrappers the rest of the crate uses. Everything here was first run in the
//! durability probes on the target machine; the packing of `FILE_RENAME_INFO`
//! in particular is copied from there, not from memory.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
/// The three attribute bits that mark a file whose bytes are not on this
/// disk: offline, recall on open (seen in listings) and recall on data access.
pub(crate) const ATTR_PLACEHOLDER: u32 = 0x1000 | 0x0004_0000 | 0x0040_0000;

const FILE_LIST_DIRECTORY: u32 = 1;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// Opens a link itself and not the file it leads to.
pub(crate) const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const INVALID_HANDLE: isize = -1;

const CLASS_RENAME: u32 = 3;
const CLASS_DISPOSITION: u32 = 4;
const CLASS_RENAME_EX: u32 = 22;
const RENAME_REPLACE: u32 = 1;
const RENAME_POSIX: u32 = 2;

const SUPPORTS_POSIX_RENAME: u32 = 0x400;
const SUPPORTS_OPEN_BY_ID: u32 = 0x0100_0000;

const NOTIFY_FILE_NAME: u32 = 0x1;
const NOTIFY_DIR_NAME: u32 = 0x2;
const NOTIFY_ATTRIBUTES: u32 = 0x4;
const NOTIFY_SIZE: u32 = 0x8;
const NOTIFY_LAST_WRITE: u32 = 0x10;

const WAIT_TIMEOUT: u32 = 258;

/// 100 ns ticks between 1601 and 1970.
const FILETIME_EPOCH: u64 = 116_444_736_000_000_000;

#[repr(C)]
#[derive(Default)]
struct RawInfo {
    attrs: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[repr(C)]
struct FileId {
    size: u32,
    kind: u32,
    id: i64,
    pad: u64,
}

#[repr(C)]
#[derive(Default)]
struct WallFields {
    year: u16,
    month: u16,
    weekday: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
    fn CloseHandle(handle: isize) -> i32;
    fn SetFileInformationByHandle(handle: isize, class: u32, info: *const c_void, size: u32) -> i32;
    fn GetFileInformationByHandle(handle: isize, info: *mut RawInfo) -> i32;
    fn GetVolumeInformationByHandleW(
        handle: isize,
        name: *mut u16,
        name_len: u32,
        serial: *mut u32,
        component: *mut u32,
        flags: *mut u32,
        system: *mut u16,
        system_len: u32,
    ) -> i32;
    fn OpenFileById(hint: isize, id: *const FileId, access: u32, share: u32, security: *const c_void, flags: u32) -> isize;
    fn GetFinalPathNameByHandleW(handle: isize, buffer: *mut u16, len: u32, flags: u32) -> u32;
    fn FindFirstChangeNotificationW(path: *const u16, subtree: i32, filter: u32) -> isize;
    fn FindNextChangeNotification(handle: isize) -> i32;
    fn FindCloseChangeNotification(handle: isize) -> i32;
    fn WaitForMultipleObjects(count: u32, handles: *const isize, all: i32, ms: u32) -> u32;
    fn CreateEventW(security: *const c_void, manual: i32, initial: i32, name: *const u16) -> isize;
    fn SetEvent(handle: isize) -> i32;
    fn FileTimeToSystemTime(time: *const u64, out: *mut WallFields) -> i32;
    fn SystemTimeToTzSpecificLocalTime(zone: *const c_void, utc: *const WallFields, local: *mut WallFields) -> i32;
}

fn raw(file: &File) -> isize {
    file.as_raw_handle() as isize
}

fn last_error() -> u32 {
    // SAFETY: reads the calling thread's last-error value; no arguments.
    unsafe { GetLastError() }
}

/// The Win32 error number inside an `io::Error`, or 0 when it has none.
pub(crate) fn code(error: &io::Error) -> u32 {
    error.raw_os_error().map(|c| c as u32).unwrap_or(0)
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

/// Size, last-write time and file index through an open handle.
pub(crate) fn info(file: &File) -> io::Result<Info> {
    let mut r = RawInfo::default();
    // SAFETY: `r` is a live, correctly sized BY_HANDLE_FILE_INFORMATION and the handle is open.
    if unsafe { GetFileInformationByHandle(raw(file), &mut r) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let wide = |pair: [u32; 2]| ((pair[1] as u64) << 32) | pair[0] as u64;
    Ok(Info {
        attrs: r.attrs,
        created: wide(r.created),
        modified: wide(r.written),
        size: ((r.size_high as u64) << 32) | r.size_low as u64,
        links: r.links,
        index: ((r.index_high as u64) << 32) | r.index_low as u64,
    })
}

/// Opens a file or folder for looking only: no access asked for, everything
/// shared, so it never gets in anyone's way.
pub(crate) fn open_for_stat(path: &Path) -> io::Result<File> {
    OpenOptions::new().access_mode(0).share_mode(SHARE_ALL).custom_flags(FLAG_BACKUP_SEMANTICS).open(path)
}

/// [`info`] by path.
pub(crate) fn stat(path: &Path) -> io::Result<Info> {
    info(&open_for_stat(path)?)
}

/// Renames the file behind `file` to `to`, which must be an absolute verbatim
/// path (a bare name would be resolved against the process's current
/// directory). `posix` picks the rename that works while other programs hold
/// the target open with full sharing; without it the old rename is used, which
/// every file system has. Returns the Win32 error number on failure.
pub(crate) fn rename_handle(file: &File, to: &Path, replace: bool, posix: bool) -> Result<(), u32> {
    let name: Vec<u16> = to.as_os_str().encode_wide().collect();
    let name_bytes = name.len() * 2;
    // FILE_RENAME_INFO on x64: Flags (or ReplaceIfExists) at 0, RootDirectory
    // at 8, FileNameLength at 16, FileName at 20.
    let size = (20 + name_bytes + 2).max(24);
    let mut buffer: Vec<u64> = vec![0; size.div_ceil(8)];
    let (class, flags) = match (posix, replace) {
        (true, true) => (CLASS_RENAME_EX, RENAME_POSIX | RENAME_REPLACE),
        (true, false) => (CLASS_RENAME_EX, RENAME_POSIX),
        (false, true) => (CLASS_RENAME, 1),
        (false, false) => (CLASS_RENAME, 0),
    };
    let p = buffer.as_mut_ptr() as *mut u8;
    // SAFETY: the buffer is 8-byte aligned, zeroed and at least `size` bytes;
    // every write below stays inside it.
    unsafe {
        *(p as *mut u32) = flags;
        *(p.add(16) as *mut u32) = name_bytes as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr() as *const u8, p.add(20), name_bytes);
        if SetFileInformationByHandle(raw(file), class, p as *const c_void, size as u32) == 0 {
            return Err(last_error());
        }
    }
    Ok(())
}

/// Marks the file behind `file` to vanish when its last handle closes. This is
/// how a temp file or a spent journal is given up without reopening it by name.
pub(crate) fn delete_on_close(file: &File) -> bool {
    let yes: u8 = 1;
    // SAFETY: FILE_DISPOSITION_INFO is one BOOLEAN; the pointer is valid for the call.
    unsafe { SetFileInformationByHandle(raw(file), CLASS_DISPOSITION, &yes as *const u8 as *const c_void, 1) != 0 }
}

/// What the volume under a folder can do.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Volume {
    /// A file can be replaced by a rename while other programs have it open,
    /// in one step. Without it a save here is not atomic.
    pub atomic_replace: bool,
    /// Files can be found again by their index after being renamed.
    pub open_by_id: bool,
    /// The file system's name, such as `NTFS` or `exFAT`. Empty if unknown.
    pub file_system: String,
}

/// Asks the volume holding `folder` what it supports. Any failure reads as
/// "supports neither".
pub(crate) fn volume(folder: &Path) -> Volume {
    let Ok(dir) = open_for_stat(folder) else { return Volume::default() };
    let mut flags = 0u32;
    let mut system = [0u16; 64];
    // SAFETY: the out pointers are valid for the lengths given; unused outputs are null.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            raw(&dir),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            system.as_mut_ptr(),
            system.len() as u32,
        )
    };
    if ok == 0 {
        return Volume::default();
    }
    let end = system.iter().position(|&c| c == 0).unwrap_or(system.len());
    Volume {
        atomic_replace: flags & SUPPORTS_POSIX_RENAME != 0,
        open_by_id: flags & SUPPORTS_OPEN_BY_ID != 0,
        file_system: String::from_utf16_lossy(&system[..end]),
    }
}

/// Finds a file again by its index on the volume `hint` lives on. Every
/// failure means "not found".
pub(crate) fn open_by_id(hint: &File, index: u64) -> Option<File> {
    let id = FileId { size: std::mem::size_of::<FileId>() as u32, kind: 0, id: index as i64, pad: 0 };
    // SAFETY: `id` is a valid FILE_ID_DESCRIPTOR for the duration of the call.
    let handle = unsafe { OpenFileById(raw(hint), &id, FILE_READ_ATTRIBUTES, SHARE_ALL, std::ptr::null(), FLAG_BACKUP_SEMANTICS) };
    if handle == INVALID_HANDLE || handle == 0 {
        return None;
    }
    // SAFETY: the handle was just returned to us and nothing else owns it.
    Some(unsafe { File::from_raw_handle(handle as *mut c_void) })
}

/// The present full path of an open file, in verbatim form.
pub(crate) fn final_path(file: &File) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: the buffer is valid for `buffer.len()` units.
        let n = unsafe { GetFinalPathNameByHandleW(raw(file), buffer.as_mut_ptr(), buffer.len() as u32, 0) } as usize;
        if n == 0 {
            return None;
        }
        if n < buffer.len() {
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n])));
        }
        if n > 40_000 {
            return None;
        }
        buffer = vec![0u16; n + 1];
    }
}

/// A standing request to be told when anything under a folder changes.
pub(crate) struct Watch(isize);

impl Watch {
    pub(crate) fn new(folder: &Path) -> Option<Watch> {
        let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
        let filter = NOTIFY_FILE_NAME | NOTIFY_DIR_NAME | NOTIFY_ATTRIBUTES | NOTIFY_SIZE | NOTIFY_LAST_WRITE;
        // SAFETY: `wide` is nul-terminated and outlives the call.
        let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, filter) };
        if handle == INVALID_HANDLE || handle == 0 {
            None
        } else {
            Some(Watch(handle))
        }
    }

    /// Re-arms the watch after it has signalled. False means it is dead.
    pub(crate) fn again(&self) -> bool {
        // SAFETY: the handle is a live change-notification handle.
        unsafe { FindNextChangeNotification(self.0) != 0 }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and is closed exactly once.
        unsafe { FindCloseChangeNotification(self.0) };
    }
}

/// An auto-reset event: one thread sets it, another wakes from its wait.
pub(crate) struct Event(isize);

impl Event {
    pub(crate) fn new() -> Option<Event> {
        // SAFETY: all arguments are plain values or null.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle == 0 {
            None
        } else {
            Some(Event(handle))
        }
    }

    pub(crate) fn set(&self) {
        // SAFETY: the handle is a live event.
        unsafe { SetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Which of the two things a wait was ended by.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Woke {
    Event,
    Watch,
    Timeout,
    Failed,
}

/// Waits for the event, the watch (if there is one), or `ms` milliseconds
/// (`None` waits without end).
pub(crate) fn wait(event: &Event, watch: Option<&Watch>, ms: Option<u32>) -> Woke {
    let handles = [event.0, watch.map(|w| w.0).unwrap_or(0)];
    let count = if watch.is_some() { 2 } else { 1 };
    // SAFETY: `handles` holds `count` live handles.
    let r = unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, ms.unwrap_or(u32::MAX)) };
    match r {
        0 => Woke::Event,
        1 if count == 2 => Woke::Watch,
        WAIT_TIMEOUT => Woke::Timeout,
        _ => Woke::Failed,
    }
}

/// Milliseconds since 1970, UTC.
pub(crate) fn unix_ms_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// FILETIME ticks to milliseconds since 1970.
pub(crate) fn filetime_to_unix_ms(ticks: u64) -> u64 {
    ticks.saturating_sub(FILETIME_EPOCH) / 10_000
}

/// FILETIME ticks as a `SystemTime`, for handing a creation time to std.
pub(crate) fn filetime_to_system(ticks: u64) -> Option<SystemTime> {
    let since = ticks.checked_sub(FILETIME_EPOCH)?;
    UNIX_EPOCH.checked_add(Duration::from_nanos(since.checked_mul(100)?))
}

/// The local calendar fields of a UTC instant: year, month, day, hour,
/// minute, second. Falls back to UTC arithmetic if Windows will not say.
pub(crate) fn local_fields(unix_ms: u64) -> (u16, u8, u8, u8, u8, u8) {
    let ticks = unix_ms.saturating_mul(10_000).saturating_add(FILETIME_EPOCH);
    let mut utc = WallFields::default();
    let mut local = WallFields::default();
    // SAFETY: all three pointers are valid for the calls; a null zone means the current one.
    let ok = unsafe { FileTimeToSystemTime(&ticks, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0 };
    if ok {
        (local.year, local.month as u8, local.day as u8, local.hour as u8, local.minute as u8, local.second as u8)
    } else {
        utc_fields(unix_ms)
    }
}

/// The UTC calendar fields of an instant, by arithmetic alone.
pub(crate) fn utc_fields(unix_ms: u64) -> (u16, u8, u8, u8, u8, u8) {
    let secs = unix_ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm), valid far beyond any date a file can carry.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }) as u16;
    (year, month, day, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// The inverse of [`utc_fields`]: milliseconds since 1970 for a UTC calendar
/// time, or `None` for fields that are not a date.
pub(crate) fn unix_ms_from_utc(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<u64> {
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;
    u64::try_from(secs).ok().map(|s| s * 1000)
}

/// Opens a folder for listing, used as the hint for [`open_by_id`].
pub(crate) fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new().access_mode(FILE_LIST_DIRECTORY).share_mode(SHARE_ALL).custom_flags(FLAG_BACKUP_SEMANTICS).open(path)
}

/// The root of the volume a path is on (`\\?\C:\` for `\\?\C:\a\b`): a place
/// that still exists after the folder itself has been renamed away.
pub(crate) fn volume_root(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => out.push(part.as_os_str()),
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_fields_match_known_dates() {
        assert_eq!(utc_fields(0), (1970, 1, 1, 0, 0, 0));
        // 2026-10-11 14:32:05 UTC
        assert_eq!(utc_fields(1_791_729_125_000), (2026, 10, 11, 14, 32, 5));
        // A leap day.
        assert_eq!(utc_fields(1_709_210_096_000), (2024, 2, 29, 12, 34, 56));
    }

    #[test]
    fn filetime_conversions_agree() {
        let ms = 1_791_729_125_123u64;
        let ticks = ms * 10_000 + FILETIME_EPOCH;
        assert_eq!(filetime_to_unix_ms(ticks), ms);
        let system = filetime_to_system(ticks).unwrap();
        assert_eq!(system.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64, ms);
        assert!(filetime_to_system(5).is_none());
    }

    #[test]
    fn calendar_fields_convert_both_ways() {
        for ms in [0u64, 1_791_729_125_000, 1_709_210_096_000, 951_782_400_000, 4_102_444_799_000] {
            let (y, mo, d, h, mi, s) = utc_fields(ms);
            assert_eq!(unix_ms_from_utc(y, mo, d, h, mi, s), Some(ms), "{ms}");
        }
        assert_eq!(unix_ms_from_utc(2026, 13, 1, 0, 0, 0), None);
        assert_eq!(unix_ms_from_utc(1969, 12, 31, 23, 59, 59), None);
        assert_eq!(unix_ms_from_utc(2026, 10, 11, 24, 0, 0), None);
    }

    #[test]
    fn the_volume_root_is_the_prefix_and_nothing_more() {
        assert_eq!(volume_root(Path::new(r"\\?\C:\Users\x\Documents\ok")), PathBuf::from(r"\\?\C:\"));
        assert_eq!(volume_root(Path::new(r"C:\a\b")), PathBuf::from(r"C:\"));
        assert_eq!(volume_root(Path::new(r"\\?\UNC\server\share\a")), PathBuf::from(r"\\?\UNC\server\share\"));
    }
}
