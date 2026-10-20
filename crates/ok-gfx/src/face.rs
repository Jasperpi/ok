//! Faces as the interface uses them: one face the writer chose, Gallant
//! behind it for what that face lacks, and one call that draws a cell.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::boxes::{self, Lines};
use crate::builtin::gallant;
use crate::cluster;
use crate::compose::compose;
use crate::font::{clipped, draw_coverage, draw_glyph, AlphaGlyph, BitmapFont, Glyph, Pen};
use crate::frame::{Frame, Rect};
use crate::hexbox;
use crate::outline::OutlineFace;

/// How many marks are drawn on one letter. A run of fifty pasted from a web
/// page stays in the file and shows its first four.
pub const MAX_MARKS: usize = 4;

/// A bitmap face or an outline face.
#[derive(Clone, Debug)]
pub enum Face {
    /// Built-in faces are borrowed for the life of the program; a face read
    /// from the `faces` folder is owned.
    Bitmap(Cow<'static, BitmapFont>),
    Outline(OutlineFace),
}

impl From<&'static BitmapFont> for Face {
    fn from(font: &'static BitmapFont) -> Face {
        Face::Bitmap(Cow::Borrowed(font))
    }
}

impl From<BitmapFont> for Face {
    fn from(font: BitmapFont) -> Face {
        Face::Bitmap(Cow::Owned(font))
    }
}

impl From<OutlineFace> for Face {
    fn from(face: OutlineFace) -> Face {
        Face::Outline(face)
    }
}

/// Coverage at or above this counts as ink when a picture is measured. The
/// faint fringe an outline rasterizer leaves round a stroke does not.
const INK: u8 = 64;

/// The coverage that counts as ink in one picture: [`INK`], or half the
/// picture's strongest pixel where that is less. The threshold keeps fringe
/// out of the measuring; it does not decide whether a picture exists. A
/// thin accent in a light face at a small size may be fringe all through,
/// and it is still a mark to be placed.
fn ink_level(coverage: &[u8]) -> u8 {
    let strongest = coverage.iter().copied().max().unwrap_or(0);
    INK.min(strongest / 2).max(1)
}

/// The first and last rows of a coverage glyph that hold ink, in cell rows.
fn alpha_ink_rows(g: &AlphaGlyph) -> Option<(i64, i64)> {
    if !g.is_sound() || g.width == 0 {
        return None;
    }
    let w = g.width as usize;
    let coverage = &g.coverage[..w * g.height as usize];
    let ink = ink_level(coverage);
    let inked = |line: &[u8]| line.iter().any(|&v| v >= ink);
    let first = coverage.chunks_exact(w).position(inked)? as i64;
    let last = coverage.chunks_exact(w).rposition(inked)? as i64;
    Some((g.top as i64 + first, g.top as i64 + last))
}

/// The first and last columns of a coverage glyph that hold ink, in cell
/// columns.
fn alpha_ink_cols(g: &AlphaGlyph) -> Option<(i64, i64)> {
    if !g.is_sound() || g.width == 0 {
        return None;
    }
    let w = g.width as usize;
    let coverage = &g.coverage[..w * g.height as usize];
    let ink = ink_level(coverage);
    let inked = |c: usize| coverage.chunks_exact(w).any(|line| line[c] >= ink);
    let first = (0..w).find(|&c| inked(c))? as i64;
    let last = (0..w).rev().find(|&c| inked(c))? as i64;
    Some((g.left as i64 + first, g.left as i64 + last))
}

impl Face {
    pub fn name(&self) -> &str {
        match self {
            Face::Bitmap(f) => f.name(),
            Face::Outline(f) => f.name(),
        }
    }

    /// Cell width and height in face pixels. For a bitmap face those are
    /// font pixels; for an outline face, pixels at the height it was
    /// rasterized for.
    pub fn cell(&self) -> (u32, u32) {
        match self {
            Face::Bitmap(f) => f.cell(),
            Face::Outline(f) => f.cell(),
        }
    }

    /// Rows above the baseline, counting the row the letters stand on.
    pub fn ascent(&self) -> u32 {
        match self {
            Face::Bitmap(f) => f.ascent(),
            Face::Outline(f) => f.ascent(),
        }
    }

    pub fn has(&self, ch: char) -> bool {
        match self {
            Face::Bitmap(f) => f.has(ch),
            Face::Outline(f) => f.has(ch),
        }
    }

    /// The first row a lower-case x reaches. An outline face measures its x
    /// once that glyph has been put in, and estimates until then.
    pub fn x_top(&self) -> u32 {
        match self {
            Face::Bitmap(f) => f.x_top(),
            Face::Outline(f) => {
                let ascent = f.ascent() as i64;
                let measured = f.glyph('x').and_then(alpha_ink_rows).map(|(first, _)| first);
                measured.unwrap_or(ascent * 9 / 20).clamp(0, ascent - 1) as u32
            }
        }
    }

    /// The first row and the thickness of an underline: the face's own
    /// underscore where it has one.
    pub fn underline_rows(&self) -> (u32, u32) {
        match self {
            Face::Bitmap(f) => f.underline_rows(),
            Face::Outline(f) => {
                let h = f.cell().1 as i64;
                match f.glyph('_').and_then(alpha_ink_rows) {
                    Some((first, last)) if first >= 0 && last < h => (first as u32, (last - first + 1) as u32),
                    _ => {
                        let thick = (h / 16).max(1);
                        ((f.ascent() as i64 + thick).min(h - thick) as u32, thick as u32)
                    }
                }
            }
        }
    }

    /// The first row and the thickness of a strike through the middle of
    /// the lower-case letters.
    pub fn strike_rows(&self) -> (u32, u32) {
        match self {
            Face::Bitmap(f) => f.strike_rows(),
            Face::Outline(f) => {
                let h = f.cell().1;
                let thick = self.underline_rows().1.min(h);
                (((self.x_top() + f.ascent() - 1) / 2).min(h - thick), thick)
            }
        }
    }

    /// For an outline face, the characters `text` needs that the host has
    /// not been asked for yet. A bitmap face never needs anything.
    pub fn missing(&self, text: &str) -> Vec<char> {
        match self {
            Face::Bitmap(_) => Vec::new(),
            Face::Outline(f) => f.missing(text),
        }
    }

    pub fn as_bitmap(&self) -> Option<&BitmapFont> {
        match self {
            Face::Bitmap(f) => Some(f),
            Face::Outline(_) => None,
        }
    }

    pub fn as_outline(&self) -> Option<&OutlineFace> {
        match self {
            Face::Bitmap(_) => None,
            Face::Outline(f) => Some(f),
        }
    }

    /// The outline cache, for putting in what the host rasterized.
    pub fn as_outline_mut(&mut self) -> Option<&mut OutlineFace> {
        match self {
            Face::Bitmap(_) => None,
            Face::Outline(f) => Some(f),
        }
    }
}

/// One cluster's picture while it is being put together: 8-bit coverage,
/// one byte per face pixel, as wide as the cluster's cells. Every read and
/// write takes signed coordinates and is checked, so a mark shifted by any
/// amount in any direction cannot reach outside the cell.
#[derive(Clone)]
struct Canvas {
    w: i32,
    h: i32,
    cov: Vec<u8>,
    /// The coverage that counts as ink when this picture is measured. See
    /// [`ink_level`].
    ink: u8,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        let (w, h) = (w.clamp(1, 1024) as i32, h.clamp(1, 512) as i32);
        Canvas { w, h, cov: vec![0; (w * h) as usize], ink: INK }
    }

    /// Sets what counts as ink from what has been drawn so far. Called once
    /// a glyph is on the canvas and before anything is measured.
    fn settle(&mut self) {
        self.ink = ink_level(&self.cov);
    }

    fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            0
        } else {
            self.cov[(y * self.w + x) as usize]
        }
    }

    fn put(&mut self, x: i64, y: i64, v: u8) {
        if x >= 0 && y >= 0 && x < self.w as i64 && y < self.h as i64 {
            let p = &mut self.cov[(y * self.w as i64 + x) as usize];
            *p = (*p).max(v);
        }
    }

    fn row(&self, y: i32) -> &[u8] {
        if y < 0 || y >= self.h {
            return &[];
        }
        let start = (y * self.w) as usize;
        &self.cov[start..start + self.w as usize]
    }

    fn row_inked(&self, y: i32) -> bool {
        self.row(y).iter().any(|&v| v >= self.ink)
    }

    /// In how many columns rows `a` and `b` differ, one having ink where
    /// the other has none. The faint fringe of an outline glyph is not
    /// compared: two rows of one stem differ in it wherever something
    /// passes close by.
    fn rows_apart(&self, a: i32, b: i32) -> usize {
        let (a, b) = (self.row(a), self.row(b));
        if a.len() != b.len() {
            return self.w as usize;
        }
        a.iter().zip(b).filter(|&(&p, &q)| (p >= self.ink) != (q >= self.ink)).count()
    }

    /// True when rows `a` and `b` have their ink in the same columns.
    fn rows_alike(&self, a: i32, b: i32) -> bool {
        self.rows_apart(a, b) == 0
    }

    fn ink_rows(&self) -> Option<(i32, i32)> {
        let first = (0..self.h).find(|&y| self.row_inked(y))?;
        let last = (0..self.h).rev().find(|&y| self.row_inked(y))?;
        Some((first, last))
    }

    fn ink_cols(&self) -> Option<(i32, i32)> {
        let inked = |x: i32| (0..self.h).any(|y| self.at(x, y) >= self.ink);
        let first = (0..self.w).find(|&x| inked(x))?;
        let last = (0..self.w).rev().find(|&x| inked(x))?;
        Some((first, last))
    }

    /// Lays a bitmap glyph in with its top-left corner at (`ox`, `oy`),
    /// each font pixel a `k` by `k` block.
    fn stamp_bits(&mut self, g: Glyph<'_>, ox: i64, oy: i64, k: i64) {
        for (r, &row) in g.rows.iter().enumerate() {
            let mut rest = row;
            while rest != 0 {
                let c = rest.leading_zeros();
                rest &= !(0x8000_0000 >> c);
                if c >= g.width {
                    break;
                }
                for dy in 0..k {
                    for dx in 0..k {
                        self.put(ox + c as i64 * k + dx, oy + r as i64 * k + dy, 255);
                    }
                }
            }
        }
    }

    /// Lays a coverage glyph in for a cell whose top-left corner is at
    /// (`ox`, `oy`). Only the part that lands on the canvas is visited.
    fn stamp_alpha(&mut self, g: &AlphaGlyph, ox: i64, oy: i64) {
        if !g.is_sound() {
            return;
        }
        let (left, top) = (ox + g.left as i64, oy + g.top as i64);
        let rows = (-top).max(0)..(self.h as i64 - top).min(g.height as i64);
        for r in rows {
            let cols = (-left).max(0)..(self.w as i64 - left).min(g.width as i64);
            for c in cols {
                let v = g.coverage[(r * g.width as i64 + c) as usize];
                self.put(left + c, top + r, v);
            }
        }
    }

    /// The same picture moved by (`dx`, `dy`); what leaves the canvas is lost.
    fn shifted(&self, dx: i32, dy: i32) -> Canvas {
        let mut out = Canvas { w: self.w, h: self.h, cov: vec![0; self.cov.len()], ink: self.ink };
        for y in 0..self.h {
            for x in 0..self.w {
                let v = self.at(x, y);
                if v != 0 {
                    out.put(x as i64 + dx as i64, y as i64 + dy as i64, v);
                }
            }
        }
        out
    }

    /// Takes row `r` out and moves everything above it down one row.
    fn close_up_from_above(&mut self, r: i32) {
        if r < 0 || r >= self.h {
            return;
        }
        let w = self.w as usize;
        self.cov.copy_within(0..r as usize * w, w);
        self.cov[..w].fill(0);
    }

    /// Takes row `r` out and moves everything below it up one row.
    fn close_up_from_below(&mut self, r: i32) {
        if r < 0 || r >= self.h {
            return;
        }
        let w = self.w as usize;
        let len = self.cov.len();
        self.cov.copy_within((r as usize + 1) * w.., r as usize * w);
        self.cov[len - w..].fill(0);
    }

    /// Where the ink is once it has been grown by `rx` columns and `ry` rows
    /// in every direction: one flag per pixel. Two sweeps along the rows and
    /// two along the columns, so the cost does not grow with the distance.
    fn grown(&self, rx: i32, ry: i32) -> Vec<bool> {
        let (w, h) = (self.w as usize, self.h as usize);
        let (rx, ry) = (rx.max(0) as usize, ry.max(0) as usize);
        // `since` counts the pixels walked since the last ink; a flag is set
        // while that is within reach.
        let reach = |since: &mut Option<usize>, inked: bool, limit: usize| {
            *since = if inked { Some(0) } else { since.map(|n| n + 1) };
            since.is_some_and(|n| n <= limit)
        };
        let mut across = vec![false; w * h];
        for y in 0..h {
            let (mut forward, mut back) = (None, None);
            for x in 0..w {
                across[y * w + x] |= reach(&mut forward, self.cov[y * w + x] >= self.ink, rx);
                let x = w - 1 - x;
                across[y * w + x] |= reach(&mut back, self.cov[y * w + x] >= self.ink, rx);
            }
        }
        let mut out = vec![false; w * h];
        for x in 0..w {
            let (mut forward, mut back) = (None, None);
            for y in 0..h {
                out[y * w + x] |= reach(&mut forward, across[y * w + x], ry);
                let y = h - 1 - y;
                out[y * w + x] |= reach(&mut back, across[y * w + x], ry);
            }
        }
        out
    }

    /// True when any ink here lies within `rx` columns and `ry` rows of ink
    /// in `other`. With both zero that is plain overlap.
    fn near(&self, other: &Canvas, rx: i32, ry: i32) -> bool {
        if (self.w, self.h) != (other.w, other.h) {
            return false;
        }
        self.grown(rx, ry).iter().zip(&other.cov).any(|(&mine, &theirs)| mine && theirs >= other.ink)
    }

    /// How many pixels of ink this picture has, and how many of them fall
    /// on ink in `other`.
    fn hidden_by(&self, other: &Canvas) -> (usize, usize) {
        if (self.w, self.h) != (other.w, other.h) {
            return (0, 0);
        }
        let mine = || self.cov.iter().zip(&other.cov).filter(|&(&v, _)| v >= self.ink);
        (mine().count(), mine().filter(|&(_, &under)| under >= other.ink).count())
    }

    /// Erases whatever lies under `mark` grown by `r` pixels all round, so
    /// the mark keeps a clear edge where it has to sit on a stroke.
    fn clear_under(&mut self, mark: &Canvas, r: i32) {
        if (self.w, self.h) != (mark.w, mark.h) {
            return;
        }
        for (v, under) in self.cov.iter_mut().zip(mark.grown(r, r)) {
            if under {
                *v = 0;
            }
        }
    }

    /// One flag per pixel: ink that is joined, edge to edge or corner to
    /// corner, to ink in the rows `rows`.
    fn joined_to(&self, rows: std::ops::Range<i32>) -> Vec<bool> {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut joined = vec![false; w * h];
        let mut todo: Vec<usize> = Vec::new();
        for y in rows.start.max(0)..rows.end.min(self.h) {
            for x in 0..w {
                let at = y as usize * w + x;
                if self.cov[at] >= self.ink {
                    joined[at] = true;
                    todo.push(at);
                }
            }
        }
        while let Some(at) = todo.pop() {
            let (x, y) = (at % w, at / w);
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    let next = ny * w + nx;
                    if !joined[next] && self.cov[next] >= self.ink {
                        joined[next] = true;
                        todo.push(next);
                    }
                }
            }
        }
        joined
    }

    /// Erases the pieces a cut left hanging: ink that was joined to the
    /// letter's body in `whole`, the picture before the cut, and is joined
    /// to it no longer. `body` is the rows that are certainly letter. Ink
    /// that never was part of the letter (an accent, an earlier mark) stays.
    fn drop_cut_off(&mut self, whole: &Canvas, body: std::ops::Range<i32>) {
        if (self.w, self.h) != (whole.w, whole.h) {
            return;
        }
        let was = whole.joined_to(body.clone());
        let is = self.joined_to(body);
        for ((v, was), is) in self.cov.iter_mut().zip(was).zip(is) {
            if was && !is {
                *v = 0;
            }
        }
    }

    /// Lays `other` over this picture. If `other` is the fainter of the
    /// two, its level becomes the level of the whole, so that it is still
    /// ink to the next thing placed.
    fn merge(&mut self, other: &Canvas) {
        let mut inked = false;
        for (a, &b) in self.cov.iter_mut().zip(other.cov.iter()) {
            inked |= b >= other.ink;
            *a = (*a).max(b);
        }
        if inked {
            self.ink = self.ink.min(other.ink);
        }
    }

    /// Inks the part of a rectangle that lies on the canvas. The right
    /// column and the bottom row given are the first ones left alone.
    fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        for y in y0.max(0)..y1.min(self.h) {
            let row = (y * self.w) as usize;
            for x in x0.max(0)..x1.min(self.w) {
                self.cov[row + x as usize] = 255;
            }
        }
    }

    /// Every stroke `by` pixels wider to the right, without leaving the
    /// canvas: the picture struck a second time.
    fn thicken(&mut self, by: u32) {
        let w = self.w as usize;
        for _ in 0..by.min(w as u32) {
            for line in self.cov.chunks_exact_mut(w) {
                for x in (1..w).rev() {
                    line[x] = line[x].max(line[x - 1]);
                }
            }
        }
    }
}

