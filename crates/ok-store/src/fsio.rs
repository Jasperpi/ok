//! The file operations everything else is built from: reading a file while
//! denying writers (so a half-written file is never taken for the real one),
//! and writing a file whole through a temp that is flushed, renamed by its own
//! handle, and flushed again. Nothing here ever truncates or writes in place.

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Write as _};
use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::hash;
use crate::names;
use crate::sys::{self, Info};
use crate::wal::Stamp;
use crate::Faults;

/// Why a file operation did not happen. Each one is a state the caller can
/// act on; none of them means anything was half done to an existing file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fail {
    /// No file by that name.
    Missing,
    /// The folder (or the drive) is not there.
    FolderGone,
    /// Another program has the file open in a way that keeps us out.
    Busy,
    /// Access denied for a reason other than the read-only attribute.
    Denied,
    /// The file carries the read-only attribute.
    ReadOnly,
    /// The file has more than one name (hard links); replacing it by rename
    /// would leave the other names pointing at the old text.
    Linked,
    /// A file that must not be replaced is in the way.
    NameTaken,
    DiskFull,
    /// The folder will not take new files.
    CannotWrite,
    /// The target was swapped for another file while we worked.
    Changed,
    /// Larger than we are willing to hold in memory.
    TooLarge,
    /// A path that could not be put into the form Windows needs.
    BadPath,
    /// The volume said it had the one-step replacing rename and then refused
    /// it, and the old rename could not be made in its place this time
    /// (it cannot while the target is held open, even by our own guard).
    /// Nothing was written: try again with the old rename from the start.
    NoOneStep,
    /// The version that has to be kept before this write could not be kept,
    /// so the write was not made.
    NoHistory,
    /// The new file is in place but the flush after the rename failed, so a
    /// power cut could still undo it.
    NotDurable(u32),
    /// Anything else, with the Win32 error number.
    Io(u32),
}

/// What a Win32 error from opening or looking at a file means for us.
pub(crate) fn open_fail(code: u32) -> Fail {
    match code {
        2 => Fail::Missing,
        3 | 15 | 21 | 53 | 67 => Fail::FolderGone,
        32 | 33 => Fail::Busy,
        5 => Fail::Denied,
        123 | 161 | 206 => Fail::BadPath,
        c => Fail::Io(c),
    }
}

fn write_fail(error: &io::Error) -> Fail {
    match sys::code(error) {
        39 | 112 => Fail::DiskFull,
        3 | 15 | 21 | 53 | 67 => Fail::FolderGone,
        19 => Fail::CannotWrite,
        c => Fail::Io(c),
    }
}

/// Opens a file for reading while denying write access to everyone else.
/// If a program is in the middle of writing the file this fails with
/// [`Fail::Busy`] instead of reading whatever half is there; and while the
/// handle is held no writer can get in. Other readers, and programs that
/// replace the file by rename, are not disturbed.
pub(crate) fn open_guard(path: &Path) -> Result<(File, Info), Fail> {
    let file = OpenOptions::new().read(true).share_mode(sys::SHARE_READ | sys::SHARE_DELETE).open(path).map_err(|e| open_fail(sys::code(&e)))?;
    let info = sys::info(&file).map_err(|e| Fail::Io(sys::code(&e)))?;
    if info.attrs & sys::ATTR_DIRECTORY != 0 {
        return Err(Fail::Denied);
    }
    Ok((file, info))
}

/// Reads everything behind an open handle, refusing files over `limit` bytes.
pub(crate) fn read_all(file: &File, size: u64, limit: u64) -> Result<Vec<u8>, Fail> {
    if size > limit {
        return Err(Fail::TooLarge);
    }
    let mut bytes = Vec::with_capacity(size as usize + 1);
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| match sys::code(&e) {
        33 => Fail::Busy,
        c => Fail::Io(c),
    })?;
    if bytes.len() as u64 > limit {
        return Err(Fail::TooLarge);
    }
    Ok(bytes)
}

