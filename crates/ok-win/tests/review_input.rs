//! Review: input that arrives by a road the shell's own tests do not take.
//!
//! The shell's tests post their keys and characters into the queue and let
//! the loop hand them out one at a time. Windows also calls the window
//! procedure directly, from inside calls the shell or the program is already
//! making: a session ending, an input method committing a string, anything
//! sent from another thread. These tests come in by that road.
//!
//! Two of these failed when they were written and said what went wrong:
//! `a_session_end_is_answered_only_after_the_program_has_heard_close_even_mid_key`
//! and `a_character_that_arrives_while_the_frame_is_being_drawn_is_not_lost`.
//! Both pass since the shell was repaired. The rest pin down behaviour the
//! contract is silent on, or measure a road nothing else covers.

mod support;

#[path = "review_ffi.rs"]
mod ffi;

use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ffi::*;
use ok_gfx::{Frame, Rgb};
use ok_host::{Event, Host, Key, Mods, Program, Wants};
use ok_win::{SelfTest, Step};
use support::{Screen, BLOCK};

const GROUND: Rgb = Rgb(0x35, 0x25, 0x1B);

fn unseen(name: &str, script: Vec<Step>, program: impl Program + 'static) -> ok_win::Report {
    let report = ok_win::selftest(options(name), SelfTest::new(folder(name), script), move |_| Box::new(program));
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    assert_eq!(report.crashed, None);
    report
}

/// Keeps what it is typed and counts what it is told.
#[derive(Clone, Default)]
struct Notes {
    text: Arc<Mutex<String>>,
    closes: Arc<AtomicUsize>,
    draws: Arc<AtomicUsize>,
    /// Every call the shell made, in order, in a word or two.
    calls: Arc<Mutex<Vec<String>>>,
}

/// What a session-ending message found: how many times the program had been
/// told to close by the time the shell had answered the message.
type Answered = Arc<Mutex<Option<usize>>>;

/// A program that, when F1 is pressed, has another thread send its window a
/// message the way Windows sends the end of a session: from outside, to be
/// answered whenever the window's thread next looks at its queue.
///
/// With `while_busy` the message is already waiting when the key's handler
/// returns, so the shell meets it part way through that key. Without, it is
/// sent once the window is idle again.
struct SessionEnds {
    name: &'static str,
    message: u32,
    wparam: usize,
    while_busy: bool,
    notes: Notes,
    answered: Answered,
    dirty: bool,
    slow_draws: u32,
}