/// What the placing of one mark needs to know about the cell it goes into.
#[derive(Clone, Copy)]
struct Placing {
    /// Rows left clear between a mark and the letter under it: one in a
    /// cell of Gallant's size, more in a tall outline cell.
    gap: i32,
    /// A lower-case letter keeps that clear row even if its ascender has to
    /// be shortened for it, as the face's own l-acute is. A capital lets the
    /// mark rest on it, as the face's own accented capitals do.
    lower: bool,
    /// The first row of the letter's x-height. Rows above it are where an
    /// ascender can be shortened.
    x_top: i32,
    /// The row the letter stands on. A capital can be made shorter anywhere
    /// above it.
    baseline: i32,
    /// The first row of the x-height in the face the mark came from: a
    /// mark that lies mostly above this row sits above the letter.
    above: i32,
    /// The baseline row in the face the mark came from: a mark that lies
    /// mostly below this row sits below the letter.
    below: i32,
}

/// What the marks of one cluster share as they go on in turn.
#[derive(Default)]
struct Room {
    /// How many more rows a capital may be shortened by. It starts at the
    /// height of a capital over the x-height, so that the head of a capital
    /// is never brought lower than a lower-case letter's.
    spare: i32,
    /// A mark has been placed above the letter already.
    stacked: bool,
}

/// Where a mark belongs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Above,
    Below,
    /// Struck through the letter where it is: a stroke, a slash.
    Through,
}

/// Which side of the letter a mark with ink on rows `mt` to `mb` belongs
/// on, told by where the middle of that ink lies. The whole of it need not
/// be clear of the letter: an outline face draws its comma above low
/// enough to dip into the x-height, and its line below may start on the
/// baseline row.
fn side(mt: i32, mb: i32, above: i32, below: i32) -> Side {
    let twice_middle = mt + mb;
    if twice_middle > 2 * below {
        Side::Below
    } else if twice_middle < 2 * above {
        Side::Above
    } else {
        Side::Through
    }
}

/// Puts one mark onto the picture so far.
///
/// A mark above is moved, by a signed amount, to leave `gap` clear rows over
/// whatever is already there, and no higher than the top of the cell. If it
/// still runs into the letter, room is made in the order the face's own
/// accented letters suggest: slide the mark a pixel or two sideways (as in
/// Gallant's k-acute), shorten a plain stem (as in its l-acute), squeeze a
/// doubled row out of the mark, let the mark touch, close up a gap inside
/// the letter, and only then cut the letter away under the mark. A capital
/// that already carries an accent has no stem above its x-height to give,
/// so it is made a row or two shorter lower down and keeps its accent whole
/// under the new mark. A mark placed earlier is never cut for a later one:
/// if nothing else makes room, the two are struck one over the other.
///
/// A mark below never costs the letter anything: see [`settled_below`]. One
/// that grows out of the letter (a cedilla) is joined on where it is.
/// Anything else is struck straight through the letter.
fn add_mark(base: &mut Canvas, mut mark: Canvas, p: Placing, room: &mut Room) {
    let Some((mt, mb)) = mark.ink_rows() else { return };
    let Some((top, _)) = base.ink_rows() else {
        // On nothing, the mark goes where the face drew it, and from here
        // on there is nothing in the cell but marks.
        base.merge(&mark);
        room.stacked = true;
        return;
    };

    match side(mt, mb, p.above, p.below) {
        Side::Above => {}
        Side::Through => {
            base.merge(&mark);
            return;
        }
        Side::Below => {
            if mt > p.below + p.gap && mark.near(base, p.gap, p.gap) {
                mark = settled_below(base, mark, p);
            }
            base.merge(&mark);
            return;
        }
    }

    // A mark is never dropped lower than it would sit on an x.
    let top = top.min(p.x_top);
    let d = ((top - 1 - p.gap) - mb).max(-mt);
    if d != 0 {
        mark = mark.shifted(0, d);
    }

    let mut clear_rows = if p.lower { p.gap } else { 0 };
    // Sliding is tried once, before the letter is touched: it is for a mark
    // that fits beside a stem as the letter stands (the face's own k-acute),
    // not for one that only fits beside a letter already cut down.
    if mark.near(base, p.gap, clear_rows) {
        if let Some(moved) = nudged(base, &mark, p.gap, clear_rows) {
            mark = moved;
        }
    }
    for _ in 0..base.h * 2 + 8 {
        if !mark.near(base, p.gap, clear_rows) {
            break;
        }
        let helped = if p.lower {
            shorten_stem(base, p.x_top) || squeeze_mark(&mut mark) || {
                // Give up the clear row: from here on the mark may touch.
                let had = clear_rows > 0;
                clear_rows = 0;
                had
            } || close_gap(base, p.x_top)
        } else {
            squeeze_mark(&mut mark) || lower_head(base, p, &mut room.spare)
        };
        if !helped {
            break;
        }
    }
    // What is cut here is the letter. A mark that went on before this one
    // is a character in the file, and the scrap a cut would leave of it
    // would say nothing.
    if !room.stacked {
        cut_away_under(base, &mark, p.gap, p.x_top..base.h);
    }
    base.merge(&mark);
    room.stacked = true;
}

/// Makes room over a capital by lowering its head one row, and says
/// whether it could. First a plain piece of stem above the x-height goes,
/// then the gap under an accent the letter already carries, then a row
/// from anywhere above the baseline: one that is the same as the row under
/// it if there is one, otherwise the one that will be missed least.
///
/// Every row the letter itself loses comes out of `spare`, which runs out
/// when the head of a capital would be down at the x-height; after that
/// only plain stem above the x-height is given.
fn lower_head(base: &mut Canvas, p: Placing, spare: &mut i32) -> bool {
    if shorten_stem(base, p.x_top) {
        *spare = (*spare - 1).max(0);
        return true;
    }
    if close_gap(base, p.x_top) {
        return true;
    }
    if *spare > 0 && (shorten_stem(base, p.baseline) || squash(base, p.x_top - *spare, p.baseline)) {
        *spare -= 1;
        return true;
    }
    false
}

/// The knock-out: where the mark lands on the letter, the letter is erased
/// under the mark and for `gap` pixels round it, and any piece of the letter
/// that this cuts off from its body (the rows `body`) goes too, so no stray
/// pixel of a severed stroke is left beside the mark.
fn cut_away_under(base: &mut Canvas, mark: &Canvas, gap: i32, body: std::ops::Range<i32>) {
    if !mark.near(base, 0, 0) {
        return;
    }
    let whole = base.clone();
    base.clear_under(mark, gap);
    base.drop_cut_off(&whole, body);
}

/// The mark slid one or two steps left or right, if that alone clears the
/// letter and loses none of the mark off the edge of the cell.
fn nudged(base: &Canvas, mark: &Canvas, gap: i32, clear_rows: i32) -> Option<Canvas> {
    let (first, last) = mark.ink_cols()?;
    [gap, -gap, 2 * gap, -2 * gap].into_iter().find_map(|dx| {
        if first + dx < 0 || last + dx >= mark.w {
            return None;
        }
        let moved = mark.shifted(dx, 0);
        (!moved.near(base, gap, clear_rows)).then_some(moved)
    })
}

/// The moves tried for a mark below with a descender in its way, as
/// (columns, rows) in steps of the gap, nearest first: aside before down,
/// down before up. Aside and down is where the face's own y with a dot
/// below has its dot.
const BELOW_STEPS: [(i32, i32); 24] = [
    (1, 0),
    (-1, 0),
    (2, 0),
    (-2, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, 1),
    (1, -1),
    (-1, -1),
    (2, 1),
    (-2, 1),
    (2, -1),
    (-2, -1),
    (0, 2),
    (0, -2),
    (1, 2),
    (-1, 2),
    (1, -2),
    (-1, -2),
    (2, 2),
    (-2, 2),
    (2, -2),
    (-2, -2),
];

/// The mark where it is, or moved by the first of [`BELOW_STEPS`], so that
/// it leaves a clear column between it and the letter and `clear_rows`
/// clear rows, loses none of itself off the cell and stays under the
/// baseline. `None` when no such place is within reach.
fn placed(base: &Canvas, mark: &Canvas, p: Placing, clear_rows: i32) -> Option<Canvas> {
    let (first_col, last_col) = mark.ink_cols()?;
    let (first_row, last_row) = mark.ink_rows()?;
    [(0, 0)].iter().chain(&BELOW_STEPS).find_map(|&(dx, dy)| {
        let (dx, dy) = (dx * p.gap, dy * p.gap);
        let inside = first_col + dx >= 0 && last_col + dx < mark.w && last_row + dy < mark.h;
        if !inside || first_row + dy <= p.below {
            return None;
        }
        let moved = mark.shifted(dx, dy);
        (!moved.near(base, p.gap, clear_rows)).then_some(moved)
    })
}

/// Finds the place for a mark below that has a descender in its way, and
/// returns the mark there.
///
/// The letter is never cut for it. Cutting the tail from under a line
/// leaves a y that reads as a v, and the writer can no longer see which
/// letter is in the file. So the mark first steps a pixel or two aside, or
/// down, or up, to where it is clear of the letter, as the dot of Gallant's
/// own y with a dot below does; with a doubled row squeezed out of it, if
/// that is what lets it through. Failing that it is struck where the face
/// drew it, over the tail, which is what a typewriter does with an
/// underscore under a y and what this program's own underline does.
///
/// Struck over, a mark can be lost in the letter's own ink: a dot on the
/// bowl of a g, a line on a tail that is itself a line. A mark at least
/// half of which would be hidden is moved to where it rests on a stroke
/// without overlapping it, if there is such a place within reach. Only a
/// mark that would otherwise be hidden altogether (the letter is solid
/// there) has the letter opened under it, by its own outline and no more.
fn settled_below(base: &mut Canvas, mark: Canvas, p: Placing) -> Canvas {
    let mut thin = mark.clone();
    let thin = squeeze_mark(&mut thin).then_some(thin);
    let found = |clear_rows: i32| placed(base, &mark, p, clear_rows).or_else(|| placed(base, thin.as_ref()?, p, clear_rows));

    if let Some(clear) = found(p.gap) {
        return clear;
    }
    let (all, hidden) = mark.hidden_by(base);
    if 2 * hidden < all {
        return mark;
    }
    if let Some(resting) = found(0) {
        return resting;
    }
    if hidden < all {
        return mark;
    }
    base.clear_under(&mark, p.gap);
    mark
}

/// Shortens an ascender by one row: removes the lowest row above `limit`
/// (the x-height, or for a capital the baseline) that is the same as the
/// row under it, a plain piece of stem, and lowers everything above it.
/// False when there is no such row.
fn shorten_stem(base: &mut Canvas, limit: i32) -> bool {
    let Some((top, _)) = base.ink_rows() else { return false };
    let row = (top..limit.min(base.h - 1)).rev().find(|&r| base.row_inked(r) && base.rows_alike(r, r + 1));
    match row {
        Some(r) => {
            base.close_up_from_above(r);
            true
        }
        None => false,
    }
}

/// Makes a letter one row shorter where it shows least: takes out the row
/// from `from` down to above `to` that differs least from the row under
/// it, the lowest of them if several do, and lowers everything above. It is
/// for a capital that has no two rows the same left to give: its curve is
/// drawn a row flatter and the accent it carries keeps its shape. False
/// when there is no row to take.
fn squash(base: &mut Canvas, from: i32, to: i32) -> bool {
    let Some((top, _)) = base.ink_rows() else { return false };
    let row = (from.max(top)..to.min(base.h - 1)).filter(|&r| base.row_inked(r)).min_by_key(|&r| (base.rows_apart(r, r + 1), -r));
    match row {
        Some(r) => {
            base.close_up_from_above(r);
            true
        }
        None => false,
    }
}

/// Makes the mark one row shorter where it has two rows the same, keeping
/// its top where it is. False when no two neighbouring rows match.
fn squeeze_mark(mark: &mut Canvas) -> bool {
    let Some((mt, mb)) = mark.ink_rows() else { return false };
    match (mt..mb).find(|&r| mark.rows_alike(r, r + 1)) {
        Some(r) => {
            mark.close_up_from_below(r);
            true
        }
        None => false,
    }
}

/// Removes the lowest empty row inside the letter above its x-height (the
/// gap under an accent it already carries) and lowers what is above it.
fn close_gap(base: &mut Canvas, x_top: i32) -> bool {
    let Some((top, _)) = base.ink_rows() else { return false };
    match (top + 1..x_top.min(base.h)).rev().find(|&r| !base.row_inked(r)) {
        Some(r) => {
            base.close_up_from_above(r);
            true
        }
        None => false,
    }
}

/// The letters whose dot gives way to a mark above them (Unicode calls them
/// soft-dotted), each with the dotless letter to draw in its place where
/// one exists. Besides i and j these are the i with an ogonek, the barred j
/// and the barred i (a vowel in Chumash spelling), the j with a crossed
/// tail, the Cyrillic i and je, and the i with a tilde or a dot below.
fn soft_dotted(ch: char) -> Option<Option<char>> {
    match ch {
        'i' => Some(Some('\u{0131}')),
        'j' => Some(Some('\u{0237}')),
        '\u{012F}' | '\u{0249}' | '\u{0268}' | '\u{029D}' | '\u{0456}' | '\u{0458}' | '\u{1E2D}' | '\u{1ECB}' => Some(None),
        _ => None,
    }
}

/// Makes a dotted letter dotless when the face has no dotless glyph for
/// it: erases everything above the empty rows that part the dot from the
/// stem. A letter drawn with no such gap is left as it is.
fn remove_dot(base: &mut Canvas, x_top: i32) {
    let Some((top, _)) = base.ink_rows() else { return };
    if let Some(gap) = (top + 1..x_top.min(base.h)).rev().find(|&r| !base.row_inked(r)) {
        let w = base.w as usize;
        base.cov[..gap as usize * w].fill(0);
    }
}

/// The marks of a cluster: what follows its first character and is drawn
/// on it, [`MAX_MARKS`] at most. Slots past the count hold a space.
fn marks_of(piece: &str) -> ([char; MAX_MARKS], usize) {
    let mut marks = [' '; MAX_MARKS];
    let mut count = 0;
    for mark in piece.chars().skip(1).filter(|&m| cluster::draws_on_letter(m)).take(MAX_MARKS) {
        marks[count] = mark;
        count += 1;
    }
    (marks, count)
}

/// Which face a glyph comes from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Source {
    Active,
    Fallback,
}

/// What stands in the cell before any marks go on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Base {
    Blank,
    Glyph(Source, char),
    /// A box-drawing character or a block, drawn in code.
    Drawn(char),
    /// No face has it: the box with this code point's digits.
    Box(u32),
}

/// One cluster, decided but not yet drawn.
struct Plan {
    base: Base,
    marks: [char; MAX_MARKS],
    count: usize,
    cells: u32,
}

/// Where the fallback face sits inside one cell of the active face: each of
/// its pixels becomes a `k` by `k` block, moved over and down by these many
/// face pixels so it is centred and the two baselines agree.
#[derive(Clone, Copy)]
struct Fit {
    k: i64,
    ox: i64,
    oy: i64,
}

/// Where a mark's picture comes from.
#[derive(Clone, Copy)]
enum MarkFrom {
    Active,
    /// The fallback's glyph, each of its pixels a `k` by `k` block, with
    /// its top-left corner at (`x`, `y`) on the cluster's canvas.
    Fallback { k: i64, x: i64, y: i64 },
}

/// What a put-together cell is remembered by: what is drawn, not the string
/// it came from, so a letter under fifty marks and the same letter under
/// its first four are one entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CellKey {
    base: Base,
    marks: [char; MAX_MARKS],
    count: usize,
    cells: u32,
    bold: bool,
}

/// How many put-together cells are kept before they are all dropped and
/// built again as they come up. A page of Vietnamese has a few hundred
/// different ones.
const CELLS_KEPT: usize = 4096;

/// The same limit in bytes of coverage, for outline faces with big cells.
const BYTES_KEPT: usize = 8 << 20;

/// Cells that had to be put together, with marks placed by hand or lines
/// drawn in code, kept from one frame to the next so that drawing them
/// again is a plain copy.
#[derive(Default)]
struct Kept {
    cells: HashMap<CellKey, Canvas>,
    bytes: usize,
}

