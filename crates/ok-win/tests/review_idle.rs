//! Review: is an idle window really blocked, or only cheap?
//!
//! `tests/idle.rs` reads the process's processor time, which Windows counts
//! in steps of 15.6 ms: a loop that woke fifty times a second for a few
//! microseconds would read as 0.0 ms there. This measures the window's own
//! thread two finer ways. Its cycle count moves for every cycle the thread
//! runs, and a hook on the thread's message queue names every message the
//! loop takes out of it. Blocked means no cycles and no messages.
//!
//! One test, alone in its file, with its parts one after another, because
//! the hook's notes are kept in one place.
#![cfg(windows)]

mod support;

#[path = "review_ffi.rs"]
mod ffi;

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ffi::*;
use ok_gfx::{Frame, Rect, Rgb};
use ok_host::{Event, Host, Program, Wants};
use ok_win::{SelfTest, Step};
use support::demo::{Demo, OK_AFTER_MS, RUN_MS};

static HOOK: AtomicIsize = AtomicIsize::new(0);
/// When the window was told its size, and every message taken from the
/// queue since: milliseconds after that moment, and the message's number.
struct Taken {
    since: Option<Instant>,
    messages: Vec<(f64, u32)>,
}

static TAKEN: Mutex<Taken> = Mutex::new(Taken { since: None, messages: Vec::new() });

unsafe extern "system" fn note_message(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 && wparam == PM_REMOVE {
        if let (Some(message), Ok(mut taken)) = (unsafe { (lparam as *const MSG).as_ref() }, TAKEN.try_lock()) {
            if let Some(since) = taken.since {
                taken.messages.push((since.elapsed().as_secs_f64() * 1000.0, message.message));
            }
        }
    }
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

/// Wraps a program. When the program is first told its size, a second
/// thread starts reading the window thread's cycle count at the moments
/// asked for, and the hook starts noting messages.
struct Timed {
    inner: Box<dyn Program>,
    at_ms: Vec<u64>,
    cycles: Arc<Mutex<Vec<u64>>>,
    started: bool,
}

impl Program for Timed {
    fn event(&mut self, event: Event, host: &mut dyn Host) {
        if matches!(event, Event::Resize { .. }) && !self.started {
            self.started = true;
            let started = Instant::now();
            *TAKEN.lock().unwrap() = Taken { since: Some(started), messages: Vec::new() };
            HOOK.store(unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(note_message), 0, GetCurrentThreadId()) }, Ordering::Release);
            let (window_thread, at_ms, cycles) = (this_thread(), self.at_ms.clone(), Arc::clone(&self.cycles));
            std::thread::spawn(move || {
                for at in at_ms {
                    std::thread::sleep(Duration::from_millis(at).saturating_sub(started.elapsed()));
                    cycles.lock().unwrap().push(cycles_of(window_thread));
                }
                unsafe { CloseHandle(window_thread) };
            });
        }
        if matches!(event, Event::Close) {
            unsafe { UnhookWindowsHookEx(HOOK.swap(0, Ordering::AcqRel)) };
        }
        self.inner.event(event, host);
    }
    fn wants(&mut self, host: &dyn Host) -> Wants {
        self.inner.wants(host)
    }
    fn render(&mut self, frame: &mut Frame, host: &mut dyn Host) {
        self.inner.render(frame, host);
    }
}

/// A program with nothing to do. With `caret` it says where its text cursor
/// is, as the real interface always will.
struct Still {
    caret: bool,
}

impl Program for Still {
    fn event(&mut self, _: Event, _: &mut dyn Host) {}
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { fullscreen: false, caret: self.caret.then(|| Rect::new(240, 110, 12, 22)), ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        frame.clear(Rgb(0xEE, 0xE3, 0xC8));
    }
}

/// What the window's thread did between two moments.
struct Stretch {
    ms: u64,
    cycles: u64,
    messages: Vec<(f64, u32)>,
    /// Where the hidden system caret was when the window closed, if it had one.
    caret: Option<(i32, i32)>,
}