/// A file read whole under a deny-write guard that is still held.
pub(crate) struct Guarded {
    pub file: File,
    pub info: Info,
    pub bytes: Vec<u8>,
}

pub(crate) fn read_guarded(path: &Path, limit: u64) -> Result<Guarded, Fail> {
    let (file, info) = open_guard(path)?;
    let bytes = read_all(&file, info.size, limit)?;
    Ok(Guarded { file, info, bytes })
}

/// [`read_guarded`], waiting out a writer for the given pauses (milliseconds)
/// before giving up with [`Fail::Busy`].
pub(crate) fn read_patiently(path: &Path, limit: u64, pauses: &[u64]) -> Result<Guarded, Fail> {
    let mut pauses = pauses.iter();
    loop {
        match read_guarded(path, limit) {
            Err(Fail::Busy) => match pauses.next() {
                Some(&ms) => std::thread::sleep(Duration::from_millis(ms)),
                None => return Err(Fail::Busy),
            },
            other => return other,
        }
    }
}

/// `~<file name>.<8 random hex>.tmp`: sorts apart from his files, is not
/// listed as a text, and sync tools leave `.tmp` alone.
pub(crate) fn temp_name(target: &Path) -> String {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stem: String = name;
    while stem.encode_utf16().count() > 120 {
        stem.pop();
    }
    format!("~{stem}.{:08x}.tmp", hash::random() as u32)
}

/// True for names of the shape [`temp_name`] makes.
pub(crate) fn is_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('~').and_then(|n| n.strip_suffix(".tmp")) else { return false };
    match rest.rsplit_once('.') {
        Some((_, hex)) => hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

/// One whole-file write.
pub(crate) struct Write<'a> {
    pub target: &'a Path,
    /// The temp name already announced to the journal, used for the first try.
    pub first_temp: Option<&'a str>,
    pub bytes: &'a [u8],
    /// Creation time to carry over from the file being replaced.
    pub created: Option<u64>,
    /// False for a name believed to be new: then an existing file is never
    /// replaced and the write fails with [`Fail::NameTaken`].
    pub replace: bool,
    /// Use the rename that replaces in one step even while others hold the
    /// target open. False on volumes that lack it.
    pub posix: bool,
    pub faults: &'a Faults,
}

pub(crate) struct Written {
    pub stamp: Stamp,
    pub created: u64,
    /// The volume claimed the one-step rename and then refused it; the old
    /// rename was used instead and should be from now on.
    pub posix_refused: bool,
}

/// The two moments a whole-file write asks about the file it replaces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Look {
    /// The temp is written and flushed; the rename comes next.
    Before,
    /// The rename has been made.
    After,
}

/// The check made on either side of renaming a temp over `target`: is the
/// file there the one that was read and compared under `guard`?
///
/// Where the one-step rename exists the guard stays open. It denies writers,
/// so the text can only have changed by the file being replaced, removed or
/// renamed away, and each of those the guard's own handle can tell in a few
/// microseconds without opening the name again: a file that lost its name
/// has no links, and one that was renamed says so. That is asked just before
/// the rename. Just after it, the file that was to be replaced must have no
/// name left; if it still has one, it was renamed away at the last instant
/// and our rename replaced nothing but made a second file, which is then
/// given up again.
///
/// Where the one-step rename does not exist the guard has to be closed
/// first, because the old rename is refused while the target has any open
/// handle, ours included; size and last-write time are then the best
/// evidence there is, and nothing can be asked afterwards.
pub(crate) fn verify_unchanged(target: &Path, before: &Info, posix: bool, guard: &mut Option<File>, look: Look) -> Result<(), Fail> {
    if !posix {
        *guard = None;
    }
    if let (true, Some(file)) = (posix, guard.as_ref()) {
        let now = sys::info(file).map_err(|e| Fail::Io(sys::code(&e)))?;
        let fine = match look {
            Look::Before => now.links > 0 && sys::final_path(file).is_some_and(|path| path == target),
            Look::After => now.links == 0,
        };
        return if fine { Ok(()) } else { Err(Fail::Changed) };
    }
    if look == Look::After {
        return Ok(());
    }
    let now = sys::stat(target).map_err(|e| match open_fail(sys::code(&e)) {
        Fail::Missing => Fail::Changed,
        other => other,
    })?;
    let same = if posix { now.index == before.index } else { now.size == before.size && now.modified == before.modified };
    if same {
        Ok(())
    } else {
        Err(Fail::Changed)
    }
}