impl Kept {
    fn keep(&mut self, key: CellKey, canvas: Canvas) {
        if self.cells.len() >= CELLS_KEPT || self.bytes + canvas.cov.len() > BYTES_KEPT {
            self.clear();
        }
        self.bytes += canvas.cov.len();
        if let Some(old) = self.cells.insert(key, canvas) {
            self.bytes -= old.cov.len();
        }
    }

    fn clear(&mut self) {
        self.cells.clear();
        self.bytes = 0;
    }
}

/// The active face and Gallant behind it.
///
/// A character is drawn from the active face. One it lacks is drawn from
/// the fallback, baselines aligned, but only when the fallback's whole cell
/// fits inside the active cell, so nothing borrowed is ever cut off. One
/// neither can draw is a box of hex digits.
pub struct FaceSet {
    active: Face,
    fallback: Cow<'static, BitmapFont>,
    /// Behind a lock so that drawing needs only `&self` and a set can still
    /// be shared between threads.
    kept: Mutex<Kept>,
}

impl Clone for FaceSet {
    /// The copy has the same faces and starts with nothing kept.
    fn clone(&self) -> FaceSet {
        FaceSet { active: self.active.clone(), fallback: self.fallback.clone(), kept: Mutex::default() }
    }
}

impl fmt::Debug for FaceSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaceSet").field("active", &self.active).field("fallback", &self.fallback).finish()
    }
}

impl FaceSet {
    /// `active` with the built-in Gallant as its fallback.
    pub fn new(active: impl Into<Face>) -> FaceSet {
        FaceSet { active: active.into(), fallback: Cow::Borrowed(gallant()), kept: Mutex::default() }
    }

    /// `active` with a fallback of the caller's choosing, for tests and for
    /// a build that carries some other broad face.
    pub fn with_fallback(active: impl Into<Face>, fallback: BitmapFont) -> FaceSet {
        FaceSet { active: active.into(), fallback: Cow::Owned(fallback), kept: Mutex::default() }
    }

    pub fn active(&self) -> &Face {
        &self.active
    }

    /// The active face, for changing it in place. Whatever was kept of
    /// cells already put together is dropped, since the face may draw them
    /// differently afterwards; to add an outline face's glyphs, prefer
    /// [`FaceSet::fill`], which drops them only when something was added.
    pub fn active_mut(&mut self) -> &mut Face {
        self.forget();
        &mut self.active
    }

    pub fn set_active(&mut self, face: impl Into<Face>) {
        self.forget();
        self.active = face.into();
    }

    /// Has the host rasterize whatever `text` needs that an outline face
    /// has not been asked for yet: `rasterize` is called once for each such
    /// character and answers with its glyph, or `None` when the face has
    /// none. Returns how many were asked for. With a bitmap face active
    /// this does nothing. Call it for the text about to be drawn; once the
    /// face has seen a character it costs a lookup.
    pub fn fill(&mut self, text: &str, mut rasterize: impl FnMut(char) -> Option<AlphaGlyph>) -> usize {
        let Face::Outline(face) = &mut self.active else { return 0 };
        let wanted = face.missing(text);
        for &ch in &wanted {
            face.insert(ch, rasterize(ch));
        }
        if !wanted.is_empty() {
            self.forget();
        }
        wanted.len()
    }

