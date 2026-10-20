//! The platen: which rows of the paper are on the sheet, and how the paper
//! gets from one place to another.
//!
//! The paper moves and the typing point does not. While he types at the
//! platen row the row stays where it is and the text climbs past it; the
//! cursor is free to go anywhere on the sheet with the arrow keys, and the
//! paper only moves for those when the cursor would leave it.
//!
//! Rows here are rows of the paper, which are the rows of the text and,
//! when the text runs past its word limit, one more for the rule (see
//! [`Page::paper_row`]).

use crate::grid::Grid;
use crate::page::Page;

/// How many frames the paper takes to arrive when it rolls.
const ROLL_FRAMES: i32 = 4;

/// Rows of bare paper above the first row of a text that has been rolled
/// to the top of the sheet: its head margin, which puts the first row of
/// the text level with the first row of the contents and of the labels.
const HEAD: i64 = 1;

/// The paper part of the way to where it is going.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Roll {
    /// Pixels the paper still had to travel when it set off. Positive when
    /// it is climbing.
    pixels: i32,
    /// Frames still to come, the last of which shows the paper at rest.
    frames: i32,
}

/// Where the paper is.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct View {
    /// The row of the paper that is on row 0 of the sheet. Below 0 when
    /// the top of the text hangs lower on the sheet, as it does whenever
    /// one of its first rows is at the platen.
    top: i64,
    roll: Option<Roll>,
}

/// The row of the paper the cursor is on and how many rows it takes, for
/// keeping it in sight. On the mark of a double-size heading the cursor is
/// one cell on the heading's lower row, but it is the whole heading that
/// has to be on the sheet: half a heading is not drawn.
fn sight(page: &Page) -> (i64, i64) {
    let caret = page.caret();
    let scale = page.layout().shape(caret.line).scale.max(1);
    if caret.size < scale {
        return (page.paper_row(page.layout().first_row(caret.line)), scale as i64);
    }
    (page.paper_row(caret.row), caret.size as i64)
}

impl View {
    /// The row of the paper on row 0 of the sheet.
    pub fn top(&self) -> i64 {
        self.top
    }

    /// The least and the greatest `top` may be. The least has the first
    /// row of the text at the platen, with bare paper above it: that is
    /// where a new text starts, and the typing row is held there while the
    /// paper climbs. The greatest has the last row at the platen, or, for
    /// a text too short to reach it, the first row at the top of the sheet
    /// under its head margin, which is as far as the wheel will roll it. A
    /// sheet of a row or two has no room for a margin.
    fn bounds(page: &Page, grid: &Grid) -> (i64, i64) {
        let last = page.paper_rows() as i64 - 1;
        let head = if grid.text_rows > 2 { HEAD } else { 0 };
        let lowest = -(grid.platen as i64);
        (lowest, (last - grid.platen as i64).max(-head).max(lowest))
    }

    /// Puts row `top` of the paper on row 0 of the sheet, as near as the
    /// text allows, without rolling. While the window has no room for a
    /// grid the paper stays where it is, to be found there when it has.
    pub fn set_top(&mut self, top: i64, page: &Page, grid: &Grid) {
        self.roll = None;
        if grid.is_blank() {
            return;
        }
        let (lowest, highest) = View::bounds(page, grid);
        self.top = top.clamp(lowest, highest);
    }

    /// Puts the paper back where it is known to have been: the row a
    /// session remembered, the place the line set out from. Unlike
    /// [`View::set_top`] this is kept even while the window has no grid, to
    /// be brought within the text when it has one.
    pub fn put_back(&mut self, top: i64, page: &Page, grid: &Grid) {
        self.top = top;
        self.set_top(top, page, grid);
    }

    /// The row of the sheet the cursor is on, which may be off it.
    pub fn cursor_row(&self, page: &Page) -> i64 {
        page.paper_row(page.caret().row) - self.top
    }

    /// Brings the cursor's row to the platen at once.
    pub fn seat(&mut self, page: &Page, grid: &Grid) {
        self.set_top(page.paper_row(page.caret().row) - grid.platen as i64, page, grid);
    }