/// Reads a file with full sharing, as a last resort when a writer keeps the
/// deny-write open out. What comes back may be half of what is being written,
/// so it is only ever shown, never compared against or written from.
pub(crate) fn read_shared(path: &Path, limit: u64) -> Result<(Vec<u8>, Info), Fail> {
    let file = OpenOptions::new().read(true).share_mode(sys::SHARE_ALL).open(path).map_err(|e| open_fail(sys::code(&e)))?;
    let info = sys::info(&file).map_err(|e| Fail::Io(sys::code(&e)))?;
    let bytes = read_all(&file, info.size, limit)?;
    Ok((bytes, info))
}

/// Flushes a file that is already in place, and with it the rename that put
/// it there. Used when a file is found to hold the right bytes but nothing
/// records that it was ever flushed. Nothing is written to it.
pub(crate) fn flush_in_place(path: &Path) -> bool {
    OpenOptions::new().write(true).share_mode(sys::SHARE_READ | sys::SHARE_DELETE).open(path).and_then(|file| file.sync_all()).is_ok()
}

fn is_readonly(path: &Path) -> bool {
    sys::stat(path).map(|i| i.attrs & sys::ATTR_READONLY != 0).unwrap_or(false)
}

/// Writes `bytes` as a new file and moves it onto `target`:
/// temp created beside the target with a handle that is never closed until
/// the end; written; creation time carried over; flushed; `verify` asked
/// whether the target is still the file that was compared; renamed through
/// the same handle; `verify` asked again whether that rename replaced the
/// file it was meant to; flushed again so the rename itself is on disk;
/// stamped. On any failure before the rename the temp is given up through
/// its handle and the target has not been touched. If `verify` says no after
/// the rename, the file just put there is given up the same way: it replaced
/// nothing, and the file it was meant for lives on under another name.
pub(crate) fn write_file(w: &Write, verify: &mut dyn FnMut(Look) -> Result<(), Fail>) -> Result<Written, Fail> {
    let target = names::verbatim(w.target).ok_or(Fail::BadPath)?;
    let dir = w.target.parent().ok_or(Fail::BadPath)?;
    let mut denied = 0;
    let mut opened = None;
    for attempt in 0..5 {
        // A fresh random name every time: a temp given up while a scanner
        // still reads it keeps its name taken until the scanner lets go.
        let name = match (attempt, w.first_temp) {
            (0, Some(first)) => first.to_string(),
            _ => temp_name(w.target),
        };
        let made = OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(sys::GENERIC_WRITE | sys::DELETE | sys::SYNCHRONIZE)
            .share_mode(sys::SHARE_READ | sys::SHARE_DELETE)
            .open(dir.join(&name));
        match made {
            Ok(file) => {
                opened = Some(file);
                break;
            }
            Err(e) => match sys::code(&e) {
                80 | 183 | 32 | 33 => continue,
                5 => {
                    denied += 1;
                    if denied >= 2 {
                        return Err(Fail::CannotWrite);
                    }
                }
                19 => return Err(Fail::CannotWrite),
                _ => return Err(write_fail(&e)),
            },
        }
    }
    let Some(mut file) = opened else { return Err(Fail::CannotWrite) };
    let give_up = |file: File, fail: Fail| -> Result<Written, Fail> {
        sys::delete_on_close(&file);
        drop(file);
        Err(fail)
    };

    let injected = w.faults.temp_write.load(Ordering::Relaxed);
    let wrote = if injected != 0 { Err(io::Error::from_raw_os_error(injected as i32)) } else { file.write_all(w.bytes) };
    if let Err(e) = wrote {
        return give_up(file, write_fail(&e));
    }
    if let Some(created) = w.created.and_then(sys::filetime_to_system) {
        let _ = file.set_times(FileTimes::new().set_created(created));
    }
    // Without this flush a power cut after the rename could leave a file of
    // the right length full of zeros.
    if let Err(e) = file.sync_all() {
        return give_up(file, write_fail(&e));
    }
    if let Err(fail) = verify(Look::Before) {
        return give_up(file, fail);
    }

    let mut refused = false;
    let mut renamed = sys::rename_handle(&file, &target, w.replace, w.posix);
    if w.posix && matches!(renamed, Err(87 | 50 | 1)) {
        refused = true;
        renamed = sys::rename_handle(&file, &target, w.replace, false);
    }
    if let Err(code) = renamed {
        let fail = match code {
            5 | 32 | 33 if refused => Fail::NoOneStep,
            183 | 80 if !w.replace => Fail::NameTaken,
            183 | 80 | 32 | 33 | 5 if w.replace && is_readonly(w.target) => Fail::ReadOnly,
            32 | 33 => Fail::Busy,
            5 if w.replace => Fail::Busy,
            5 if sys::stat(w.target).is_ok() => Fail::NameTaken,
            5 | 19 => Fail::CannotWrite,
            2 | 3 | 15 | 21 | 53 | 67 => Fail::FolderGone,
            39 | 112 => Fail::DiskFull,
            123 | 161 | 206 => Fail::BadPath,
            c => Fail::Io(c),
        };
        return give_up(file, fail);
    }
    if w.replace {
        if let Err(fail) = verify(Look::After) {
            return give_up(file, fail);
        }
    }

    // The rename is file-system metadata and metadata is always cached: only
    // after this second flush is the new file sure to survive a power cut.
    let injected = w.faults.second_flush.load(Ordering::Relaxed);
    let flushed = if injected != 0 { Err(io::Error::from_raw_os_error(injected as i32)) } else { file.sync_all() };
    if let Err(e) = flushed {
        return Err(Fail::NotDurable(sys::code(&e)));
    }
    let info = sys::info(&file).map_err(|e| Fail::NotDurable(sys::code(&e)))?;
    Ok(Written { stamp: Stamp { size: info.size, modified: info.modified, index: info.index }, created: info.created, posix_refused: refused })
}

