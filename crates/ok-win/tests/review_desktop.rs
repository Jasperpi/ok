//! Review: what a self-test does to the desktop it runs on.
//!
//! The shell's own tests assert that the window was cloaked and never the
//! foreground window. These look at the rest from outside the shell: whether
//! the pointer can land on the cloaked window, whether the keys a script
//! "holds" leak into the keyboard everyone else sees, whether a key held for
//! real leaks into the script, whether the lock keys or the clipboard move,
//! and what happens to a window the cloaked one owns.
#![cfg(windows)]

mod support;

#[path = "review_ffi.rs"]
mod ffi;

use std::sync::{Arc, Mutex};

use ffi::*;
use ok_gfx::{Frame, Rgb};
use ok_host::{Event, Host, Key, Mods, Program, Wants};
use ok_win::{SelfTest, Step};

/// What the program saw of its own window, from outside the shell.
#[derive(Clone, Debug, Default)]
struct Seen {
    looked: bool,
    visible: bool,
    cloak: Option<u32>,
    /// The cloak of a window owned by the cloaked one, shown off every monitor,
    /// and of one just like it that nothing owns.
    owned_cloak: Option<u32>,
    unowned_cloak: Option<u32>,
    /// Points inside the window at which Windows said the pointer would be over it.
    hit: Vec<POINT>,
    probed: usize,
    foreground: bool,
    /// While the script holds Ctrl: this thread's view, and the machine's.
    ctrl_here: Option<bool>,
    ctrl_everywhere: Option<bool>,
    locks_meanwhile: Option<[bool; 3]>,
}

/// Caps Lock, Num Lock and Scroll Lock as a thread that has never had a
/// message queue sees them, which is how the machine as a whole has them.
fn lock_keys() -> [bool; 3] {
    std::thread::spawn(|| [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL].map(|key| unsafe { GetKeyState(key) } & 1 != 0)).join().expect("a thread that only reads")
}

/// Makes a one-pixel window outside every monitor, shows it without
/// activating it, asks the compositor about its cloak and destroys it.
unsafe fn cloak_of_a_window_owned_by(owner: HWND) -> Option<u32> {
    unsafe {
        let made = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            wide("Static").as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            owner,
            0,
            0,
            std::ptr::null_mut(),
        );
        if made == 0 {
            return None;
        }
        ShowWindow(made, SW_SHOWNOACTIVATE);
        let cloak = cloak_of(made);
        DestroyWindow(made);
        cloak
    }
}

struct Watcher {
    name: &'static str,
    fullscreen: bool,
    seen: Arc<Mutex<Seen>>,
}

impl Watcher {
    fn look(&self) {
        let hwnd = window_of(self.name);
        let mut seen = self.seen.lock().unwrap();
        seen.looked = hwnd != 0;
        if hwnd == 0 {
            return;
        }
        unsafe {
            seen.visible = IsWindowVisible(hwnd) != 0;
            seen.cloak = cloak_of(hwnd);
            seen.foreground = GetForegroundWindow() == hwnd;
            let mut at = RECT::default();
            GetWindowRect(hwnd, &mut at);
            let (w, h) = (at.right - at.left, at.bottom - at.top);
            for (fx, fy) in [(1, 2), (1, 4), (3, 4), (1, 8), (7, 8)] {
                let point = POINT { x: at.left + w * fx / 8, y: at.top + h * fy / 8 };
                seen.probed += 1;
                if WindowFromPoint(point) == hwnd {
                    seen.hit.push(point);
                }
            }
            // A one-pixel window owned by the cloaked one, placed where no
            // monitor is and never activated, and beside it one that nothing
            // owns. All that is wanted of them is what the compositor says
            // about their cloaks.
            seen.owned_cloak = cloak_of_a_window_owned_by(hwnd);
            seen.unowned_cloak = cloak_of_a_window_owned_by(0);
        }
    }
}

