//! The picture of a character no face can draw: a small box holding the
//! code point's hex digits. It says which character is there, where a
//! question mark or U+FFFD would say only that something is.

use crate::font::{clipped, Pen};
use crate::frame::{Frame, Rect};

/// The sixteen hex digits, 3 pixels wide and 5 tall. Each row is three bits,
/// the high bit leftmost. B and D are drawn in their small forms, as on a
/// seven-segment display and for the same reason: at this size a capital B
/// is an 8 and a capital D is a 0 with two corners off.
const DIGITS: [[u8; 5]; 16] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b011, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
    [0b010, 0b101, 0b111, 0b101, 0b101],
    [0b100, 0b100, 0b110, 0b101, 0b110],
    [0b011, 0b100, 0b100, 0b100, 0b011],
    [0b001, 0b001, 0b011, 0b101, 0b011],
    [0b111, 0b100, 0b110, 0b100, 0b111],
    [0b111, 0b100, 0b110, 0b100, 0b100],
];

/// What is drawn round the digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    /// Four walls: the box.
    Walls,
    /// A rule above and a rule below, for a cell too narrow for walls.
    Rules,
    /// Nothing, for a cell too short for the rules as well.
    Bare,
}

/// How the digits are arranged: `unit` is the size of one digit pixel in
/// font pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    unit: i32,
    cols: i32,
    rows: i32,
    edge: Edge,
}

impl Layout {
    fn width(&self) -> i32 {
        let digits = 3 * self.cols + (self.cols - 1);
        self.unit * if self.edge == Edge::Walls { digits + 4 } else { digits }
    }

    fn height(&self) -> i32 {
        let digits = 5 * self.rows + (self.rows - 1);
        self.unit * if self.edge == Edge::Bare { digits } else { digits + 4 }
    }
}

/// Picks the arrangement for `n` digits in `w` by `h` font pixels: walls if
/// they fit, then rules, then the digits alone; within each the biggest
/// digits, then the fewest rows.
///
/// Walls must leave a column of the cell clear. A box as wide as its cell
/// stands wall to wall with the box in the next cell, and a run of them
/// reads as a ladder.
fn layout(n: i32, w: i32, h: i32) -> Option<Layout> {
    for edge in [Edge::Walls, Edge::Rules, Edge::Bare] {
        for unit in (1..=8).rev() {
            for rows in 1..=3 {
                let cols = (n + rows - 1) / rows;
                let l = Layout { unit, cols, rows, edge };
                let room = if edge == Edge::Walls { w - 1 } else { w };
                if l.width() <= room && l.height() <= h {
                    return Some(l);
                }
            }
        }
    }
    None
}

/// Paints the box for code point `cp` in a space of `w` by `h` font pixels
/// (one cell, or two for a wide character), as filled rectangles in font
/// pixels from the space's top-left corner. The box stands on the baseline
/// like a letter when it fits above it, and otherwise as little below it
/// as the space allows; `ascent` is the rows above the baseline in that
/// space. Every rectangle lies inside the space.
pub(crate) fn paint(cp: u32, (w, h): (u32, u32), ascent: u32, fill: &mut dyn FnMut(Rect)) {
    let (w, h) = (w.min(4096) as i32, h.min(4096) as i32);
    let ascent = (ascent.min(4096) as i32).min(h);
    let n = ((32 - cp.leading_zeros() as i32 + 3) / 4).max(4);

    let Some(l) = layout(n, w, h) else {
        // Too small for digits: a plain hollow box, or a full block when
        // even that has no inside.
        if w >= 3 && h >= 3 {
            let (bw, bh) = (w - (w > 4) as i32, h - (h > 4) as i32);
            fill(Rect::new(0, 0, bw, 1));
            fill(Rect::new(0, bh - 1, bw, 1));
            fill(Rect::new(0, 1, 1, bh - 2));
            fill(Rect::new(bw - 1, 1, 1, bh - 2));
        } else {
            fill(Rect::new(0, 0, w, h));
        }
        return;
    };

    let (bw, bh, u) = (l.width(), l.height(), l.unit);
    let left = (w - bw) / 2;
    let top = (ascent - bh).clamp(0, h - bh);
    if l.edge != Edge::Bare {
        fill(Rect::new(left, top, bw, u));
        fill(Rect::new(left, top + bh - u, bw, u));
    }
    if l.edge == Edge::Walls {
        fill(Rect::new(left, top + u, u, bh - 2 * u));
        fill(Rect::new(left + bw - u, top + u, u, bh - 2 * u));
    }

    let slots = l.cols * l.rows;
    let inset_x = if l.edge == Edge::Walls { 2 * u } else { 0 };
    let inset_y = if l.edge == Edge::Bare { 0 } else { 2 * u };
    for i in 0..slots {
        // Zero-filled on the left so every row is full.
        let place = slots - 1 - i;
        let digit = if place * 4 < 32 { (cp >> (place * 4)) & 0xF } else { 0 };
        let dx = left + inset_x + (i % l.cols) * 4 * u;
        let dy = top + inset_y + (i / l.cols) * 6 * u;
        for (r, bits) in DIGITS[digit as usize].iter().enumerate() {
            for c in 0..3 {
                if bits >> (2 - c) & 1 == 1 {
                    fill(Rect::new(dx + c * u, dy + r as i32 * u, u, u));
                }
            }
        }
    }
}

