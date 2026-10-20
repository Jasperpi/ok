//! The clocks: frame ticks at the display's even frame period for as long as
//! the program animates and not one tick longer, and neither they nor a
//! one-shot wake held back by a queue that is never empty.
//!
//! One test, alone in its file and so alone in its process, because what it
//! measures is time. Its parts run one after another for the same reason.

mod support;

use ok_win::{Report, Step, TickSource};
use support::demo::{OK_AFTER_MS, RUN_MS};
use support::{run, Screen};

fn ticks_from(report: &Report, source: TickSource) -> Vec<f64> {
    report.ticks.iter().filter(|t| t.source == source).map(|t| t.at_ms).collect()
}

fn frame_ticks(report: &Report) -> Vec<f64> {
    ticks_from(report, TickSource::Frame)
}

fn type_a() -> Step {
    Step::Type(String::from("a"))
}

/// Moves posted all at once. Every move makes the demo draw, so handling
/// them is one long stretch of work during which the queue is never empty.
/// An optimized build draws several times faster and is given more to do.
fn trail() -> Step {
    let moves = if cfg!(debug_assertions) { 1500 } else { 4000 };
    Step::MouseTrail((0..moves).map(|i| (100 + i % 400, 100 + i % 97)).collect())
}

/// Where the last move of [`trail`] leaves the pointer, as the demo says it.
fn trail_end() -> String {
    let Step::MouseTrail(points) = trail() else { unreachable!() };
    let (x, y) = points[points.len() - 1];
    format!("Mouse at {x}, {y}")
}

#[test]
fn the_clocks_keep_time() {
    frames_come_at_the_frame_period_and_then_stop();
    a_plain_timer_stands_in_while_windows_runs_its_own_loop();
    a_busy_queue_does_not_hold_frames_back();
    a_busy_queue_does_not_hold_a_wake_back();
}

fn frames_come_at_the_frame_period_and_then_stop() {
    // One key: the demo animates for a second. Then well over a second of nothing.
    let report = run("cadence", vec![type_a(), Step::Wait(OK_AFTER_MS + 500), Step::Probe]);
    let period = report.frame_period_ms;
    // The even division of any refresh rate from 50 to 240 Hz that is nearest 60 a second.
    assert!((12.0..=21.0).contains(&period), "a frame period of {period:.2} ms");

    let ticks = frame_ticks(&report);
    let expected = RUN_MS as f64 / period;
    assert!(
        ticks.len() as f64 >= expected * 0.85 && ticks.len() as f64 <= expected + 2.0,
        "{} frame ticks in a second at {period:.2} ms each; about {expected:.0} were due",
        ticks.len()
    );
    let mut gaps: Vec<f64> = ticks.windows(2).map(|pair| pair[1] - pair[0]).collect();
    gaps.sort_by(|a, b| a.total_cmp(b));
    let median = gaps[gaps.len() / 2];
    println!(
        "{} frame ticks at a period of {period:.2} ms: gaps from {:.2} to {:.2} ms, median {median:.2}",
        ticks.len(),
        gaps[0],
        gaps[gaps.len() - 1]
    );
    assert!((median - period).abs() < 1.5, "the usual gap between ticks was {median:.2} ms, not {period:.2}");
    let on_time = gaps.iter().filter(|gap| (**gap - period).abs() < 4.0).count();
    assert!(on_time * 10 >= gaps.len() * 9, "only {on_time} of {} gaps were within 4 ms of the period", gaps.len());

    // It stops: the run of ticks is a second long, and the probe at the end found no more.
    let ran = ticks[ticks.len() - 1] - ticks[0];
    assert!(ran <= RUN_MS as f64 + 2.0 * period, "ticks went on for {ran:.0} ms");
    let probe = report.probes[0];
    assert!(probe.at_ms - ticks[ticks.len() - 1] > 700.0, "the last tick was long before the end");
    assert_eq!(probe.ticks, report.ticks.len(), "nothing ticked after the probe either");
    assert!(!Screen::of(&report).marker_showing(), "the marker was taken off the desk by the last frame");

    // Caught part way through, the marker is on the desk.
    let midway = run("cadence-midway", vec![type_a(), Step::Wait(RUN_MS / 3)]);
    assert!(Screen::of(&midway).marker_showing());
    assert!(frame_ticks(&midway).len() >= 5);
}

/// While a window is dragged or resized Windows runs a loop of its own, the
/// frame clock cannot run, and a plain timer keeps the ticks coming.
fn a_plain_timer_stands_in_while_windows_runs_its_own_loop() {
    let script = vec![type_a(), Step::Moving(true), Step::Wait(500), Step::Moving(false), Step::Wait(RUN_MS + 300)];
    let moved = run("cadence-moving", script);
    let moving = ticks_from(&moved, TickSource::Moving);
    println!("{} ticks in half a second while moving", moving.len());
    assert!(moving.len() >= 12, "{} ticks while moving", moving.len());
    let (first, last) = (moving[0], moving[moving.len() - 1]);
    let frames_meanwhile = frame_ticks(&moved).iter().filter(|at| **at > first && **at < last).count();
    assert_eq!(frames_meanwhile, 0, "the two clocks never run at once");
    assert!(frame_ticks(&moved).iter().any(|at| *at > last), "the frame clock takes over again afterwards");
    assert!(!Screen::of(&moved).marker_showing());
}

fn a_busy_queue_does_not_hold_frames_back() {
    // The two probes mark where the trail starts and ends; the stretch
    // looked at stops short of the end of the demo's second of animation.
    let busy = run("cadence-busy", vec![type_a(), Step::Probe, trail(), Step::Probe]);
    let (from, to) = (busy.probes[0].at_ms, busy.probes[1].at_ms.min(busy.probes[0].at_ms + RUN_MS as f64 - 100.0));
    let meanwhile = frame_ticks(&busy).iter().filter(|at| **at > from && **at < to).count();
    let due = (to - from) / busy.frame_period_ms;
    println!("{meanwhile} frame ticks in {:.0} ms of unbroken messages; {due:.0} were due", to - from);
    assert!(to - from >= 100.0, "the trail was handled in {:.0} ms, too short a stretch to say anything", to - from);
    assert!(meanwhile as f64 >= due * 0.6, "{meanwhile} frame ticks while the queue was never empty; {due:.0} were due");
    assert_eq!(Screen::of(&busy).last_event(), trail_end(), "and every move was still handled, in order");
}

/// The demo asks for a wake a while after the last character. Here it falls
/// due about 300 ms into a trail.
fn a_busy_queue_does_not_hold_a_wake_back() {
    let script = vec![type_a(), Step::Wait(OK_AFTER_MS - 300), Step::Probe, trail(), Step::Probe];
    let busy = run("cadence-busy-wake", script);
    let (from, to) = (busy.probes[0].at_ms, busy.probes[1].at_ms);
    let wakes = ticks_from(&busy, TickSource::Wake);
    println!("the wake came {:.0} ms into {:.0} ms of unbroken messages", wakes.first().map_or(f64::NAN, |at| at - from), to - from);
    assert!(to - from >= 700.0, "the trail was handled in {:.0} ms, too short a stretch to say anything", to - from);
    assert_eq!(wakes.len(), 1, "one wake, once");
    assert!(wakes[0] > from && wakes[0] < from + 500.0, "the wake came {:.0} ms into the trail", wakes[0] - from);
    assert_eq!(Screen::of(&busy).machine_line(), "ok");
}
