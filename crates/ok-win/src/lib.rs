//! The window shell: one Win32 window, a message loop and the machine behind
//! [`ok_host::Host`].
//!
//! The interface is an [`ok_host::Program`]. It never sees a window handle;
//! this crate is the only place that does. [`run`] opens the window and stays
//! until the program quits. [`selftest()`] does the same with made-up input and
//! a window nobody can see, and hands back the last frame. [`stamp()`] writes a
//! manifest and icons into a built exe.
//!
//! Win32 is declared by hand in `win32.rs`; there are no dependencies outside
//! this workspace.
#![cfg(windows)]

mod crash;
mod fonts;
mod host;
mod keys;
mod placement;
mod selftest;
mod sfnt;
mod shell;
mod stamp;
mod win32;

use std::path::PathBuf;

use ok_host::{Host, Program};

pub use crash::EXIT_CRASH;
pub use selftest::{Probe, Report, SelfTest, Step, Tick, TickSource, EXIT_STUCK};
pub use stamp::{stamp, MANIFEST};

/// One image of the application's icon: `width * height` pixels, top row
/// first, each `0xAARRGGBB` with alpha that is not premultiplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
}

impl Icon {
    /// True when the pixel count matches the size and the size is one an
    /// icon can have.
    pub fn is_well_formed(&self) -> bool {
        (1..=256).contains(&self.width)
            && (1..=256).contains(&self.height)
            && self.argb.len() == self.width as usize * self.height as usize
    }
}

/// What the shell needs to know before the program exists.
#[derive(Clone, Debug)]
pub struct Options {
    /// Names the application to Windows: taskbar grouping, the lock that
    /// keeps it to one instance, and the window class. Two different values
    /// are two different applications.
    pub app_id: String,
    /// The application's folder name under `%LOCALAPPDATA%` and `%APPDATA%`.
    pub name: String,
    /// The window and taskbar icon, in as many sizes as there are. 16 and 32
    /// pixels are the two Windows asks for at ordinary scaling.
    pub icons: Vec<Icon>,
    /// The file named on the command line. If ok is already running, that
    /// instance is handed the file and this launch ends. Otherwise the
    /// program made by this launch is sent [`ok_host::Event::Open`] for it,
    /// after its first [`ok_host::Event::Resize`] and before anything is
    /// seen. Either way the path arrives absolute.
    pub open: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options { app_id: String::from("JasperMeller.ok"), name: String::from("ok"), icons: Vec::new(), open: None }
    }
}

/// Opens the window and runs `make_program`'s program in it until the
/// program asks to quit or the window is closed. Returns the process exit
/// code: 0, [`EXIT_CRASH`] if the program panicked, or 1 if Windows would
/// not make the window.
///
/// `make_program` is called once, after the window exists, so the host it is
/// given is complete. If another instance with the same `app_id` is already
/// running, that instance is brought forward and handed `options.open`, and
/// this call returns 0 without making a program.
///
/// Call it first thing in `main`: it makes the process aware of each
/// monitor's own scaling, and Windows only allows that before anything has
/// asked about the screen.
pub fn run<F>(options: Options, make_program: F) -> i32
where
    F: FnOnce(&mut dyn Host) -> Box<dyn Program>,
{
    shell::run_shell(options, None, Box::new(make_program)).code
}

/// Runs the program through the real window procedure and message loop with
/// the script in `test` as its input, in a window that is cloaked and never
/// activated. Nothing appears on screen, the real clipboard is not touched
/// and no browser is opened. Returns what happened, including the last frame.
///
/// A test that runs past [`SelfTest::timeout_ms`] is closed and says so in
/// [`Report::timed_out`]. One whose program never returns from a call cannot
/// be closed: this function then never returns either, and the process is
/// ended with [`EXIT_STUCK`].
///
/// This is what a `--selftest` flag should call.
pub fn selftest<F>(options: Options, test: SelfTest, make_program: F) -> Report
where
    F: FnOnce(&mut dyn Host) -> Box<dyn Program>,
{
    shell::run_shell(options, Some(test), Box::new(make_program)).report
}

/// An empty folder for one test's files, inside the build's own directory.
#[cfg(test)]
pub(crate) fn scratch(name: &str) -> PathBuf {
    let beside_exe = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let dir = beside_exe.unwrap_or_else(std::env::temp_dir).join("ok-win-scratch").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder can be made");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Win32 function declared in `win32.rs` has its address taken
    /// here, which makes the linker resolve all of them. A misspelt or
    /// missing import fails this build rather than a user's launch.
    #[test]
    fn every_import_resolves() {
        let mut total = 0;
        for (library, imports) in win32::all_imports() {
            for (name, address) in &imports {
                assert_ne!(*address, 0, "{library}!{name}");
            }
            total += imports.len();
        }
        assert!(total >= 218, "{total} imports");
    }

    #[test]
    fn icons_are_checked_before_they_are_used() {
        assert!(Icon { width: 2, height: 2, argb: vec![0; 4] }.is_well_formed());
        assert!(!Icon { width: 2, height: 2, argb: vec![0; 3] }.is_well_formed());
        assert!(!Icon { width: 0, height: 2, argb: vec![] }.is_well_formed());
        assert!(!Icon { width: 257, height: 1, argb: vec![0; 257] }.is_well_formed());
    }
}
