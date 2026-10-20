//! A self-test that cannot finish must still end.
//!
//! There are three ways for one to get stuck, and `SelfTest::timeout_ms` has
//! to end all of them. A script that is merely too long, and a queue that is
//! never empty (which keeps Windows from ever handing out a timer, so the
//! clock cannot be one), leave the window handling messages: it is closed
//! and its report says so. A program that never returns from a call leaves
//! nobody to close anything or report anything, so the whole process is
//! ended. That one is tried in a child process, which is this same test
//! program started again with one test named.
#![cfg(windows)]

#[path = "review_ffi.rs"]
mod ffi;

use std::io::Read as _;
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ffi::*;
use ok_gfx::{Frame, Rgb};
use ok_host::{Event, Host, Key, Program, Wants};
use ok_win::{SelfTest, Step, EXIT_STUCK};

/// Set in the child process, which is how the test below knows it is the
/// one that is meant to get stuck.
const CHILD: &str = "OK_WIN_WATCHDOG_CHILD";
/// Keeps the child from getting a console window of its own.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What the program does when it is given a character or F1.
enum Habit {
    Nothing,
    /// Answers every character with another, posted to its own window, so
    /// the queue is never empty again.
    Chatters(&'static str),
    /// Never comes back from F1.
    Sleeps,
}

struct Stuck {
    habit: Habit,
    characters: Arc<AtomicUsize>,
}

impl Program for Stuck {
    fn event(&mut self, event: Event, _: &mut dyn Host) {
        match (&self.habit, event) {
            (Habit::Chatters(name), Event::Text(_)) => {
                self.characters.fetch_add(1, Ordering::AcqRel);
                unsafe { PostMessageW(window_of(name), WM_CHAR, b'x' as usize, 1) };
            }
            (Habit::Sleeps, Event::Key { key: Key::F(1), .. }) => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            _ => {}
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants { fullscreen: false, ..Wants::default() }
    }
    fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
        frame.clear(Rgb(0x35, 0x25, 0x1B));
    }
}

/// Runs a script with 300 ms to finish in. Returns the report, how many
/// characters the program was given, and how long the run took.
fn run_short(name: &'static str, habit: Habit, script: Vec<Step>) -> (ok_win::Report, usize, Duration) {
    let characters = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&characters);
    let test = SelfTest { timeout_ms: 300, ..SelfTest::new(folder(name), script) };
    let started = Instant::now();
    let report = ok_win::selftest(options(name), test, move |_| Box::new(Stuck { habit, characters: counted }));
    (report, characters.load(Ordering::Acquire), started.elapsed())
}

#[test]
fn a_script_that_is_too_long_is_closed_and_says_so() {
    let (report, _, took) = run_short("watchdog-long", Habit::Nothing, vec![Step::Wait(30_000)]);
    assert!(report.timed_out, "the test was not stopped when its time ran out");
    assert!(report.cloaked && !report.was_foreground);
    assert_eq!((report.exit_code, report.crashed), (0, None));
    assert!(took >= Duration::from_millis(280) && took < Duration::from_secs(5), "took {took:?}");

    // A script that finishes in time is left alone.
    let (report, _, _) = run_short("watchdog-in-time", Habit::Nothing, vec![Step::Wait(50)]);
    assert!(!report.timed_out);
}

/// Windows hands out a timer only when the queue is empty, so a clock kept
/// by a timer on the window's own thread would never get to speak here.
#[test]
fn a_queue_that_is_never_empty_does_not_keep_the_clock_from_ending_the_test() {
    let name = "watchdog-chatter";
    let (report, characters, took) = run_short(name, Habit::Chatters(name), vec![Step::Chars(vec![b'x' as u16])]);
    println!("{characters} characters went round in {took:?}");
    assert!(report.timed_out, "the test was not stopped when its time ran out");
    assert!(report.cloaked && !report.was_foreground);
    assert!(characters > 100, "the queue was meant to be kept busy: {characters} characters");
    assert!(took < Duration::from_secs(5), "took {took:?}");
}

/// The window's thread never comes back, so nothing on it can end the test:
/// the watchdog's own thread says why on standard error and ends the
/// process with [`EXIT_STUCK`]. The process ended is a child, so that this
/// test can see it happen. Its window is cloaked from the moment it is made
/// like any other self-test's, and it gets no console.
#[test]
fn a_program_that_never_returns_is_ended_with_its_process() {
    const THIS: &str = "a_program_that_never_returns_is_ended_with_its_process";
    if std::env::var_os(CHILD).is_some() {
        // Never returns: the watchdog ends the process from its own thread.
        run_short("watchdog-stuck", Habit::Sleeps, vec![Step::Press(Key::F(1))]);
        return;
    }

    let exe = std::env::current_exe().expect("this test's own program");
    let started = Instant::now();
    let mut child = Command::new(exe)
        .args([THIS, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("the child starts");
    // 300 ms for the test and a second of grace. If the child is still there
    // long after that the watchdog has failed, and this test ends it.
    let give_up = started + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("the child can be asked how it is") {
            Some(status) => break Some(status),
            None if Instant::now() >= give_up => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let took = started.elapsed();
    let mut said = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut said);
    }
    let Some(status) = status else { panic!("the stuck child was still running after {took:?} and had to be killed. It said: {said}") };
    println!("the child ended after {took:?} with {status}");
    assert_eq!(status.code(), Some(EXIT_STUCK), "{said}");
    assert!(said.contains("A call into the program never returned"), "{said}");
    assert!(took >= Duration::from_millis(1_200), "ended before the test's time and its grace were up: {took:?}");
}
