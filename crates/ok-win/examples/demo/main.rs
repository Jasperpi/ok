//! A small program for trying the window shell by hand, and the one its
//! tests drive.
//!
//! It draws a sheet on a desk, echoes what is typed in Gallant with a steady
//! block cursor, and says on the bottom row, in words, the last event it was
//! given. After every key a marker runs along the desk for one second, which
//! keeps the frame clock going for exactly that long. `ok` appears on the
//! sheet's machine line a moment after typing stops and the marker has come
//! to rest, by way of a one-shot wake.
//!
//! | Keys | Does |
//! | --- | --- |
//! | F11 | Between full screen and a window. |
//! | Ctrl C, Ctrl V | Copies everything typed; pastes at the end. |
//! | Ctrl Enter | Opens what is typed in the browser, if it is a web address. |
//! | Esc | Clears the sheet. |
//! | F9 | Panics on purpose, to try what the shell does about it. |
//! | Ctrl Q, Alt F4 | Leaves. |
//!
//! What is typed is written to `demo-words.txt` when the window closes, and
//! also after a panic, and is brought back at the next start.
//!
//! ```text
//! cargo run -p ok-win --example demo --offline
//! cargo run -p ok-win --example demo --offline -- --selftest shots/demo.png
//! ```
//!
//! The second form shows nothing: it types into a cloaked window and writes
//! the frame it ends on.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    windows::main();
}

/// The shell is Win32 only; elsewhere there is nothing to show.
#[cfg(not(windows))]
fn main() {}
