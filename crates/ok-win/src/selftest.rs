//! Driving the real window with made-up input, where nobody can see it.
//!
//! A self-test builds the same window, runs the same window procedure and the
//! same message loop as a real run. Three things differ, all so that the
//! person at the computer is never disturbed: the window is cloaked before it
//! is first shown and never activated, the clipboard and the browser are
//! stand-ins, and the three folders live under a directory the test names.
//! Input comes from a [`Step`] list that is posted to the window's own handle
//! one step at a time, each step only once the one before it has been fully
//! handled.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use ok_host::{Key, Mods};

use crate::keys::vk_for_key;
use crate::win32::*;

/// The exit code of a process that a self-test's watchdog had to end,
/// because the window's thread never came back from a call into the program.
pub const EXIT_STUCK: i32 = 102;

/// One thing a script does.
#[derive(Clone, PartialEq, Debug)]
pub enum Step {
    /// Holds exactly these modifiers from now on, posting a key down or key
    /// up for each of Ctrl, Shift and Alt that changes.
    Hold(Mods),
    /// Presses and releases one key with whatever is held.
    Press(Key),
    /// Holds the modifiers, presses the key, then goes back to what was held.
    Chord(Key, Mods),
    /// One more key down for a key that is already down, as auto-repeat sends.
    Repeat(Key),
    /// A raw virtual key going down (`true`) or up, for keys the interface
    /// has no name for, such as `0xE7` (injected text) or the number pad.
    Vk(u16, bool),
    /// Types text the way a keyboard does: a key down, which Windows turns
    /// into a character, and a key up, with Shift as the layout needs.
    /// Characters the layout cannot type are posted as characters.
    Type(String),
    /// Posts these UTF-16 code units as character messages, all at once, as
    /// dictation, the emoji panel or an input method would.
    Chars(Vec<u16>),
    /// The left button going down at a point in the client area.
    MouseDown(i32, i32),
    MouseUp(i32, i32),
    MouseMove(i32, i32),
    /// Posts a move to each of these points at once, as a pointer does that
    /// moves faster than the window can keep up with: the queue is not
    /// empty again until the last of them has been handled.
    MouseTrail(Vec<(i32, i32)>),
    /// The wheel turned by `delta` (120 is one notch away from the user) with
    /// the pointer over a point in the client area.
    Wheel { x: i32, y: i32, delta: i32 },
    /// Posts a size message claiming the client area is this big, without
    /// moving the window. Reaches sizes Windows itself would refuse.
    Size(u32, u32),
    /// Really resizes the window so its client area is this big, subject to
    /// the smallest size the program allows.
    Client(u32, u32),
    /// Asks the window what a drag to this client size would settle on; the
    /// answer is added to [`Report::snaps`].
    Drag(u32, u32),
    Focus(bool),
    /// Lets this much real time pass, with the loop waiting as it would.
    Wait(u64),
    /// Notes the time, the processor time used so far and the counters in
    /// [`Report::probes`].
    Probe,
    /// What a second launch does: hands the window a file to open.
    Open(PathBuf),
    /// The start (`true`) or end of the loop Windows runs while a window is
    /// being dragged or resized.
    Moving(bool),
    /// Windows asking to end the session, and then ending it (`true`) or
    /// calling it off.
    EndSession(bool),
    /// Writes the frame as it is now to a PNG.
    Shot(PathBuf),
    /// Asks the window to close with the system's close command, which is
    /// what Alt F4 and the system menu turn into. A script that runs out
    /// closes the window by itself.
    Close,
}

/// A self-test: what to do and where its files go.
#[derive(Clone, Debug)]
pub struct SelfTest {
    /// What happens to the window, in order. When it runs out the window is
    /// closed as Alt F4 would close it.
    pub script: Vec<Step>,
    /// Where the last frame is written when the window goes.
    pub png: Option<PathBuf>,
    /// Stands in for the user's folders: `Documents`, `Local` and `Roaming`
    /// are made beneath it as they are needed.
    pub data_dir: PathBuf,
    /// The client size to open with when the program wants a window, in
    /// pixels. `None` uses the remembered placement, as a real run does. A
    /// program that wants full screen gets the whole monitor either way.
    pub client: Option<(u32, u32)>,
    /// Gives up after this many milliseconds, so a stuck test cannot hang.
    ///
    /// A test whose window is still handling messages (a script that is too
    /// long, a queue that never runs empty) is closed and reports
    /// [`Report::timed_out`]. A test whose program never returns from a call
    /// cannot report anything: a quarter of this time later, and no sooner
    /// than one second or later than ten, a line is written to standard
    /// error and the whole process ends with [`EXIT_STUCK`].
    pub timeout_ms: u64,
}