    /// Drops every cell kept, for when a face changes under them.
    fn forget(&mut self) {
        self.kept.get_mut().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// The kept cells. A panic while they were held leaves them usable:
    /// an entry is only ever put in whole.
    fn kept(&self) -> MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn fallback(&self) -> &BitmapFont {
        &self.fallback
    }

    /// The active face's name.
    pub fn name(&self) -> &str {
        self.active.name()
    }

    /// The active face's cell in face pixels: multiply by the pen's scale
    /// for screen pixels.
    pub fn cell(&self) -> (u32, u32) {
        self.active.cell()
    }

    /// The active face's ascent in face pixels.
    pub fn ascent(&self) -> u32 {
        self.active.ascent()
    }

    /// True when the fallback's cell fits inside the active cell, so what
    /// the active face lacks can be borrowed instead of boxed.
    pub fn fallback_fits(&self) -> bool {
        self.fit().is_some()
    }

    /// See [`Face::missing`]: what to have the host rasterize before `text`
    /// is drawn.
    pub fn missing(&self, text: &str) -> Vec<char> {
        self.active.missing(text)
    }

    fn fit(&self) -> Option<Fit> {
        let (aw, ah) = self.active.cell();
        let (fw, fh) = self.fallback.cell();
        let k = (aw / fw).min(ah / fh) as i64;
        if k == 0 {
            return None;
        }
        let room = ah as i64 - fh as i64 * k;
        let oy = (self.active.ascent() as i64 - self.fallback.ascent() as i64 * k).clamp(0, room);
        Some(Fit { k, ox: (aw as i64 - fw as i64 * k) / 2, oy })
    }

    /// How many face pixels to the right a double strike lands: one, or
    /// what the host set for an outline face.
    fn strike_width(&self) -> u32 {
        match &self.active {
            Face::Bitmap(_) => 1,
            Face::Outline(f) => f.strike(),
        }
    }

    fn has(&self, source: Source, ch: char) -> bool {
        match source {
            Source::Active => self.active.has(ch),
            Source::Fallback => self.fallback.has(ch),
        }
    }

    /// The lines that box-drawing characters are drawn with in the active
    /// face's cell, and whether the face's own glyphs for them may be used.
    ///
    /// They may when its `─` runs from edge to edge of the cell and its
    /// `│` from top to bottom, so that boxes join. Thickness and place
    /// are measured from those two glyphs when the face has them at the
    /// width of one cell; without them a line is as thick as the face's
    /// underscore and runs through the middle.
    fn box_lines(&self) -> (Lines, bool) {
        let (aw, ah) = self.active.cell();
        // Each line as (first row or column, thickness, reaches both edges).
        let (across, down) = match &self.active {
            Face::Bitmap(f) => {
                let narrow = |ch: char| f.glyph(ch).filter(|g| g.width == aw);
                let across = narrow('\u{2500}').and_then(|g| {
                    let (first, last) = g.ink_rows()?;
                    let all = g.rows.iter().fold(0, |all, &row| all | row);
                    let reaches = all & 0x8000_0000 != 0 && all & (0x8000_0000 >> (aw - 1)) != 0;
                    Some((first, last - first + 1, reaches))
                });
                let down = narrow('\u{2502}').and_then(|g| {
                    let all = g.rows.iter().fold(0, |all, &row| all | row);
                    let first = (all != 0).then(|| all.leading_zeros())?;
                    let reaches = g.rows.first().is_some_and(|&r| r != 0) && g.rows.last().is_some_and(|&r| r != 0);
                    Some((first, 32 - all.trailing_zeros() - first, reaches))
                });
                (across, down)
            }
            Face::Outline(f) => {
                let narrow = |ch: char| f.glyph(ch).filter(|g| f.cells(g) == 1);
                // Ink bounds across the line's thickness and along its length.
                let measure = |thick: Option<(i64, i64)>, long: Option<(i64, i64)>, length: u32| {
                    let ((first, last), (from, to)) = (thick?, long?);
                    let first = first.max(0);
                    (last >= first).then_some((first as u32, (last - first + 1) as u32, from <= 0 && to >= length as i64 - 1))
                };
                let across = narrow('\u{2500}').and_then(|g| measure(alpha_ink_rows(g), alpha_ink_cols(g), aw));
                let down = narrow('\u{2502}').and_then(|g| measure(alpha_ink_cols(g), alpha_ink_rows(g), ah));
                (across, down)
            }
        };
        let joins = across.is_some_and(|line| line.2) && down.is_some_and(|line| line.2);
        let place = |line: Option<(u32, u32, bool)>| line.map(|(first, thick, _)| (first, thick));
        (Lines::new(aw, ah, place(across), place(down), self.active.underline_rows().1), joins)
    }

    /// How many cells of the active face a glyph takes.
    fn cells_of(&self, source: Source, ch: char) -> u32 {
        let aw = self.active.cell().0 as i64;
        match (source, &self.active) {
            (Source::Active, Face::Bitmap(f)) => 1 + f.glyph(ch).is_some_and(|g| g.width as i64 > aw) as u32,
            (Source::Active, Face::Outline(f)) => f.glyph(ch).map_or(1, |g| f.cells(g)),
            (Source::Fallback, _) => {
                let k = self.fit().map_or(1, |f| f.k);
                1 + self.fallback.glyph(ch).is_some_and(|g| g.width as i64 * k > aw) as u32
            }
        }
    }

    /// Decides what a cluster is drawn with, without drawing it.
    fn plan(&self, piece: &str) -> Option<Plan> {
        self.plan_from(piece, true)
    }

    /// [`FaceSet::plan`], and with `own` false the same decision as if the
    /// active face had none of the cluster's letters: what the fallback
    /// would draw. That is asked when the active face's glyph turns out
    /// wider than the cells the layout gave it.
    fn plan_from(&self, piece: &str, own: bool) -> Option<Plan> {
        let first = piece.chars().next()?;
        // A soft hyphen riding on a letter is a place to break, not an
        // accent, and is left out of the marks with the other characters
        // that have no picture.
        let (mut marks, mut count) = marks_of(piece);

        let base = match first {
            '\t' | '\n' | '\r' => Base::Blank,
            c if c.is_control() || cluster::has_no_picture(c) => Base::Box(c as u32),
            // A mark with nothing before it is a mark on nothing, and is
            // drawn as it would be on a space: the first character of a
            // line can be one. A mark no face can draw is a box, which says
            // which mark it is.
            c if cluster::is_zero_width(c) => {
                let (aw, ah) = self.active.cell();
                match self.mark_from(c, aw as i32, ah as i32) {
                    Some(_) => {
                        marks.copy_within(0..MAX_MARKS - 1, 1);
                        marks[0] = c;
                        count = (count + 1).min(MAX_MARKS);
                        Base::Blank
                    }
                    None => Base::Box(c as u32),
                }
            }
            // A line or a block is the face's own picture when its lines
            // join from cell to cell, and is otherwise drawn in code. It is
            // never borrowed: a borrowed line would stop short of the edges
            // of a larger cell.
            c if boxes::covers(c) => match own && self.active.has(c) && self.box_lines().1 {
                true => Base::Glyph(Source::Active, c),
                false => Base::Drawn(c),
            },
            c => {
                let fold = |source: Source| {
                    let (mut base, mut used) = (c, 0);
                    for &m in &marks[..count] {
                        match compose(base, m) {
                            Some(joined) if self.has(source, joined) => {
                                base = joined;
                                used += 1;
                            }
                            _ => break,
                        }
                    }
                    self.has(source, base).then_some((base, used))
                };
                let found = own
                    .then(|| fold(Source::Active))
                    .flatten()
                    .map(|f| (Source::Active, f))
                    .or_else(|| self.fit().and_then(|_| fold(Source::Fallback)).map(|f| (Source::Fallback, f)));
                match found {
                    Some((source, (base, used))) => {
                        marks.copy_within(used..count, 0);
                        count -= used;
                        Base::Glyph(source, base)
                    }
                    // A space no face has is still a space.
                    None if c.is_whitespace() => Base::Blank,
                    None => Base::Box(c as u32),
                }
            }
        };
        let cells = match base {
            Base::Glyph(source, ch) => self.cells_of(source, ch),
            Base::Blank | Base::Drawn(_) | Base::Box(_) => 1,
        };
        // Slots past the count are blanked so that two clusters drawn the
        // same way are the same plan, whatever they dropped on the way.
        marks[count..].fill(' ');
        Some(Plan { base, marks, count, cells })
    }

    /// How many cells of the active face `cluster` takes when drawn: 0 for
    /// an empty string, otherwise 1 or 2. Layout should take widths from
    /// its own table and use this only as a check on a face.
    pub fn cluster_cells(&self, cluster: &str) -> u32 {
        self.plan(cluster).map_or(0, |p| p.cells)
    }

    /// Draws one cluster, a base character and the zero-width marks that
    /// follow it, with its first cell's top-left corner at (`x`, `y`), and
    /// returns how many cells it took: 0 for an empty string, otherwise 1
    /// or 2.
    ///
    /// The letter and as many of its marks as have one precomposed glyph
    /// are drawn with that glyph. Marks left over, four at most, are placed
    /// on the letter here: above it with room made for them, below it
    /// without the letter losing anything. i, j and the other dotted
    /// letters lose their dots under a mark that sits above. A mark with
    /// nothing before it is drawn as on a space. A no-break space is a
    /// blank cell. A box-drawing character or a block is the face's own
    /// when its lines reach the edges of the cell, and is otherwise drawn
    /// in code so that boxes join. A tab or a line break handed in by
    /// mistake is a blank cell; any other control code, and any character
    /// no face has, is a box of hex digits, and marks typed after it are
    /// drawn over and under the box, which makes room for them. So nothing
    /// in the file is ever invisible, and U+FFFD appears only where the
    /// file really holds one.
    pub fn draw_cluster(&self, frame: &mut Frame, cluster: &str, x: i32, y: i32, pen: Pen) -> u32 {
        self.draw_cluster_in(frame, cluster, x, y, 0, pen)
    }

    /// [`FaceSet::draw_cluster`] for a layout that has already decided how
    /// many cells the cluster gets (`cells`, 1 or 2; 0 means "as drawn").
    ///
    /// Width belongs to the layout's own table, never to the face, or the
    /// cursor drifts off the text when the face changes. So the picture is
    /// made to fit the cells given: a narrow glyph is centred in two cells,
    /// and a glyph too wide for one cell (the dashes and quotation marks of
    /// a Japanese face are) is drawn from the fallback if that has it at
    /// the right width, in code if it is a line or a block, and otherwise
    /// replaced by the hex box rather than spilled over its neighbour. The
    /// return value is still what the active face would have taken, so a
    /// caller can notice the disagreement.
    ///
    /// A cluster that had to be put together (marks placed by hand, lines
    /// drawn in code, a letter borrowed at twice its size) is kept once
    /// that is done, so drawing it again on the next frame is a copy.
    pub fn draw_cluster_in(&self, frame: &mut Frame, cluster: &str, x: i32, y: i32, cells: u32, pen: Pen) -> u32 {
        let Some(mut plan) = self.plan(cluster) else { return 0 };
        let natural = plan.cells;
        if pen.alpha == 0 {
            return natural;
        }
        let s = pen.step() as i64;
        let (aw, ah) = self.active.cell();
        let slot = if cells == 0 { natural } else { cells.min(2) };

        if plan.cells > slot {
            plan = match self.plan_from(cluster, false) {
                Some(narrower) if narrower.cells <= slot => narrower,
                // Too wide whoever draws it: the box for its first
                // character, with every mark that was typed on it.
                _ => {
                    let (marks, count) = marks_of(cluster);
                    let first = cluster.chars().next().map_or(0, |c| c as u32);
                    Plan { base: Base::Box(first), marks, count, cells: slot }
                }
            };
        }
        // A box fills the cells it is given.
        if let Base::Box(_) = plan.base {
            plan.cells = slot;
        }
        // A narrow picture given two cells sits in the middle of them. The
        // shift is worked out in face pixels and then enlarged, so that at
        // every scale the picture stays on the grid its neighbours are on.
        let shift = (slot - plan.cells) as i64 * aw as i64 / 2 * s;
        let x = (x as i64 + shift).clamp(i32::MIN as i64, i32::MAX as i64) as i32;

        match plan.base {
            Base::Blank if plan.count == 0 => {}
            Base::Box(cp) if plan.count == 0 => hexbox::draw(frame, cp, x, y, (aw * slot, ah), self.active.ascent(), pen),
            Base::Glyph(source, ch) if plan.count == 0 && self.draws_direct(source, pen) => {
                self.draw_plain(frame, source, ch, plan.cells, (x, y), pen);
            }
            _ => {
                // The digits of a box are too small to strike twice.
                let bold = pen.bold && !matches!(plan.base, Base::Box(_));
                let key = CellKey { base: plan.base, marks: plan.marks, count: plan.count, cells: plan.cells, bold };
                let mut kept = self.kept();
                let lay = |frame: &mut Frame, c: &Canvas| {
                    draw_coverage(frame, &c.cov, (c.w as usize, c.h as usize), (x, y), s as i32, pen.color, pen.alpha);
                };
                match kept.cells.get(&key) {
                    Some(canvas) => lay(frame, canvas),
                    None => {
                        let mut canvas = self.composed(&plan);
                        if bold {
                            canvas.thicken(self.strike_width());
                        }
                        lay(frame, &canvas);
                        kept.keep(key, canvas);
                    }
                }
            }
        }
        natural
    }

    /// True when a glyph that carries no marks can go straight from its
    /// face to the frame, which it can whenever that gives the picture the
    /// cell would have if it were put together first. A glyph borrowed at
    /// more than its own size cannot: struck twice by one of its own pixels
    /// it would be heavier than the same letter under a mark, and enlarged
    /// by the pen it would stop growing at the largest scale a pen has.
    fn draws_direct(&self, source: Source, pen: Pen) -> bool {
        match source {
            Source::Active => true,
            Source::Fallback => self.fit().is_some_and(|fit| fit.k == 1) && (!pen.bold || self.strike_width() == 1),
        }
    }

    /// Draws a glyph that carries no marks, the common case, without
    /// building a picture of the cell first.
    fn draw_plain(&self, frame: &mut Frame, source: Source, ch: char, cells: u32, (x, y): (i32, i32), pen: Pen) {
        let s = pen.step() as i64;
        let (aw, ah) = self.active.cell();
        let screen = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        match (source, &self.active) {
            (Source::Active, Face::Bitmap(f)) => {
                if let Some(g) = f.glyph(ch) {
                    draw_glyph(frame, g, x, y, pen);
                }
            }
            (Source::Active, Face::Outline(f)) => {
                let Some(g) = f.glyph(ch) else { return };
                let (w, h) = ((aw * cells) as i64, ah as i64);
                let (left, top) = (g.left as i64, g.top as i64);
                let inside = left >= 0 && top >= 0 && left + g.width as i64 <= w && top + g.height as i64 <= h;
                if inside && !pen.bold {
                    let (gx, gy) = (screen(x as i64 + left * s), screen(y as i64 + top * s));
                    draw_coverage(frame, &g.coverage, (g.width as usize, g.height as usize), (gx, gy), s as i32, pen.color, pen.alpha);
                } else {
                    // Cut to the cell, and thickened inside it, on a canvas.
                    let mut canvas = Canvas::new(aw * cells, ah);
                    canvas.stamp_alpha(g, 0, 0);
                    if pen.bold {
                        canvas.thicken(self.strike_width());
                    }
                    draw_coverage(frame, &canvas.cov, (canvas.w as usize, canvas.h as usize), (x, y), s as i32, pen.color, pen.alpha);
                }
            }
            (Source::Fallback, _) => {
                // At its own size the fallback's whole cell lies inside the
                // active one, so the glyph needs no cutting.
                let (Some(g), Some(fit)) = (self.fallback.glyph(ch), self.fit()) else { return };
                let spare = (aw * cells) as i64 - g.width as i64 * fit.k;
                draw_glyph(frame, g, screen(x as i64 + spare / 2 * s), screen(y as i64 + fit.oy * s), pen);
            }
        }
    }

    /// The picture of a cluster that is put together before it is drawn:
    /// one with marks left to place, one drawn in code, a letter borrowed
    /// at more than its own size.
    fn composed(&self, plan: &Plan) -> Canvas {
        let (aw, ah) = self.active.cell();
        let mut canvas = Canvas::new(aw * plan.cells, ah);
        let gap = ((ah as i32 + 11) / 22).max(1);
        let fit = self.fit();
        let marks: Vec<(Canvas, i32, i32)> =
            plan.marks[..plan.count].iter().map(|&m| self.mark_picture(m, canvas.w, canvas.h, gap)).collect();

        let (x_top, baseline) = (self.active.x_top() as i32, self.active.ascent() as i32 - 1);
        let mut placing = Placing { gap, lower: false, x_top, baseline, above: 0, below: 0 };
        match plan.base {
            Base::Blank => {}
            Base::Box(cp) => return self.boxed(cp, canvas, marks, placing),
            Base::Drawn(ch) => {
                boxes::paint(ch, self.box_lines().0, &mut |x0, y0, x1, y1| canvas.fill(x0, y0, x1, y1));
            }
            Base::Glyph(source, mut ch) => {
                // A dotted letter loses its dot under any mark that sits above it.
                let marked_above =
                    marks.iter().any(|(m, above, below)| m.ink_rows().is_some_and(|(mt, mb)| side(mt, mb, *above, *below) == Side::Above));
                let mut rub_out_dot = false;
                match soft_dotted(ch).filter(|_| marked_above) {
                    Some(Some(bare)) if self.has(source, bare) => ch = bare,
                    Some(_) => rub_out_dot = true,
                    None => {}
                }
                placing.lower = ch.is_lowercase();
                match (source, &self.active) {
                    (Source::Active, Face::Bitmap(f)) => {
                        if let Some(g) = f.glyph(ch) {
                            canvas.stamp_bits(g, 0, 0, 1);
                        }
                    }
                    (Source::Active, Face::Outline(f)) => {
                        if let Some(g) = f.glyph(ch) {
                            canvas.stamp_alpha(g, 0, 0);
                        }
                    }
                    (Source::Fallback, _) => {
                        if let (Some(g), Some(fit)) = (self.fallback.glyph(ch), fit) {
                            let spare = canvas.w as i64 - g.width as i64 * fit.k;
                            canvas.stamp_bits(g, spare / 2, fit.oy, fit.k);
                            placing.x_top = (self.fallback.x_top() as i64 * fit.k + fit.oy) as i32;
                            placing.baseline = (self.fallback.ascent() as i64 * fit.k + fit.oy) as i32 - 1;
                        }
                    }
                }
                canvas.settle();
                if rub_out_dot {
                    remove_dot(&mut canvas, placing.x_top);
                }
            }
        }
        // A capital stands about two fifths of the x-height above it, and
        // may be made shorter by that much and no more. What does not stand
        // above the x-height at all has nothing to give: a hyphen under two
        // marks is not a capital, and keeps both its rows.
        let head = canvas.ink_rows().map_or(0, |(top, _)| placing.x_top - top);
        let spare = ((placing.baseline + 1 - placing.x_top) * 2 / 5).min(head).max(0);
        let mut room = Room { spare, stacked: false };
        for (mark, above, below) in marks {
            add_mark(&mut canvas, mark, Placing { above, below, ..placing }, &mut room);
        }
        canvas
    }

    /// The hex box for `cp` with marks on it.
    ///
    /// The box is information and is never cut or squeezed for a mark, and
    /// a mark is a character in the file and is never dropped. So the
    /// marks are placed first, as they would be on a space, and the box is
    /// laid out in the rows left between them: lower under a mark above,
    /// with smaller digits or without its rules if that is what fits. A
    /// mark struck through the letter (a stroke, a slash) would strike out
    /// the digits, and is shown as the small hollow square that stands for
    /// a mark that cannot be drawn.
    fn boxed(&self, cp: u32, mut canvas: Canvas, marks: Vec<(Canvas, i32, i32)>, placing: Placing) -> Canvas {
        let (w, h, gap) = (canvas.w, canvas.h, placing.gap);
        let mut above = Canvas::new(w as u32, h as u32);
        let mut below = Canvas::new(w as u32, h as u32);
        let (mut over_it, mut under_it) = (Room::default(), Room::default());
        for (mark, over, under) in marks {
            let Some((mt, mb)) = mark.ink_rows() else { continue };
            match side(mt, mb, over, under) {
                Side::Above => add_mark(&mut above, mark, Placing { above: over, below: under, ..placing }, &mut over_it),
                Side::Below => add_mark(&mut below, mark, Placing { above: over, below: under, ..placing }, &mut under_it),
                Side::Through => {
                    let (square, over, under) = placeholder(w, h, gap);
                    add_mark(&mut above, square, Placing { above: over, below: under, ..placing }, &mut over_it);
                }
            }
        }
        let mut top = above.ink_rows().map_or(0, |(_, last)| last + 1 + gap);
        let mut bottom = below.ink_rows().map_or(h, |(first, _)| first - gap);
        // With no room left for even a hollow box, the box keeps the cell
        // and the marks are struck over it.
        if bottom - top < 3 {
            (top, bottom) = (0, h);
        }
        let ascent = (self.active.ascent() as i32 - top).clamp(0, bottom - top);
        hexbox::paint(cp, (w as u32, (bottom - top) as u32), ascent as u32, &mut |r: Rect| {
            canvas.fill(r.x, r.y + top, r.x + r.w, r.y + top + r.h);
        });
        canvas.merge(&above);
        canvas.merge(&below);
        canvas
    }

    /// Which face draws a mark on a cluster whose canvas is `w` by `h`, or
    /// `None` if neither can.
    ///
    /// The mark comes from the active face if it has it. Otherwise it is
    /// borrowed from the fallback, and for a mark that is allowed even when
    /// the fallback's cell does not fit the active one, as long as the
    /// mark's own ink does: Terminus is a pixel narrower than Gallant and
    /// has no comma above, and mitsqanaqan should still get its comma.
    fn mark_from(&self, m: char, w: i32, h: i32) -> Option<MarkFrom> {
        if self.active.has(m) {
            return Some(MarkFrom::Active);
        }
        let g = self.fallback.glyph(m)?;
        let aw = self.active.cell().0;
        let (fw, fh) = self.fallback.cell();
        let centre = (w as i64 - aw as i64) / 2;
        let (k, ox, oy) = match self.fit() {
            Some(fit) => (fit.k, fit.ox, fit.oy),
            None => (1, (aw as i64 - fw as i64).div_euclid(2), self.active.ascent() as i64 - self.fallback.ascent() as i64),
        };
        // Measure the ink where it would land, then slide it back inside
        // if a smaller cell cut it. If it cannot fit at all, there is no
        // borrowing it.
        let mut probe = Canvas::new(fw * 2, fh);
        probe.stamp_bits(g, 0, 0, 1);
        let ((r0, r1), (c0, c1)) = (probe.ink_rows()?, probe.ink_cols()?);
        let wide = (g.width as i64 - fw as i64).max(0) * k / 2;
        let (x0, x1) = (centre + ox - wide + c0 as i64 * k, centre + ox - wide + (c1 as i64 + 1) * k);
        let (y0, y1) = (oy + r0 as i64 * k, oy + (r1 as i64 + 1) * k);
        // A mark that is nothing but lines ruled from edge to edge of its
        // cell (a line below, a double line above) loses nothing by being
        // cut to a narrower one, and stays where it is.
        let all = if g.width >= 32 { u32::MAX } else { !(u32::MAX >> g.width) };
        let ruled = g.rows.iter().all(|&row| row == 0 || row == all);
        if (x1 - x0 > w as i64 && !ruled) || y1 - y0 > h as i64 {
            return None;
        }
        let dx = if x1 - x0 > w as i64 { 0 } else { (-x0).max(0) - (x1 - w as i64).max(0) };
        let dy = (-y0).max(0) - (y1 - h as i64).max(0);
        Some(MarkFrom::Fallback { k, x: centre + ox - wide + dx, y: oy + dy })
    }

    /// One mark as its face draws it, on a canvas the size of the cluster,
    /// with the two rows that tell a mark above from a mark below. A mark
    /// no face has is a tiny hollow square above the letter, so it is seen
    /// to be there.
    fn mark_picture(&self, m: char, w: i32, h: i32, gap: i32) -> (Canvas, i32, i32) {
        let aw = self.active.cell().0 as i64;
        let mut canvas = Canvas::new(w as u32, h as u32);
        let centre = (w as i64 - aw) / 2;
        let own = (self.active.x_top() as i32, self.active.ascent() as i32 - 1);
        // A space handed in as if it were a mark has nothing to show.
        if cluster::is_blank(m) {
            return (canvas, own.0, own.1);
        }

        let (above, below) = match (self.mark_from(m, w, h), &self.active) {
            (None, _) => return placeholder(w, h, gap),
            (Some(MarkFrom::Active), Face::Bitmap(f)) => {
                if let Some(g) = f.glyph(m) {
                    canvas.stamp_bits(g, (w as i64 - g.width as i64) / 2, 0, 1);
                }
                own
            }
            (Some(MarkFrom::Active), Face::Outline(f)) => {
                if let Some(g) = f.glyph(m) {
                    // A mark rasterized alone may sit in its own cell, hang
                    // back over the cell before it, or lie anywhere. Going
                    // by the middle of its ink, the first stays where it
                    // is, the second comes forward one cell, and anything
                    // else is centred.
                    let ox = match alpha_ink_cols(g) {
                        Some((first, last)) if (0..2 * aw).contains(&(first + last + 1)) => 0,
                        Some((first, last)) if (-2 * aw..0).contains(&(first + last + 1)) => aw,
                        Some((first, last)) => (aw - (last - first + 1)) / 2 - first,
                        None => 0,
                    };
                    canvas.stamp_alpha(g, centre + ox, 0);
                }
                own
            }
            (Some(MarkFrom::Fallback { k, x, y }), _) => {
                if let Some(g) = self.fallback.glyph(m) {
                    canvas.stamp_bits(g, x, y, k);
                }
                let above = self.fallback.x_top() as i64 * k + y;
                let below = self.fallback.ascent() as i64 * k - 1 + y;
                (above as i32, below as i32)
            }
        };
        canvas.settle();
        (canvas, above, below)
    }

    /// Draws the hex box for code point `cp` across `cells` cells, for a
    /// reveal mode that wants to show a character by number.
    pub fn draw_hex_box(&self, frame: &mut Frame, cp: u32, x: i32, y: i32, cells: u32, pen: Pen) {
        let (aw, ah) = self.active.cell();
        hexbox::draw(frame, cp, x, y, (aw * cells.clamp(1, 2), ah), self.active.ascent(), pen);
    }

    /// Draws `text` cluster by cluster from (`x`, `y`) and returns the x
    /// where the next cell would start. This is for captions and specimens:
    /// it lays out by what the faces draw, where the page must lay out by
    /// its own width table and call [`FaceSet::draw_cluster_in`]. A tab or a
    /// line break is one blank cell.
    pub fn draw_text(&self, frame: &mut Frame, text: &str, x: i32, y: i32, pen: Pen) -> i32 {
        let step = self.active.cell().0 as i64 * pen.step() as i64;
        let mut at = x as i64;
        for c in cluster::clusters(text) {
            let here = at.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            at += self.draw_cluster(frame, c, here, y, pen) as i64 * step;
        }
        at.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    fn rule(&self, frame: &mut Frame, x: i32, y: i32, cells: u32, rows: (u32, u32), pen: Pen) {
        let s = pen.step() as i64;
        let w = cells as i64 * self.active.cell().0 as i64 * s;
        let stroke = clipped(frame.clip(), x as i64, y as i64 + rows.0 as i64 * s, w, rows.1 as i64 * s);
        frame.blend(stroke, pen.color, pen.alpha);
    }

    /// Underlines `cells` cells whose top-left corner is at (`x`, `y`): one
    /// unbroken stroke where the face's own underscore would print, which is
    /// how a typewriter underlines.
    pub fn underline(&self, frame: &mut Frame, x: i32, y: i32, cells: u32, pen: Pen) {
        self.rule(frame, x, y, cells, self.active.underline_rows(), pen);
    }

    /// Strikes through `cells` cells at the middle of the lower-case letters.
    pub fn strike(&self, frame: &mut Frame, x: i32, y: i32, cells: u32, pen: Pen) {
        self.rule(frame, x, y, cells, self.active.strike_rows(), pen);
    }
}

/// What stands for a mark that cannot be drawn: a tiny hollow square at the
/// top of the cell, with rows that side it above the letter whatever the
/// face.
fn placeholder(w: i32, h: i32, gap: i32) -> (Canvas, i32, i32) {
    let mut canvas = Canvas::new(w as u32, h as u32);
    let side = 3 * gap as i64;
    let left = (w as i64 - side) / 2;
    for i in 0..side {
        for j in 0..side {
            let edge = i < gap as i64 || j < gap as i64 || i >= side - gap as i64 || j >= side - gap as i64;
            if edge {
                canvas.put(left + i, j, 255);
            }
        }
    }
    (canvas, h.max(1), h.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{spleen, terminus};
    use crate::color::Rgb;

    const WHITE: Rgb = Rgb(255, 255, 255);

    /// The cluster drawn at scale 1 in a frame exactly its size: one string
    /// per row, `#` for ink.
    fn rows(set: &FaceSet, cluster: &str) -> Vec<String> {
        let (cw, ch) = set.cell();
        let cells = set.cluster_cells(cluster).max(1);
        let mut frame = Frame::new((cw * cells) as usize, ch as usize);
        frame.clear(Rgb(0, 0, 0));
        set.draw_cluster(&mut frame, cluster, 0, 0, Pen::new(WHITE));
        (0..ch as i32)
            .map(|y| (0..(cw * cells) as i32).map(|x| if frame.get(x, y) == Some(WHITE) { '#' } else { '.' }).collect())
            .collect()
    }

    fn blank(row: &str) -> bool {
        !row.contains('#')
    }

    fn top(rows: &[String]) -> usize {
        rows.iter().position(|r| !blank(r)).unwrap_or(rows.len())
    }

    const COMMA: [&str; 4] = ["....####....", "....####....", "......##....", "....###....."];

    #[test]
    fn the_comma_of_mitsqanaqan_sits_two_rows_down_with_a_clear_row_under_it() {
        let set = FaceSet::new(gallant());
        let got = rows(&set, "n\u{0313}");
        let plain = rows(&set, "n");
        assert!(got[..2].iter().all(|r| blank(r)));
        assert_eq!(&got[2..6], &COMMA);
        assert!(blank(&got[6]));
        assert_eq!(&got[7..], &plain[7..]);
    }

    #[test]
    fn tall_letters_are_shortened_the_way_the_face_shortens_its_own_l() {
        let set = FaceSet::new(gallant());
        for (letter, first_row) in [('l', "...####....."), ('t', ".....#......"), ('f', "......###...")] {
            let got = rows(&set, &format!("{letter}\u{0313}"));
            let plain = rows(&set, &letter.to_string());
            assert_eq!(&got[..4], &COMMA, "{letter}");
            assert!(blank(&got[4]), "{letter}");
            assert_eq!(got[5], first_row, "{letter}");
            // Nothing changes from the x-height down.
            assert_eq!(&got[9..], &plain[9..], "{letter}");
        }
        // The face's own l-acute has the same build: accent, clear row, serif, stem.
        let own = rows(&set, "\u{013A}");
        assert!(blank(&own[3]) && own[4] == "...####.....");
    }

    #[test]
    fn a_mark_that_fits_beside_a_stem_leaves_the_letter_whole() {
        let set = FaceSet::new(gallant());
        for letter in ['k', 'b', 'h', 'd'] {
            let got = rows(&set, &format!("{letter}\u{0313}"));
            let plain = rows(&set, &letter.to_string());
            let kept = plain.iter().zip(&got).all(|(p, g)| p.chars().zip(g.chars()).all(|(p, g)| p != '#' || g == '#'));
            assert!(kept, "{letter} lost ink");
            let extra: usize = got.iter().zip(&plain).map(|(g, p)| g.matches('#').count() - p.matches('#').count()).sum();
            assert_eq!(extra, 13, "{letter}: the whole comma is there");
            assert_eq!(top(&got), 0, "{letter}");
        }
    }

    #[test]
    fn i_and_j_lose_their_dots_under_a_mark_above_and_keep_them_over_a_mark_below() {
        let set = FaceSet::new(gallant());
        assert_eq!(rows(&set, "i\u{0313}"), rows(&set, "\u{0131}\u{0313}"));
        assert_eq!(rows(&set, "j\u{0313}"), rows(&set, "\u{0237}\u{0313}"));
        let got = rows(&set, "i\u{0313}");
        assert_eq!(&got[2..6], &COMMA);
        assert!(blank(&got[6]) && got[7] == "...####.....");
        // i with a low line: no precomposed glyph, and the dot stays.
        let low = rows(&set, "i\u{0331}");
        assert_eq!(&low[..17], &rows(&set, "i")[..17]);
        assert!(!blank(&low[19]));
        // A mark above takes the dot wherever it comes among the marks.
        let both = rows(&set, "i\u{0331}\u{0301}");
        assert_eq!(&both[..17], &rows(&set, "\u{0131}\u{0301}")[..17]);
        assert_eq!(&both[17..], &low[17..]);
        // The barred i has no dotless twin, so its dot is rubbed out.
        let barred = rows(&set, "\u{0268}\u{0301}");
        assert_eq!(&barred[..7], &rows(&set, "\u{0131}\u{0301}")[..7]);
        assert_eq!(&barred[7..], &rows(&set, "\u{0268}")[7..]);
        assert!(!blank(&rows(&set, "\u{0268}")[3]));
    }

    #[test]
    fn a_face_with_no_dotless_letters_has_the_dot_rubbed_out() {
        // Spleen has a dotless i but no dotless j.
        assert!(spleen().has('\u{0131}') && !spleen().has('\u{0237}'));
        let set = FaceSet::new(spleen());
        let got = rows(&set, "j\u{0313}");
        let plain = rows(&set, "j");
        let x_top = spleen().x_top() as usize;
        assert_eq!(&got[x_top..], &plain[x_top..]);
        // Comma, then nothing until the stem: the dot is gone.
        let comma_rows: Vec<usize> = (0..x_top).filter(|&r| !blank(&got[r])).collect();
        assert_eq!(comma_rows.len(), 4);
        assert_eq!(comma_rows[3] + 2, x_top);
    }

    #[test]
    fn precomposed_glyphs_are_used_when_the_face_has_them() {
        let set = FaceSet::new(gallant());
        assert_eq!(rows(&set, "e\u{0301}"), rows(&set, "\u{00E9}"));
        assert_eq!(rows(&set, "n\u{0303}"), rows(&set, "\u{00F1}"));
        assert_eq!(rows(&set, "e\u{0302}\u{0301}"), rows(&set, "\u{1EBF}"));
        assert_eq!(rows(&set, "a\u{0323}\u{0302}"), rows(&set, "\u{1EAD}"));
        // The comma joins nothing, so the acute after it is placed by hand too.
        assert_ne!(rows(&set, "e\u{0313}\u{0301}"), rows(&set, "\u{00E9}\u{0313}"));
        // A letter that is already accented takes a further mark above its accent.
        let got = rows(&set, "s\u{030C}\u{0313}");
        assert_eq!(got, rows(&set, "\u{0161}\u{0313}"));
        assert_eq!(top(&got), 0);
        assert_eq!(&got[7..], &rows(&set, "s")[7..]);
    }

    #[test]
    fn a_face_without_the_precomposed_letter_builds_it() {
        let glyph = |cp: u32, rows: [u16; 8]| format!("{cp:04X}:{}\n", rows.map(|r| format!("{r:04X}")).concat());
        let mut src = String::from("# Width: 12\n# Height: 8\n");
        src.push_str(&glyph(0x65, [0, 0, 0, 0x1F80, 0x2040, 0x3FC0, 0x2000, 0x1F80]));
        src.push_str(&glyph(0x78, [0, 0, 0, 0x2040, 0x1080, 0x0F00, 0x1080, 0x2040]));
        src.push_str(&glyph(0x301, [0x0300, 0x0C00, 0, 0, 0, 0, 0, 0]));
        let face = BitmapFont::from_hex("tiny", &src).unwrap();
        let set = FaceSet::new(face);
        assert!(!set.fallback_fits());
        let got = rows(&set, "e\u{0301}");
        // The accent is as high as the cell allows and the letter is whole.
        assert_eq!(&got[..2], &["......##....", "....##......"]);
        assert_eq!(&got[3..], &rows(&set, "e")[3..]);
        // e-acute itself is in neither usable face, so it is a box.
        assert_ne!(rows(&set, "\u{00E9}"), got);
    }

    #[test]
    fn only_the_first_four_marks_are_drawn() {
        let set = FaceSet::new(gallant());
        let four = "n\u{0323}\u{0313}\u{0331}\u{0303}";
        let fifty = format!("{four}{}", "\u{0308}".repeat(46));
        assert_eq!(fifty.chars().count(), 51);
        assert_eq!(rows(&set, &fifty), rows(&set, four));
        assert_ne!(rows(&set, four), rows(&set, "n\u{0323}\u{0313}\u{0331}"));
        // Characters with no picture do not use up the four.
        let padded = "n\u{200B}\u{0323}\u{200D}\u{0313}\u{FE0F}\u{0331}\u{2060}\u{0303}";
        assert_eq!(rows(&set, padded), rows(&set, four));
        assert_eq!(rows(&set, "a\u{200B}"), rows(&set, "a"));
        assert_eq!(rows(&set, "a\u{034F}\u{FEFF}"), rows(&set, "a"));
    }

    #[test]
    fn a_mark_alone_or_on_a_blank_is_drawn_where_the_face_drew_it() {
        let set = FaceSet::new(gallant());
        let alone = rows(&set, "\u{0313}");
        assert_eq!(&alone[..4], &COMMA);
        assert!(alone[4..].iter().all(|r| blank(r)));
        assert_eq!(rows(&set, "\u{00A0}\u{0313}"), alone);
        assert_eq!(rows(&set, " \u{0313}"), alone);
        assert_eq!(rows(&set, "\t\u{0313}"), alone);
    }

    /// True when every pixel of `plain` is still there in `got`.
    fn whole(plain: &[String], got: &[String]) -> bool {
        plain.iter().zip(got).all(|(p, g)| p.chars().zip(g.chars()).all(|(p, g)| p != '#' || g == '#'))
    }

    #[test]
    fn a_mark_below_leaves_the_letter_whole_and_an_overlay_strikes_through() {
        let set = FaceSet::new(gallant());
        let dotted = rows(&set, "g\u{0323}");
        let g = rows(&set, "g");
        // The g keeps its bowl. The dot has a row squeezed out of it and
        // floats in the bowl with clear ground all round.
        assert!(whole(&g, &dotted));
        assert_eq!(&dotted[17..], &["..#......##.", ".#...##...#.", ".#........#.", ".#########..", "..#######..."]);
        // A dot under a letter with no descender is where the face drew it.
        let x = rows(&set, "x\u{0323}");
        assert_eq!(&x[..17], &rows(&set, "x")[..17]);
        assert_eq!(&x[17..], &rows(&set, "\u{0323}")[17..]);
        // A stroke through the middle only adds ink.
        let struck = rows(&set, "l\u{0335}");
        let l = rows(&set, "l");
        assert!(l.iter().zip(&struck).all(|(p, s)| p.chars().zip(s.chars()).all(|(p, s)| p != '#' || s == '#')));
        assert_ne!(struck, l);
    }

    #[test]
    fn capitals_carry_the_mark_on_their_heads_as_the_face_does() {
        let set = FaceSet::new(gallant());
        let got = rows(&set, "K\u{0313}");
        // A row squeezed out of the comma; the capital untouched.
        assert_eq!(&got[..3], &["....####....", "......##....", "....###....."]);
        assert_eq!(&got[3..], &rows(&set, "K")[3..]);
        let q = rows(&set, "Q\u{0301}");
        assert_eq!(&q[..3], &["......##....", ".....##.....", "....##......"]);
        assert_eq!(&q[3..], &rows(&set, "Q")[3..]);
    }

    #[test]
    fn a_mark_no_face_has_still_shows() {
        let set = FaceSet::new(gallant());
        assert!(!gallant().has('\u{1DC4}'));
        let got = rows(&set, "x\u{1DC4}");
        let x = rows(&set, "x");
        assert_eq!(&got[7..], &x[7..]);
        assert_eq!(&got[3..6], &["....###.....", "....#.#.....", "....###....."]);
    }

    #[test]
    fn terminus_borrows_the_comma_though_gallant_is_a_pixel_too_wide() {
        let set = FaceSet::new(terminus());
        assert!(!set.fallback_fits() && !terminus().has('\u{0313}'));
        let got = rows(&set, "n\u{0313}");
        let n = rows(&set, "n");
        assert_eq!(&got[7..], &n[7..]);
        assert_eq!(&got[2..6], &["...####....", "...####....", ".....##....", "...###....."]);
        assert!(blank(&got[6]));
        // A whole letter is not borrowed: it would not fit. The box for it
        // has rules and no walls, so that it keeps clear of its neighbours.
        assert_eq!(set.cluster_cells("\u{0294}"), 1);
        let glottal = rows(&set, "\u{0294}");
        assert_eq!(glottal[2], "..#######..");
        assert_eq!(glottal[16], "..#######..");
        // A line ruled from edge to edge is borrowed and cut to the cell.
        assert!(!terminus().has('\u{0332}'));
        let lined = rows(&set, "x\u{0332}");
        assert_eq!(&lined[..17], &rows(&set, "x")[..17]);
        assert_eq!(&lined[20..], &["###########", "###########"]);
    }

    #[test]
    fn spleen_borrows_from_gallant_with_the_baselines_level() {
        let set = FaceSet::new(spleen());
        assert!(set.fallback_fits() && !spleen().has('\u{0294}'));
        let got = rows(&set, "\u{0294}");
        let own = rows(&FaceSet::new(gallant()), "\u{0294}");
        assert!(blank(&got[0]) && blank(&got[1]));
        assert_eq!(&got[2..], &own[..]);
        // A borrowed wide glyph takes two cells.
        assert_eq!(set.cluster_cells("\u{2614}"), 2);
        assert_eq!(&rows(&set, "\u{2614}")[2..], &rows(&FaceSet::new(gallant()), "\u{2614}")[..]);
    }

    #[test]
    fn what_no_face_has_is_a_box_and_never_the_replacement_character() {
        let set = FaceSet::new(gallant());
        let real = rows(&set, "\u{FFFD}");
        for missing in ['\u{0E01}', '\u{4E00}', '\u{1F600}', '\u{E000}', '\u{10FFFF}'] {
            assert!(!gallant().has(missing));
            let got = rows(&set, &missing.to_string());
            assert!(got.iter().any(|r| !blank(r)), "{:X} is invisible", missing as u32);
            assert_ne!(got, real);
        }
        assert_ne!(rows(&set, "\u{0E01}"), rows(&set, "\u{0E02}"));
        // Spleen has no U+FFFD of its own, so a real one comes from Gallant.
        assert_eq!(&rows(&FaceSet::new(spleen()), "\u{FFFD}")[2..], &real[..]);
    }

    #[test]
    fn control_codes_are_blank_or_boxed_and_never_their_stored_pictures() {
        let set = FaceSet::new(gallant());
        for quiet in ["\t", "\n", "\r", "\u{00A0}", "\u{202F}", " "] {
            assert!(rows(&set, quiet).iter().all(|r| blank(r)), "{quiet:?}");
            assert_eq!(set.cluster_cells(quiet), 1);
        }
        let mut frame = Frame::new(12, 22);
        for cp in [0x00u32, 0x01, 0x07, 0x1B, 0x1F, 0x7F, 0x80, 0x9F, 0x200B, 0xFEFF] {
            let ch = char::from_u32(cp).unwrap();
            let got = rows(&set, &ch.to_string());
            frame.clear(Rgb(0, 0, 0));
            set.draw_hex_box(&mut frame, cp, 0, 0, 1, Pen::new(WHITE));
            let boxed: Vec<String> =
                (0..22).map(|y| (0..12).map(|x| if frame.get(x, y) == Some(WHITE) { '#' } else { '.' }).collect()).collect();
            assert_eq!(got, boxed, "U+{cp:04X}");
        }
        assert_eq!(rows(&set, "\u{2011}"), rows(&set, "\u{2010}"));
    }

    #[test]
    fn the_count_returned_is_what_the_faces_take() {
        let set = FaceSet::new(gallant());
        let mut frame = Frame::new(48, 22);
        let pen = Pen::new(WHITE);
        assert_eq!(set.draw_cluster(&mut frame, "", 0, 0, pen), 0);
        assert_eq!(set.draw_cluster(&mut frame, "a", 0, 0, pen), 1);
        assert_eq!(set.draw_cluster(&mut frame, "n\u{0313}", 0, 0, pen), 1);
        assert_eq!(set.draw_cluster(&mut frame, "\u{2614}", 0, 0, pen), 2);
        assert_eq!(set.draw_cluster(&mut frame, "\u{30F3}\u{0301}", 0, 0, pen), 2);
        assert_eq!(set.draw_cluster(&mut frame, "\u{1F600}", 0, 0, pen), 1);
        assert_eq!(set.draw_cluster(&mut frame, "a", 0, 0, pen.alpha(0)), 1);
        assert_eq!(set.draw_text(&mut frame, "ok \u{2614}n\u{0313}", 5, 0, pen.alpha(0)), 5 + 6 * 12);
        assert_eq!(set.draw_text(&mut frame, "ok", 5, 0, pen.alpha(0).scale(3)), 5 + 2 * 36);
    }

    fn lit(frame: &Frame, x0: i32, x1: i32) -> usize {
        (0..frame.height() as i32).map(|y| (x0..x1).filter(|&x| frame.get(x, y) != Some(Rgb(0, 0, 0))).count()).sum()
    }

    #[test]
    fn the_layout_decides_the_width_and_the_picture_obeys() {
        let set = FaceSet::new(gallant());
        let pen = Pen::new(WHITE);
        let mut frame = Frame::new(48, 22);

        // A wide glyph given one cell becomes a one-cell box.
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster_in(&mut frame, "\u{2614}", 12, 0, 1, pen), 2);
        assert!(lit(&frame, 12, 24) > 0);
        assert_eq!(lit(&frame, 0, 12) + lit(&frame, 24, 48), 0);
        let mut boxed = Frame::new(48, 22);
        boxed.clear(Rgb(0, 0, 0));
        set.draw_hex_box(&mut boxed, 0x2614, 12, 0, 1, pen);
        assert_eq!(frame.pixels(), boxed.pixels());

        // A narrow glyph given two cells is centred in them.
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster_in(&mut frame, "l", 12, 0, 2, pen), 1);
        let mut centred = Frame::new(48, 22);
        centred.clear(Rgb(0, 0, 0));
        set.draw_cluster(&mut centred, "l", 18, 0, pen);
        assert_eq!(frame.pixels(), centred.pixels());

        // A character no face has gets a box as wide as the layout says.
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster_in(&mut frame, "\u{1F600}", 12, 0, 2, pen), 1);
        assert!(lit(&frame, 12, 24) > 0 && lit(&frame, 24, 36) > 0);
        assert_eq!(lit(&frame, 0, 12) + lit(&frame, 36, 48), 0);

        // Nonsense cell counts are held to two.
        frame.clear(Rgb(0, 0, 0));
        set.draw_cluster_in(&mut frame, "\u{0E01}", 0, 0, u32::MAX, pen);
        assert_eq!(lit(&frame, 24, 48), 0);
    }