impl Program for SessionEnds {
    fn event(&mut self, event: Event, _: &mut dyn Host) {
        match event {
            Event::Key { key: Key::F(1), .. } => {
                let (hwnd, message, wparam, wait) = (window_of(self.name), self.message, self.wparam, if self.while_busy { 0 } else { 400 });
                let (closes, answered) = (Arc::clone(&self.notes.closes), Arc::clone(&self.answered));
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(wait));
                    unsafe { SendMessageW(hwnd, message, wparam, 0) };
                    *answered.lock().unwrap() = Some(closes.load(Ordering::Acquire));
                });
                if self.while_busy {
                    // Long enough for the other thread's message to be waiting.
                    std::thread::sleep(Duration::from_millis(120));
                    self.slow_draws = 1;
                }
                self.dirty = true;
            }
            Event::Close => {
                self.notes.closes.fetch_add(1, Ordering::AcqRel);
            }
            _ => {}
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { redraw: self.dirty, fullscreen: false, ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        self.dirty = false;
        frame.clear(GROUND);
        if self.slow_draws > 0 {
            // Stands in for a real frame's work, and keeps this test from
            // depending on which thread runs first after the answer.
            self.slow_draws -= 1;
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

fn closes_heard_before_the_answer(name: &'static str, message: u32, wparam: usize, while_busy: bool) -> usize {
    let (notes, answered) = (Notes::default(), Answered::default());
    let program = SessionEnds { name, message, wparam, while_busy, notes, answered: Arc::clone(&answered), dirty: false, slow_draws: 0 };
    unseen(name, vec![Step::Press(Key::F(1)), Step::Wait(900)], program);
    let heard = answered.lock().unwrap().take();
    heard.expect("the message was answered")
}

/// The contract: when the session ends everything must be on disk before
/// `event` returns, and the shell says so to Windows by answering the
/// message. Windows may end the process any time after the last answer.
///
/// Idle, the shell tells the program first and answers second. When the
/// message is waiting while the program handles a key that changes the
/// picture, the shell meets it inside its own check for more typing
/// (`keys_waiting`, a `PeekMessageW`, which is where Windows delivers sent
/// messages). The shell is still busy with the key then. It used to queue
/// `Close`, answer, draw the frame and only then tell the program; now the
/// program, which is not inside a call at that moment, is told on the spot.
#[test]
fn a_session_end_is_answered_only_after_the_program_has_heard_close_even_mid_key() {
    // Idle: as the contract says.
    assert_eq!(closes_heard_before_the_answer("end-query-idle", WM_QUERYENDSESSION, 0, false), 1);
    assert_eq!(closes_heard_before_the_answer("end-really-idle", WM_ENDSESSION, 1, false), 1);

    let query = closes_heard_before_the_answer("end-query-busy", WM_QUERYENDSESSION, 0, true);
    let end = closes_heard_before_the_answer("end-really-busy", WM_ENDSESSION, 1, true);
    println!("mid-key: Close heard {query} times before WM_QUERYENDSESSION was answered, {end} times before WM_ENDSESSION was");
    assert_eq!(query, 1, "Windows was told the program was ready for the session to end before the program had been asked to save");
    assert_eq!(end, 1, "Windows was told the session could end before the program had been asked to save");
}

/// A fact the contract does not state: `Close` is not the last event and
/// does not come once. A session that really ends sends it twice (when
/// Windows asks, and when it ends); one that is called off sends it once
/// and the program carries on.
#[test]
fn close_comes_twice_when_a_session_ends_and_is_not_final_when_one_is_called_off() {
    struct Counts {
        notes: Notes,
        at_f1: Arc<AtomicUsize>,
    }
    impl Program for Counts {
        fn event(&mut self, event: Event, _: &mut dyn Host) {
            match event {
                Event::Close => {
                    self.notes.closes.fetch_add(1, Ordering::AcqRel);
                }
                Event::Key { key: Key::F(1), .. } => self.at_f1.store(self.notes.closes.load(Ordering::Acquire), Ordering::Release),
                _ => {}
            }
        }
        fn wants(&mut self, _: &dyn Host) -> Wants {
            Wants { fullscreen: false, ..Wants::default() }
        }
        fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
            frame.clear(GROUND);
        }
    }

    let notes = Notes::default();
    unseen("close-twice", vec![Step::EndSession(true)], Counts { notes: notes.clone(), at_f1: Arc::default() });
    assert_eq!(notes.closes.load(Ordering::Acquire), 2);

    let (notes, at_f1) = (Notes::default(), Arc::new(AtomicUsize::new(usize::MAX)));
    unseen("close-called-off", vec![Step::EndSession(false), Step::Press(Key::F(1))], Counts { notes: notes.clone(), at_f1: Arc::clone(&at_f1) });
    assert_eq!(at_f1.load(Ordering::Acquire), 1, "a key arrived after Close, and Close had come once");
    assert_eq!(notes.closes.load(Ordering::Acquire), 2, "and once more when the window did close");
}

/// The window an input method's string is sent to, and how many times the
/// program had drawn when the sending began and when it was over.
static COMMIT_TO: AtomicIsize = AtomicIsize::new(0);
static COMMIT_DRAWS: AtomicUsize = AtomicUsize::new(0);
static COMMIT_COST: Mutex<Option<(usize, usize)>> = Mutex::new(None);
const COMMITTED: &str = "the quick brown fox jumps over the lazy dog and keeps on going until the sentence is long \u{1F600}";

/// Runs once from the message loop, as a message of its own. Sends the
/// string the way an input method's result reaches a window that does not
/// speak to input methods itself: `DefWindowProcW` turns the result into one
/// `WM_IME_CHAR` per UTF-16 unit, each sent, not posted, from inside the
/// handling of one message.
unsafe extern "system" fn commit(_: HWND, _: u32, id: usize, _: u32) {
    unsafe { KillTimer(0, id) };
    let before = COMMIT_DRAWS.load(Ordering::Acquire);
    for unit in COMMITTED.encode_utf16() {
        unsafe { SendMessageW(COMMIT_TO.load(Ordering::Acquire), WM_IME_CHAR, unit as usize, 1) };
    }
    if let Ok(mut cost) = COMMIT_COST.try_lock() {
        *cost = Some((before, COMMIT_DRAWS.load(Ordering::Acquire)));
    }
}

/// A fact, measured because nothing else covers this road: dictation, the
/// emoji panel and input methods hand a finished string to a plain window
/// as `WM_IME_CHAR` messages sent one inside another. The shell passes them
/// to `DefWindowProcW`, which on this Windows posts a character message for
/// each, so they join the queue, the shell's guard against drawing once per
/// character sees them there, and a surrogate pair is still joined. Had
/// `DefWindowProcW` sent them instead, each would have been drawn alone.
#[test]
fn a_string_committed_by_an_input_method_arrives_whole_and_is_drawn_once() {
    struct Typist {
        name: &'static str,
        notes: Notes,
        dirty: bool,
        armed: bool,
    }
    impl Program for Typist {
        fn event(&mut self, event: Event, _: &mut dyn Host) {
            match event {
                Event::Resize { .. } if !self.armed => {
                    self.armed = true;
                    COMMIT_TO.store(window_of(self.name), Ordering::Release);
                    unsafe { SetTimer(0, 0, 60, Some(commit)) };
                }
                Event::Text(ch) => {
                    self.notes.text.lock().unwrap().push(ch);
                    self.dirty = true;
                }
                _ => {}
            }
        }
        fn wants(&mut self, _: &dyn Host) -> Wants {
            Wants { redraw: self.dirty, fullscreen: false, ..Wants::default() }
        }
        fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
            self.dirty = false;
            COMMIT_DRAWS.fetch_add(1, Ordering::AcqRel);
            frame.clear(GROUND);
        }
    }

    let notes = Notes::default();
    unseen("ime-commit", vec![Step::Wait(500)], Typist { name: "ime-commit", notes: notes.clone(), dirty: false, armed: false });
    assert_eq!(*notes.text.lock().unwrap(), COMMITTED, "every character of the string arrived, in order, the last one joined from its two halves");
    let (before, when_sent) = COMMIT_COST.lock().unwrap().take().expect("the string was sent");
    let in_all = COMMIT_DRAWS.load(Ordering::Acquire) - before;
    let count = COMMITTED.chars().count();
    println!("{count} characters committed at once: drawn {} times while they were being sent, {in_all} times in all", when_sent - before);
    assert!(in_all <= 3, "{count} characters committed at once were drawn {in_all} times");
}

