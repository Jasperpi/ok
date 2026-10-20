//! Box drawing and block elements (U+2500 to U+259F) drawn in code.
//!
//! A face's own glyphs are used when it has them and its lines reach the
//! edges of the cell, so that boxes join. Otherwise the characters are drawn
//! here, at whatever size the cell is, with the thickness and place of the
//! face's own `─` and `│` when it has those: lines always meet at the cell
//! edges, and a face that has half the set gets the other half in its own
//! weight instead of a row of hex boxes.

/// True for the characters this module can draw.
pub(crate) fn covers(ch: char) -> bool {
    (0x2500..=0x259F).contains(&(ch as u32))
}

/// Where lines run in a cell: the columns of a light vertical line and the
/// rows of a light horizontal one. Heavy and double lines are built round
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lines {
    pub w: i32,
    pub h: i32,
    /// First column and thickness of `│`.
    pub vx: i32,
    pub tv: i32,
    /// First row and thickness of `─`.
    pub hy: i32,
    pub th: i32,
}

impl Lines {
    /// The lines of a `w` by `h` cell. `across` is the first row and the
    /// thickness of the face's `─` and `down` the first column and the
    /// thickness of its `│`, where it has them; a line that is missing is
    /// `weight` pixels thick and runs through the middle of the cell.
    pub(crate) fn new(w: u32, h: u32, across: Option<(u32, u32)>, down: Option<(u32, u32)>, weight: u32) -> Lines {
        let (w, h) = (w.clamp(1, 4096) as i32, h.clamp(1, 4096) as i32);
        // A line thicker than a third of the cell, or one that runs off it,
        // is no line; the plain one is taken in its place.
        let place = |measured: Option<(u32, u32)>, size: i32| {
            let widest = (size / 3).max(1);
            let sane = measured
                .map(|(first, thick)| (first.min(8192) as i32, thick.min(8192) as i32))
                .filter(|&(first, thick)| thick >= 1 && thick <= widest && first + thick <= size);
            sane.unwrap_or_else(|| {
                let thick = (weight.clamp(1, 8192) as i32).min(widest);
                ((size - thick) / 2, thick)
            })
        };
        let (vx, tv) = place(down, w);
        let (hy, th) = place(across, h);
        Lines { w, h, vx, tv, hy, th }
    }

    /// How far a heavy line reaches past a light one on each side, which is
    /// also the thickness of each stroke of a double line: a double line is
    /// a heavy one with the light one taken out of its middle.
    fn ev(&self) -> i32 {
        (self.tv / 2).max(1)
    }

    fn eh(&self) -> i32 {
        (self.th / 2).max(1)
    }
}

/// The weight of each arm of a line character: 0 none, 1 light, 2 heavy,
/// 3 double.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Arms {
    up: u8,
    right: u8,
    down: u8,
    left: u8,
}

/// The arms of the characters from U+2500 to U+257F, packed two bits an arm
/// as up, right, down, left from the high bits down. Dashed lines are given
/// as their solid forms, arcs as corners; the diagonals have no arms.
const ARMS: [u8; 128] = [
    0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88, 0x14, 0x24, 0x18, 0x28, // 2500
    0x05, 0x06, 0x09, 0x0A, 0x50, 0x60, 0x90, 0xA0, 0x41, 0x42, 0x81, 0x82, 0x54, 0x64, 0x94, 0x58, // 2510
    0x98, 0xA4, 0x68, 0xA8, 0x45, 0x46, 0x85, 0x49, 0x89, 0x86, 0x4A, 0x8A, 0x15, 0x16, 0x25, 0x26, // 2520
    0x19, 0x1A, 0x29, 0x2A, 0x51, 0x52, 0x61, 0x62, 0x91, 0x92, 0xA1, 0xA2, 0x55, 0x56, 0x65, 0x66, // 2530
    0x95, 0x59, 0x99, 0x96, 0xA5, 0x5A, 0x69, 0xA6, 0x6A, 0x9A, 0xA9, 0xAA, 0x11, 0x22, 0x44, 0x88, // 2540
    0x33, 0xCC, 0x34, 0x1C, 0x3C, 0x07, 0x0D, 0x0F, 0x70, 0xD0, 0xF0, 0x43, 0xC1, 0xC3, 0x74, 0xDC, // 2550
    0xFC, 0x47, 0xCD, 0xCF, 0x37, 0x1D, 0x3F, 0x73, 0xD1, 0xF3, 0x77, 0xDD, 0xFF, 0x14, 0x05, 0x41, // 2560
    0x50, 0x00, 0x00, 0x00, 0x01, 0x40, 0x10, 0x04, 0x02, 0x80, 0x20, 0x08, 0x21, 0x48, 0x12, 0x84, // 2570
];