/// [`write_file`] for files of our own (history, state): nothing to verify,
/// and the one-step rename is tried first with the old one as fallback.
pub(crate) fn write_plain(target: &Path, bytes: &[u8], replace: bool) -> Result<Written, Fail> {
    write_plain_with(target, bytes, replace, &Faults::default())
}

/// [`write_plain`] for a file in the writing folder, where a test's injected
/// failures apply as they do to a save.
pub(crate) fn write_plain_with(target: &Path, bytes: &[u8], replace: bool, faults: &Faults) -> Result<Written, Fail> {
    write_file(&Write { target, first_temp: None, bytes, created: None, replace, posix: true, faults }, &mut |_| Ok(()))
}

/// Writes `bytes` at `base`, or at `base` with `-2`, `-3`… before its ending
/// if that name is taken, never replacing anything. If a file already there
/// holds exactly these bytes it is taken as done, which makes a repeated
/// recovery harmless; but only once it has been flushed where it lies,
/// because the run that put it there may have been stopped before its own
/// flush, and whoever called this is about to let a journal go on the
/// strength of the file. Returns the path used.
pub(crate) fn write_numbered(base: &Path, bytes: &[u8], faults: &Faults) -> Result<PathBuf, Fail> {
    for n in 1..=99 {
        let path = names::numbered(base, n);
        match write_plain_with(&path, bytes, false, faults) {
            Ok(_) => return Ok(path),
            Err(Fail::NameTaken) => {
                if fs::read(&path).map(|there| there == bytes).unwrap_or(false) {
                    return if flush_in_place(&path) { Ok(path) } else { Err(Fail::NotDurable(0)) };
                }
            }
            Err(other) => return Err(other),
        }
    }
    Err(Fail::NameTaken)
}