/// Draws the box for code point `cp` inside a cell (or two, for a wide
/// character) of `w` by `h` font pixels whose top-left corner is at
/// (`x`, `y`). `pen.bold` is ignored: the digits are too small to thicken.
pub(crate) fn draw(frame: &mut Frame, cp: u32, x: i32, y: i32, cell: (u32, u32), ascent: u32, pen: Pen) {
    let s = pen.step() as i64;
    paint(cp, cell, ascent, &mut |r: Rect| {
        let on_screen = clipped(frame.clip(), x as i64 + r.x as i64 * s, y as i64 + r.y as i64 * s, r.w as i64 * s, r.h as i64 * s);
        frame.blend(on_screen, pen.color, pen.alpha);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[test]
    fn layouts_for_the_cells_that_matter() {
        let walls = |unit, cols, rows| Some(Layout { unit, cols, rows, edge: Edge::Walls });
        // Gallant: four digits as two rows of two, six as three rows of two.
        assert_eq!(layout(4, 12, 22), walls(1, 2, 2));
        assert_eq!(layout(6, 12, 22), walls(1, 2, 3));
        // Two Gallant cells: four or five digits on one row.
        assert_eq!(layout(4, 24, 22), walls(1, 4, 1));
        assert_eq!(layout(5, 24, 22), walls(1, 5, 1));
        assert_eq!(layout(6, 24, 22), walls(1, 3, 2));
        // Terminus is 11 wide, and so is the box with its walls: it would
        // touch the box in the next cell, so it gets the rules alone.
        assert_eq!(layout(4, 11, 22), Some(Layout { unit: 1, cols: 2, rows: 2, edge: Edge::Rules }));
        // An 8 by 16 cell has no room for walls either.
        assert_eq!(layout(4, 8, 16), Some(Layout { unit: 1, cols: 2, rows: 2, edge: Edge::Rules }));
        // The DEC terminal face is 8 by 13: two rows of digits are 11 rows
        // and the rules would make 15, so the digits stand alone.
        for (w, h) in [(8, 13), (8, 14), (7, 13), (7, 11)] {
            assert_eq!(layout(4, w, h), Some(Layout { unit: 1, cols: 2, rows: 2, edge: Edge::Bare }), "{w} by {h}");
        }
        // A big outline cell gets bigger digits.
        assert_eq!(layout(4, 24, 44).map(|l| l.unit), Some(2));
        assert_eq!(layout(4, 6, 8), None);
        assert_eq!(layout(4, 8, 10), None);
    }

    fn lit(frame: &Frame) -> usize {
        frame.pixels().iter().filter(|&&p| p != 0).count()
    }

    #[test]
    fn every_digit_is_a_different_picture() {
        let apart = |a: &[u8; 5], b: &[u8; 5]| a.iter().zip(b).map(|(p, q)| (p ^ q).count_ones()).sum::<u32>();
        for (a, one) in DIGITS.iter().enumerate() {
            for (b, other) in DIGITS.iter().enumerate().skip(a + 1) {
                assert_ne!(one, other, "{a:X} and {b:X}");
            }
        }
        // The letters that are easiest to take for a digit are well clear of it.
        assert!(apart(&DIGITS[0xB], &DIGITS[8]) >= 5 && apart(&DIGITS[0xB], &DIGITS[6]) >= 4);
        assert!(apart(&DIGITS[0xD], &DIGITS[0]) >= 5);
    }

    #[test]
    fn stays_in_its_cell_at_every_size() {
        let pen = Pen::new(Rgb(255, 255, 255));
        for w in 1..40u32 {
            for h in 1..50u32 {
                for cp in [0u32, 0x7F, 0x2614, 0x1F600, 0x10FFFF, u32::MAX] {
                    let mut frame = Frame::new(w as usize + 4, h as usize + 4);
                    frame.clear(Rgb(0, 0, 0));
                    draw(&mut frame, cp, 2, 2, (w, h), h * 3 / 4, pen);
                    assert!(lit(&frame) > 0, "{w} by {h}, {cp:X}");
                    for y in 0..h as i32 + 4 {
                        for x in 0..w as i32 + 4 {
                            let inside = x >= 2 && y >= 2 && x < 2 + w as i32 && y < 2 + h as i32;
                            assert!(inside || frame.get(x, y) == Some(Rgb(0, 0, 0)), "{w} by {h} leaks at {x},{y}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_box_with_walls_never_touches_the_box_in_the_next_cell() {
        let pen = Pen::new(Rgb(255, 255, 255));
        for w in 3..40u32 {
            let mut frame = Frame::new(2 * w as usize, 22);
            frame.clear(Rgb(0, 0, 0));
            draw(&mut frame, 0x0E01, 0, 0, (w, 22), 17, pen);
            draw(&mut frame, 0x0E02, w as i32, 0, (w, 22), 17, pen);
            // Somewhere between the two there is a column with nothing in it,
            // unless the cell is too narrow for any arrangement but its own width.
            let clear = (1..2 * w as i32 - 1).any(|x| (0..22).all(|y| frame.get(x, y) == Some(Rgb(0, 0, 0))));
            assert!(clear || w <= 7, "{w} wide");
        }
    }

    #[test]
    fn different_code_points_draw_different_boxes() {
        let pen = Pen::new(Rgb(255, 255, 255));
        let picture = |cp: u32, cell: (u32, u32)| {
            let mut frame = Frame::new(cell.0 as usize, cell.1 as usize);
            draw(&mut frame, cp, 0, 0, cell, cell.1 * 3 / 4, pen);
            frame.pixels().to_vec()
        };
        for cell in [(12, 22), (11, 22), (8, 16), (8, 13), (7, 13)] {
            assert_ne!(picture(0x0313, cell), picture(0x0314, cell), "{cell:?}");
            assert_ne!(picture(0x0E01, cell), picture(0x0E02, cell), "{cell:?}");
        }
        assert_ne!(picture(0x1F600, (12, 22)), picture(0xF600, (12, 22)));
    }

    #[test]
    fn scale_and_wild_positions_are_harmless() {
        let mut frame = Frame::new(40, 40);
        for scale in [0, 1, 2, 3, 64, u32::MAX] {
            for &(x, y) in &[(0, 0), (-7, -9), (35, 35), (i32::MAX, i32::MAX), (i32::MIN, i32::MIN)] {
                draw(&mut frame, 0x2603, x, y, (12, 22), 17, Pen::new(Rgb(1, 2, 3)).scale(scale));
                draw(&mut frame, 0x2603, x, y, (u32::MAX, u32::MAX), u32::MAX, Pen::new(Rgb(1, 2, 3)).scale(scale));
            }
        }
    }
}