impl SelfTest {
    /// A script for a window of 960 by 528 pixels, which is 80 by 24 cells
    /// of Gallant, given a minute to finish and writing no picture.
    pub fn new(data_dir: impl Into<PathBuf>, script: Vec<Step>) -> SelfTest {
        SelfTest { script, png: None, data_dir: data_dir.into(), client: Some((960, 528)), timeout_ms: 60_000 }
    }
}

/// Where a [`ok_host::Event::Tick`] came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TickSource {
    /// The frame clock, because the program is animating.
    Frame,
    /// The one-shot wake the program asked for.
    Wake,
    /// The stand-in for the frame clock while a window is dragged or resized.
    Moving,
    /// Another thread asked for it through [`ok_host::Host::waker`].
    Waker,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tick {
    /// Milliseconds since the window was made.
    pub at_ms: f64,
    pub source: TickSource,
}

/// The counters at one moment of a script.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Probe {
    pub at_ms: f64,
    /// Processor time the whole process has used, user and kernel together.
    pub cpu_ms: f64,
    pub ticks: usize,
    pub renders: u64,
}

/// What a self-test saw.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub exit_code: i32,
    /// The last frame the program drew: `width * height` pixels of `0x00RRGGBB`.
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    /// The window's real client size when it closed.
    pub client: (i32, i32),
    pub fullscreen: bool,
    pub title: String,
    pub ticks: Vec<Tick>,
    pub probes: Vec<Probe>,
    /// The frame period the clock runs at, in milliseconds.
    pub frame_period_ms: f64,
    /// How many times the program was asked to draw.
    pub renders: u64,
    /// How many paint messages were answered from the frame already drawn.
    pub paints: u64,
    /// Client sizes that [`Step::Drag`] settled on.
    pub snaps: Vec<(i32, i32)>,
    /// Times a menu was asked for and refused: the menu bar for a bare Alt
    /// or F10, and the system menu for Alt Space while the window is full
    /// screen and has none.
    pub menus_refused: u32,
    /// Times Alt Space asked for the system menu while the window had one.
    /// A real run opens it. A self-test only counts, because the menu would
    /// be a real one on the screen.
    pub system_menus: u32,
    /// Where the hidden system caret was left, in client pixels.
    pub caret: Option<(i32, i32)>,
    /// What the program put on the stand-in clipboard.
    pub clipboard: Option<String>,
    /// Addresses the program asked to open in the browser.
    pub opened: Vec<String>,
    /// The panic message, when the program panicked.
    pub crashed: Option<String>,
    /// When the program panicked: whether the dialog a real run then shows
    /// would have been given to the window to own. It must not be while the
    /// window cannot be seen, because Windows cloaks a dialog together with
    /// the cloaked window that owns it. A self-test's window is always
    /// cloaked, so here this is always `Some(false)`.
    pub crash_dialog_owned: Option<bool>,
    /// The test ran out of time and was closed. See [`SelfTest::timeout_ms`].
    pub timed_out: bool,
    /// Whether the window's own surface, read back at the end, held exactly
    /// the last frame (and the ground colour wherever the frame did not
    /// reach). `None` when the surface could not be read. It can only be
    /// true while the whole client area lies on a monitor: Windows neither
    /// draws nor reads back the part of a window that hangs off every
    /// screen, as an ordinary window as large as the monitor does.
    pub on_glass: Option<bool>,
    /// True when the window carried a large and a small icon when it closed.
    pub has_icons: bool,
    /// True when Windows confirmed the window was cloaked for its whole life.
    pub cloaked: bool,
    /// True if the window was ever the foreground window. It never should be.
    pub was_foreground: bool,
}

/// Ends a self-test that has run too long. It keeps time on a thread of its
/// own, because both ways a test gets stuck put the window's thread out of
/// reach of a timer: a queue that is never empty keeps Windows from ever
/// handing a timer out, and a program that never returns from a call keeps
/// the loop from turning at all.
pub(crate) struct Watchdog {
    over: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// Starts the clock. When `timeout_ms` have passed the window is posted
    /// `message`, which the shell answers by closing with
    /// [`Report::timed_out`] set. If the run is still not over a while after
    /// that, the window's thread is not turning its loop and there is
    /// nobody left to report to: the process is ended with [`EXIT_STUCK`].
    pub(crate) fn start(hwnd: HWND, message: u32, timeout_ms: u64) -> Watchdog {
        let over = Arc::new((Mutex::new(false), Condvar::new()));
        let watched = Arc::clone(&over);
        let watch = move || {
            if over_within(&watched, timeout_ms) {
                return;
            }
            unsafe { PostMessageW(hwnd, message, 0, 0) };
            let grace_ms = grace_ms(timeout_ms);
            if over_within(&watched, grace_ms) {
                return;
            }
            // Written without `eprintln!`, which panics when standard error
            // is closed: nothing may keep the next line from being reached.
            let _ = writeln!(
                std::io::stderr(),
                "ok self-test: the window's thread has not come back {grace_ms} ms after the test's {timeout_ms} ms ran out. \
                 A call into the program never returned. Ending the process with exit code {EXIT_STUCK}."
            );
            std::process::exit(EXIT_STUCK);
        };
        let thread = std::thread::Builder::new().name(String::from("ok self-test watchdog")).spawn(watch).ok();
        Watchdog { over, thread }
    }
}

