//! The program's icon: the word on its sheet, as the machine prints it.

use crate::{gallant, FaceSet, Frame, Pen, Rect, Rgb};

/// The icon at `size` pixels square: the word on its sheet, as the machine
/// prints it. It is drawn at 32 pixels; smaller sizes are that picture
/// averaged down, larger ones the same pixels made bigger.
pub fn icon(size: u32) -> Frame {
    const PAPER: Rgb = Rgb(0xEE, 0xE3, 0xC8);
    const INK: Rgb = Rgb(0x22, 0x1F, 0x1B);
    let faces = FaceSet::new(gallant());
    let (cell_w, cell_h) = faces.cell();
    let mut frame = Frame::new(32, 32);
    frame.clear(PAPER);
    let (x, y) = ((32 - 2 * cell_w as i32) / 2, (32 - cell_h as i32) / 2);
    faces.draw_text(&mut frame, "ok", x, y, Pen::new(INK));
    let size = size.max(1) as i32;
    let mut out = Frame::new(size as usize, size as usize);
    if size >= 32 {
        for sy in 0..size {
            for sx in 0..size {
                let colour = frame.get(sx * 32 / size, sy * 32 / size).unwrap_or(PAPER);
                out.fill(Rect::new(sx, sy, 1, 1), colour);
            }
        }
        return out;
    }
    let step = (32 / size).max(1);
    for sy in 0..size {
        for sx in 0..size {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..step {
                for dx in 0..step {
                    if let Some(Rgb(pr, pg, pb)) = frame.get(sx * step + dx, sy * step + dy) {
                        r += pr as u32;
                        g += pg as u32;
                        b += pb as u32;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.fill(Rect::new(sx, sy, 1, 1), Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8));
        }
    }
    out
}