/// The shell's own account of itself: an event that arrives while the
/// program is busy waits its turn in a queue. That always held while the
/// program was in `event`. While it was in `render`, reached from the loop
/// rather than from an event (which is how most frames are drawn during
/// fast typing, when the draw is put off until the keys run out), nothing
/// marked the shell busy: an event that arrived then found the program
/// borrowed and was dropped without a word. The shell is now busy for the
/// length of that draw too, and empties the queue when the frame is done.
///
/// Nothing in Windows calls the window procedure from inside GDI, so this
/// takes a program whose `render` lets messages in. It is here because the
/// thing that was dropped is a character.
#[test]
fn a_character_that_arrives_while_the_frame_is_being_drawn_is_not_lost() {
    struct Painter {
        name: &'static str,
        notes: Notes,
        dirty: bool,
        sent: bool,
    }
    impl Program for Painter {
        fn event(&mut self, event: Event, _: &mut dyn Host) {
            if let Event::Text(ch) = event {
                self.notes.text.lock().unwrap().push(ch);
                self.dirty = true;
                if ch == 'a' {
                    // A key coming up behind the character: the shell sees
                    // more keyboard waiting and puts the draw off.
                    unsafe { PostMessageW(window_of(self.name), WM_KEYUP, b'A' as usize, 0xC01E_0001u32 as isize) };
                }
            }
        }
        fn wants(&mut self, _: &dyn Host) -> Wants {
            Wants { redraw: self.dirty, fullscreen: false, ..Wants::default() }
        }
        fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
            self.dirty = false;
            frame.clear(GROUND);
            if !self.sent && *self.notes.text.lock().unwrap() == "a" {
                self.sent = true;
                unsafe { SendMessageW(window_of(self.name), WM_CHAR, b'x' as usize, 1) };
            }
        }
    }

    let notes = Notes::default();
    unseen("char-mid-draw", vec![Step::Chars(vec![b'a' as u16]), Step::Wait(100)], Painter { name: "char-mid-draw", notes: notes.clone(), dirty: false, sent: false });
    assert_eq!(*notes.text.lock().unwrap(), "ax", "the character sent while the frame was being drawn never reached the program");
}

