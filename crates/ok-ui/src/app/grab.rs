//! Taking hold of a frame with the mouse: its left edge, the corners and
//! the rule of its top and bottom move it, its right edge makes it wider or
//! narrower. While the button is down an outline shows where it will go;
//! letting go writes the new place or width into its fence, one edit that
//! Ctrl Z takes back. The fence's words and everything inside the frame
//! are text, and a click there is a click in text.
//!
//! On a board a block goes wherever it is put. In the flow of the text it
//! moves up or down past the parts it is dropped beyond.

use ok_gfx::{Rect, Rgb};
use ok_text::arrange::{self, Order, Way};
use ok_text::blocks::{self, Fence};
use ok_text::history::{Cursor, EditKind};
use ok_text::wrap::{Frame, FRAME_PAD};
use ok_text::{Pos, Span};

use crate::look::Surface;

use super::App;

/// The narrowest a frame can be pulled to.
const NARROWEST: usize = 8;

/// A frame held by the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Grab {
    frame: Frame,
    /// The right edge was taken: the frame is being made wider or narrower.
    resize: bool,
    /// The block sits on a board, where it can go anywhere.
    board: bool,
    /// The cell the button went down on and the one it is over now, as
    /// columns and rows of the text.
    from: (i64, i64),
    now: (i64, i64),
}

impl App {
    /// The column and row of the text under a pixel: the column counted
    /// from the first of the text, the row a row of the text.
    fn text_cell(&self, x: i32, y: i32) -> Option<(i64, i64)> {
        let (col, row) = self.grid.cell_at(x, y);
        if !self.grid.sheet.contains(col) || row < 0 || row >= self.grid.text_rows {
            return None;
        }
        let paper = self.view.top() + row as i64;
        let row = match self.page.text_row(paper) {
            Some(row) => row as i64,
            None => paper,
        };
        Some(((col - self.grid.text.first) as i64, row))
    }

    /// The frame edge under a pixel, if a frame's edge is there: the frame
    /// on top where frames lie over one another.
    pub(crate) fn grab_at(&self, x: i32, y: i32) -> Option<Grab> {
        if self.showing() || self.page.read_only {
            return None;
        }
        let (cell, row) = self.text_cell(x, y)?;
        let layout = self.page.layout();
        for frame in layout.frames().iter().rev() {
            let (top, bottom) = (frame.top as i64, frame.bottom as i64);
            let (left, right) = (frame.left as i64, (frame.left + frame.width) as i64);
            if row < top || row >= bottom || cell < left || cell >= right {
                continue;
            }
            let pad = FRAME_PAD as i64;
            let resize = cell >= right - pad;
            let words = |line: usize| layout.rows(line).first().map_or(0, |found| (found.indent + found.cells) as i64);
            let edge = if resize || cell < left + pad {
                true
            } else if row == top {
                cell > left + pad + words(frame.open)
            } else if row == bottom - 1 {
                cell > left + pad + words(frame.close)
            } else {
                false
            };
            if !edge {
                // Inside the frame on top: text, whatever is under it.
                return None;
            }
            let board = matches!(blocks::fence(self.page.doc().line(frame.open)), Some(Fence::Open(spec)) if spec.at.is_some());
            return Some(Grab { frame: *frame, resize, board, from: (cell, row), now: (cell, row) });
        }
        None
    }

    /// The mouse moved while a frame is held.
    pub(crate) fn grab_move(&mut self, x: i32, y: i32) {
        let Some(grab) = &mut self.grab else { return };
        let (col, row) = self.grid.cell_at(x, y);
        let paper = self.view.top() + row as i64;
        let row = self.page.text_row(paper).map_or(paper, |row| row as i64);
        let now = ((col - self.grid.text.first) as i64, row);
        if now != grab.now {
            grab.now = now;
            self.redraw = true;
        }
    }

