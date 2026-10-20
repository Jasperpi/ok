//! Review: a message loop that is not the shell's own.
//!
//! While a window is dragged, resized or showing its menu, Windows runs a
//! loop of its own inside `DefWindowProcW` and the shell's `run` does not
//! turn. The shell's tests stand in for that by posting the two messages
//! that announce such a loop and letting `run` carry on, so nothing there
//! ever handles a message while `run` is stopped. A real drag cannot be
//! started without moving his pointer, so this does the next nearest thing:
//! a loop of its own, started from a timer on the window's thread, that
//! takes and hands out messages for half a second exactly as Windows' does.
//!
//! One test, alone in its file: what happened inside the loop is counted in
//! one place.
#![cfg(windows)]

#[path = "review_ffi.rs"]
mod ffi;

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ffi::*;
use ok_gfx::{Frame, Rgb};
use ok_host::{Event, Host, Program, Wants};
use ok_win::{SelfTest, Step, TickSource};

const TYPED: &str = "typed";

static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Whether the loop says it is there, as Windows does for a drag.
static ANNOUNCED: AtomicBool = AtomicBool::new(false);
static INSIDE: AtomicBool = AtomicBool::new(false);
static TEXT_INSIDE: AtomicUsize = AtomicUsize::new(0);
static TICKS_INSIDE: AtomicUsize = AtomicUsize::new(0);
static DRAWS_INSIDE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn a_loop_of_its_own(_: HWND, _: u32, id: usize, _: u32) {
    unsafe {
        KillTimer(0, id);
        let (hwnd, announced) = (WINDOW.load(Ordering::Acquire), ANNOUNCED.load(Ordering::Acquire));
        if announced {
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
        }
        INSIDE.store(true, Ordering::Release);
        let until = Instant::now() + Duration::from_millis(500);
        let mut message: MSG = std::mem::zeroed();
        'held: loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), left.as_millis() as u32 + 1, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            while PeekMessageW(&mut message, 0, 0, 0, PM_REMOVE as u32) != 0 {
                if message.message == WM_QUIT {
                    PostQuitMessage(message.wParam as i32);
                    break 'held;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        INSIDE.store(false, Ordering::Release);
        if announced {
            SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
        }
    }
}

/// Animates the whole time. While the other loop runs, a second thread
/// types a word into the window and then calls the waker.
struct Held {
    name: &'static str,
    text: Arc<Mutex<String>>,
    started: bool,
    dirty: bool,
}

impl Program for Held {
    fn event(&mut self, event: Event, host: &mut dyn Host) {
        let inside = INSIDE.load(Ordering::Acquire);
        match event {
            Event::Resize { .. } if !self.started => {
                self.started = true;
                let (hwnd, wake) = (window_of(self.name), host.waker());
                WINDOW.store(hwnd, Ordering::Release);
                unsafe { SetTimer(0, 0, 60, Some(a_loop_of_its_own)) };
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(250));
                    for unit in TYPED.encode_utf16() {
                        unsafe { PostMessageW(hwnd, WM_CHAR, unit as usize, 1) };
                    }
                    std::thread::sleep(Duration::from_millis(60));
                    wake();
                });
            }
            Event::Text(ch) => {
                self.text.lock().unwrap().push(ch);
                self.dirty = true;
                TEXT_INSIDE.fetch_add(inside as usize, Ordering::AcqRel);
            }
            Event::Tick => {
                self.dirty = true;
                TICKS_INSIDE.fetch_add(inside as usize, Ordering::AcqRel);
            }
            _ => {}
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { redraw: self.dirty, animating: true, fullscreen: false, ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        self.dirty = false;
        DRAWS_INSIDE.fetch_add(INSIDE.load(Ordering::Acquire) as usize, Ordering::AcqRel);
        frame.clear(Rgb(0x35, 0x25, 0x1B));
    }
}

/// What happened while the other loop ran: characters delivered, ticks
/// delivered, frames drawn, and the whole run's report.
fn hold(name: &'static str, announced: bool) -> (usize, usize, usize, String, ok_win::Report) {
    ANNOUNCED.store(announced, Ordering::Release);
    for count in [&TEXT_INSIDE, &TICKS_INSIDE, &DRAWS_INSIDE] {
        count.store(0, Ordering::Release);
    }
    let text = Arc::new(Mutex::new(String::new()));
    let kept = Arc::clone(&text);
    let report = ok_win::selftest(options(name), SelfTest::new(folder(name), vec![Step::Wait(900)]), move |_| {
        Box::new(Held { name, text: kept, started: false, dirty: false })
    });
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    assert_eq!(report.crashed, None);
    let text = text.lock().unwrap().clone();
    (TEXT_INSIDE.load(Ordering::Acquire), TICKS_INSIDE.load(Ordering::Acquire), DRAWS_INSIDE.load(Ordering::Acquire), text, report)
}

fn count(report: &ok_win::Report, source: TickSource) -> usize {
    report.ticks.iter().filter(|tick| tick.source == source).count()
}

#[test]
fn typing_and_ticks_get_through_a_loop_that_is_not_the_shells() {
    // Announced, as a drag or a menu is.
    let (text_inside, ticks_inside, draws_inside, text, report) = hold("modal-announced", true);
    let (moving, wakers) = (count(&report, TickSource::Moving), count(&report, TickSource::Waker));
    println!("announced: {text_inside} characters, {ticks_inside} ticks ({moving} from the stand-in timer, {wakers} from the waker) and {draws_inside} frames inside the loop");
    assert_eq!(text, TYPED);
    assert_eq!(text_inside, TYPED.len(), "what was typed during the loop arrived during the loop");
    assert!(draws_inside >= 2, "and was drawn there");
    assert!(moving >= 12, "the stand-in timer kept the animation going: {moving} ticks in half a second");
    assert_eq!(wakers, 1);
    assert!(ticks_inside > moving, "the waker's tick came inside the loop too");

    // Unannounced: any other loop, such as the one a host call runs while it
    // waits on another program. Typing must still arrive and be drawn. The
    // frame clock and the waker have nothing to stand in for them here.
    let (text_inside, ticks_inside, draws_inside, text, report) = hold("modal-unannounced", false);
    println!(
        "unannounced: {text_inside} characters, {ticks_inside} ticks and {draws_inside} frames inside the loop; {} waker ticks in the whole run",
        count(&report, TickSource::Waker)
    );
    assert_eq!(text, TYPED);
    assert_eq!(text_inside, TYPED.len(), "what was typed during the loop arrived during the loop");
    assert!(draws_inside >= 1, "and was drawn there");
    assert_eq!(count(&report, TickSource::Waker), 1, "the waker's tick was not lost, only kept until the loop ended");
}