fn arms(index: usize) -> Arms {
    let packed = ARMS[index & 0x7F];
    Arms { up: packed >> 6, right: packed >> 4 & 3, down: packed >> 2 & 3, left: packed & 3 }
}

/// How many dashes a dashed line is broken into, by its place in the block.
fn dashes(index: usize) -> i32 {
    match index {
        0x04..=0x07 => 3,
        0x08..=0x0B => 4,
        0x4C..=0x4F => 2,
        _ => 1,
    }
}

/// Paints `ch` for a cell with the lines `l`, as filled rectangles handed
/// to `fill` as (left, top, right, bottom), the last two not included.
/// Rectangles may reach past the cell; the caller cuts them to it. Returns
/// false, having painted nothing, for a character this module does not
/// cover.
pub(crate) fn paint(ch: char, l: Lines, fill: &mut dyn FnMut(i32, i32, i32, i32)) -> bool {
    if !covers(ch) {
        return false;
    }
    let index = (ch as u32 - 0x2500) as usize;
    let mut inked = false;
    let mut watched = |x0: i32, y0: i32, x1: i32, y1: i32| {
        inked |= x0.max(0) < x1.min(l.w) && y0.max(0) < y1.min(l.h);
        fill(x0, y0, x1, y1);
    };
    match index {
        0x71..=0x73 => diagonals(index, l, &mut watched),
        0x6D..=0x70 => arc(index, l, &mut watched),
        0x00..=0x7F => lines(arms(index), dashes(index), l, &mut watched),
        _ => block(index, l, &mut watched),
    }
    // A cell of a pixel or two has no upper half and no room beside a
    // line; rather than nothing, such a character fills it.
    if !inked {
        fill(0, 0, l.w, l.h);
    }
    true
}