    #[test]
    fn a_glyph_too_wide_for_its_cell_comes_from_gallant_or_is_boxed() {
        // A face that draws the em dash two cells wide, as Japanese faces do.
        let (narrow, wide) = ("0FF0".repeat(22), "0FFFF0".repeat(22));
        let face = BitmapFont::from_hex("wide dash", &format!("# Width: 12\n# Height: 22\n0041:{narrow}\n2014:{wide}\n3042:{wide}\n")).unwrap();
        let set = FaceSet::new(face);
        let pen = Pen::new(WHITE);
        let drawn = |set: &FaceSet, cluster: &str, cells: u32| {
            let mut frame = Frame::new(48, 22);
            frame.clear(Rgb(0, 0, 0));
            let n = set.draw_cluster_in(&mut frame, cluster, 12, 0, cells, pen);
            (n, frame)
        };

        // Left to itself the dash takes two cells.
        assert_eq!(set.cluster_cells("\u{2014}"), 2);
        assert!(lit(&drawn(&set, "\u{2014}", 0).1, 24, 36) > 0);
        // Given one, it is Gallant's dash, and the answer still says two.
        let (n, frame) = drawn(&set, "\u{2014}", 1);
        assert_eq!(n, 2);
        assert!(frame.pixels() == drawn(&FaceSet::new(gallant()), "\u{2014}", 1).1.pixels());
        // With a mark on it too, all inside the one cell.
        let (n, frame) = drawn(&set, "\u{2014}\u{0301}", 1);
        assert_eq!(n, 2);
        assert!(frame.pixels() == drawn(&FaceSet::new(gallant()), "\u{2014}\u{0301}", 1).1.pixels());
        assert_eq!(lit(&frame, 0, 12) + lit(&frame, 24, 48), 0);
        // Gallant has no hiragana a, so that is the box in its one cell.
        let (n, frame) = drawn(&set, "\u{3042}", 1);
        let mut boxed = Frame::new(48, 22);
        boxed.clear(Rgb(0, 0, 0));
        set.draw_hex_box(&mut boxed, 0x3042, 12, 0, 1, pen);
        assert!(n == 2 && frame.pixels() == boxed.pixels());

        // A face too small to borrow from boxes the dash as well.
        let small = BitmapFont::from_hex("small", &format!("0041:{}\n2014:{}\n", "18".repeat(16), "1818".repeat(16))).unwrap();
        let set = FaceSet::new(small);
        let mut frame = Frame::new(24, 16);
        let mut boxed = Frame::new(24, 16);
        assert_eq!(set.draw_cluster_in(&mut frame, "\u{2014}", 8, 0, 1, pen), 2);
        set.draw_hex_box(&mut boxed, 0x2014, 8, 0, 1, pen);
        assert!(lit(&frame, 0, 24) > 0 && frame.pixels() == boxed.pixels());
    }