    /// Where the held frame would go now: its corner and its width, in
    /// cells and rows of the text.
    fn grab_target(&self, grab: &Grab) -> (i64, i64, i64) {
        let (dx, dy) = (grab.now.0 - grab.from.0, grab.now.1 - grab.from.1);
        let frame = grab.frame;
        let measure = self.page.measure() as i64;
        if grab.resize {
            let width = (frame.width as i64 + dx).clamp(NARROWEST as i64, measure - frame.left as i64);
            return (frame.left as i64, frame.top as i64, width);
        }
        let left = if grab.board { (frame.left as i64 + dx).clamp(0, (measure - frame.width as i64).max(0)) } else { frame.left as i64 };
        (left, (frame.top as i64 + dy).max(0), frame.width as i64)
    }

    /// The button came up: the frame goes where it was put.
    pub(crate) fn grab_drop(&mut self, host: &mut dyn ok_host::Host) {
        let Some(grab) = self.grab.take() else { return };
        self.redraw = true;
        if grab.now == grab.from {
            // A click on an edge without a drag: the cursor goes into the
            // frame's first line.
            let pos = Pos::new((grab.frame.open + 1).min(grab.frame.close), 0);
            self.page.place(pos, false);
            return;
        }
        let (left, top, width) = self.grab_target(&grab);
        let frame = grab.frame;
        let doc = self.page.doc();
        let open = frame.open;
        let order = if grab.resize {
            Order::Width(Some(width as usize))
        } else if grab.board {
            // The block's place is counted from the board's corner.
            let Some(Fence::Open(spec)) = blocks::fence(doc.line(open)) else { return };
            let (x, y) = spec.at.unwrap_or_default();
            let x = (x as i64 + left - frame.left as i64).max(0) as usize;
            let y = (y as i64 + top - frame.top as i64).max(0) as usize;
            Order::MoveTo(x, y)
        } else {
            let (parts, at) = arrange::parts_beside(doc, open);
            let Some(at) = at else { return };
            let layout = self.page.layout();
            let first = |k: usize| layout.first_row(parts[k].0) as i64;
            if top > frame.top as i64 {
                let past = (at + 1..parts.len()).filter(|&k| first(k) <= top).count();
                if past == 0 {
                    return;
                }
                Order::Move(Way::Down, Some(past))
            } else {
                let past = (0..at).filter(|&k| first(k) >= top).count();
                if past == 0 {
                    return;
                }
                Order::Move(Way::Up, Some(past))
            }
        };
        let head = Pos::new(open, 0);
        let plan = match arrange::plan(doc, Span { start: head, end: head }, head, self.page.measure(), &order) {
            Ok(plan) => plan,
            Err(why) => return self.say(why),
        };
        // The cursor stays on its words, unless the block moved in the
        // flow, when it goes with the block.
        let keep = matches!(order, Order::Move(..)).then(|| Cursor::at(plan.cursor)).unwrap_or(self.page.cursor);
        if self.edit(host, |page, clock| page.replace(plan.span, &plan.text, EditKind::Other, Some(keep), clock)) {
            self.say(plan.done);
        }
    }

    /// The outline of where the held frame would go, as rectangles to fill
    /// in the mark colour.
    pub(crate) fn grab_draw(&self) -> Vec<(Rect, Rgb)> {
        let Some(grab) = &self.grab else { return Vec::new() };
        if grab.now == grab.from {
            return Vec::new();
        }
        let (left, top, width) = self.grab_target(grab);
        let tall = (grab.frame.bottom - grab.frame.top) as i32;
        let row = (self.page.paper_row(top.max(0) as usize) - self.view.top()) as i32;
        let place = self.grid.cells(self.grid.text.first + left as i32, row, width as i32, tall);
        let line = self.grid.scale.max(1) * 2;
        let colour = self.look.mark_on(Surface::Sheet);
        vec![
            (Rect::new(place.x, place.y, place.w, line), colour),
            (Rect::new(place.x, place.bottom() - line, place.w, line), colour),
            (Rect::new(place.x, place.y, line, place.h), colour),
            (Rect::new(place.right() - line, place.y, line, place.h), colour),
        ]
    }
}
