//! The folder lock that keeps two instances off one writing folder, and the
//! small named files (window placement, last text) kept under the data
//! directory and always replaced whole.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::fsio::{self, Fail};
use crate::names;
use crate::sys;

/// Who holds a writing folder.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Owner {
    /// The process id of the instance that has the folder. 0 if it could not
    /// be read.
    pub pid: u32,
    /// Whatever that instance passed as its note when it opened the folder:
    /// the interface puts its window handle here so a second launch can hand
    /// over to the first.
    pub note: String,
    /// The folder as that instance spelled it.
    pub folder: PathBuf,
    /// True when the folder is not open in any window: an instance started
    /// on another folder holds it for the moment it takes to bring back a
    /// crashed session's text here. [`crate::Store::open`] waits such a
    /// holder out, so this is seen only if it stayed for seconds; there is
    /// then nobody to hand over to, and opening again will work.
    pub passing: bool,
}

/// The lock on one writing folder. It is a file held open so that no second
/// open can succeed; Windows lets go of it the moment the process ends, so
/// there is never a stale lock to clear.
pub(crate) struct Lock {
    _file: File,
}

pub(crate) enum LockFail {
    Held(Owner),
    Io(u32),
}

fn lock_path(data: &Path, root: &Path) -> PathBuf {
    data.join("lock").join(format!("{}.lock", names::folder_key(root)))
}

fn read_owner(path: &Path) -> Owner {
    let mut owner = Owner::default();
    // The winner writes its details just after winning; give it a moment.
    for _ in 0..20 {
        let mut text = String::new();
        if let Ok(mut file) = OpenOptions::new().read(true).share_mode(sys::SHARE_ALL).open(path) {
            let _ = file.read_to_string(&mut text);
        }
        for line in text.lines() {
            if let Some(pid) = line.strip_prefix("pid=") {
                owner.pid = pid.trim().parse().unwrap_or(0);
            } else if let Some(note) = line.strip_prefix("note=") {
                owner.note = note.to_string();
            } else if let Some(folder) = line.strip_prefix("folder=") {
                owner.folder = PathBuf::from(folder);
            } else if let Some(passing) = line.strip_prefix("passing=") {
                owner.passing = passing.trim() == "1";
            }
        }
        if owner.pid != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    owner
}

/// Takes the lock on `root`, or says who has it. `passing` marks a lock held
/// only for the length of one recovery by an instance whose own folder is
/// another one, so that the folder's own instance, starting meanwhile, knows
/// to wait a moment instead of handing over to a window that is not there.
pub(crate) fn take_lock(data: &Path, root: &Path, note: &str, passing: bool) -> Result<Lock, LockFail> {
    let path = lock_path(data, root);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| LockFail::Io(sys::code(&e)))?;
    }
    match OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(sys::SHARE_READ).open(&path) {
        Ok(mut file) => {
            let note: String = note.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
            let text = format!("pid={}\npassing={}\nnote={note}\nfolder={}\n", std::process::id(), passing as u8, root.display());
            let _ = file.set_len(0);
            let _ = file.write_all(text.as_bytes());
            let _ = file.flush();
            Ok(Lock { _file: file })
        }
        Err(e) => match sys::code(&e) {
            32 | 33 => Err(LockFail::Held(read_owner(&path))),
            code => Err(LockFail::Io(code)),
        },
    }
}

/// Where a named state file lives, or `None` for a name that is not a plain
/// file name.
pub(crate) fn state_path(data: &Path, name: &str) -> Option<PathBuf> {
    names::check_name(name).ok()?;
    Some(data.join("state").join(name))
}

/// Reads a named state file whole. `None` if it is not there.
pub(crate) fn read_state(data: &Path, name: &str) -> Option<Vec<u8>> {
    fs::read(state_path(data, name)?).ok()
}

/// Replaces a named state file whole: after a crash it holds the old bytes
/// or the new, never a mixture.
pub(crate) fn write_state(data: &Path, name: &str, bytes: &[u8]) -> Result<(), Fail> {
    let path = state_path(data, name).ok_or(Fail::BadPath)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| fsio::open_fail(sys::code(&e)))?;
    }
    fsio::write_plain(&path, bytes, true).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn the_lock_admits_one_and_names_the_holder() {
        let dir = Scratch::new("state-lock");
        let root = Path::new(r"\\?\C:\Users\x\Documents\ok");
        let first = take_lock(dir.path(), root, "window 1234\nsecond line", false);
        assert!(first.is_ok());
        // A different spelling of the same folder meets the same lock.
        match take_lock(dir.path(), Path::new(r"\\?\c:\users\X\documents\OK"), "late", false) {
            Err(LockFail::Held(owner)) => {
                assert_eq!(owner.pid, std::process::id());
                assert_eq!(owner.note, "window 1234 second line");
                assert_eq!(owner.folder, root);
                assert!(!owner.passing);
            }
            _ => panic!("the second taker must be refused"),
        }
        // Another folder is another lock.
        assert!(take_lock(dir.path(), Path::new(r"\\?\C:\elsewhere"), "", false).is_ok());
        drop(first);
        // Held only in passing, for a recovery: whoever is refused is told so.
        let passing = take_lock(dir.path(), root, "again", true);
        assert!(passing.is_ok());
        match take_lock(dir.path(), root, "the folder's own window", false) {
            Err(LockFail::Held(owner)) => assert!(owner.passing && owner.note == "again"),
            _ => panic!("the second taker must be refused"),
        }
    }

    #[test]
    fn state_files_are_replaced_whole_and_names_are_checked() {
        let dir = Scratch::new("state-files");
        assert_eq!(read_state(dir.path(), "window"), None);
        write_state(dir.path(), "window", b"0 0 1920 1080").unwrap();
        write_state(dir.path(), "window", b"10 10 800 600").unwrap();
        assert_eq!(read_state(dir.path(), "window").unwrap(), b"10 10 800 600");
        assert_eq!(fs::read_dir(dir.path().join("state")).unwrap().count(), 1);
        for bad in [r"..\escape", "a/b", "", "CON", "x:y"] {
            assert_eq!(write_state(dir.path(), bad, b"x").err(), Some(Fail::BadPath), "{bad}");
            assert_eq!(read_state(dir.path(), bad), None);
        }
    }
}