/// Moves a file to a new name on the same volume without ever replacing
/// what is there. The file is opened only for the move and nothing is
/// copied. A link is moved as the link it is, not the file it leads to.
pub(crate) fn move_no_replace(from: &Path, to: &Path) -> Result<(), Fail> {
    let to = names::verbatim(to).ok_or(Fail::BadPath)?;
    let file = OpenOptions::new().access_mode(sys::DELETE | sys::SYNCHRONIZE).share_mode(sys::SHARE_ALL).custom_flags(sys::FLAG_OPEN_REPARSE_POINT).open(from).map_err(|e| open_fail(sys::code(&e)))?;
    sys::rename_handle(&file, &to, false, false).map_err(|code| match code {
        183 | 80 => Fail::NameTaken,
        32 | 33 => Fail::Busy,
        5 => Fail::Denied,
        3 | 15 | 21 | 53 | 67 => Fail::FolderGone,
        c => Fail::Io(c),
    })
}

/// Moves a leftover temp file into the data directory's trash, by rename when
/// both are on one volume and by copy, compare, remove when they are not.
/// Returns false if the file is still where it was.
pub(crate) fn to_trash(data: &Path, file: &Path, now_ms: u64) -> bool {
    let trash = data.join("trash");
    if fs::create_dir_all(&trash).is_err() {
        return false;
    }
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (y, mo, d, h, mi, s) = sys::utc_fields(now_ms);
    let base = trash.join(format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02} {name}"));
    let mut landed = None;
    for n in 1..=99 {
        let dest = names::numbered(&base, n);
        match move_no_replace(file, &dest) {
            Ok(()) => {
                landed = Some(dest);
                break;
            }
            Err(Fail::NameTaken) => continue,
            Err(Fail::Io(17)) => {
                // Another volume: copy, check the copy, then let the original go.
                let Ok(bytes) = fs::read(file) else { return false };
                match write_plain(&dest, &bytes, false) {
                    Ok(_) => {
                        if fs::read(&dest).map(|copy| copy == bytes).unwrap_or(false) && fs::remove_file(file).is_ok() {
                            landed = Some(dest);
                        }
                        break;
                    }
                    Err(Fail::NameTaken) => continue,
                    Err(_) => return false,
                }
            }
            Err(_) => return false,
        }
    }
    let Some(dest) = landed else { return false };
    let line = format!("{now_ms}\t{}\t{}\r\n", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), file.display());
    if let Ok(mut index) = OpenOptions::new().create(true).append(true).open(trash.join("index.tsv")) {
        let _ = index.write_all(line.as_bytes());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn ok() -> impl FnMut(Look) -> Result<(), Fail> {
        |_| Ok(())
    }

    #[test]
    fn a_new_file_is_written_whole_and_leaves_no_temp() {
        let dir = Scratch::new("fsio-new");
        let target = dir.path().join("essay.md");
        let faults = Faults::default();
        let w = Write { target: &target, first_temp: Some("~essay.md.00000001.tmp"), bytes: b"first", created: None, replace: false, posix: true, faults: &faults };
        let written = write_file(&w, &mut ok()).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        assert_eq!(written.stamp.size, 5);
        assert_eq!(written.stamp, { let i = sys::stat(&target).unwrap(); Stamp { size: i.size, modified: i.modified, index: i.index } });
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_name_believed_new_is_never_replaced() {
        let dir = Scratch::new("fsio-noreplace");
        let target = dir.path().join("essay.md");
        fs::write(&target, b"already here").unwrap();
        for posix in [true, false] {
            let faults = Faults::default();
            let w = Write { target: &target, first_temp: None, bytes: b"mine", created: None, replace: false, posix, faults: &faults };
            assert_eq!(write_file(&w, &mut ok()).err(), Some(Fail::NameTaken));
            assert_eq!(fs::read(&target).unwrap(), b"already here");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "the temp was given up");
        }
    }

    #[test]
    fn replacing_keeps_the_creation_time_on_both_rename_paths() {
        let dir = Scratch::new("fsio-replace");
        let target = dir.path().join("essay.md");
        fs::write(&target, b"old").unwrap();
        let created = sys::stat(&target).unwrap().created;
        std::thread::sleep(Duration::from_millis(30));
        for (posix, text) in [(true, &b"new one"[..]), (false, &b"new two"[..])] {
            let faults = Faults::default();
            let w = Write { target: &target, first_temp: None, bytes: text, created: Some(created), replace: true, posix, faults: &faults };
            let written = write_file(&w, &mut ok()).unwrap();
            assert!(!written.posix_refused);
            assert_eq!(fs::read(&target).unwrap(), text);
            assert_eq!(sys::stat(&target).unwrap().created, created);
            assert_eq!(written.created, created);
        }
    }

    #[test]
    fn a_failed_verify_or_write_leaves_the_target_alone() {
        let dir = Scratch::new("fsio-verify");
        let target = dir.path().join("essay.md");
        fs::write(&target, b"old").unwrap();
        let faults = Faults::default();
        let w = Write { target: &target, first_temp: None, bytes: b"new", created: None, replace: true, posix: true, faults: &faults };
        assert_eq!(write_file(&w, &mut |_| Err(Fail::Changed)).err(), Some(Fail::Changed));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        faults.temp_write.store(112, Ordering::Relaxed);
        assert_eq!(write_file(&w, &mut ok()).err(), Some(Fail::DiskFull));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        faults.temp_write.store(0, Ordering::Relaxed);
        faults.second_flush.store(1117, Ordering::Relaxed);
        assert_eq!(write_file(&w, &mut ok()).err(), Some(Fail::NotDurable(1117)));
        assert_eq!(fs::read(&target).unwrap(), b"new", "the rename had happened; only its durability is unknown");
    }

    /// A save as the saver makes it, with something another program does to
    /// the file slipped in at a chosen moment: after the compare and before
    /// the store's last look, or between that look and the rename.
    fn save_with(dir: &Path, before_look: &dyn Fn(), after_look: &dyn Fn()) -> Result<Written, Fail> {
        let target = fs::canonicalize(dir).unwrap().join("essay.md");
        let found = read_guarded(&target, 1000).unwrap();
        assert_eq!(found.bytes, b"old");
        let info = found.info;
        let mut guard = Some(found.file);
        let faults = Faults::default();
        let w = Write { target: &target, first_temp: None, bytes: b"new", created: None, replace: true, posix: true, faults: &faults };
        write_file(&w, &mut |look| {
            if look == Look::Before {
                before_look();
            }
            let answer = verify_unchanged(&target, &info, true, &mut guard, look);
            if look == Look::Before && answer.is_ok() {
                after_look();
            }
            answer
        })
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn what_another_program_does_before_the_last_look_is_always_seen() {
        if !sys::volume(&crate::testing::scratch_root()).atomic_replace {
            return;
        }
        type Meddle = fn(&Path);
        let meddles: [(&str, Meddle); 4] = [
            ("renames its own version over the file", |dir| {
                fs::write(dir.join("theirs.tmp"), b"theirs").unwrap();
                fs::rename(dir.join("theirs.tmp"), dir.join("essay.md")).unwrap();
            }),
            ("deletes the file", |dir| fs::remove_file(dir.join("essay.md")).unwrap()),
            ("renames the file away", |dir| fs::rename(dir.join("essay.md"), dir.join("renamed.md")).unwrap()),
            ("changes the capitals of its name", |dir| fs::rename(dir.join("essay.md"), dir.join("Essay.md")).unwrap()),
        ];
        for (what, meddle) in meddles {
            let dir = Scratch::new("fsio-look");
            fs::write(dir.path().join("essay.md"), b"old").unwrap();
            let result = save_with(dir.path(), &|| meddle(dir.path()), &|| {});
            assert_eq!(result.err(), Some(Fail::Changed), "another program {what}");
            // Nothing of ours is left, and what the other program did stands.
            let names = listing(dir.path());
            assert_eq!(names.len(), if what.starts_with("deletes") { 0 } else { 1 }, "another program {what}: {names:?}");
            match what {
                "renames its own version over the file" => assert_eq!(fs::read(dir.path().join("essay.md")).unwrap(), b"theirs"),
                "renames the file away" => assert_eq!(fs::read(dir.path().join("renamed.md")).unwrap(), b"old"),
                "changes the capitals of its name" => assert_eq!(names, ["Essay.md"]),
                _ => {}
            }
        }
    }

    #[test]
    fn a_file_renamed_away_between_the_last_look_and_the_rename_is_not_left_as_two() {
        if !sys::volume(&crate::testing::scratch_root()).atomic_replace {
            return;
        }
        // The rename finds no file under the name and makes one. The look
        // after it sees that the file it meant to replace still has a name,
        // and the file just made is given up again.
        let dir = Scratch::new("fsio-look-after");
        fs::write(dir.path().join("essay.md"), b"old").unwrap();
        let result = save_with(dir.path(), &|| {}, &|| fs::rename(dir.path().join("essay.md"), dir.path().join("renamed.md")).unwrap());
        assert_eq!(result.err(), Some(Fail::Changed));
        assert_eq!(listing(dir.path()), ["renamed.md"]);
        assert_eq!(fs::read(dir.path().join("renamed.md")).unwrap(), b"old");

        // Nothing in the way: the save goes through and both looks are content.
        let dir = Scratch::new("fsio-look-plain");
        fs::write(dir.path().join("essay.md"), b"old").unwrap();
        save_with(dir.path(), &|| {}, &|| {}).unwrap();
        assert_eq!(listing(dir.path()), ["essay.md"]);
        assert_eq!(fs::read(dir.path().join("essay.md")).unwrap(), b"new");
    }

    #[test]
    fn a_copy_already_in_place_is_flushed_before_it_is_taken_as_done() {
        let dir = Scratch::new("fsio-numbered-flush");
        let base = dir.path().join("copy.md");
        let faults = Faults::default();
        assert_eq!(write_numbered(&base, b"one", &faults).unwrap(), base);
        // A copy that cannot be flushed where it lies (here: it is read-only)
        // is not vouched for: the caller keeps its journal.
        let mut perms = fs::metadata(&base).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&base, perms.clone()).unwrap();
        assert_eq!(write_numbered(&base, b"one", &faults).err(), Some(Fail::NotDurable(0)));
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&base, perms).unwrap();
        assert_eq!(write_numbered(&base, b"one", &faults).unwrap(), base);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        // A copy written through the injected failures of a save fails as a save does.
        faults.temp_write.store(112, Ordering::Relaxed);
        assert_eq!(write_numbered(&dir.path().join("other.md"), b"x", &faults).err(), Some(Fail::DiskFull));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_read_only_target_is_reported_and_untouched() {
        let dir = Scratch::new("fsio-readonly");
        let target = dir.path().join("essay.md");
        fs::write(&target, b"old").unwrap();
        let mut perms = fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&target, perms.clone()).unwrap();
        for posix in [true, false] {
            let faults = Faults::default();
            let w = Write { target: &target, first_temp: None, bytes: b"new", created: None, replace: true, posix, faults: &faults };
            assert_eq!(write_file(&w, &mut ok()).err(), Some(Fail::ReadOnly));
            assert_eq!(fs::read(&target).unwrap(), b"old");
        }
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&target, perms).unwrap();
    }

    #[test]
    fn a_missing_folder_is_reported_as_such() {
        let dir = Scratch::new("fsio-gone");
        let target = dir.path().join("no such folder").join("essay.md");
        assert_eq!(write_plain(&target, b"x", false).err(), Some(Fail::FolderGone));
        assert_eq!(read_guarded(&target, 100).err(), Some(Fail::FolderGone));
        assert_eq!(read_guarded(&dir.path().join("none.md"), 100).err(), Some(Fail::Missing));
        assert_eq!(open_guard(dir.path()).err(), Some(Fail::Denied));
    }

    #[test]
    fn the_guard_denies_writers_and_is_refused_by_one() {
        let dir = Scratch::new("fsio-guard");
        let target = dir.path().join("essay.md");
        fs::write(&target, b"whole text").unwrap();
        // A writer already in: we do not read what may be half a file.
        let writer = OpenOptions::new().write(true).open(&target).unwrap();
        assert_eq!(read_guarded(&target, 100).err(), Some(Fail::Busy));
        assert_eq!(read_patiently(&target, 100, &[1, 1]).err(), Some(Fail::Busy));
        drop(writer);
        let guarded = read_guarded(&target, 100).unwrap();
        assert_eq!(guarded.bytes, b"whole text");
        // While we hold the guard a writer cannot get in; a reader can.
        let refused = OpenOptions::new().write(true).open(&target);
        assert_eq!(refused.err().map(|e| sys::code(&e)), Some(32));
        assert_eq!(fs::read(&target).unwrap(), b"whole text");
        drop(guarded);
        assert!(OpenOptions::new().write(true).open(&target).is_ok());
        assert_eq!(read_guarded(&target, 4).err(), Some(Fail::TooLarge));
    }

    #[test]
    fn numbered_writes_never_overwrite_and_are_idempotent() {
        let dir = Scratch::new("fsio-numbered");
        let base = dir.path().join("copy.md");
        let faults = Faults::default();
        assert_eq!(write_numbered(&base, b"one", &faults).unwrap(), base);
        assert_eq!(write_numbered(&base, b"one", &faults).unwrap(), base, "same bytes: already done");
        assert_eq!(write_numbered(&base, b"two", &faults).unwrap(), dir.path().join("copy-2.md"));
        assert_eq!(write_numbered(&base, b"three", &faults).unwrap(), dir.path().join("copy-3.md"));
        assert_eq!(write_numbered(&base, b"two", &faults).unwrap(), dir.path().join("copy-2.md"));
        assert_eq!(fs::read(&base).unwrap(), b"one");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn moving_never_replaces_and_trash_keeps_everything() {
        let dir = Scratch::new("fsio-move");
        let a = dir.path().join("a.md");
        let b = dir.path().join("b.md");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        assert_eq!(move_no_replace(&a, &b).err(), Some(Fail::NameTaken));
        assert_eq!((fs::read(&a).unwrap(), fs::read(&b).unwrap()), (b"a".to_vec(), b"b".to_vec()));
        let c = dir.path().join("c.md");
        move_no_replace(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(fs::read(&c).unwrap(), b"a");
        assert_eq!(move_no_replace(&a, &c).err(), Some(Fail::Missing));

        let data = dir.path().join("data");
        let t1 = dir.path().join("~x.md.0000aaaa.tmp");
        fs::write(&t1, b"first").unwrap();
        assert!(to_trash(&data, &t1, 1_791_729_125_000));
        fs::write(&t1, b"second").unwrap();
        assert!(to_trash(&data, &t1, 1_791_729_125_000));
        assert!(!t1.exists());
        let kept: Vec<_> = fs::read_dir(data.join("trash")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(kept.iter().filter(|n| n.contains("~x.md")).count(), 2, "{kept:?}");
        let index = fs::read_to_string(data.join("trash").join("index.tsv")).unwrap();
        assert_eq!(index.lines().count(), 2);
    }

    #[test]
    fn temp_names_have_the_expected_shape() {
        let name = temp_name(Path::new(r"C:\w\The orchard.md"));
        assert!(name.starts_with("~The orchard.md.") && name.ends_with(".tmp"), "{name}");
        assert!(is_temp_name(&name));
        assert!(is_temp_name("~a.md.0a1B2c3d.tmp"));
        for no in ["a.md", "~a.md.tmp", "~a.md.0a1b2c3.tmp", "~a.md.0a1b2c3g.tmp", "a.md.0a1b2c3d.tmp", "~$essay.docx"] {
            assert!(!is_temp_name(no), "{no}");
        }
        let long = temp_name(Path::new(&format!("{}.md", "x".repeat(250))));
        assert!(long.encode_utf16().count() < 140);
    }
}
