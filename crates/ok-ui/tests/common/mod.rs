//! What the test files share: a full-screen rig holding a text, and short
//! names for where the cursor is.
#![allow(dead_code)]

use ok_text::Pos;
use ok_ui::harness::Rig;

/// A 1920 by 1080 window (160 by 49 cells) holding `text`, the cursor at
/// its end, and the paper rolled up as far as the wheel will take it: a
/// text shorter than the sheet has its first row on row 1, under its head
/// margin, where its rows are easiest to name; a longer one has its last
/// row at the platen, as it was opened. (As opened, a short text has its
/// last row at the platen and bare paper above; `tests/platen.rs` is where
/// that is looked at.)
pub fn rig(text: &str) -> Rig {
    let mut rig = Rig::with_text(text, 1920, 1080);
    rig.wheel(-1000);
    rig
}

/// Puts the cursor at a line and byte, with nothing selected.
pub fn put(rig: &mut Rig, line: usize, col: usize) {
    rig.app.page_mut().place(Pos::new(line, col), false);
}

/// Where the cursor is, as (line, byte).
pub fn head(rig: &Rig) -> (usize, usize) {
    let head = rig.app.page().cursor.head;
    (head.line, head.col)
}

/// What is selected, or an empty string.
pub fn selected(rig: &Rig) -> String {
    rig.app.page().selection().unwrap_or_default()
}

/// Presses several chords in turn.
pub fn presses(rig: &mut Rig, chords: &[&str]) {
    for chord in chords {
        rig.press(chord);
    }
}

/// What the say row reads.
pub fn said(rig: &mut Rig) -> String {
    let (text, say) = (rig.app.grid().text, rig.app.grid().say.unwrap_or(-1));
    rig.screen().span(text, say)
}

/// What a stretch of one row of the screen reads.
pub fn read(rig: &mut Rig, first: i32, last: i32, row: i32) -> String {
    rig.screen().span(ok_ui::grid::Cols { first, last }, row)
}