/// Straight lines, corners, tees and crosses in the three weights.
fn lines(mut a: Arms, dashes: i32, l: Lines, fill: &mut dyn FnMut(i32, i32, i32, i32)) {
    let (vx, tv, hy, th, ev, eh) = (l.vx, l.tv, l.hy, l.th, l.ev(), l.eh());
    // A cell with no room for both strokes of a double line gets a heavy
    // line in its place, which is at least something.
    let heavier = |weight: u8, room: bool| if weight == 3 && !room { 2 } else { weight };
    let (flat_room, upright_room) = (hy - eh >= 0 && hy + th + eh <= l.h, vx - ev >= 0 && vx + tv + ev <= l.w);
    a = Arms { up: heavier(a.up, upright_room), right: heavier(a.right, flat_room), down: heavier(a.down, upright_room), left: heavier(a.left, flat_room) };
    let double = |weight: u8| weight == 3;
    let solid = |weight: u8| weight == 1 || weight == 2;
    let heavy = |weight: u8| weight == 2;

    // How far in from its edge a light or heavy arm runs. An arm with one
    // opposite runs through the middle. Otherwise it stops where it meets
    // the arms across it: at their outer edge for a corner, at the nearer
    // stroke where it butts against a double line that passes, and in the
    // middle when there is nothing to meet.
    //
    // `first` and `thick` place the light line that crosses this arm, `e`
    // is that line's heavy reach, `(one, other)` are the two arms across
    // and `opposite` the arm on the far side. `toward_low` is true for an
    // arm that comes in from the right or the bottom edge and false for
    // one from the left or the top; the answer is the column or row where
    // it stops.
    let reach = |toward_low: bool, opposite: u8, one: u8, other: u8, first: i32, thick: i32, e: i32| -> i32 {
        let (low, high) = (first, first + thick);
        if opposite != 0 {
            return if toward_low { low } else { high };
        }
        if double(one) && double(other) {
            return if toward_low { high } else { low };
        }
        if double(one) || double(other) {
            return if toward_low { low - e } else { high + e };
        }
        if one == 0 && other == 0 {
            return if toward_low { low + thick / 2 } else { high - thick / 2 };
        }
        let grow = if heavy(one) || heavy(other) { e } else { 0 };
        if toward_low {
            low - grow
        } else {
            high + grow
        }
    };

    // The same for one stroke of a double arm. `near` is the arm across on
    // this stroke's own side and `far` the one on the other side.
    let stroke_reach = |toward_low: bool, opposite: u8, near: u8, far: u8, first: i32, thick: i32, e: i32| -> i32 {
        let (low, high) = (first, first + thick);
        if double(near) {
            // An inner corner: stop at the far edge of that arm's nearer stroke.
            return if toward_low { high } else { low };
        }
        if solid(near) || opposite != 0 || solid(far) {
            return if toward_low { low } else { high };
        }
        if double(far) {
            // An outer corner: run on to the outside of that arm's farther stroke.
            return if toward_low { low - e } else { high + e };
        }
        if toward_low {
            low + thick / 2
        } else {
            high - thick / 2
        }
    };

    // Each arm as a list of (band start, band end, run start, run end):
    // the band is across the arm and the run is along it.
    let mut across: Vec<(i32, i32, i32, i32)> = Vec::new();
    let mut down: Vec<(i32, i32, i32, i32)> = Vec::new();

    for (weight, toward_low, opposite) in [(a.right, true, a.left), (a.left, false, a.right)] {
        let run = |from: i32| if toward_low { (from, l.w) } else { (0, from) };
        match weight {
            1 | 2 => {
                let grow = if heavy(weight) { eh } else { 0 };
                let (x0, x1) = run(reach(toward_low, opposite, a.up, a.down, vx, tv, ev));
                across.push((hy - grow, hy + th + grow, x0, x1));
            }
            3 => {
                let (x0, x1) = run(stroke_reach(toward_low, opposite, a.up, a.down, vx, tv, ev));
                across.push((hy - eh, hy, x0, x1));
                let (x0, x1) = run(stroke_reach(toward_low, opposite, a.down, a.up, vx, tv, ev));
                across.push((hy + th, hy + th + eh, x0, x1));
            }
            _ => {}
        }
    }
    for (weight, toward_low, opposite) in [(a.down, true, a.up), (a.up, false, a.down)] {
        let run = |from: i32| if toward_low { (from, l.h) } else { (0, from) };
        match weight {
            1 | 2 => {
                let grow = if heavy(weight) { ev } else { 0 };
                let (y0, y1) = run(reach(toward_low, opposite, a.left, a.right, hy, th, eh));
                down.push((vx - grow, vx + tv + grow, y0, y1));
            }
            3 => {
                let (y0, y1) = run(stroke_reach(toward_low, opposite, a.left, a.right, hy, th, eh));
                down.push((vx - ev, vx, y0, y1));
                let (y0, y1) = run(stroke_reach(toward_low, opposite, a.right, a.left, hy, th, eh));
                down.push((vx + tv, vx + tv + ev, y0, y1));
            }
            _ => {}
        }
    }

    if dashes > 1 {
        // A dashed line is a straight one with a gap at the head of each
        // of its equal parts.
        let mut dashed = |band: (i32, i32), length: i32, flat: bool| {
            for i in 0..dashes {
                let (from, to) = (i * length / dashes, (i + 1) * length / dashes);
                // In a cell too small for a gap the dash keeps its pixel.
                let gap = ((to - from) / 4).max(1).min((to - from - 1).max(0));
                if flat {
                    fill(from + gap, band.0, to, band.1);
                } else {
                    fill(band.0, from + gap, band.1, to);
                }
            }
        };
        if let Some(&(y0, y1, _, _)) = across.first() {
            dashed((y0, y1), l.w, true);
        }
        if let Some(&(x0, x1, _, _)) = down.first() {
            dashed((x0, x1), l.h, false);
        }
        return;
    }
    for (y0, y1, x0, x1) in across {
        fill(x0, y0, x1, y1);
    }
    for (x0, x1, y0, y1) in down {
        fill(x0, y0, x1, y1);
    }
}