impl Program for Watcher {
    fn event(&mut self, event: Event, _: &mut dyn Host) {
        match event {
            Event::Key { key: Key::F(1), .. } => self.look(),
            Event::Key { key: Key::F(2), mods, .. } if mods.ctrl => {
                let mut seen = self.seen.lock().unwrap();
                seen.ctrl_here = Some(unsafe { GetKeyState(VK_CONTROL) } < 0);
                seen.ctrl_everywhere = Some(unsafe { GetAsyncKeyState(VK_CONTROL) } < 0);
                seen.locks_meanwhile = Some(lock_keys());
            }
            _ => {}
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { fullscreen: self.fullscreen, ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        frame.clear(Rgb(0x35, 0x25, 0x1B));
    }
}

fn watch(name: &'static str, fullscreen: bool) -> (Seen, ok_win::Report) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let kept = Arc::clone(&seen);
    let script = vec![Step::Wait(100), Step::Press(Key::F(1)), Step::Hold(Mods::CTRL), Step::Press(Key::F(2)), Step::Hold(Mods::NONE)];
    let report = ok_win::selftest(options(name), SelfTest::new(folder(name), script), move |_| Box::new(Watcher { name, fullscreen, seen: kept }));
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    let seen = seen.lock().unwrap().clone();
    (seen, report)
}

/// A cloaked window is shown as far as Windows is concerned, and the
/// full-screen ones cover the whole monitor. If the pointer could land on
/// it, every click made while a test ran would go to the test.
#[test]
fn the_pointer_cannot_land_on_a_cloaked_test_window() {
    for (name, fullscreen) in [("desktop-window", false), ("desktop-fullscreen", true)] {
        let (seen, report) = watch(name, fullscreen);
        assert!(seen.looked, "{name}: the window was found by its class");
        assert!(seen.visible, "{name}: the window counts as shown, which is what makes this worth asking");
        assert_eq!(report.fullscreen, fullscreen);
        println!("{name}: cloak {:?}, {} of {} points inside it are under the pointer's reach", seen.cloak, seen.hit.len(), seen.probed);
        assert_eq!(seen.cloak, Some(1), "{name}: cloaked by the application");
        assert!(seen.hit.is_empty(), "{name}: a click at {:?} would go to the test window", seen.hit);
        assert!(!seen.foreground);
    }
}

/// The script's Ctrl exists only in the window thread's own key state. The
/// machine's keyboard, which is what the person's own program reads, never
/// has it down, and the lock keys stay as they were.
///
/// The person at the keyboard may really press Ctrl or a lock key while
/// this runs, so one clean run in three is what is asked for.
#[test]
fn a_scripts_held_keys_stay_inside_the_test() {
    let mut said = Vec::new();
    for name in ["desktop-keys-1", "desktop-keys-2", "desktop-keys-3"] {
        let before = lock_keys();
        let (seen, _) = watch(name, false);
        let after = lock_keys();
        assert_eq!(seen.ctrl_here, Some(true), "the window's thread sees the script's Ctrl");
        said.push(format!(
            "Ctrl down for everyone else: {:?}; Caps, Num and Scroll Lock before {before:?}, while the script held Ctrl {:?}, after {after:?}",
            seen.ctrl_everywhere, seen.locks_meanwhile
        ));
        if seen.ctrl_everywhere == Some(false) && seen.locks_meanwhile == Some(before) && after == before {
            println!("{}", said[said.len() - 1]);
            if !before.contains(&true) {
                println!("no lock key was on, so this run could not have shown one being turned off");
            }
            return;
        }
    }
    panic!("the script's keys reached the rest of the machine, or a lock key moved, in three runs out of three: {said:#?}");
}

/// Stands in for the person pressing a modifier in their own program while a
/// test runs. Windows may bring a thread's key state into line with the real
/// keyboard whenever the thread looks at its queue, so after each character
/// it is given this marks `keys` as held in the window thread's own state,
/// which is the only place such a press would show.
struct Meddler {
    keys: &'static [i32],
    heard: Arc<Mutex<Vec<String>>>,
}

impl Program for Meddler {
    fn event(&mut self, event: Event, _: &mut dyn Host) {
        let word = match &event {
            Event::Text(ch) => format!("text {ch}"),
            Event::Mods(mods) => format!("holding {mods:?}"),
            Event::Mouse { mods, .. } => format!("mouse with {mods:?}"),
            Event::Focus(gained) => format!("focus {gained}"),
            _ => return,
        };
        self.heard.lock().unwrap().push(word);
        if matches!(event, Event::Text(_)) {
            let mut state = [0u8; 256];
            unsafe {
                GetKeyboardState(state.as_mut_ptr());
                for key in self.keys {
                    state[*key as usize] |= 0x80;
                }
                SetKeyboardState(state.as_ptr());
            }
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { fullscreen: false, ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        frame.clear(Rgb(0x35, 0x25, 0x1B));
    }
}

fn heard_while_meddling(name: &'static str, keys: &'static [i32], script: Vec<Step>) -> Vec<String> {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&heard);
    let report = ok_win::selftest(options(name), SelfTest::new(folder(name), script), move |_| Box::new(Meddler { keys, heard: kept }));
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    assert_eq!(report.crashed, None);
    let heard = heard.lock().unwrap().clone();
    heard
}

/// The other direction from the test above: a modifier that goes down for
/// real between two steps of a script must not reach the script's input.
/// The script used to restate its own keys only when it pressed one, so a
/// step of characters, a mouse step or a focus step that ran while Ctrl or
/// Alt was really held saw it down: the characters were dropped as a chord
/// and the mouse gained Alt. Every step now starts from the script's keys.
#[test]
fn a_modifier_held_for_real_between_steps_does_not_reach_the_script() {
    let chars = |text: &str| Step::Chars(text.encode_utf16().collect());

    let script = vec![chars("a"), chars("b"), Step::Focus(true), chars("c")];
    let heard = heard_while_meddling("desktop-meddle-ctrl", &[VK_CONTROL, VK_LCONTROL], script);
    assert_eq!(heard, ["text a", "text b", "focus true", "text c"], "a Ctrl that was not the script's dropped a character or was reported as held");

    let script = vec![chars("a"), Step::MouseMove(7, 9)];
    let heard = heard_while_meddling("desktop-meddle-alt", &[VK_MENU, VK_LMENU], script);
    let plain_move = format!("mouse with {:?}", Mods::NONE);
    assert_eq!(heard, ["text a", plain_move.as_str()], "an Alt that was not the script's reached the mouse");
}

/// As above, the person may copy something of their own meanwhile, so one
/// clean run in three is what is asked for.
#[test]
fn copying_and_pasting_in_a_test_leaves_the_real_clipboard_alone() {
    for name in ["review-clipboard-1", "review-clipboard-2", "review-clipboard-3"] {
        let before = unsafe { GetClipboardSequenceNumber() };
        let script = vec![Step::Type(String::from("kept to itself")), Step::Chord(Key::Char('c'), Mods::CTRL), Step::Chord(Key::Char('v'), Mods::CTRL)];
        let report = support::run(name, script);
        assert_eq!(report.clipboard.as_deref(), Some("kept to itself"));
        if unsafe { GetClipboardSequenceNumber() } == before {
            return;
        }
    }
    panic!("the real clipboard changed during three runs out of three");
}

/// Measured, because one line of the shell depends on it: a window owned by
/// a cloaked window is cloaked too.
///
/// The real first show cloaks the window, shows it, paints it and only then
/// uncloaks it. Showing it sends the program `Focus(true)` (and `Resize`
/// when it opens maximized), each followed by `wants` and usually `render`.
/// If the program panicked in any of those, `Shell::crashed` saw a window
/// that was "visible" and gave it the crash dialog to own. That dialog
/// would inherit the cloak: it has the keyboard, nobody can see it, the
/// process waits on it for ever holding the single-instance lock, and every
/// later launch finds the invisible window and hands over to it. The shell
/// now asks the compositor first and gives the dialog no owner while the
/// window is cloaked (`Report::crash_dialog_owned`); this measurement is why.
#[test]
fn a_window_owned_by_a_cloaked_window_is_cloaked_too() {
    let (seen, _) = watch("desktop-owned", false);
    println!(
        "the cloaked window reports {:?}; a window it owns reports {:?}; the same window owned by nothing reports {:?}",
        seen.cloak, seen.owned_cloak, seen.unowned_cloak
    );
    assert_eq!(seen.unowned_cloak, Some(0), "a window nothing owns is not cloaked, so the next line means what it says");
    let owned = seen.owned_cloak.expect("the compositor answered for the owned window");
    assert_ne!(owned, 0, "the owned window was not cloaked, so a crash dialog raised during the first show would be seen");
}
