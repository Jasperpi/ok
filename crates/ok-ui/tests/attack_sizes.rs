//! Sizes: every small client size from 0 by 0 up, then several hundred
//! random ones, the scale stepped through 1, 2 and 3 at each, with keys
//! going in between. The same model and the same invariants as the editing
//! campaigns are checked after every event: nothing panics, a resize never
//! touches the text or the cursor, the tables stay the ones a fresh page
//! would have, the screen reads as the text does, and the cursor is drawn
//! inside the sheet.
//!
//! `OK_ATTACK_SCALE=10 cargo test -p ok-ui --offline --test attack_sizes`
//! runs ten times as many random sizes.

mod attack_support;

use attack_support::{random_events, report, run, scale, Checks, Ev, Known, Rng, Run, Setup, Snap};
use ok_host::{Key, Mods};
use ok_ui::harness::Rig;

const TEXT: &str = "# The orchard\n\nThe orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm.\n\n## What he did\n\n- thinned the fruit\n  - [ ] and a nested one\n1. first\n\n> It felt like **waste**. I filled buckets with fruit that would never ripen.\n\n```\nlet rows = 12;\n```\n\ne\u{0301} \u{4E00}\u{4E8C} tab\there \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} TK [[Grandfather interview]]\n\nI think about that tree whenever I am asked to add one more thing.";

fn known() -> Known {
    Known::default()
}

fn checks() -> Checks {
    Checks { known: known(), draw: true, undo_all: false }
}

/// At each size: the resize, then each scale with a few random events after
/// it. Fails with the run cut down to the events that matter.
fn sweep(text: &str, sizes: &[(usize, usize)], seed: u64, burst: usize) {
    let mut rng = Rng::new(seed);
    let mut events = Vec::new();
    for &(width, height) in sizes {
        events.push(Ev::Size(width, height));
        for step in [2, 3, 1] {
            events.push(Ev::Scale(step));
            let until = events.len() + burst;
            while events.len() < until {
                random_events(&mut rng, &mut events, &[(width, height)]);
            }
        }
    }
    let setup = Setup::new(text);
    if run(&setup, &events, checks()).is_err() {
        let (kind, told) = report(&setup, &events, checks(), seed).unwrap_or_default();
        panic!("{kind} in a sweep of {} sizes:\n\n{told}", sizes.len());
    }
}

/// Every width and height in pixels up to a few cells each way: no cell,
/// one cell, the first row of text, the say row, the machine line.
#[test]
fn every_client_size_from_nothing_up_to_a_few_cells() {
    let sizes: Vec<(usize, usize)> = (0..=40).flat_map(|width| (0..=120).step_by(3).map(move |height| (width * 3, height))).collect();
    sweep(TEXT, &sizes, 1, 1);
}

/// Every whole number of cells from 0 by 0 to past the smallest useful
/// window, and one pixel either side of each.
#[test]
fn every_size_in_cells_up_to_the_smallest_useful_window_and_past_it() {
    let mut sizes: Vec<(usize, usize)> = Vec::new();
    for cols in (0..=70usize).step_by(2) {
        for rows in 0..=16usize {
            let nudge = (cols + rows) % 3;
            sizes.push(((cols * 12 + nudge).saturating_sub(1), (rows * 22 + nudge).saturating_sub(1)));
        }
    }
    sweep(TEXT, &sizes, 2, 2);
}

/// The widths at which the contents, the map and the labels come and go,
/// at every height from none to a full screen.
#[test]
fn every_width_where_a_side_comes_or_goes() {
    let mut sizes = Vec::new();
    for cols in [79, 80, 81, 92, 93, 94, 105, 106, 107, 108, 124, 125, 126, 127, 130, 131, 132, 159, 160, 161, 200] {
        for rows in [0, 1, 2, 3, 4, 5, 6, 7, 12, 24, 49] {
            sizes.push((cols * 12, rows * 22));
        }
    }
    sweep(TEXT, &sizes, 3, 2);
}

/// Several hundred sizes nobody chose.
#[test]
fn several_hundred_random_sizes_with_keys_between() {
    let mut rng = Rng::new(77);
    let count = 320 * scale() as usize;
    let sizes: Vec<(usize, usize)> = (0..count)
        .map(|_| match rng.below(10) {
            0 => (rng.below(200), rng.below(200)),
            1 | 2 => (rng.below(900), rng.below(600)),
            3 => (rng.below(4000), rng.below(40)),
            4 => (rng.below(40), rng.below(2400)),
            _ => (rng.below(2200), rng.below(1300)),
        })
        .collect();
    for (part, sizes) in sizes.chunks(80).enumerate() {
        sweep(TEXT, sizes, 100 + part as u64, 3);
    }
}