    /// A bitmap face's own glyph as rows of `#` and `.`.
    fn own_rows(face: &BitmapFont, ch: char) -> Vec<String> {
        let g = face.glyph(ch).expect("the face has it");
        g.rows.iter().map(|&row| (0..g.width).map(|c| if row & (0x8000_0000 >> c) != 0 { '#' } else { '.' }).collect()).collect()
    }

    #[test]
    fn a_face_whose_lines_join_keeps_its_own_box_drawing() {
        for face in [gallant(), spleen(), terminus()] {
            let set = FaceSet::new(face);
            assert!(set.box_lines().1, "{}", face.name());
            // Among them the tee Gallant draws its own way, not as the code would.
            for ch in ['\u{2500}', '\u{2502}', '\u{250C}', '\u{253C}', '\u{2554}', '\u{255F}', '\u{2588}', '\u{2591}'] {
                assert_eq!(rows(&set, &ch.to_string()), own_rows(face, ch), "{} U+{:04X}", face.name(), ch as u32);
            }
        }
        let mut painted = Canvas::new(12, 22);
        boxes::paint('\u{255F}', FaceSet::new(gallant()).box_lines().0, &mut |x0, y0, x1, y1| painted.fill(x0, y0, x1, y1));
        let painted: Vec<String> = (0..22).map(|y| (0..12).map(|x| if painted.at(x, y) != 0 { '#' } else { '.' }).collect()).collect();
        assert_ne!(painted, own_rows(gallant(), '\u{255F}'));
    }

