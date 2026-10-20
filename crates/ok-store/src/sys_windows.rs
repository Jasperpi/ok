//! The kernel32 calls std does not reach, declared by hand, and the thin safe
//! wrappers the rest of the crate uses. Everything here was first run in the
//! durability probes on the target machine; the packing of `FILE_RENAME_INFO`
//! in particular is copied from there, not from memory.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};

pub(crate) use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

/// A path as the UTF-16 units it is kept as in the journal.
pub(crate) fn wide_units(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

/// The path kept as `units`.
pub(crate) fn wide_path(units: &[u16]) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_wide(units))
}
use std::path::{Path, PathBuf};

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

pub(crate) use crate::calendar::{filetime_to_system, filetime_to_unix_ms, unix_ms_from_utc, unix_ms_now, utc_fields, FILETIME_EPOCH};

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
    fn the_volume_root_is_the_prefix_and_nothing_more() {
        assert_eq!(volume_root(Path::new(r"\\?\C:\Users\x\Documents\ok")), PathBuf::from(r"\\?\C:\"));
        assert_eq!(volume_root(Path::new(r"C:\a\b")), PathBuf::from(r"C:\"));
        assert_eq!(volume_root(Path::new(r"\\?\UNC\server\share\a")), PathBuf::from(r"\\?\UNC\server\share\"));
    }
}