/// The four arcs: a light corner with the corner itself left out, and
/// where the line is thick enough a step laid across to round it.
fn arc(index: usize, l: Lines, fill: &mut dyn FnMut(i32, i32, i32, i32)) {
    let (vx, tv, hy, th) = (l.vx, l.tv, l.hy, l.th);
    // Down and right, down and left, up and left, up and right.
    let (to_right, to_bottom) = match index {
        0x6D => (true, true),
        0x6E => (false, true),
        0x6F => (false, false),
        _ => (true, false),
    };
    if to_right {
        fill(vx + tv, hy, l.w, hy + th);
    } else {
        fill(0, hy, vx, hy + th);
    }
    if to_bottom {
        fill(vx, hy + th, vx + tv, l.h);
    } else {
        fill(vx, 0, vx + tv, hy);
    }
    if tv >= 2 && th >= 2 {
        let x = if to_right { vx + tv / 2 } else { vx - tv / 2 };
        let y = if to_bottom { hy + th / 2 } else { hy - th / 2 };
        fill(x, y, x + tv, y + th);
    }
    // In a cell with no room for either arm the corner is all there is.
    let no_flat = if to_right { vx + tv >= l.w } else { vx <= 0 };
    let no_upright = if to_bottom { hy + th >= l.h } else { hy <= 0 };
    if no_flat && no_upright {
        fill(vx, hy, vx + tv, hy + th);
    }
}

/// The diagonals, corner to corner, as thin as half a light line.
fn diagonals(index: usize, l: Lines, fill: &mut dyn FnMut(i32, i32, i32, i32)) {
    let thick = l.ev();
    let span = (l.w - thick).max(0);
    for y in 0..l.h {
        // The column a line from the top-left corner has reached by this row.
        let x = if l.h > 1 { (y as i64 * span as i64 / (l.h - 1) as i64) as i32 } else { 0 };
        if index != 0x71 {
            fill(x, y, x + thick, y + 1);
        }
        if index != 0x72 {
            fill(span - x, y, span - x + thick, y + 1);
        }
    }
}

/// `eighths` eighths of `size` in whole pixels, rounded up as Gallant's
/// own blocks are, so the thinnest is never nothing. More than half is
/// measured from the far side, so that a block and the one that fills the
/// rest of the cell meet without a gap or an overlap.
fn part(size: i32, eighths: i32) -> i32 {
    let near = |n: i32| ((size * n + 7) / 8).max(1);
    if eighths > 4 {
        (size - near(8 - eighths)).max(1)
    } else {
        near(eighths).min(size)
    }
}