    /// Scrolls no further than it takes to have the cursor on the sheet.
    /// A cursor that has gone more than a couple of rows off it has jumped
    /// (to the end of the text, to a heading), and is brought to the platen
    /// instead of being left on the last row. When it is the first row of
    /// the text that comes into sight, its head margin comes with it.
    pub fn reveal(&mut self, page: &Page, grid: &Grid) {
        let (row, tall) = sight(page);
        let rows = grid.text_rows.max(1) as i64;
        let above = self.top - row;
        let below = row + tall - (self.top + rows);
        if above > 2 || below > 2 {
            self.seat(page, grid);
        } else if above > 0 {
            let head = if grid.text_rows > 2 { HEAD } else { 0 };
            self.set_top(if row == 0 { -head } else { row }, page, grid);
        } else if below > 0 {
            self.set_top(row + tall - rows, page, grid);
        }
    }

    /// Follows an edit. `was` is the row of the sheet the cursor was on
    /// before it. The typing row is held at the platen: if it was there it
    /// stays there, the paper moving under it either way, and it is never
    /// left below the platen. Above the platen the cursor is left to come
    /// down to it. With `platen` off the paper only keeps the cursor in
    /// sight. With `smooth` the paper takes four frames to arrive.
    pub fn follow_edit(&mut self, page: &Page, grid: &Grid, was: i64, platen: bool, smooth: bool) {
        let before = self.top;
        let now = self.cursor_row(page);
        let rows = grid.text_rows.max(1) as i64;
        let held = platen && (was == grid.platen as i64 || now > grid.platen as i64 || now < 0 || now >= rows);
        if held {
            self.seat(page, grid);
        } else {
            self.reveal(page, grid);
        }
        let moved = self.top - before;
        // The first of the four frames is the one this edit is drawn in.
        let rolls = smooth && moved != 0 && moved.abs() <= rows;
        self.roll = rolls.then(|| Roll { pixels: (moved * grid.cell_h as i64) as i32, frames: ROLL_FRAMES - 1 });
    }

    /// True when the row at the top of the sheet is the lower half of a
    /// double-size row, whose upper half is above the sheet.
    fn splits(&self, page: &Page) -> bool {
        let Some(row) = page.text_row(self.top) else { return false };
        let (line, _) = page.layout().line_of_row(row);
        let scale = page.layout().shape(line).scale;
        scale == 2 && (row - page.layout().first_row(line)) % 2 == 1
    }

    /// The wheel: positive rows bring earlier text into view. The cursor
    /// stays where it is in the text, and may leave the sheet. A
    /// double-size row is never left cut in two at the top of the sheet,
    /// where it would not be drawn: the paper goes one row further the way
    /// it was going.
    pub fn wheel(&mut self, rows: i32, page: &Page, grid: &Grid) {
        let before = self.top;
        self.set_top(self.top - rows as i64, page, grid);
        if self.top != before && self.splits(page) {
            let on = if self.top > before { 1 } else { -1 };
            self.set_top(self.top + on, page, grid);
            if self.splits(page) {
                // The end of the text was in the way: back the other way.
                self.set_top(self.top - 2 * on, page, grid);
            }
        }
    }

    /// Moves the paper by whole rows, as Page Up and Page Down do.
    pub fn scroll(&mut self, rows: i64, page: &Page, grid: &Grid) {
        self.set_top(self.top + rows, page, grid);
    }

    /// Keeps the paper within the text after the text or the grid changed
    /// under it.
    pub fn settle(&mut self, page: &Page, grid: &Grid) {
        self.set_top(self.top, page, grid);
    }

    /// Pixels the paper is drawn below where it will rest (above it, when
    /// negative) on this frame of a roll. 0 at rest.
    pub fn shift(&self) -> i32 {
        self.roll.map_or(0, |roll| (roll.pixels as i64 * roll.frames as i64 / ROLL_FRAMES as i64) as i32)
    }

    pub fn rolling(&self) -> bool {
        self.roll.is_some()
    }

    /// One frame on. True while there are more to come.
    pub fn step(&mut self) -> bool {
        self.roll = match self.roll {
            Some(roll) if roll.frames > 1 => Some(Roll { frames: roll.frames - 1, ..roll }),
            _ => None,
        };
        self.roll.is_some()
    }

    /// Ends a roll at once, the paper where it was going.
    pub fn finish(&mut self) {
        self.roll = None;
    }
}