/// Runs a program unseen and reports the stretch from `from_ms` to `to_ms`
/// after it was told its size. The script must outlast `to_ms`.
fn measure(name: &str, script: Vec<Step>, from_ms: u64, to_ms: u64, make: impl FnOnce(&mut dyn Host) -> Box<dyn Program>) -> Stretch {
    let cycles = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&cycles);
    let report = ok_win::selftest(options(name), SelfTest::new(folder(name), script), move |host| {
        Box::new(Timed { inner: make(host), at_ms: vec![from_ms, to_ms], cycles: kept, started: false })
    });
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    assert_eq!(report.crashed, None);
    let cycles = cycles.lock().unwrap().clone();
    assert_eq!(cycles.len(), 2, "{name}: the window closed before the stretch was over");
    let messages = TAKEN.lock().unwrap().messages.iter().copied().filter(|(at, _)| *at >= from_ms as f64 && *at < to_ms as f64).collect();
    Stretch { ms: to_ms - from_ms, cycles: cycles[1] - cycles[0], messages, caret: report.caret }
}

fn say(what: &str, stretch: &Stretch, per_ms: f64) -> f64 {
    let micros = stretch.cycles as f64 / per_ms * 1000.0;
    let mut kinds: Vec<u32> = stretch.messages.iter().map(|(_, message)| *message).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let named: Vec<String> = kinds.iter().map(|k| format!("{k:#06x} x{}", stretch.messages.iter().filter(|(_, m)| m == k).count())).collect();
    println!(
        "{what}: {} cycles ({micros:.0} microseconds of work) in {} ms; {} messages taken from the queue [{}]",
        stretch.cycles,
        stretch.ms,
        stretch.messages.len(),
        named.join(", ")
    );
    micros
}

#[test]
fn an_idle_window_thread_does_not_run_at_all() {
    let per_ms = cycles_per_ms();
    println!("this machine charges about {:.2} million cycles for a millisecond of work", per_ms / 1e6);

    // Open and waiting, never focused, nothing asked for.
    let plain = measure("idle-plain", vec![Step::Wait(3000)], 600, 2600, |_| Box::new(Still { caret: false }));
    let plain_micros = say("idle, never focused", &plain, per_ms);

    // The same with the focus and a text cursor, which is how ok really
    // sits: the shell then keeps a hidden system caret in the cursor's cell.
    let focused = measure("idle-focused", vec![Step::Focus(true), Step::Wait(3000)], 600, 2600, |_| Box::new(Still { caret: true }));
    let focused_micros = say("idle, focused, with a caret", &focused, per_ms);
    assert_eq!(focused.caret, Some((240, 110)), "the hidden caret was really there while this was measured");

    // After the demo has animated for its second and printed ok.
    let after_ms = RUN_MS.max(OK_AFTER_MS) + 700;
    let script = vec![Step::Type(String::from("a")), Step::Wait(after_ms + 2400)];
    let after = measure("idle-after", script, after_ms, after_ms + 2000, |host| Box::new(Demo::new(host)));
    let after_micros = say("idle, after animating", &after, per_ms);

    // For scale: the same thread while the demo animates.
    let script = vec![Step::Type(String::from("a")), Step::Wait(RUN_MS + 300)];
    let busy = measure("idle-scale", script, 150, 850, |host| Box::new(Demo::new(host)));
    say("for scale, animating", &busy, per_ms);

    assert!(plain.messages.is_empty(), "an idle window took {} messages from its queue: {:?}", plain.messages.len(), plain.messages);
    assert!(plain_micros < 200.0, "an idle window's thread worked for {plain_micros:.0} microseconds in two seconds");
    assert!(after.messages.is_empty(), "idle after animating, the window took messages from its queue: {:?}", after.messages);
    assert!(after_micros < 200.0, "idle after animating, the window's thread worked for {after_micros:.0} microseconds in two seconds");
    // With the focus the hidden caret's blink timer may wake the loop; that
    // is Windows' doing and is allowed for, but nothing else is.
    const SYSTEM_TIMER: u32 = 0x0118;
    let other: Vec<_> = focused.messages.iter().filter(|(_, message)| *message != SYSTEM_TIMER).collect();
    assert!(other.is_empty(), "idle with the focus, the window took messages that are not the caret's timer: {other:?}");
    assert!(focused_micros < 2000.0, "idle with the focus, the window's thread worked for {focused_micros:.0} microseconds in two seconds");
}
