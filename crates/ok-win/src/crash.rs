//! What happens when the program panics: the message is caught, written
//! down, and the person is told once, in plain words.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::win32::*;

/// The exit code of a run that ended in a panic. It is the code Rust itself
/// uses for one.
pub const EXIT_CRASH: i32 = 101;

thread_local! {
    /// True while this thread is running a shell, so panics elsewhere in the
    /// process are left alone.
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    /// The last panic on this thread, with the place it happened, which the
    /// payload handed to `catch_unwind` does not carry.
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

static HOOK: Once = Once::new();

/// Starts noting where panics on this thread happen. The hook that was there
/// before still runs afterwards, so the program's own hook keeps working.
pub(crate) fn watch(on: bool) {
    HOOK.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if WATCHING.with(|w| w.get()) {
                let mut text = payload_text(info.payload());
                if let Some(at) = info.location() {
                    let _ = write!(text, "\n  at {}:{}:{}", at.file(), at.line(), at.column());
                }
                let _ = write!(text, "\n{}", std::backtrace::Backtrace::force_capture());
                LAST.with(|last| {
                    if let Ok(mut last) = last.try_borrow_mut() {
                        *last = Some(text);
                    }
                });
            }
            before(info);
        }));
    });
    WATCHING.with(|w| w.set(on));
}

fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        String::from("a panic that carried no message")
    }
}

/// The message of a caught panic: the hook's fuller note when it ran, the
/// bare payload otherwise.
pub(crate) fn describe(payload: &(dyn Any + Send)) -> String {
    LAST.with(|last| last.try_borrow_mut().ok().and_then(|mut l| l.take())).unwrap_or_else(|| payload_text(payload))
}

/// Adds one entry to `crash.txt` in `dir`. Returns the file's path when it
/// was written.
pub(crate) fn write_report(dir: &Path, doing: &str, message: &str) -> Option<PathBuf> {
    let mut now = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut now) };
    let entry = format!(
        "ok stopped on {:04}-{:02}-{:02} at {:02}:{:02}:{:02}, while {doing}.\r\n{}\r\n\r\n",
        now.wYear,
        now.wMonth,
        now.wDay,
        now.wHour,
        now.wMinute,
        now.wSecond,
        message.replace("\r\n", "\n").replace('\n', "\r\n")
    );
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join("crash.txt");
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    file.write_all(entry.as_bytes()).ok()?;
    file.sync_all().ok()?;
    Some(path)
}

/// The one dialog ok ever shows. It belongs to `owner`, the window that
/// stopped, so it sits over the page it is about and cannot be lost behind
/// another program's window. With no window on screen yet (a panic while
/// starting) it stands alone and asks to come to the front.
pub(crate) fn tell(owner: HWND) {
    let text = wide("ok stopped.\n\nThe words you typed are in the journal and will be brought back at the next start.");
    let title = wide("ok");
    let style = if owner == 0 { MB_OK | MB_SETFOREGROUND } else { MB_OK };
    unsafe { MessageBoxW(owner, text.as_ptr(), title.as_ptr(), style) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caught_panic_is_described_with_its_place() {
        watch(true);
        let caught = std::panic::catch_unwind(|| panic!("the platen jammed at row {}", 28)).unwrap_err();
        let text = describe(caught.as_ref());
        watch(false);
        assert!(text.starts_with("the platen jammed at row 28"), "{text}");
        assert!(text.contains("crash.rs"), "{text}");
        // Taken once: a second ask falls back to the payload alone.
        assert_eq!(describe(caught.as_ref()), "the platen jammed at row 28");
    }

    #[test]
    fn payloads_of_every_kind_have_words() {
        assert_eq!(payload_text(&"plain"), "plain");
        assert_eq!(payload_text(&String::from("owned")), "owned");
        assert_eq!(payload_text(&42u8), "a panic that carried no message");
    }

    #[test]
    fn reports_are_added_to_the_file_not_written_over_it() {
        let dir = crate::scratch("crash-report");
        let path = write_report(&dir, "drawing", "first\nsecond line").expect("written");
        write_report(&dir, "handling Close", "again").expect("written");
        let text = std::fs::read_to_string(&path).expect("readable");
        assert_eq!(text.matches("ok stopped on ").count(), 2);
        assert!(text.contains("while drawing.\r\nfirst\r\nsecond line\r\n"));
        assert!(text.contains("while handling Close.\r\nagain\r\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