/// The block elements, U+2580 to U+259F.
fn block(index: usize, l: Lines, fill: &mut dyn FnMut(i32, i32, i32, i32)) {
    let (w, h) = (l.w, l.h);
    let (mid_x, mid_y) = (w / 2, h / 2);
    match index {
        // The halves split where the quadrants do, so that the two agree.
        0x80 => fill(0, 0, w, mid_y),
        0x84 => fill(0, mid_y, w, h),
        0x88 => fill(0, 0, w, h),
        0x81..=0x87 => fill(0, h - part(h, (index - 0x80) as i32), w, h),
        0x8C => fill(0, 0, mid_x, h),
        0x89..=0x8F => fill(0, 0, part(w, (0x90 - index) as i32), h),
        0x90 => fill(mid_x, 0, w, h),
        0x91..=0x93 => {
            // A quarter, a half and three quarters of the pixels, in the
            // patterns Gallant uses.
            for y in 0..h {
                for x in 0..w {
                    let on = match index {
                        0x91 => y % 2 == 0 && x % 2 == 0,
                        0x92 => (x / 2 + y) % 2 == 1,
                        _ => y % 2 == 0 || x % 2 == 0,
                    };
                    if on {
                        fill(x, y, x + 1, y + 1);
                    }
                }
            }
        }
        0x94 => fill(0, 0, w, part(h, 1)),
        0x95 => fill(w - part(w, 1), 0, w, h),
        _ => {
            // Quadrants: upper left, upper right, lower left, lower right.
            let quarters: [bool; 4] = match index {
                0x96 => [false, false, true, false],
                0x97 => [false, false, false, true],
                0x98 => [true, false, false, false],
                0x99 => [true, false, true, true],
                0x9A => [true, false, false, true],
                0x9B => [true, true, true, false],
                0x9C => [true, true, false, true],
                0x9D => [false, true, false, false],
                0x9E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let corners = [(0, 0, mid_x, mid_y), (mid_x, 0, w, mid_y), (0, mid_y, mid_x, h), (mid_x, mid_y, w, h)];
            for (on, (x0, y0, x1, y1)) in quarters.into_iter().zip(corners) {
                if on {
                    fill(x0, y0, x1, y1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{gallant, spleen, terminus};
    use crate::font::BitmapFont;

    /// A character painted into rows of `#` and `.`.
    fn painted(ch: char, l: Lines) -> Vec<String> {
        let mut cells = vec![vec![b'.'; l.w as usize]; l.h as usize];
        assert!(paint(ch, l, &mut |x0, y0, x1, y1| {
            for y in y0.max(0)..y1.min(l.h) {
                for x in x0.max(0)..x1.min(l.w) {
                    cells[y as usize][x as usize] = b'#';
                }
            }
        }));
        cells.into_iter().map(|row| String::from_utf8(row).unwrap()).collect()
    }

    /// A face's own glyph in the same form.
    fn own(face: &BitmapFont, ch: char) -> Option<Vec<String>> {
        let g = face.glyph(ch)?;
        Some(g.rows.iter().map(|&row| (0..g.width).map(|c| if row & (0x8000_0000 >> c) != 0 { '#' } else { '.' }).collect()).collect())
    }

    /// The lines of a face, measured off its own `─` and `│`.
    fn lines_of(face: &BitmapFont) -> Lines {
        let (w, h) = face.cell();
        let across = face.glyph('\u{2500}').and_then(|g| g.ink_rows()).map(|(first, last)| (first, last - first + 1));
        let down = face.glyph('\u{2502}').map(|g| g.rows.iter().fold(0, |all, &r| all | r)).filter(|&all| all != 0);
        let down = down.map(|all| (all.leading_zeros(), 32 - all.trailing_zeros() - all.leading_zeros()));
        Lines::new(w, h, across, down, 1)
    }

    /// What a picture shows at the four edges of its cell, which is what
    /// decides whether it joins its neighbours: top row, right column,
    /// bottom row, left column.
    fn edges(picture: &[String]) -> [String; 4] {
        let column = |x: usize| picture.iter().map(|row| row.as_bytes()[x] as char).collect::<String>();
        let width = picture[0].len();
        [picture[0].clone(), column(width - 1), picture[picture.len() - 1].clone(), column(0)]
    }

    const GALLANT: Lines = Lines { w: 12, h: 22, vx: 5, tv: 2, hy: 10, th: 2 };

    #[test]
    fn the_lines_of_the_three_faces_are_measured_off_their_own_glyphs() {
        assert_eq!(lines_of(gallant()), GALLANT);
        assert_eq!(lines_of(spleen()), Lines { w: 12, h: 24, vx: 5, tv: 2, hy: 11, th: 2 });
        assert_eq!(lines_of(terminus()), Lines { w: 11, h: 22, vx: 5, tv: 1, hy: 10, th: 1 });
        // With nothing to measure the lines run through the middle.
        assert_eq!(Lines::new(12, 22, None, None, 2), GALLANT);
        assert_eq!(Lines::new(11, 22, None, None, 1), Lines { w: 11, h: 22, vx: 5, tv: 1, hy: 10, th: 1 });
        // A measurement that cannot be a line is not believed.
        assert_eq!(Lines::new(12, 22, Some((0, 22)), Some((11, 5)), 2), GALLANT);
        assert_eq!(Lines::new(0, 0, None, None, 0), Lines { w: 1, h: 1, vx: 0, tv: 1, hy: 0, th: 1 });
        // Numbers no face could give still make lines that lie in the cell.
        for (across, down) in [(Some((u32::MAX, u32::MAX)), None), (Some((0, u32::MAX)), Some((u32::MAX, 1))), (None, Some((5, 0)))] {
            for weight in [0, 1, u32::MAX] {
                let l = Lines::new(u32::MAX, 9, across, down, weight);
                assert_eq!((l.w, l.h), (4096, 9));
                assert!(l.tv >= 1 && l.vx >= 0 && l.vx + l.tv <= l.w, "{l:?}");
                assert!(l.th >= 1 && l.th <= 3 && l.hy >= 0 && l.hy + l.th <= l.h, "{l:?}");
            }
        }
    }

    #[test]
    fn every_line_character_meets_the_edges_gallant_s_own_glyph_meets() {
        // The table of arms is checked against a face that draws all 128:
        // the same edges touched, in the same weight, means the same arms.
        for cp in 0x2500..=0x257Fu32 {
            let ch = char::from_u32(cp).unwrap();
            let (mine, theirs) = (painted(ch, GALLANT), own(gallant(), ch).expect("Gallant has every one"));
            if (0x2571..=0x2573).contains(&cp) {
                // A diagonal runs from corner to corner.
                let corner = |p: &[String], x: usize, y: usize| p[y].as_bytes()[x] == b'#';
                for (x, y) in [(0, 0), (11, 21), (11, 0), (0, 21)] {
                    assert_eq!(corner(&mine, x, y), corner(&theirs, x, y), "U+{cp:04X} at {x},{y}");
                }
            } else if dashes((cp - 0x2500) as usize) > 1 {
                // Dashes are spaced differently; they lie on the same rows
                // and columns.
                let rows = |p: &[String]| p.iter().map(|r| r.contains('#')).collect::<Vec<bool>>();
                let columns = |p: &[String]| (0..12).map(|x| p.iter().any(|r| r.as_bytes()[x] == b'#')).collect::<Vec<bool>>();
                let flat = arms((cp - 0x2500) as usize).left != 0;
                if flat {
                    assert_eq!(rows(&mine), rows(&theirs), "U+{cp:04X}");
                } else {
                    assert_eq!(columns(&mine), columns(&theirs), "U+{cp:04X}");
                }
                assert!(mine.concat().matches('#').count() < theirs.concat().matches('#').count() * 2);
            } else {
                for (side, (a, b)) in edges(&mine).iter().zip(edges(&theirs)).enumerate() {
                    assert_eq!(*a, b, "U+{cp:04X} side {side}:
{}

{}", mine.join("
"), theirs.join("
"));
                }
            }
        }
    }

    #[test]
    fn most_line_characters_come_out_as_gallant_draws_them() {
        let same: Vec<u32> = (0x2500..=0x257Fu32).filter(|&cp| own(gallant(), char::from_u32(cp).unwrap()) == Some(painted(char::from_u32(cp).unwrap(), GALLANT))).collect();
        // Light and heavy lines, corners, tees and crosses, and the double set.
        for cp in [0x2500, 0x2501, 0x2502, 0x2503, 0x250C, 0x250F, 0x2510, 0x2514, 0x2518, 0x251B, 0x251C, 0x2524, 0x252C, 0x2534, 0x253C, 0x254B] {
            assert!(same.contains(&cp), "U+{cp:04X}:\n{}", painted(char::from_u32(cp).unwrap(), GALLANT).join("\n"));
        }
        for cp in [0x2550, 0x2551, 0x2552, 0x2554, 0x2557, 0x255A, 0x255D, 0x255E, 0x2560, 0x2563, 0x2564, 0x2566, 0x2569, 0x256A, 0x256C] {
            assert!(same.contains(&cp), "U+{cp:04X}:\n{}", painted(char::from_u32(cp).unwrap(), GALLANT).join("\n"));
        }
        assert!(same.len() >= 100, "only {} of 128 match: {:X?}", same.len(), same);
    }

    #[test]
    fn the_blocks_come_out_as_gallant_draws_them() {
        for cp in 0x2580..=0x259Fu32 {
            let ch = char::from_u32(cp).unwrap();
            assert_eq!(Some(painted(ch, GALLANT)), own(gallant(), ch), "U+{cp:04X}");
        }
    }

    #[test]
    fn lines_drawn_in_a_face_s_weight_meet_the_edges_its_own_glyphs_meet() {
        for face in [spleen(), terminus()] {
            let l = lines_of(face);
            let mut checked = 0;
            for cp in 0x2500..=0x256Cu32 {
                let ch = char::from_u32(cp).unwrap();
                let Some(theirs) = own(face, ch) else { continue };
                if dashes((cp - 0x2500) as usize) > 1 {
                    continue;
                }
                let mine = painted(ch, l);
                for (side, (a, b)) in edges(&mine).iter().zip(edges(&theirs)).enumerate() {
                    assert_eq!(a.contains('#'), b.contains('#'), "{} U+{cp:04X} side {side}:\n{}\n\n{}", face.name(), mine.join("\n"), theirs.join("\n"));
                }
                checked += 1;
            }
            assert!(checked > 60, "{}: {checked}", face.name());
            // The plain lines are theirs exactly.
            for ch in ['\u{2500}', '\u{2502}', '\u{250C}', '\u{2518}', '\u{253C}'] {
                assert_eq!(Some(painted(ch, l)), own(face, ch), "{} U+{:04X}", face.name(), ch as u32);
            }
        }
    }

    #[test]
    fn every_character_is_painted_in_every_cell_without_leaving_it_blank() {
        let mut calls = 0u64;
        for (w, h) in [(1, 1), (2, 3), (5, 8), (8, 16), (11, 22), (12, 24), (13, 25), (32, 64), (38, 66), (255, 256)] {
            for weight in [1, 2, 3, 9] {
                let l = Lines::new(w, h, None, None, weight);
                for cp in 0x2500..=0x259Fu32 {
                    let mut inked = false;
                    assert!(paint(char::from_u32(cp).unwrap(), l, &mut |x0, y0, x1, y1| {
                        calls += 1;
                        inked |= x0.max(0) < x1.min(l.w) && y0.max(0) < y1.min(l.h);
                    }));
                    assert!(inked, "U+{cp:04X} in {w} by {h} at weight {weight} is blank");
                }
            }
        }
        assert!(calls > 0);
        assert!(!paint('a', GALLANT, &mut |_, _, _, _| panic!("not a box character")));
        assert!(!paint('\u{25A0}', GALLANT, &mut |_, _, _, _| panic!("not a box character")));
    }

    #[test]
    fn blocks_that_share_a_cell_fill_it_between_them() {
        // The lower n eighths and the rest above them never overlap or
        // leave a gap, at sizes where the eighths do not come out even.
        for size in 2..70 {
            for n in 1..4 {
                assert_eq!(part(size, n) + part(size, 8 - n), size, "{n} eighths of {size}");
            }
            for n in 1..8 {
                assert!(part(size, n) >= 1 && part(size, n) <= size);
            }
        }
        assert_eq!((part(1, 1), part(1, 7), part(22, 1), part(22, 7), part(12, 7)), (1, 1, 3, 19, 10));
    }
}