impl Drop for Watchdog {
    /// The run is over: stops the clock and waits for its thread to go.
    fn drop(&mut self) {
        let (over, bell) = &*self.over;
        *over.lock().unwrap_or_else(PoisonError::into_inner) = true;
        bell.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How long after its time has run out a test is given to close before the
/// process is ended.
fn grace_ms(timeout_ms: u64) -> u64 {
    (timeout_ms / 4).clamp(1_000, 10_000)
}

/// Waits up to `ms` milliseconds for the run to be over. True when it is.
fn over_within(over: &(Mutex<bool>, Condvar), ms: u64) -> bool {
    let (over, bell) = over;
    let waiting = over.lock().unwrap_or_else(PoisonError::into_inner);
    // Some weeks are as good as for ever, and keep the arithmetic in range.
    let limit = Duration::from_millis(ms.min(1 << 31));
    let (over, _) = bell.wait_timeout_while(waiting, limit, |over| !*over).unwrap_or_else(PoisonError::into_inner);
    *over
}

/// The smallest things a script is made of: each is one message posted, or
/// one call made, between two turns of the message loop.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Micro {
    Key { vk: i32, down: bool, repeat: bool },
    Chars(Vec<u16>),
    Mouse { message: u32, x: i32, y: i32 },
    Trail(Vec<(i32, i32)>),
    Wheel { x: i32, y: i32, delta: i32 },
    Size(u32, u32),
    Client(u32, u32),
    Drag(u32, u32),
    Focus(bool),
    Wait(u64),
    Probe,
    Open(PathBuf),
    Moving(bool),
    EndSession(bool),
    Shot(PathBuf),
    Close,
    /// The script has run out: the window is told to close.
    End,
}

/// Breaks a script into single messages. Must run on the thread that will own
/// the window, because what a character's key is depends on that thread's
/// keyboard layout.
pub(crate) fn expand(script: &[Step]) -> VecDeque<Micro> {
    let mut out = VecDeque::new();
    let mut held = Mods::NONE;
    for step in script {
        match step {
            Step::Hold(mods) => hold(&mut out, &mut held, *mods),
            Step::Press(key) => press(&mut out, *key),
            Step::Chord(key, mods) => {
                let before = held;
                hold(&mut out, &mut held, *mods);
                press(&mut out, *key);
                hold(&mut out, &mut held, before);
            }
            Step::Repeat(key) => {
                if let Some(vk) = vk_for_key(*key) {
                    out.push_back(Micro::Key { vk, down: true, repeat: true });
                }
            }
            Step::Vk(vk, down) => out.push_back(Micro::Key { vk: *vk as i32, down: *down, repeat: false }),
            Step::Type(text) => {
                for ch in text.chars() {
                    type_char(&mut out, &mut held, ch);
                }
            }
            Step::Chars(units) => out.push_back(Micro::Chars(units.clone())),
            Step::MouseDown(x, y) => out.push_back(Micro::Mouse { message: WM_LBUTTONDOWN, x: *x, y: *y }),
            Step::MouseUp(x, y) => out.push_back(Micro::Mouse { message: WM_LBUTTONUP, x: *x, y: *y }),
            Step::MouseMove(x, y) => out.push_back(Micro::Mouse { message: WM_MOUSEMOVE, x: *x, y: *y }),
            Step::MouseTrail(points) => out.push_back(Micro::Trail(points.clone())),
            Step::Wheel { x, y, delta } => out.push_back(Micro::Wheel { x: *x, y: *y, delta: *delta }),
            Step::Size(w, h) => out.push_back(Micro::Size(*w, *h)),
            Step::Client(w, h) => out.push_back(Micro::Client(*w, *h)),
            Step::Drag(w, h) => out.push_back(Micro::Drag(*w, *h)),
            Step::Focus(on) => out.push_back(Micro::Focus(*on)),
            Step::Wait(ms) => out.push_back(Micro::Wait(*ms)),
            Step::Probe => out.push_back(Micro::Probe),
            Step::Open(path) => out.push_back(Micro::Open(path.clone())),
            Step::Moving(on) => out.push_back(Micro::Moving(*on)),
            Step::EndSession(really) => out.push_back(Micro::EndSession(*really)),
            Step::Shot(path) => out.push_back(Micro::Shot(path.clone())),
            Step::Close => out.push_back(Micro::Close),
        }
    }
    out
}

/// Presses in the order a hand does (Ctrl, Shift, Alt) and lets go in reverse.
fn hold(out: &mut VecDeque<Micro>, held: &mut Mods, want: Mods) {
    let key = |vk: i32, down: bool| Micro::Key { vk, down, repeat: false };
    if held.alt && !want.alt {
        out.push_back(key(VK_MENU, false));
    }
    if held.shift && !want.shift {
        out.push_back(key(VK_SHIFT, false));
    }
    if held.ctrl && !want.ctrl {
        out.push_back(key(VK_CONTROL, false));
    }
    if want.ctrl && !held.ctrl {
        out.push_back(key(VK_CONTROL, true));
    }
    if want.shift && !held.shift {
        out.push_back(key(VK_SHIFT, true));
    }
    if want.alt && !held.alt {
        out.push_back(key(VK_MENU, true));
    }
    *held = want;
}

fn press(out: &mut VecDeque<Micro>, key: Key) {
    if let Some(vk) = vk_for_key(key) {
        out.push_back(Micro::Key { vk, down: true, repeat: false });
        out.push_back(Micro::Key { vk, down: false, repeat: false });
    }
}

/// A character as the key that types it on this thread's layout, or, when no
/// plain or shifted key does, as the character itself.
fn type_char(out: &mut VecDeque<Micro>, held: &mut Mods, ch: char) {
    let mut pair = [0u16; 2];
    let units = ch.encode_utf16(&mut pair);
    if units.len() == 1 && !held.ctrl && !held.alt {
        let scan = unsafe { VkKeyScanW(units[0]) };
        let (vk, shift_state) = ((scan & 0xFF) as i32, (scan >> 8) & 0xFF);
        if scan != -1 && vk != 0xFF && (shift_state == 0 || shift_state == 1) {
            let before = *held;
            hold(out, held, Mods { shift: shift_state == 1, ..Mods::NONE });
            out.push_back(Micro::Key { vk, down: true, repeat: false });
            out.push_back(Micro::Key { vk, down: false, repeat: false });
            hold(out, held, before);
            return;
        }
    }
    out.push_back(Micro::Chars(units.to_vec()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(vk: i32, down: bool) -> Micro {
        Micro::Key { vk, down, repeat: false }
    }

    #[test]
    fn a_chord_presses_and_releases_in_hand_order() {
        let got: Vec<Micro> = expand(&[Step::Chord(Key::Char('z'), Mods { ctrl: true, shift: true, alt: false })]).into();
        assert_eq!(
            got,
            vec![
                key(VK_CONTROL, true),
                key(VK_SHIFT, true),
                key(b'Z' as i32, true),
                key(b'Z' as i32, false),
                key(VK_SHIFT, false),
                key(VK_CONTROL, false),
            ]
        );
    }

    #[test]
    fn hold_changes_only_what_differs() {
        let got: Vec<Micro> = expand(&[Step::Hold(Mods::CTRL), Step::Hold(Mods { ctrl: true, alt: true, shift: false }), Step::Hold(Mods::NONE)]).into();
        assert_eq!(got, vec![key(VK_CONTROL, true), key(VK_MENU, true), key(VK_MENU, false), key(VK_CONTROL, false)]);
    }

    #[test]
    fn typing_uses_keys_where_the_layout_has_them_and_characters_elsewhere() {
        // An emoji is on no key of any layout.
        let got: Vec<Micro> = expand(&[Step::Type("\u{1F600}".into())]).into();
        assert_eq!(got, vec![Micro::Chars(vec![0xD83D, 0xDE00])]);
        // A capital needs Shift on every Latin layout.
        let got: Vec<Micro> = expand(&[Step::Type("A".into())]).into();
        if got.len() == 4 {
            assert_eq!(got[0], key(VK_SHIFT, true));
            assert_eq!(got[3], key(VK_SHIFT, false));
        } else {
            assert_eq!(got, vec![Micro::Chars(vec![b'A' as u16])]);
        }
    }

    #[test]
    fn a_key_with_no_virtual_key_is_skipped() {
        assert!(expand(&[Step::Press(Key::F(40)), Step::Press(Key::Char('\u{1F600}'))]).is_empty());
    }
}