/// A line break reaches the program only as the Enter key. When it comes as
/// a character with no Enter key behind it (injected text arrives behind
/// the packet key, 0xE7; an input method's string arrives as bare
/// characters) the shell used to drop it as a control character and the two
/// lines ran together. It now sends the Enter key for it: once for a
/// carriage return, once for a line feed, and once, not twice, for the two
/// together. Whether his dictation sends "new line" as a key or as a
/// character could not be found out without typing into his own window;
/// either way it is now a line break.
///
/// (This test first pinned the drop, as `..._is_dropped`. The drop was the
/// finding; the assertion changed with the repair.)
#[test]
fn a_line_break_that_comes_as_a_character_and_not_as_the_enter_key_is_still_a_line_break() {
    let script = vec![
        Step::Type(String::from("one")),
        Step::Vk(0xE7, true),
        Step::Chars(vec![0x0D]),
        Step::Vk(0xE7, false),
        Step::Chars(vec![0x0D, 0x0A]),
        Step::Type(String::from("two")),
    ];
    let screen = Screen::of(&support::run("review-line-break", script));
    assert_eq!(screen.text_row(0), "one", "the carriage return behind the packet key broke the line");
    assert_eq!(screen.text_row(1), "", "and the bare pair broke it once more, not twice");
    assert_eq!(screen.text_row(2), format!("two{BLOCK}"));
    // The key itself breaks the line once: its own character is not a second break.
    let script = vec![Step::Type(String::from("one")), Step::Press(Key::Enter), Step::Type(String::from("two"))];
    let screen = Screen::of(&support::run("review-line-break-key", script));
    assert_eq!((screen.text_row(0), screen.text_row(1)), (String::from("one"), format!("two{BLOCK}")));
}

/// A fact, and the one piece of AltGr that a US keyboard can show: a
/// character that arrives while Ctrl and Alt are both held is text, which
/// is what AltGr typing is. A printable character that arrives while Ctrl
/// alone is held is dropped whatever sent it, which is the price of
/// keeping Ctrl Space from typing a space.
#[test]
fn a_character_is_text_under_ctrl_with_alt_and_dropped_under_ctrl_alone() {
    let euro = Step::Chars(vec![0x20AC]);
    let ctrl_alt = Mods { ctrl: true, alt: true, shift: false };
    let report = support::run("review-altgr", vec![Step::Hold(ctrl_alt), euro.clone(), Step::Hold(Mods::NONE)]);
    assert_eq!(report.title, "ok demo, 1 character", "AltGr text was dropped");
    let report = support::run("review-ctrl-text", vec![Step::Hold(Mods::CTRL), euro, Step::Hold(Mods::NONE)]);
    assert_eq!(report.title, "ok demo, 0 characters");
}

/// A fact the contract does not state: `wants` is asked once before the
/// program has been told any size, to learn the cell, the smallest window
/// and whether to open full screen. Whatever it works out from its size
/// (the caret's place, say) is worked out from nothing that first time.
#[test]
fn wants_is_asked_before_the_first_resize() {
    struct Ordered(Notes);
    impl Program for Ordered {
        fn event(&mut self, event: Event, _: &mut dyn Host) {
            let word = match event {
                Event::Resize { .. } => "resize",
                Event::Close => "close",
                _ => "other",
            };
            self.0.calls.lock().unwrap().push(word.to_string());
        }
        fn wants(&mut self, _: &dyn Host) -> Wants {
            self.0.calls.lock().unwrap().push(String::from("wants"));
            Wants { fullscreen: false, ..Wants::default() }
        }
        fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
            self.0.calls.lock().unwrap().push(String::from("render"));
            self.0.draws.fetch_add(1, Ordering::AcqRel);
            frame.clear(GROUND);
        }
    }
    let notes = Notes::default();
    unseen("order", vec![], Ordered(notes.clone()));
    let calls = notes.calls.lock().unwrap().clone();
    println!("{calls:?}");
    assert_eq!(calls[..4], ["wants", "resize", "wants", "render"]);
    assert_eq!(calls.last().map(String::as_str), Some("close"), "nothing is asked of the program after its last Close");
}

/// A fact, tried because the builder could only reason about it: a
/// character typed as an Alt code on the number pad (Alt 0 2 3 3 for an e
/// with an acute accent) is text, and the digits are not chords.
#[test]
fn an_alt_code_typed_on_the_number_pad_is_text() {
    let pad = |digit: u16| [Step::Vk(0x60 + digit, true), Step::Vk(0x60 + digit, false)];
    let mut script = vec![Step::Type(String::from("a")), Step::Hold(Mods::ALT)];
    for digit in [0, 2, 3, 3] {
        script.extend(pad(digit));
    }
    script.push(Step::Hold(Mods::NONE));
    let report = support::run("review-alt-code", script);
    let screen = Screen::of(&report);
    println!("after Alt 0233: {:?}, last event {:?}", report.title, screen.last_event());
    assert_eq!(report.title, "ok demo, 2 characters", "the composed character did not arrive");
    assert_eq!(screen.last_event(), "Text U+00E9");
}
