//! A bitmap face written out as a TrueType font, so that the phone page can
//! set its words in Gallant: every lit pixel becomes a square, runs of them
//! one rectangle. At a CSS size equal to the cell height each font pixel is
//! one screen pixel (or a whole number of them), and the letters are the
//! machine's own.

use ok_gfx::BitmapFont;

/// Font units to one pixel of the bitmap.
const UNIT: i32 = 64;

/// The characters the phone needs: Latin, its punctuation and a few signs.
pub fn wanted() -> Vec<char> {
    let ranges = [(0x20u32, 0x7E), (0xA0, 0x17F), (0x2010, 0x2027), (0x2030, 0x203A), (0x20AC, 0x20AC), (0x2122, 0x2122), (0x2190, 0x2193), (0x2588, 0x2588)];
    ranges.iter().flat_map(|&(a, b)| (a..=b).filter_map(char::from_u32)).collect()
}

struct Outline {
    rects: Vec<(i32, i32, i32, i32)>,
}

/// The lit pixels of one glyph as rectangles (x0, y0, x1, y1) in font units,
/// y up from the baseline.
fn outline(rows: &[u32], width: u32, ascent: i32) -> Outline {
    // Runs on each row, then runs that stack exactly are merged downward.
    let mut open: Vec<(u32, u32, usize)> = Vec::new(); // (start, end, first row)
    let mut done: Vec<(u32, u32, usize, usize)> = Vec::new();
    for (r, &row) in rows.iter().enumerate() {
        let mut runs = Vec::new();
        let mut x = 0;
        while x < width {
            if row >> (31 - x) & 1 == 1 {
                let start = x;
                while x < width && row >> (31 - x) & 1 == 1 {
                    x += 1;
                }
                runs.push((start, x));
            } else {
                x += 1;
            }
        }
        let mut next = Vec::new();
        for (start, end, first) in open.drain(..) {
            if let Some(i) = runs.iter().position(|&(s, e)| s == start && e == end) {
                runs.remove(i);
                next.push((start, end, first));
            } else {
                done.push((start, end, first, r));
            }
        }
        for (s, e) in runs {
            next.push((s, e, r));
        }
        open = next;
    }
    for (start, end, first) in open {
        done.push((start, end, first, rows.len()));
    }
    let rects = done
        .into_iter()
        .map(|(s, e, top, bottom)| (s as i32 * UNIT, (ascent - bottom as i32) * UNIT, e as i32 * UNIT, (ascent - top as i32) * UNIT))
        .collect();
    Outline { rects }
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut word = [0u8; 4];
        word[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// The face as a `.ttf`, named `family`, holding [`wanted`] characters it has.
pub fn build(face: &BitmapFont, family: &str) -> Vec<u8> {
    let (cell_w, cell_h) = face.cell();
    let ascent = face.ascent() as i32;
    let descent = cell_h as i32 - ascent;
    let em = cell_h as i32 * UNIT;

    // Glyph 0 is .notdef, an empty box; then one glyph per character.
    let mut chars: Vec<char> = wanted().into_iter().filter(|&c| face.glyph(c).is_some()).collect();
    chars.sort();
    let mut glyphs: Vec<(Outline, u32)> = Vec::new();
    let notdef = Outline {
        rects: vec![(UNIT, -descent * UNIT + UNIT, (cell_w as i32 - 1) * UNIT, -descent * UNIT + 2 * UNIT), (UNIT, (ascent - 2) * UNIT, (cell_w as i32 - 1) * UNIT, (ascent - 1) * UNIT)],
    };
    glyphs.push((notdef, cell_w));
    for &c in &chars {
        let g = face.glyph(c).expect("filtered above");
        glyphs.push((outline(g.rows, g.width, ascent), g.width));
    }

    // glyf and loca.
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    let mut hmtx = Vec::new();
    let (mut max_points, mut max_contours) = (0u16, 0u16);
    let (mut x_min, mut y_min, mut x_max, mut y_max) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (o, width) in &glyphs {
        be32(&mut loca, glyf.len() as u32);
        let advance = (*width as i32 * UNIT) as u16;
        if o.rects.is_empty() {
            be16(&mut hmtx, advance);
            be16(&mut hmtx, 0);
            continue;
        }
        let gx0 = o.rects.iter().map(|r| r.0).min().unwrap_or(0);
        let gy0 = o.rects.iter().map(|r| r.1).min().unwrap_or(0);
        let gx1 = o.rects.iter().map(|r| r.2).max().unwrap_or(0);
        let gy1 = o.rects.iter().map(|r| r.3).max().unwrap_or(0);
        (x_min, y_min, x_max, y_max) = (x_min.min(gx0), y_min.min(gy0), x_max.max(gx1), y_max.max(gy1));
        be16(&mut hmtx, advance);
        be16(&mut hmtx, gx0 as i16 as u16);
        let n = o.rects.len();
        max_contours = max_contours.max(n as u16);
        max_points = max_points.max((n * 4) as u16);
        be16(&mut glyf, n as u16);
        for v in [gx0, gy0, gx1, gy1] {
            be16(&mut glyf, v as i16 as u16);
        }
        for i in 0..n {
            be16(&mut glyf, (i * 4 + 3) as u16);
        }
        be16(&mut glyf, 0); // no instructions
        glyf.extend(std::iter::repeat_n(1u8, n * 4)); // on curve, long coordinates
        // Clockwise: up the left side, along the top, down the right, back.
        let points: Vec<(i32, i32)> = o.rects.iter().flat_map(|&(x0, y0, x1, y1)| [(x0, y0), (x0, y1), (x1, y1), (x1, y0)]).collect();
        let mut last = 0;
        for &(x, _) in &points {
            be16(&mut glyf, (x - last) as i16 as u16);
            last = x;
        }
        last = 0;
        for &(_, y) in &points {
            be16(&mut glyf, (y - last) as i16 as u16);
            last = y;
        }
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    be32(&mut loca, glyf.len() as u32);
    if x_min > x_max {
        (x_min, y_min, x_max, y_max) = (0, 0, 0, 0);
    }
    let advance_max = glyphs.iter().map(|(_, w)| *w as i32 * UNIT).max().unwrap_or(0);

    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0); // checkSumAdjustment, filled in at the end
    be32(&mut head, 0x5F0F_3CF5);
    be16(&mut head, 0x000B);
    be16(&mut head, em as u16);
    // Created and modified: 2026-01-01, in seconds since 1904.
    for _ in 0..2 {
        head.extend_from_slice(&3_850_070_400u64.to_be_bytes());
    }
    for v in [x_min, y_min, x_max, y_max] {
        be16(&mut head, v as i16 as u16);
    }
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16(&mut head, 2);
    be16(&mut head, 1); // long loca
    be16(&mut head, 0);

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    be16(&mut hhea, (ascent * UNIT) as u16);
    be16(&mut hhea, (-descent * UNIT) as i16 as u16);
    be16(&mut hhea, 0);
    be16(&mut hhea, advance_max as u16);
    be16(&mut hhea, 0);
    be16(&mut hhea, 0);
    be16(&mut hhea, x_max as u16);
    be16(&mut hhea, 1);
    be16(&mut hhea, 0);
    be16(&mut hhea, 0);
    hhea.extend_from_slice(&[0; 8]);
    be16(&mut hhea, 0);
    be16(&mut hhea, glyphs.len() as u16);

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, glyphs.len() as u16);
    be16(&mut maxp, max_points);
    be16(&mut maxp, max_contours);
    be16(&mut maxp, 0);
    be16(&mut maxp, 0);
    be16(&mut maxp, 2);
    for _ in 0..8 {
        be16(&mut maxp, 0);
    }

    let mut os2 = Vec::new();
    be16(&mut os2, 4);
    be16(&mut os2, (cell_w as i32 * UNIT) as u16);
    be16(&mut os2, 400);
    be16(&mut os2, 5);
    be16(&mut os2, 0);
    for v in [em / 2, em / 2, 0, em / 4, em / 2, em / 2, 0, em / 2] {
        be16(&mut os2, v as u16);
    }
    be16(&mut os2, UNIT as u16); // yStrikeoutSize
    be16(&mut os2, ((ascent - face.strike_rows().0 as i32) * UNIT) as u16);
    be16(&mut os2, 0);
    os2.extend_from_slice(&[2, 0, 0, 9, 0, 0, 0, 0, 0, 0]); // panose: monospaced
    be32(&mut os2, 0x0000_0003); // Basic Latin, Latin-1
    be32(&mut os2, 0);
    be32(&mut os2, 0);
    be32(&mut os2, 0);
    os2.extend_from_slice(b"NONE");
    be16(&mut os2, 0x00C0); // regular, use typo metrics
    be16(&mut os2, chars.first().map_or(0x20, |&c| c as u32).min(0xFFFF) as u16);
    be16(&mut os2, chars.last().map_or(0x20, |&c| c as u32).min(0xFFFF) as u16);
    be16(&mut os2, (ascent * UNIT) as u16);
    be16(&mut os2, (-descent * UNIT) as i16 as u16);
    be16(&mut os2, 0);
    be16(&mut os2, (ascent * UNIT) as u16);
    be16(&mut os2, (descent * UNIT) as u16);
    be32(&mut os2, 1);
    be32(&mut os2, 0);
    be16(&mut os2, ((ascent - face.x_top() as i32) * UNIT) as u16);
    be16(&mut os2, ((ascent - 2) * UNIT) as u16);
    be16(&mut os2, 0);
    be16(&mut os2, 0x20);
    be16(&mut os2, 1);

    let mut name = Vec::new();
    let records: Vec<(u16, String)> = vec![(1, family.to_string()), (2, "Regular".into()), (3, format!("{family} Regular")), (4, family.to_string()), (5, "Version 1.0".into()), (6, family.replace(' ', ""))];
    be16(&mut name, 0);
    be16(&mut name, records.len() as u16);
    be16(&mut name, (6 + 12 * records.len()) as u16);
    let mut strings = Vec::new();
    for (id, text) in &records {
        let utf16: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        for v in [3, 1, 0x409, *id, utf16.len() as u16, strings.len() as u16] {
            be16(&mut name, v);
        }
        strings.extend_from_slice(&utf16);
    }
    name.extend_from_slice(&strings);

    // cmap format 4: runs of consecutive characters with consecutive glyphs.
    let mut segments: Vec<(u16, u16, u16)> = Vec::new(); // (start, end, first glyph)
    for (i, &c) in chars.iter().enumerate() {
        let (cp, gid) = (c as u32 as u16, (i + 1) as u16);
        match segments.last_mut() {
            Some(last) if last.1 + 1 == cp && last.2 + (cp - last.0) == gid => last.1 = cp,
            _ => segments.push((cp, cp, gid)),
        }
    }
    segments.push((0xFFFF, 0xFFFF, 0));
    let seg_count = segments.len() as u16;
    let mut sub = Vec::new();
    let search = 2 * (1u16 << (15 - seg_count.leading_zeros()));
    for v in [4, 0, 0, seg_count * 2, search, search.trailing_zeros() as u16 - 1, seg_count * 2 - search] {
        be16(&mut sub, v);
    }
    for s in &segments {
        be16(&mut sub, s.1);
    }
    be16(&mut sub, 0);
    for s in &segments {
        be16(&mut sub, s.0);
    }
    for s in &segments {
        let delta = if s.0 == 0xFFFF { 1 } else { s.2.wrapping_sub(s.0) };
        be16(&mut sub, delta);
    }
    for _ in &segments {
        be16(&mut sub, 0);
    }
    let length = sub.len() as u16;
    sub[2..4].copy_from_slice(&length.to_be_bytes());
    let mut cmap = Vec::new();
    for v in [0u16, 1, 3, 1] {
        be16(&mut cmap, v);
    }
    be32(&mut cmap, 12);
    cmap.extend_from_slice(&sub);

    let mut post = Vec::new();
    be32(&mut post, 0x0003_0000);
    be32(&mut post, 0);
    be16(&mut post, (-(face.underline_rows().0 as i32 - ascent) * UNIT) as i16 as u16);
    be16(&mut post, UNIT as u16);
    be32(&mut post, 1);
    post.extend_from_slice(&[0; 16]);

    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"OS/2", os2), (b"cmap", cmap), (b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp), (b"name", name), (b"post", post)];
    let n = tables.len() as u16;
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, n);
    let search = 16 * (1u16 << (15 - n.leading_zeros()));
    be16(&mut out, search);
    be16(&mut out, (search / 16).trailing_zeros() as u16);
    be16(&mut out, n * 16 - search);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_at = 0;
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, checksum(data));
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        if *tag == b"head" {
            head_at = offset;
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_at + 8..head_at + 12].copy_from_slice(&adjust.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_run_becomes_one_rectangle_and_stacks_merge() {
        let rows = [0b0110 << 28, 0b0110 << 28, 0b1000 << 28];
        let o = outline(&rows, 4, 3);
        assert_eq!(o.rects.len(), 2);
        assert!(o.rects.contains(&(UNIT, UNIT, 3 * UNIT, 3 * UNIT)));
        assert!(o.rects.contains(&(0, 0, UNIT, UNIT)));
    }

    #[test]
    fn gallant_makes_a_well_formed_font() {
        let ttf = build(ok_gfx::gallant(), "ok Gallant");
        assert_eq!(&ttf[..4], &[0, 1, 0, 0]);
        assert_eq!(checksum(&ttf), 0xB1B0_AFBA, "the whole file sums to the magic number");
        assert!(ttf.len() < 200_000, "{} bytes", ttf.len());
    }
}