    #[test]
    fn lines_and_blocks_a_face_lacks_are_drawn_in_its_own_weight() {
        // Terminus has no dashed lines and not all of the blocks.
        let set = FaceSet::new(terminus());
        for lacks in ['\u{2504}', '\u{254C}', '\u{2594}', '\u{2595}'] {
            assert!(!terminus().has(lacks), "U+{:04X}", lacks as u32);
            assert_eq!(set.cluster_cells(&lacks.to_string()), 1);
        }
        // Three dashes on the row of its own line, one pixel thick as that is.
        assert_eq!(own_rows(terminus(), '\u{2500}')[10], "###########");
        let dashed = rows(&set, "\u{2504}");
        assert_eq!(dashed[10], ".##.###.###");
        assert!(dashed.iter().enumerate().all(|(y, row)| blank(row) == (y != 10)));
        // The upper eighth and the right eighth.
        let eighth = rows(&set, "\u{2594}");
        assert!(eighth[..3].iter().all(|row| row == "###########") && eighth[3..].iter().all(|row| blank(row)));
        assert!(rows(&set, "\u{2595}").iter().all(|row| row == ".........##"));
        // Given two cells by the layout, a drawn character sits in the middle of them.
        let mut frame = Frame::new(33, 22);
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster_in(&mut frame, "\u{2594}", 0, 0, 2, Pen::new(WHITE)), 1);
        assert_eq!((lit(&frame, 0, 5), lit(&frame, 5, 16), lit(&frame, 16, 33)), (0, 33, 0));
    }

    #[test]
    fn a_face_whose_lines_stop_short_of_the_cell_has_them_drawn_so_they_join() {
        // Its line across has a pixel of margin at each end, and its corner
        // is a solid block that would be plain to see if it were used.
        let glyph = |cp: u32, rows: [u16; 8]| format!("{cp:04X}:{}\n", rows.map(|r| format!("{r:04X}")).concat());
        let mut src = String::from("# Width: 12\n# Height: 8\n");
        src.push_str(&glyph(0x2500, [0, 0, 0, 0, 0x7FE0, 0, 0, 0]));
        src.push_str(&glyph(0x2502, [0x0600; 8]));
        src.push_str(&glyph(0x250C, [0xFFF0; 8]));
        let set = FaceSet::new(BitmapFont::from_hex("short lines", &src).unwrap());
        assert!(!set.box_lines().1);
        assert_eq!(set.box_lines().0, Lines { w: 12, h: 8, vx: 5, tv: 2, hy: 4, th: 1 });

        let across = rows(&set, "\u{2500}");
        assert!(across.iter().enumerate().all(|(y, row)| if y == 4 { row == "############" } else { blank(row) }));
        let corner = rows(&set, "\u{250C}");
        assert_eq!(&corner[3..6], &["............", ".....#######", ".....##....."]);
        // A box of three by two cells: every line runs unbroken from corner to corner.
        let mut frame = Frame::new(36, 16);
        frame.clear(Rgb(0, 0, 0));
        set.draw_text(&mut frame, "\u{250C}\u{2500}\u{2510}", 0, 0, Pen::new(WHITE));
        set.draw_text(&mut frame, "\u{2514}\u{2500}\u{2518}", 0, 8, Pen::new(WHITE));
        assert!((5..31).all(|x| frame.get(x, 4) == Some(WHITE) && frame.get(x, 12) == Some(WHITE)));
        assert!((4..13).all(|y| frame.get(5, y) == Some(WHITE) && frame.get(30, y) == Some(WHITE)));
        assert_eq!(frame.get(4, 4), Some(Rgb(0, 0, 0)));
    }

    #[test]
    fn an_outline_face_gets_boxes_that_join_whatever_it_has() {
        // No box-drawing glyphs at all: the two plain lines are asked for,
        // and when the face has neither, everything is drawn in code.
        let mut set = FaceSet::new(OutlineFace::new("no lines", 30, (17, 31), 24));
        let text = "\u{250C}\u{2500}\u{2510}\u{2591}";
        assert_eq!(set.missing(text), vec!['x', '_', '\u{250C}', '\u{2500}', '\u{2502}', '\u{2510}', '\u{2591}']);
        assert_eq!(set.fill(text, |_| None), 7);
        assert_eq!(set.box_lines(), (Lines { w: 17, h: 31, vx: 8, tv: 1, hy: 15, th: 1 }, false));
        let mut frame = Frame::new(51, 31);
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_text(&mut frame, "\u{250C}\u{2500}\u{2510}", 0, 0, Pen::new(WHITE)), 51);
        assert!((8..43).all(|x| frame.get(x, 15) == Some(WHITE)));
        assert!((15..31).all(|y| frame.get(8, y) == Some(WHITE) && frame.get(42, y) == Some(WHITE)));
        assert_eq!(lit(&frame, 0, 8) + lit(&frame, 43, 51), 0);

        // A Japanese face draws its lines two cells wide. Each is given one
        // cell by the layout, where it is drawn in code and joins the next.
        let mut face = OutlineFace::new("wide lines", 24, (12, 24), 19);
        face.insert('\u{2500}', Some(AlphaGlyph { width: 24, height: 2, left: 0, top: 11, coverage: vec![255; 48] }));
        face.insert('\u{2502}', Some(AlphaGlyph { width: 2, height: 24, left: 11, top: 0, coverage: vec![255; 48] }));
        let set = FaceSet::new(face);
        assert!(!set.box_lines().1);
        let mut frame = Frame::new(36, 24);
        frame.clear(Rgb(0, 0, 0));
        for (col, cluster) in ["\u{251C}", "\u{2500}", "\u{2524}"].into_iter().enumerate() {
            assert_eq!(set.draw_cluster_in(&mut frame, cluster, col as i32 * 12, 0, 1, Pen::new(WHITE)), 1);
        }
        let row = (0..24).find(|&y| frame.get(18, y) == Some(WHITE)).expect("a line across the middle cell");
        assert!((6..30).all(|x| frame.get(x, row) == Some(WHITE)));
        assert!((0..24).all(|y| frame.get(5, y) == Some(WHITE) && frame.get(29, y) == Some(WHITE)));

        // A face with lines that reach the edges of its cell keeps them.
        let mut face = OutlineFace::new("own lines", 24, (12, 24), 19);
        face.insert('\u{2500}', Some(AlphaGlyph { width: 14, height: 3, left: -1, top: 10, coverage: vec![200; 42] }));
        face.insert('\u{2502}', Some(AlphaGlyph { width: 3, height: 24, left: 4, top: 0, coverage: vec![200; 72] }));
        let set = FaceSet::new(face);
        assert_eq!(set.box_lines(), (Lines { w: 12, h: 24, vx: 4, tv: 3, hy: 10, th: 3 }, true));
        let own = rows_of_any(&set, "\u{2500}");
        assert!(own.iter().enumerate().all(|(y, row)| if (10..13).contains(&y) { row == "############" } else { blank(row) }));
        // The corner it lacks is drawn to match: three pixels thick, in the same place.
        let corner = rows_of_any(&set, "\u{250C}");
        assert_eq!(&corner[9..14], &["............", "....########", "....########", "....########", "....###....."]);
    }

    /// Like `rows`, for a pen whose ink is not full strength: any pixel
    /// that is not ground counts.
    fn rows_of_any(set: &FaceSet, cluster: &str) -> Vec<String> {
        let (cw, ch) = set.cell();
        let mut frame = Frame::new(cw as usize, ch as usize);
        frame.clear(Rgb(0, 0, 0));
        set.draw_cluster(&mut frame, cluster, 0, 0, Pen::new(WHITE));
        (0..ch as i32).map(|y| (0..cw as i32).map(|x| if frame.get(x, y) != Some(Rgb(0, 0, 0)) { '#' } else { '.' }).collect()).collect()
    }

    #[test]
    fn a_face_set_can_be_shared_between_threads() {
        fn shared<T: Send + Sync>(_: &T) {}
        let set = FaceSet::new(gallant());
        shared(&set);
        let pictures: Vec<Vec<String>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4).map(|_| scope.spawn(|| rows(&set, "l\u{0313}"))).collect();
            jobs.into_iter().map(|job| job.join().expect("no panic while drawing")).collect()
        });
        assert!(pictures.iter().all(|p| *p == pictures[0]));
        assert_eq!(kept_cells(&set), 1);
    }

    #[test]
    fn bold_stays_inside_the_cell_with_and_without_marks() {
        let set = FaceSet::new(gallant());
        for cluster in ["m", "m\u{0313}", "\u{2614}", "\u{0E01}"] {
            let cells = set.cluster_cells(cluster) as i32;
            let mut frame = Frame::new(60, 30);
            frame.clear(Rgb(0, 0, 0));
            set.draw_cluster(&mut frame, cluster, 12, 4, Pen::new(WHITE).bold(true));
            let inside: usize = (4..26).map(|y| (12..12 + 12 * cells).filter(|&x| frame.get(x, y) == Some(WHITE)).count()).sum();
            let all = frame.pixels().iter().filter(|&&p| p != 0).count();
            assert_eq!(inside, all, "{cluster:?}");
        }
        let plain = rows(&set, "m\u{0313}").concat().matches('#').count();
        let mut frame = Frame::new(12, 22);
        frame.clear(Rgb(0, 0, 0));
        set.draw_cluster(&mut frame, "m\u{0313}", 0, 0, Pen::new(WHITE).bold(true));
        assert!(lit(&frame, 0, 12) > plain);
    }

    #[test]
    fn underline_and_strike_follow_the_face() {
        let set = FaceSet::new(gallant());
        let mut frame = Frame::new(80, 50);
        frame.clear(Rgb(0, 0, 0));
        set.underline(&mut frame, 4, 2, 3, Pen::new(WHITE));
        // Gallant's underscore is rows 18 and 19.
        for y in 0..50 {
            let n = (0..80).filter(|&x| frame.get(x, y) == Some(WHITE)).count();
            assert_eq!(n, if y == 20 || y == 21 { 36 } else { 0 }, "row {y}");
        }
        assert_eq!(frame.get(3, 20), Some(Rgb(0, 0, 0)));
        assert_eq!(frame.get(40, 20), Some(Rgb(0, 0, 0)));

        frame.clear(Rgb(0, 0, 0));
        set.strike(&mut frame, 0, 0, 2, Pen::new(WHITE).scale(2));
        // Rows 11 and 12, the height of the hyphen, doubled.
        for y in 0..50 {
            let n = (0..80).filter(|&x| frame.get(x, y) == Some(WHITE)).count();
            assert_eq!(n, if (22..26).contains(&y) { 48 } else { 0 }, "row {y}");
        }
        assert_eq!(gallant().glyph('-').unwrap().ink_rows(), Some((11, 12)));

        for &(x, y) in &[(i32::MAX, i32::MAX), (i32::MIN, i32::MIN), (-5, 45), (70, -30)] {
            set.underline(&mut frame, x, y, u32::MAX, Pen::new(WHITE).scale(u32::MAX));
            set.strike(&mut frame, x, y, 0, Pen::new(WHITE).scale(0));
        }
    }

    /// One of Gallant's glyphs handed over the way a rasterizer would: a
    /// tight box of coverage with an offset into the cell.
    fn rasterized(ch: char) -> Option<AlphaGlyph> {
        let g = gallant().glyph(ch)?;
        let mut canvas = Canvas::new(g.width, 22);
        canvas.stamp_bits(g, 0, 0, 1);
        Some(match (canvas.ink_rows(), canvas.ink_cols()) {
            (Some((r0, r1)), Some((c0, c1))) => {
                let coverage = (r0..=r1).flat_map(|y| (c0..=c1).map(move |x| (x, y))).map(|(x, y)| canvas.at(x, y)).collect();
                AlphaGlyph { width: (c1 - c0 + 1) as u32, height: (r1 - r0 + 1) as u32, left: c0, top: r0, coverage }
            }
            _ => AlphaGlyph::default(),
        })
    }

    fn gallant_as_outline(text: &str) -> FaceSet {
        let mut set = FaceSet::new(OutlineFace::new("Gallant, rasterized", 22, (12, 22), 17));
        assert!(set.fill(text, rasterized) > 0);
        assert!(set.missing(text).is_empty());
        assert_eq!(set.fill(text, |_| panic!("asked twice")), 0);
        set
    }

    #[test]
    fn an_outline_face_draws_what_the_same_glyphs_draw_as_a_bitmap() {
        let text = "mitsqanaqan\u{0313} l\u{0313}k\u{0313}t\u{0313}i\u{0313}j\u{0313}f\u{0313}h\u{0313}d\u{0313} e\u{0302}\u{0301} \
                    \u{0161}\u{0313} K\u{0313} g\u{0323} l\u{0335} \u{2614}\u{30F3} \u{2014}\u{201C}\u{2019} \u{2502}\u{0301} \u{FFFD} a\u{00A0}b \u{2011}";
        let outline = gallant_as_outline(text);
        let bitmap = FaceSet::new(gallant());
        assert_eq!((outline.cell(), outline.ascent(), outline.active().x_top()), ((12, 22), 17, 7));
        assert_eq!(outline.active().underline_rows(), (18, 2));
        assert_eq!(outline.active().strike_rows(), (11, 2));
        for pen in [Pen::new(WHITE), Pen::new(WHITE).scale(2), Pen::new(WHITE).bold(true), Pen::new(WHITE).alpha(128)] {
            for cluster in cluster::clusters(text) {
                let mut a = Frame::new(60, 50);
                let mut b = Frame::new(60, 50);
                a.clear(Rgb(0, 0, 0));
                b.clear(Rgb(0, 0, 0));
                let n = outline.draw_cluster(&mut a, cluster, 3, 2, pen);
                assert_eq!(n, bitmap.draw_cluster(&mut b, cluster, 3, 2, pen), "{cluster:?}");
                assert!(a.pixels() == b.pixels(), "{cluster:?} with {pen:?}");
            }
        }
    }

    #[test]
    fn an_outline_glyph_is_cut_to_its_cell_and_a_missing_one_is_borrowed_or_boxed() {
        let mut face = OutlineFace::new("f", 22, (12, 22), 17);
        // A glyph that sprawls over every neighbour.
        face.insert('W', Some(AlphaGlyph { width: 40, height: 60, left: -10, top: -20, coverage: vec![200; 2400] }));
        // A combining mark the rasterizer hung back over the cell before.
        face.insert('\u{0301}', Some(AlphaGlyph { width: 3, height: 2, left: -8, top: 1, coverage: vec![255; 6] }));
        face.insert('o', Some(AlphaGlyph { width: 6, height: 8, left: 3, top: 9, coverage: vec![255; 48] }));
        face.insert('q', None);
        let set = FaceSet::new(face);
        let pen = Pen::new(WHITE);

        let mut frame = Frame::new(60, 60);
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster(&mut frame, "W", 24, 19, pen), 2);
        for y in 0..60 {
            for x in 0..60 {
                let inside = (24..48).contains(&x) && (19..41).contains(&y);
                assert_eq!(frame.get(x, y) != Some(Rgb(0, 0, 0)), inside, "{x},{y}");
            }
        }

        // The mark lands over the o, in the o's cell, above it.
        let got = rows(&set, "o\u{0301}");
        assert!(got[..9].iter().any(|r| !blank(r)));
        assert_eq!(&got[9..], &rows(&set, "o")[9..]);
        let mark_cols: Vec<usize> = (0..12).filter(|&x| got[..8].iter().any(|r| r.as_bytes()[x] == b'#')).collect();
        assert_eq!(mark_cols, vec![4, 5, 6]);

        // q is known to be lacking, z was never asked for: both come from Gallant.
        assert_eq!(rows(&set, "q"), rows(&FaceSet::new(gallant()), "q"));
        assert_eq!(rows(&set, "z"), rows(&FaceSet::new(gallant()), "z"));
        assert!(rows(&set, "\u{0E01}").iter().any(|r| !blank(r)));
    }

    #[test]
    fn a_big_outline_cell_borrows_gallant_at_a_whole_multiple() {
        let set = FaceSet::new(OutlineFace::new("f", 44, (26, 50), 38));
        assert!(set.fallback_fits());
        let got = rows(&set, "H");
        let own = rows(&FaceSet::new(gallant()), "H");
        // Twice the size, centred, standing on the big cell's baseline (row 37).
        assert!(blank(&got[38]) && !blank(&got[37]));
        assert_eq!(got[37], format!(".{}.", own[16].chars().flat_map(|c| [c, c]).collect::<String>()));
        assert_eq!(top(&got), 38 - 17 * 2 + 3 * 2);
        // Marks keep proportion: two clear rows under the comma.
        let marked = rows(&set, "n\u{0313}");
        let n_top = 38 - 34 + 14;
        assert!(blank(&marked[n_top - 1]) && blank(&marked[n_top - 2]) && !blank(&marked[n_top - 3]));
    }

    #[test]
    fn an_outline_mark_the_host_drew_nothing_for_is_borrowed_from_gallant() {
        // GDI draws a combining mark to the left of its origin; a host that
        // cuts every glyph to its cell hands back a glyph with nothing in it.
        let host = |ch: char| match ch {
            '\u{0313}' | '\u{0301}' => Some(AlphaGlyph::default()),
            '\u{0323}' => Some(AlphaGlyph { width: 3, height: 3, left: 4, top: 18, coverage: vec![0; 9] }),
            _ => rasterized(ch),
        };
        for cell in [(12, 22), (10, 22), (14, 26)] {
            let mut set = FaceSet::new(OutlineFace::new("f", 22, cell, 17));
            set.fill("n\u{0313} q\u{0301}\u{0323}", host);
            for mark in ['\u{0313}', '\u{0301}', '\u{0323}'] {
                let face = set.active().as_outline().unwrap();
                assert!(face.knows(mark) && !face.has(mark), "U+{:04X}", mark as u32);
            }
            // The space is a glyph with no ink too, and is kept as one.
            assert!(set.active().has(' '));

            let (plain, marked) = (rows(&set, "n"), rows(&set, "n\u{0313}"));
            assert_eq!(&marked[7..], &plain[7..], "{cell:?}");
            let above: usize = marked[..6].iter().map(|r| r.matches('#').count()).sum();
            assert_eq!(above, 13, "{cell:?}: the whole comma, from Gallant");
            assert!(blank(&marked[6]), "{cell:?}");
            // A dot below and an acute above, both borrowed, on a letter of the face's own.
            let q = rows(&set, "q\u{0301}\u{0323}");
            assert!(top(&q) < top(&rows(&set, "q")) && !blank(&q[19]), "{cell:?}");
        }
    }

    #[test]
    fn an_outline_faces_own_marks_are_sided_by_where_most_of_their_ink_lies() {
        // Sizes and places as GDI gives them for Courier New 22 pixels tall,
        // filled in solid: its comma above dips two rows into the x-height.
        let block = |width: u32, height: u32, left: i32, top: i32| AlphaGlyph { width, height, left, top, coverage: vec![255; (width * height) as usize] };
        let mut face = OutlineFace::new("Courier New", 22, (12, 22), 17);
        face.insert('n', Some(block(9, 9, 2, 8)));
        face.insert('x', Some(block(10, 9, 1, 8)));
        face.insert('g', Some(block(8, 13, 2, 8)));
        face.insert('\u{0313}', Some(block(4, 6, 3, 4)));
        face.insert('\u{0331}', Some(block(6, 1, 3, 19)));
        face.insert('\u{0327}', Some(block(2, 3, 4, 17)));
        let set = FaceSet::new(face);
        assert_eq!(set.active().x_top(), 8);

        // Lifted whole to leave a clear row over the letter, not struck through it.
        let n = rows(&set, "n\u{0313}");
        assert!(blank(&n[0]) && blank(&n[7]));
        assert!(n[1..7].iter().all(|r| r == "...####....."));
        assert_eq!(&n[8..], &rows(&set, "n")[8..]);
        // A line that would be lost in a solid descender is moved to rest
        // under it. The letter is not cut for it.
        let g = rows(&set, "g\u{0331}");
        assert_eq!(&g[..21], &rows(&set, "g")[..21]);
        assert_eq!(g[21], "...######...");
        // A cedilla starts on the row under the baseline: it is joined on
        // where the face put it, and here that is inside the letter.
        assert_eq!(rows(&set, "g\u{0327}"), rows(&set, "g"));

        assert_eq!(side(4, 9, 8, 16), Side::Above);
        assert_eq!(side(19, 19, 8, 16), Side::Below);
        assert_eq!(side(17, 19, 8, 16), Side::Below);
        assert_eq!(side(10, 11, 8, 16), Side::Through);
        assert_eq!(side(3, 18, 8, 16), Side::Through);
        assert_eq!(side(i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2), Side::Above);
    }

    #[test]
    fn a_mark_below_steps_aside_from_a_stem_and_is_struck_over_a_tail_it_cannot_avoid() {
        let set = FaceSet::new(gallant());

        // Beside a p's stem the line moves two pixels over and the p is whole.
        let (p, lined) = (rows(&set, "p"), rows(&set, "p\u{0331}"));
        assert!(whole(&p, &lined));
        assert_eq!(&lined[19..21], &[".##..######.", ".##..######."]);
        // The face's own y with a dot below sets the example: the dot beside the tail.
        assert_eq!(&rows(&set, "\u{1EF5}")[20], ".####.##....");

        // Under a y there is nowhere to go. The line is struck over the
        // tail, as an underscore is on a typewriter, and the y is still a y.
        let (y, lined) = (rows(&set, "y"), rows(&set, "y\u{0331}"));
        assert!(whole(&y, &lined));
        assert_eq!(&lined[17..], &[".....#......", "....##......", "...######...", ".########...", ".###........"]);
        assert_ne!(&lined[17..], &rows(&set, "v\u{0331}")[17..]);

        // A cedilla grows out of the letter, so it stays where it joins.
        let (v, tailed) = (rows(&set, "v"), rows(&set, "v\u{0327}"));
        assert!(whole(&v, &tailed));
        assert_eq!(&tailed[17..], &rows(&set, "\u{0327}")[17..]);
        // A line under a letter with no descender is not moved at all.
        assert_eq!(&rows(&set, "x\u{0331}")[17..], &rows(&set, "\u{0331}")[17..]);
    }

    #[test]
    fn no_mark_below_costs_a_letter_any_of_its_ink() {
        let below: Vec<char> = (0x0316..=0x0319u32)
            .chain(0x031C..=0x0320)
            .chain(0x0323..=0x0333)
            .chain([0x0339, 0x033A, 0x033B, 0x033C, 0x0345, 0x0347, 0x0348, 0x0349, 0x034D, 0x034E, 0x0353, 0x0354, 0x0355, 0x0356, 0x0359, 0x035A, 0x035C, 0x035F, 0x0362])
            .filter_map(char::from_u32)
            .collect();
        for face in [gallant(), spleen(), terminus()] {
            let set = FaceSet::new(face);
            // What a mark that cannot be drawn in this face looks like: the
            // square over the letter, which is not a mark below.
            let stand_in = rows(&set, " \u{1DC4}");
            for letter in ['g', 'j', 'p', 'q', 'y', 'Q', ',', ';', '(', '_'] {
                let plain = rows(&set, &letter.to_string());
                for &mark in &below {
                    // A letter and mark with one glyph between them are the face's business.
                    if compose(letter, mark).is_some_and(|joined| face.has(joined) || gallant().has(joined)) {
                        continue;
                    }
                    if rows(&set, &format!(" {mark}")) == stand_in {
                        continue;
                    }
                    let got = rows(&set, &format!("{letter}{mark}"));
                    assert!(whole(&plain, &got), "{}: {letter} under U+{:04X} lost ink", face.name(), mark as u32);
                    assert_ne!(got, plain, "{}: U+{:04X} under {letter} does not show", face.name(), mark as u32);
                }
            }
        }
    }

    #[test]
    fn a_small_mark_below_finds_clear_ground_inside_or_beside_a_tail() {
        // Spleen's g has an open tail: the dot floats inside it, whole.
        let set = FaceSet::new(spleen());
        let (g, dotted) = (rows(&set, "g"), rows(&set, "g\u{0323}"));
        assert!(whole(&g, &dotted));
        assert_eq!(&dotted[19..], &["........##..", ".....##..##.", ".....##..##.", "........##..", "..#######..."]);
        // Its j has a foot: the dot, a row squeezed out of it, goes beside
        // that, as Gallant's own does beside the tail of its y.
        let dotted = rows(&set, "j\u{0323}");
        assert_eq!(&dotted[22..], &["....##......", ".####..##..."]);
        // In Terminus the tail of the y runs where the line would: the line
        // has a row squeezed out and is drawn inside the tail, so that
        // neither is taken for the other.
        let set = FaceSet::new(terminus());
        let (y, lined) = (rows(&set, "y"), rows(&set, "y\u{0331}"));
        assert!(whole(&y, &lined));
        assert_eq!(&lined[17..], &["........#..", "######..#..", ".......#...", "..#####....", "..........."]);
    }

    #[test]
    fn a_mark_hidden_altogether_has_the_base_opened_under_it() {
        // A full block has no ground anywhere for a dot below to show on.
        let set = FaceSet::new(gallant());
        let got = rows(&set, "\u{2588}\u{0323}");
        assert_eq!(&got[..18], &rows(&set, "\u{2588}")[..18]);
        assert_eq!(&got[18..], &["####....####", "####.##.####", "####.##.####", "####....####"]);
    }

    #[test]
    fn an_accented_capital_makes_room_for_a_second_mark_and_keeps_its_accent() {
        for face in [gallant(), spleen(), terminus()] {
            let set = FaceSet::new(face);
            let x_top = face.x_top() as usize;
            for (capital, bare) in [('\u{0160}', 'S'), ('\u{010C}', 'C'), ('\u{017D}', 'Z')] {
                // The caron is whatever the accented capital has above the bare one's head.
                let (accented, plain) = (rows(&set, &capital.to_string()), rows(&set, &bare.to_string()));
                let caron: Vec<String> = accented[..top(&plain)].iter().filter(|r| !blank(r)).cloned().collect();
                assert_eq!(caron.len(), 3, "{} {capital}: a caron of three rows", face.name());
                for mark in ['\u{0313}', '\u{0301}'] {
                    let got = rows(&set, &format!("{capital}{mark}"));
                    // The new mark at the top, then the caron with every pixel it had.
                    assert_eq!(top(&got), 0, "{} {capital}", face.name());
                    let at = (1..got.len() - 3).find(|&r| whole(&caron, &got[r..r + 3]));
                    let at = at.unwrap_or_else(|| panic!("{} {capital} under U+{:04X}: the caron is gone\n{}", face.name(), mark as u32, got.join("\n")));
                    // The capital is shorter, and still stands taller than a lower-case letter.
                    let head = (at + 3..got.len()).find(|&r| !blank(&got[r])).expect("the letter");
                    assert!(head < x_top, "{} {capital}: its head is at row {head}", face.name());
                    // Its foot has not moved, and nothing has slipped under the baseline.
                    let base = face.ascent() as usize - 1;
                    assert_eq!(got[base], accented[base], "{} {capital}", face.name());
                    assert!(got[base + 1..].iter().all(|r| blank(r)));
                }
            }
        }
        // S, caron and acute typed one after another are the same picture.
        let set = FaceSet::new(gallant());
        assert_eq!(rows(&set, "S\u{030C}\u{0301}"), rows(&set, "\u{0160}\u{0301}"));
        assert_eq!(
            &rows(&set, "\u{0160}\u{0313}")[..7],
            &["....####....", "......##....", "....###.....", "...##...##..", "....##.##...", ".....###....", "...########."]
        );
    }

    #[test]
    fn only_what_stands_above_the_x_height_is_shortened_for_a_pile_of_marks() {
        let set = FaceSet::new(gallant());
        // A hyphen under two marks keeps both its rows: the marks make room
        // among themselves, the comma giving up its doubled row.
        let got = rows(&set, "-\u{0313}\u{0301}");
        assert_eq!(&got[..6], &["......##....", ".....##.....", "....##......", "....####....", "......##....", "....###....."]);
        assert_eq!(&got[6..], &rows(&set, "-")[6..]);
        // A digit stands as tall as a capital and gives rows as one does.
        let got = rows(&set, "1\u{0313}\u{0301}");
        assert_eq!(&got[..6], &rows(&set, "-\u{0313}\u{0301}")[..6]);
        assert_eq!(top(&got[6..]), 0, "the 1 starts right under the comma");
        assert_eq!(got[16], rows(&set, "1")[16]);
    }

    #[test]
    fn a_later_mark_never_cuts_an_earlier_one() {
        // Three marks above a capital are more than the cell has rows for.
        // The comma and the acute run into each other; neither is erased.
        let set = FaceSet::new(gallant());
        let two = rows(&set, "\u{0160}\u{0313}");
        let three = rows(&set, "\u{0160}\u{0313}\u{0301}");
        let ink = |p: &[String]| p.iter().map(|r| r.matches('#').count()).sum::<usize>();
        assert!(ink(&three[..8]) >= ink(&two[..7]), "the comma or the caron lost ink to the acute");
        assert_ne!(three, rows(&set, "\u{0160}\u{0301}"));
    }

    #[test]
    fn a_mark_on_a_boxed_character_is_drawn_and_the_box_makes_room() {
        let set = FaceSet::new(gallant());
        let boxed = rows(&set, "\u{0E01}");
        assert_eq!((top(&boxed), boxed[2].as_str(), boxed[16].as_str()), (2, "###########.", "###########."));
        // An acute the face has: drawn at the top, the box lowered whole under it.
        let got = rows(&set, "\u{0E01}\u{0301}");
        assert_eq!(&got[..4], &["......##....", ".....##.....", "....##......", "............"]);
        assert_eq!(&got[4..19], &boxed[2..17]);
        // A dot below: under the box, which stays where it was.
        let got = rows(&set, "\u{0E01}\u{0323}");
        assert_eq!(&got[..17], &boxed[..17]);
        assert_eq!(&got[17..], &rows(&set, "\u{0323}")[17..]);
        // A mark no face has, and one that would strike out the digits: the
        // small square that stands for a mark, and the box under it.
        for mark in ['\u{0E34}', '\u{0335}'] {
            let got = rows(&set, &format!("\u{0E01}{mark}"));
            assert_eq!(&got[..4], &["....###.....", "....#.#.....", "....###.....", "............"], "U+{:04X}", mark as u32);
            assert_eq!(&got[4..19], &boxed[2..17]);
        }
        // Marks above and below leave room for the digits alone, and they are all there.
        let got = rows(&set, "\u{0E01}\u{0301}\u{0323}");
        assert_eq!(&got[6..11], &["..###.###...", "..#.#.#.....", "..#.#.##....", "..#.#.#.....", "..###.###..."]);
        // The digits of a box are never struck twice, whatever rides on it.
        let drawn = |bold: bool| {
            let mut frame = Frame::new(12, 22);
            frame.clear(Rgb(0, 0, 0));
            set.draw_cluster(&mut frame, "\u{0E01}\u{0301}", 0, 0, Pen::new(WHITE).bold(bold));
            frame
        };
        assert!(drawn(true).pixels() == drawn(false).pixels());
        // A control code and a character with no picture are boxes too, and keep their marks.
        for base in ['\u{0007}', '\u{200B}', '\u{1F600}'] {
            assert_ne!(rows(&set, &format!("{base}\u{0313}")), rows(&set, &base.to_string()), "U+{:04X}", base as u32);
        }
        // Given two cells by the layout, the box fills both and the mark sits over the middle.
        let mut frame = Frame::new(24, 22);
        frame.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster_in(&mut frame, "\u{1F600}\u{0301}", 0, 0, 2, Pen::new(WHITE)), 1);
        assert!(lit(&frame, 0, 4) > 0 && lit(&frame, 20, 24) > 0);
        assert_eq!(frame.get(13, 0), Some(WHITE));
    }

    #[test]
    fn a_mark_with_nothing_before_it_is_a_mark_on_nothing() {
        // What the host hands over for a combining acute: ink left of the origin.
        let mut face = OutlineFace::new("f", 22, (12, 22), 17);
        face.insert('\u{0301}', Some(AlphaGlyph { width: 3, height: 2, left: -8, top: 1, coverage: vec![255; 6] }));
        let set = FaceSet::new(face);
        let alone = rows(&set, "\u{0301}");
        assert_eq!(&alone[1..3], &["....###.....", "....###....."]);
        assert_eq!(alone, rows(&set, " \u{0301}"));
        // Terminus has no comma above: alone, it is borrowed as it is on a letter.
        let set = FaceSet::new(terminus());
        assert_eq!(&rows(&set, "\u{0313}")[..4], &["...####....", "...####....", ".....##....", "...###....."]);
        // A mark no face can draw is a box alone, which says which mark it is.
        let set = FaceSet::new(gallant());
        let mut boxed = Frame::new(12, 22);
        boxed.clear(Rgb(0, 0, 0));
        set.draw_hex_box(&mut boxed, 0x1DC4, 0, 0, 1, Pen::new(WHITE));
        let mut got = Frame::new(12, 22);
        got.clear(Rgb(0, 0, 0));
        assert_eq!(set.draw_cluster(&mut got, "\u{1DC4}", 0, 0, Pen::new(WHITE)), 1);
        assert!(got.pixels() == boxed.pixels());
        // Further marks after the first stack on it; five are held to four.
        assert_ne!(rows(&set, "\u{0313}\u{0301}"), rows(&set, "\u{0313}"));
        assert_eq!(rows(&set, "\u{0313}\u{0301}\u{0323}\u{0331}\u{0308}"), rows(&set, "\u{0313}\u{0301}\u{0323}\u{0331}"));
    }

    #[test]
    fn a_space_no_face_has_is_still_a_space() {
        let face = BitmapFont::from_hex("no space", &format!("0041:{}\n", "18".repeat(16))).unwrap();
        let set = FaceSet::new(face);
        assert!(!set.fallback_fits() && !set.active().has(' '));
        for space in [" ", "\u{2003}", "\u{3000}"] {
            assert!(rows(&set, space).iter().all(|r| blank(r)), "{space:?}");
        }
        assert!(rows(&set, "B").iter().any(|r| !blank(r)));
    }

    #[test]
    fn a_faint_mark_is_placed_like_any_other() {
        // Every pixel of the accent is under the coverage that counts as
        // ink in a strong glyph. It is measured by its own strength.
        let block = |width: u32, height: u32, left: i32, top: i32, v: u8| AlphaGlyph { width, height, left, top, coverage: vec![v; (width * height) as usize] };
        let mut face = OutlineFace::new("light", 22, (12, 22), 17);
        face.insert('l', Some(block(2, 15, 5, 2, 255)));
        face.insert('x', Some(block(8, 10, 2, 7, 255)));
        face.insert('\u{0301}', Some(block(4, 3, 4, 1, 60)));
        let set = FaceSet::new(face);
        let got = rows_of_any(&set, "l\u{0301}");
        // The accent at the top of the cell, a clear row, and the stem shortened for it.
        assert_eq!(&got[..5], &["....####....", "....####....", "....####....", "............", ".....##....."]);
        // A whole face that light keeps its marks off its letters as well.
        let mut face = OutlineFace::new("lighter", 22, (12, 22), 17);
        face.insert('l', Some(block(2, 15, 5, 2, 50)));
        face.insert('x', Some(block(8, 10, 2, 7, 50)));
        face.insert('\u{0301}', Some(block(4, 3, 4, 1, 40)));
        assert_eq!(rows_of_any(&FaceSet::new(face), "l\u{0301}"), got);
    }

    #[test]
    fn a_borrowed_letter_is_struck_twice_by_one_pixel_of_the_cell_it_is_in() {
        // Twice Gallant's size: every borrowed pixel is two by two.
        let mut set = FaceSet::new(OutlineFace::new("twice", 44, (24, 44), 34));
        let bold = |set: &FaceSet, cluster: &str| {
            let mut frame = Frame::new(24, 44);
            frame.clear(Rgb(0, 0, 0));
            set.draw_cluster(&mut frame, cluster, 0, 0, Pen::new(WHITE).bold(true));
            (0..44).map(|y| (0..24).map(|x| if frame.get(x, y) == Some(WHITE) { '#' } else { '.' }).collect::<String>()).collect::<Vec<String>>()
        };
        // The stem of the l is four pixels wide plain and five struck twice: one more, not two.
        let widest = |p: &[String]| p[20].matches('#').count();
        assert_eq!((widest(&rows(&set, "l")), widest(&bold(&set, "l"))), (4, 5));
        assert_eq!(&bold(&set, "n\u{0313}")[14..], &bold(&set, "n")[14..]);
        // The host says a strike is two pixels at this size, and it is, with and without a mark.
        set.active_mut().as_outline_mut().expect("an outline face").set_strike(2);
        assert_eq!(widest(&bold(&set, "l")), 6);
        assert_eq!(&bold(&set, "n\u{0313}")[14..], &bold(&set, "n")[14..]);
    }

    #[test]
    fn a_soft_hyphen_riding_on_a_letter_is_not_struck_through_it() {
        let set = FaceSet::new(gallant());
        assert!(gallant().has('\u{00AD}'));
        assert_eq!(rows(&set, "a\u{00AD}"), rows(&set, "a"));
        assert_eq!(rows(&set, "a\u{00AD}\u{0313}"), rows(&set, "a\u{0313}"));
        // In a cell of its own it is the face's picture of it.
        assert!(rows(&set, "\u{00AD}").iter().any(|r| !blank(r)));
    }

    fn kept_cells(set: &FaceSet) -> usize {
        set.kept().cells.len()
    }

    #[test]
    fn put_together_cells_are_kept_and_draw_the_same_again() {
        let mut set = FaceSet::new(gallant());
        let first = rows(&set, "n\u{0313}");
        assert_eq!(kept_cells(&set), 1);
        assert_eq!(rows(&set, "n\u{0313}"), first);
        assert_eq!(kept_cells(&set), 1);
        // The same picture by another road is the same entry: marks past the
        // fourth, characters with no picture, a letter that joined its accent.
        rows(&set, &format!("n\u{0313}{}", "\u{200B}".repeat(40)));
        rows(&set, "s\u{030C}\u{0313}");
        rows(&set, "\u{0161}\u{0313}");
        rows(&set, &format!("\u{0161}\u{0313}\u{200D}{}", "\u{034F}".repeat(9)));
        assert_eq!(kept_cells(&set), 2);
        // Plain letters and precomposed ones are drawn straight from the face.
        rows(&set, "n");
        rows(&set, "e\u{0301}");
        rows(&set, "\u{0E01}");
        assert_eq!(kept_cells(&set), 2);

        // Double strike is another picture; scale, colour and place are not.
        let mut frame = Frame::new(40, 70);
        for pen in [Pen::new(WHITE).bold(true), Pen::new(WHITE).scale(3), Pen::new(Rgb(9, 9, 9)).alpha(100).bold(true)] {
            set.draw_cluster(&mut frame, "n\u{0313}", 2, 1, pen);
        }
        assert_eq!(kept_cells(&set), 3);

        // A copy starts with nothing kept and draws the same.
        let copy = set.clone();
        assert_eq!(kept_cells(&copy), 0);
        assert_eq!(rows(&copy, "n\u{0313}"), first);
        // Changing the face drops what was kept for the old one.
        set.set_active(spleen());
        assert_eq!(kept_cells(&set), 0);
        assert_eq!(rows(&set, "n\u{0313}"), rows(&FaceSet::new(spleen()), "n\u{0313}"));
        assert_eq!(kept_cells(&set), 1);
        assert_eq!(set.active_mut().name(), "Spleen");
        assert_eq!(kept_cells(&set), 0);
    }

    #[test]
    fn filling_an_outline_face_drops_cells_put_together_without_the_new_glyphs() {
        let mut set = FaceSet::new(OutlineFace::new("f", 22, (12, 22), 17));
        assert_eq!(set.fill("n", rasterized), 3);
        // The comma has not been asked for, so it is borrowed, which for
        // this cell is the same picture; what matters is that it is kept.
        let before = rows(&set, "n\u{0313}");
        assert_eq!(kept_cells(&set), 1);
        assert_eq!(set.fill("n", |_| panic!("already there")), 0);
        assert_eq!(kept_cells(&set), 1);
        // Now the face gets a comma of its own, a different one.
        let own = AlphaGlyph { width: 2, height: 2, left: 5, top: 1, coverage: vec![255; 4] };
        assert_eq!(set.fill("n\u{0313}", |ch| (ch == '\u{0313}').then(|| own.clone())), 1);
        assert_eq!(kept_cells(&set), 0);
        let after = rows(&set, "n\u{0313}");
        assert_ne!(after, before);
        assert_eq!(&after[4..6], &[".....##.....", ".....##....."]);
        // A bitmap face has nothing to fill.
        assert_eq!(FaceSet::new(gallant()).fill("anything", |_| panic!("not asked")), 0);
    }

    #[test]
    fn the_kept_cells_are_bounded_in_number_and_in_bytes() {
        // More different put-together cells than are kept: one letter under
        // every pair of marks from the combining block.
        let set = FaceSet::new(gallant());
        let fresh = FaceSet::new(gallant());
        let marks: Vec<char> = (0x0300..=0x036Fu32).filter_map(char::from_u32).collect();
        let mut frame = Frame::new(12, 22);
        let (mut drawn, mut most) = (0, 0);
        for &first in &marks[..40] {
            for &second in &marks {
                let cluster: String = ['x', first, second].iter().collect();
                set.draw_cluster(&mut frame, &cluster, 0, 0, Pen::new(WHITE));
                drawn += 1;
                most = most.max(kept_cells(&set));
                assert!(kept_cells(&set) <= CELLS_KEPT);
                if drawn % 997 == 0 {
                    assert_eq!(rows(&set, &cluster), rows(&fresh, &cluster), "{cluster:?}");
                }
            }
        }
        assert!(drawn > CELLS_KEPT && most == CELLS_KEPT, "{drawn} drawn, {most} kept at most");
        let kept = set.kept();
        assert_eq!(kept.bytes, kept.cells.values().map(|c| c.cov.len()).sum::<usize>());
        drop(kept);

        // A cell so large that the bytes run out long before the count.
        let big = FaceSet::new(OutlineFace::new("big", 256, (256, 256), 200));
        let mut frame = Frame::new(16, 16);
        for &mark in &marks {
            for bold in [false, true] {
                big.draw_cluster(&mut frame, &format!(" {mark}"), 0, 0, Pen::new(WHITE).bold(bold));
                let kept = big.kept();
                assert!(kept.bytes <= BYTES_KEPT && kept.cells.len() <= BYTES_KEPT / (256 * 256));
            }
        }
        assert!(kept_cells(&big) > 0);
    }

    #[test]
    fn nothing_is_ever_drawn_outside_the_cells_given() {
        let clusters = [
            "n\u{0313}",
            "l\u{0313}\u{0301}\u{0302}\u{0303}",
            "S\u{030C}\u{0301}",
            "\u{2502}\u{0301}",
            "\u{2588}\u{0313}\u{0323}",
            "\u{FFFD}\u{0301}",
            "\u{0313}",
            "\u{2614}\u{0301}",
            "\u{1F600}",
            "g\u{0323}\u{0331}\u{0328}",
            "W",
            "\u{0007}",
        ];
        for face in [gallant(), spleen(), terminus()] {
            let set = FaceSet::new(face);
            let (cw, ch) = (set.cell().0 as i32, set.cell().1 as i32);
            for cluster in clusters {
                for scale in 1..=3 {
                    for cells in 0..=2u32 {
                        for bold in [false, true] {
                            let mut frame = Frame::new(240, 200);
                            frame.clear(Rgb(0, 0, 0));
                            let n = set.draw_cluster_in(&mut frame, cluster, 60, 50, cells, Pen::new(WHITE).scale(scale as u32).bold(bold));
                            let slot = if cells == 0 { n } else { cells } as i32;
                            for y in 0..200 {
                                for x in 0..240 {
                                    let inside = (60..60 + slot * cw * scale).contains(&x) && (50..50 + ch * scale).contains(&y);
                                    assert!(inside || frame.get(x, y) == Some(Rgb(0, 0, 0)), "{} {cluster:?} x{scale} leaks at {x},{y}", face.name());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn canvas_reads_and_writes_are_safe_anywhere() {
        let mut c = Canvas::new(0, 0);
        assert_eq!((c.w, c.h), (1, 1));
        let mut c2 = Canvas::new(u32::MAX, u32::MAX);
        c2.put(i64::MAX, i64::MIN, 9);
        c.put(-1, 0, 9);
        c.put(0, 0, 200);
        assert_eq!((c.at(0, 0), c.at(-1, 0), c.at(0, 1), c.at(i32::MAX, i32::MIN)), (200, 0, 0, 0));
        assert!(c.row(-1).is_empty() && c.row(1).is_empty());
        c.close_up_from_above(-3);
        c.close_up_from_below(7);
        let moved = c.shifted(i32::MAX, i32::MIN);
        assert_eq!(moved.at(0, 0), 0);
        c.stamp_alpha(&AlphaGlyph { width: 2, height: 2, left: i32::MIN, top: i32::MAX, coverage: vec![255; 4] }, i64::MAX / 4, i64::MIN / 4);
        c.stamp_bits(Glyph { width: 32, rows: &[u32::MAX; 3] }, -1_000_000, 1_000_000, 3);
        assert_eq!(c.at(0, 0), 200);
    }
}