/// A text of nothing, and a text that is one very long word, through the
/// same small sizes.
#[test]
fn small_sizes_with_an_empty_text_and_with_one_long_word() {
    let sizes: Vec<(usize, usize)> = (0..=30).flat_map(|cols| (0..=8).map(move |rows| (cols * 12 + 5, rows * 22 + 9))).collect();
    sweep("", &sizes, 5, 2);
    sweep(&"x".repeat(5000), &sizes, 6, 1);
}

/// A resize by itself, to any size and back, leaves the text, the cursor
/// and the screen exactly as they were.
#[test]
fn a_resize_to_any_size_and_back_shows_the_same_screen() {
    let mut rng = Rng::new(9);
    let mut rig = Rig::with_text(TEXT, 1920, 1080);
    for line in 0..6 {
        for _ in 0..line {
            rig.key(Key::Up, Mods::NONE);
        }
        for _ in 0..40 {
            // As things stand before this resize: one through a smaller
            // sheet may have moved the paper to keep the cursor on it, and
            // a text shorter than the sheet has room to stay where that
            // left it.
            let before = (Snap::of(&rig), rig.app.view().top(), rig.screen().lines());
            let (width, height) = match rng.below(4) {
                0 => (0, 0),
                1 => (rng.below(30), rng.below(30)),
                // Wide enough that the measure is not taken down, so the
                // text wraps as it did.
                _ => (960 + rng.below(1800), rng.below(1300)),
            };
            rig.size(width, height);
            rig.draw();
            rig.size(1920, 1080);
            let now = (Snap::of(&rig), rig.app.view().top(), rig.screen().lines());
            assert_eq!(now.0, before.0, "the text or the cursor changed going through {width} by {height}");
            // The paper may only have moved to keep the cursor on a smaller
            // sheet; from a size as tall as this one it has not moved.
            if height >= 1080 || width < 12 || height < 22 {
                assert_eq!(now.1, before.1, "the paper moved going through {width} by {height}");
                assert_eq!(now.2, before.2, "the screen changed going through {width} by {height}");
            }
        }
    }
}

/// The caret the shell is told about, for the emoji panel and input
/// methods, is the cell the cursor is drawn in, and it is inside the
/// window whenever the window is at least the smallest useful size.
#[test]
fn the_caret_handed_to_the_shell_is_where_the_cursor_is_drawn() {
    let mut rng = Rng::new(31);
    let mut run = Run::start(&Setup::new(TEXT), known());
    for round in 0..200 {
        let (cols, rows) = (60 + rng.below(110), 12 + rng.below(40));
        let step = 1 + rng.below(3) as u32;
        run.rig.size(cols * 12 * step as usize + rng.below(12), rows * 22 * step as usize + rng.below(22));
        run.rig.app.set_scale(step);
        let mut events = Vec::new();
        random_events(&mut rng, &mut events, &[(1920, 1080)]);
        for ev in events.iter().filter(|ev| !matches!(ev, Ev::Size(..) | Ev::Wheel(_) | Ev::Focus(_) | Ev::Click { .. } | Ev::Drag { .. })) {
            run.send(ev);
        }
        run.rig.draw();
        let grid = run.rig.app.grid().clone();
        let wants = run.rig.wants().clone();
        // Ctrl = and Ctrl - among the keys may have changed the scale.
        let step = run.rig.app.prefs().scale;
        assert_eq!(wants.cell, (12 * step, 22 * step), "round {round}");
        if grid.cols < 60 || grid.rows < 12 {
            continue;
        }
        let drawn = run.rig.app.screen().cursor;
        if run.rig.app.page().cursor.has_selection() {
            continue;
        }
        let Some(drawn) = drawn else {
            panic!("round {round}: no cursor on a window of {cols} by {rows} cells at scale {step}");
        };
        let caret = wants.caret.unwrap_or_else(|| panic!("round {round}: the cursor is drawn at {drawn:?} and the shell is told of no caret"));
        let cell = grid.cells(drawn.col, drawn.row, 1, 1);
        assert_eq!((caret.x, caret.y), (cell.x, cell.y), "round {round}: the caret is not where the cursor is drawn");
        assert!(caret.x >= 0 && caret.y >= 0 && caret.x + caret.w <= grid.width && caret.y + caret.h <= grid.height, "round {round}: the caret {caret:?} is outside a window of {} by {}", grid.width, grid.height);
    }
}
