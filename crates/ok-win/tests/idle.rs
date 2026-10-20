//! Waiting costs nothing.
//!
//! This is the only test in its file, so it is the only thing running in its
//! process: the processor time measured is the whole process's
//! (`GetProcessTimes`), and with nothing animating it should not move.
#![cfg(windows)]

mod support;

use ok_win::Step;
use support::demo::OK_AFTER_MS;
use support::run;

/// Two ticks of the scheduler's clock. Processor time is counted in units of
/// 15.6 ms, so this is the smallest allowance that cannot fail by rounding.
const ESSENTIALLY_NOTHING_MS: f64 = 31.25;

#[test]
fn idle_uses_no_processor_time() {
    // Nothing has been typed: the window is open and waiting.
    let report = run("idle", vec![Step::Wait(500), Step::Probe, Step::Wait(2000), Step::Probe]);
    let (before, after) = (report.probes[0], report.probes[1]);
    let waited = after.at_ms - before.at_ms;
    let used = after.cpu_ms - before.cpu_ms;
    println!("idle before typing: {used:.1} ms of processor time in {waited:.0} ms");
    assert!(waited >= 1900.0, "the pause was {waited:.0} ms");
    assert!(used <= ESSENTIALLY_NOTHING_MS, "{used:.1} ms of processor time in {waited:.0} ms of waiting");
    assert_eq!(after.ticks, before.ticks, "no tick was sent to an idle program");
    assert_eq!(after.renders, before.renders, "and nothing was drawn");

    // And again after the frame clock has run and stopped: it leaves nothing behind.
    let script = vec![Step::Type(String::from("some typing")), Step::Wait(OK_AFTER_MS + 500), Step::Probe, Step::Wait(2000), Step::Probe];
    let report = run("idle-after", script);
    let (before, after) = (report.probes[0], report.probes[1]);
    let used = after.cpu_ms - before.cpu_ms;
    println!("idle after animating: {used:.1} ms of processor time in {:.0} ms", after.at_ms - before.at_ms);
    assert!(before.ticks > 20, "the animation ran first: {} ticks", before.ticks);
    assert!(used <= ESSENTIALLY_NOTHING_MS, "{used:.1} ms of processor time while idle after animating");
    assert_eq!(after.ticks, before.ticks);
    assert_eq!(after.renders, before.renders);
}
