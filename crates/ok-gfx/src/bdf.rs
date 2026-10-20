//! Reading BDF, the text form X11 bitmap faces are published in.

use crate::font::{err, BitmapFont, FontError, MAX_CELL_H, MAX_CELL_W};

/// No real face has a bounding box anywhere near this; a larger number is a
/// damaged file, and refusing it keeps every sum below well inside `i64`.
const MAX_BOX: i64 = 4096;

/// The whole numbers after a keyword, or `None` if any is missing or is not
/// a number that fits in 32 bits.
fn ints<const N: usize>(words: &mut std::str::SplitAsciiWhitespace<'_>) -> Option<[i64; N]> {
    let mut out = [0i64; N];
    for slot in out.iter_mut() {
        *slot = words.next()?.parse::<i32>().ok()? as i64;
    }
    Some(out)
}

/// What the header says, gathered before any glyph is placed.
struct Header {
    cell_w: u32,
    cell_h: u32,
    ascent: i64,
    /// The advance for a glyph that gives none of its own.
    advance: Option<i64>,
}

fn header(text: &str) -> Result<Header, FontError> {
    let mut bounds: Option<[i64; 4]> = None;
    let (mut ascent, mut descent) = (None, None);
    let mut advance: Option<i64> = None;
    let mut narrowest: Option<(i64, usize)> = None;
    let (mut in_glyph, mut in_bitmap, mut started, mut ended) = (false, false, false, false);

    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let mut words = line.split_ascii_whitespace();
        let Some(key) = words.next() else { continue };
        if !started {
            if key != "STARTFONT" {
                return err(n, "not a BDF face: the first word is not STARTFONT");
            }
            started = true;
            continue;
        }
        if in_bitmap {
            in_bitmap = key != "ENDCHAR";
            in_glyph = in_bitmap;
            continue;
        }
        match key {
            "FONTBOUNDINGBOX" if !in_glyph => match ints::<4>(&mut words) {
                Some(b) => bounds = Some(b),
                None => return err(n, "FONTBOUNDINGBOX needs four numbers"),
            },
            "FONT_ASCENT" if !in_glyph => ascent = ints::<1>(&mut words).map(|v| v[0]),
            "FONT_DESCENT" if !in_glyph => descent = ints::<1>(&mut words).map(|v| v[0]),
            "STARTCHAR" => in_glyph = true,
            "ENDCHAR" => in_glyph = false,
            "BITMAP" => in_bitmap = true,
            "DWIDTH" => {
                let Some([dx]) = ints::<1>(&mut words) else {
                    return err(n, "DWIDTH is not followed by a number");
                };
                if dx <= 0 {
                    return err(n, format!("a glyph advances {dx} pixels; only fixed-pitch faces can be used"));
                }
                if !in_glyph {
                    advance = Some(dx);
                }
                if narrowest.is_none_or(|(w, _)| dx < w) {
                    narrowest = Some((dx, n));
                }
            }
            "ENDFONT" => {
                ended = true;
                break;
            }
            _ => {}
        }
    }

    if !started {
        return err(0, "not a BDF face: the file is empty");
    }
    if !ended {
        return err(0, "the file stops before ENDFONT");
    }
    let Some((cell_w, at)) = narrowest else {
        return err(0, "no glyph gives its width");
    };
    let (ascent, cell_h) = match (ascent, descent, bounds) {
        (Some(a), Some(d), _) => (a, a + d),
        (_, _, Some([_, h, _, y])) => (h + y, h),
        _ => return err(0, "the face gives neither FONT_ASCENT and FONT_DESCENT nor FONTBOUNDINGBOX"),
    };
    if !(1..=MAX_CELL_W as i64).contains(&cell_w) || !(1..=MAX_CELL_H as i64).contains(&cell_h) {
        return err(at, format!("unsupported cell size {cell_w} by {cell_h}"));
    }
    Ok(Header {
        cell_w: cell_w as u32,
        cell_h: cell_h as u32,
        ascent,
        advance,
    })
}

/// One glyph record while its lines are being read.
#[derive(Default)]
struct Record {
    encoding: Option<i64>,
    advance: Option<i64>,
    bbx: Option<[i64; 4]>,
    drawn: bool,
}

impl BitmapFont {
    /// Reads a BDF 2.1 face.
    ///
    /// The cell width is the smallest advance (`DWIDTH`) in the file and a
    /// glyph is wide when its advance is twice that; any other advance means
    /// the face is proportional and it is refused. `FONTBOUNDINGBOX` is not
    /// trusted for the width, because Gallant's own BDF gives the narrow box
    /// and then holds 88 glyphs twice as wide. The cell height is ascent
    /// plus descent. Each glyph is placed by its own `BBX`, measured from
    /// the glyph's origin (the left edge of its cell, on the baseline), and
    /// cut to its own one or two cells. The offset of `FONTBOUNDINGBOX` is
    /// not used for that: a converter writes the box that encloses every
    /// glyph, so one accent hanging left of its origin would move every
    /// letter in the face. Glyphs with a negative `ENCODING` have no code
    /// point and are skipped. After a glyph's bitmap only `ENDCHAR` may
    /// follow: an advance or a box given later would contradict the rows
    /// already read, and the file is refused instead of half read.
    pub fn from_bdf(name: &str, text: &str) -> Result<BitmapFont, FontError> {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let head = header(text)?;
        let (cell_w, cell_h) = (head.cell_w as i64, head.cell_h as i64);
        let mut font = BitmapFont::empty(name, head.cell_w, head.cell_h);
        let mut rows = vec![0u32; head.cell_h as usize];
        let mut glyph: Option<Record> = None;
        let mut lines = text.lines().enumerate();
        let mut ended = false;

        while let Some((i, line)) = lines.next() {
            let n = i + 1;
            let mut words = line.split_ascii_whitespace();
            let Some(key) = words.next() else { continue };
            if key != "ENDCHAR" && glyph.as_ref().is_some_and(|g| g.drawn) {
                return err(
                    n,
                    match key {
                        "BITMAP" => "a glyph has two BITMAP sections",
                        "STARTCHAR" => "STARTCHAR before the last glyph's ENDCHAR",
                        "ENCODING" | "SWIDTH" | "DWIDTH" | "SWIDTH1" | "DWIDTH1" | "VVECTOR" | "BBX" => "a glyph's measurements come after its bitmap",
                        _ => "a glyph's bitmap has more rows than its BBX says",
                    },
                );
            }
            match key {
                "STARTCHAR" => {
                    if glyph.is_some() {
                        return err(n, "STARTCHAR before the last glyph's ENDCHAR");
                    }
                    glyph = Some(Record::default());
                    rows.fill(0);
                }
                "ENCODING" => {
                    let Some(g) = glyph.as_mut() else { continue };
                    match ints::<1>(&mut words) {
                        Some([cp]) => g.encoding = Some(cp),
                        None => return err(n, "ENCODING is not followed by a number"),
                    }
                }
                "DWIDTH" => {
                    if let Some(g) = glyph.as_mut() {
                        g.advance = ints::<1>(&mut words).map(|v| v[0]);
                    }
                }
                "BBX" => {
                    let Some(g) = glyph.as_mut() else { continue };
                    match ints::<4>(&mut words) {
                        Some(b) if (0..=MAX_BOX).contains(&b[0]) && (0..=MAX_BOX).contains(&b[1]) => g.bbx = Some(b),
                        Some(_) => return err(n, "BBX is negative or far too large"),
                        None => return err(n, "BBX needs four numbers"),
                    }
                }
                "BITMAP" => {
                    let Some(g) = glyph.as_mut() else {
                        return err(n, "BITMAP outside a glyph");
                    };
                    let Some([bw, bh, bx, by]) = g.bbx else {
                        return err(n, "BITMAP before the glyph's BBX");
                    };
                    g.drawn = true;
                    let advance = g.advance.or(head.advance).unwrap_or(cell_w);
                    // A row holds 32 pixels; a wider glyph is dropped at ENDCHAR.
                    let width = if advance == 2 * cell_w { (2 * cell_w).min(32) } else { cell_w };
                    let top = head.ascent - (bh + by);
                    let left = bx;
                    let digits = 2 * ((bw + 7) / 8) as usize;
                    for r in 0..bh {
                        let Some((j, data)) = lines.next() else {
                            return err(n, "the file stops inside a glyph's bitmap");
                        };
                        let data = data.trim().as_bytes();
                        if data.len() < digits || !data.iter().all(|b| b.is_ascii_hexdigit()) {
                            return err(j + 1, format!("expected a bitmap row of {digits} hex digits"));
                        }
                        let y = top + r;
                        if !(0..cell_h).contains(&y) {
                            continue;
                        }
                        let mut bits = 0u32;
                        for (k, &d) in data[..digits].iter().enumerate() {
                            let nibble = (d as char).to_digit(16).unwrap_or(0);
                            for b in 0..4i64 {
                                let col = k as i64 * 4 + b;
                                let x = left + col;
                                if nibble >> (3 - b) & 1 == 1 && col < bw && (0..width).contains(&x) {
                                    bits |= 1 << (31 - x);
                                }
                            }
                        }
                        rows[y as usize] |= bits;
                    }
                }
                "ENDCHAR" => {
                    let Some(g) = glyph.take() else {
                        return err(n, "ENDCHAR with no STARTCHAR");
                    };
                    let advance = g.advance.or(head.advance).unwrap_or(cell_w);
                    if advance != cell_w && advance != 2 * cell_w {
                        return err(
                            n,
                            format!("a glyph advances {advance} pixels in a face {cell_w} wide; only fixed-pitch faces can be used"),
                        );
                    }
                    // Unencoded glyphs and wide glyphs too wide for a row are left out.
                    let cp = g.encoding.and_then(|cp| u32::try_from(cp).ok()).filter(|&cp| char::from_u32(cp).is_some());
                    if let Some(cp) = cp {
                        if advance <= 32 {
                            font.insert(cp, advance as u32, &rows);
                        }
                    }
                }
                "ENDFONT" => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }

        if glyph.is_some() || !ended {
            return err(0, "the file stops before ENDFONT");
        }
        if font.is_empty() {
            return err(0, "no glyphs found");
        }
        let ascent = head.ascent.clamp(1, cell_h) as u32;
        Ok(font.finish(Some(ascent)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(props: &str, glyphs: &str) -> String {
        format!("STARTFONT 2.1\nFONT -test-\nSIZE 8 75 75\n{props}\nCHARS 1\n{glyphs}ENDFONT\n")
    }

    fn glyph(cp: i64, dwidth: i64, bbx: &str, rows: &[&str]) -> String {
        format!("STARTCHAR g\nENCODING {cp}\nSWIDTH 500 0\nDWIDTH {dwidth} 0\nBBX {bbx}\nBITMAP\n{}\nENDCHAR\n", rows.join("\n"))
    }

    const PROPS: &str = "FONTBOUNDINGBOX 6 8 0 -2\nSTARTPROPERTIES 2\nFONT_ASCENT 6\nFONT_DESCENT 2\nENDPROPERTIES";

    #[test]
    fn a_full_cell_glyph_lands_row_for_row() {
        let rows = ["FC", "84", "84", "84", "84", "FC", "00", "30"];
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "6 8 0 -2", &rows))).unwrap();
        assert_eq!((f.cell(), f.ascent(), f.len()), ((6, 8), 6, 1));
        let g = f.glyph('A').unwrap();
        assert_eq!(g.width, 6);
        assert_eq!(g.rows, &[0xFC00_0000, 0x8400_0000, 0x8400_0000, 0x8400_0000, 0x8400_0000, 0xFC00_0000, 0, 0x3000_0000]);
    }

    #[test]
    fn a_tight_box_is_placed_by_its_offsets() {
        // Two rows of ink standing on the baseline, two pixels in from the left.
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "2 2 2 0", &["C0", "40"]))).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0, 0, 0, 0, 0x3000_0000, 0x1000_0000, 0, 0]);
        // One row hanging one pixel below the baseline: a descender.
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "6 1 0 -1", &["FC"]))).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0, 0, 0, 0, 0, 0, 0xFC00_0000, 0]);
    }

    #[test]
    fn ink_outside_the_cell_is_cut_off() {
        // Sixteen pixels of ink, shifted left by three: only six columns survive.
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "16 12 -3 -4", &["FFFF"; 12]))).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0xFC00_0000; 8]);
        // Pad bits beyond BBw are not ink.
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "3 1 0 0", &["FF"]))).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows[5], 0xE000_0000);
        // Extra digits on a row are ignored, as other readers do.
        let f = BitmapFont::from_bdf("t", &face(PROPS, &glyph(65, 6, "3 1 0 0", &["A0FFFF"]))).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows[5], 0xA000_0000);
    }

    #[test]
    fn wide_glyphs_are_found_by_their_advance_not_the_font_box() {
        // The font box is the narrow cell, as in gallant.bdf.
        let narrow = glyph(65, 6, "6 8 0 -2", &["FC"; 8]);
        let wide = glyph(0x4E00, 12, "12 8 0 -2", &["FFF0"; 8]);
        let f = BitmapFont::from_bdf("t", &face(PROPS, &format!("{wide}{narrow}"))).unwrap();
        assert_eq!((f.cell(), f.len(), f.wide_len()), ((6, 8), 2, 1));
        let g = f.glyph('\u{4E00}').unwrap();
        assert_eq!((g.width, g.rows[0]), (12, 0xFFF0_0000));
        assert_eq!(f.glyph('A').unwrap().width, 6);

        // A font box as wide as the wide glyphs gives the same face.
        let props = PROPS.replace("FONTBOUNDINGBOX 6", "FONTBOUNDINGBOX 12");
        let f = BitmapFont::from_bdf("t", &face(&props, &format!("{narrow}{wide}"))).unwrap();
        assert_eq!((f.cell(), f.wide_len()), ((6, 8), 1));
    }

    #[test]
    fn glyphs_are_placed_from_their_own_origin_whatever_the_font_box_says() {
        // The font box as a converter writes it: round every glyph, one of
        // which hangs two pixels left of its origin.
        let a = glyph(65, 6, "6 8 0 -2", &["FC"; 8]);
        let accent = glyph(0x301, 6, "3 2 -2 4", &["E0", "E0"]);
        for font_box in ["FONTBOUNDINGBOX 6 8 0 -2", "FONTBOUNDINGBOX 8 8 -2 -2", "FONTBOUNDINGBOX 40 40 -17 -9"] {
            let props = PROPS.replace("FONTBOUNDINGBOX 6 8 0 -2", font_box);
            let f = BitmapFont::from_bdf("t", &face(&props, &format!("{a}{accent}"))).unwrap();
            assert_eq!(f.cell(), (6, 8), "{font_box}");
            assert_eq!(f.glyph('A').unwrap().rows, &[0xFC00_0000; 8], "{font_box}");
            // What hangs left of the origin is cut off; the column inside stays.
            assert_eq!(&f.raw_glyph(0x301).unwrap().rows[..2], &[0x8000_0000; 2], "{font_box}");
        }
    }

    #[test]
    fn nothing_but_endchar_may_follow_a_bitmap() {
        let narrow = glyph(65, 6, "6 8 0 -2", &["FC"; 8]);
        // The advance that would have made this glyph wide comes too late.
        let late = "STARTCHAR w
ENCODING 19968
BBX 12 8 0 -2
BITMAP
".to_string() + &"FFF0
".repeat(8) + "DWIDTH 12 0
ENDCHAR
";
        let e = BitmapFont::from_bdf("t", &face(PROPS, &format!("{narrow}{late}"))).unwrap_err();
        assert!(e.what.contains("after its bitmap"), "{e}");
        for stray in ["BBX 6 8 0 -2", "ENCODING 66", "FC", "COMMENT late"] {
            let bad = glyph(65, 6, "6 8 0 -2", &["FC"; 8]).replace("ENDCHAR", &format!("{stray}
ENDCHAR"));
            assert!(BitmapFont::from_bdf("t", &face(PROPS, &bad)).is_err(), "{stray}");
        }
    }

    #[test]
    fn a_proportional_face_is_refused() {
        let a = glyph(65, 6, "6 8 0 -2", &["FC"; 8]);
        let b = glyph(66, 9, "6 8 0 -2", &["FC"; 8]);
        let e = BitmapFont::from_bdf("t", &face(PROPS, &format!("{a}{b}"))).unwrap_err();
        assert!(e.what.contains("fixed-pitch"), "{e}");
        let zero = glyph(0x301, 0, "6 8 0 -2", &["FC"; 8]);
        assert!(BitmapFont::from_bdf("t", &face(PROPS, &format!("{a}{zero}"))).is_err());
    }

    #[test]
    fn unencoded_glyphs_are_skipped() {
        let a = glyph(65, 6, "6 8 0 -2", &["FC"; 8]);
        let none = glyph(-1, 6, "6 8 0 -2", &["FC"; 8]).replace("ENCODING -1", "ENCODING -1 200");
        let surrogate = glyph(0xD800, 6, "6 8 0 -2", &["FC"; 8]);
        let beyond = glyph(0x110000, 6, "6 8 0 -2", &["FC"; 8]);
        let f = BitmapFont::from_bdf("t", &face(PROPS, &format!("{none}{a}{surrogate}{beyond}"))).unwrap();
        assert_eq!(f.code_points(), vec![65]);
    }

    #[test]
    fn height_falls_back_to_the_font_box() {
        let rows = ["FC"; 8];
        let f = BitmapFont::from_bdf("t", &face("FONTBOUNDINGBOX 6 8 0 -2", &glyph(65, 6, "6 8 0 -2", &rows))).unwrap();
        assert_eq!((f.cell(), f.ascent()), ((6, 8), 6));
        assert!(BitmapFont::from_bdf("t", &face("", &glyph(65, 6, "6 8 0 -2", &rows))).is_err());
    }

    #[test]
    fn damaged_files_are_refused() {
        let good = face(PROPS, &glyph(65, 6, "6 8 0 -2", &["FC"; 8]));
        assert!(BitmapFont::from_bdf("t", &good).is_ok());
        for (from, to) in [
            ("ENDFONT\n", ""),
            ("STARTFONT 2.1\n", ""),
            ("ENDCHAR\n", ""),
            ("BBX 6 8 0 -2\n", ""),
            ("BBX 6 8 0 -2", "BBX 6 9 0 -2"),
            ("BBX 6 8 0 -2", "BBX 6 7 0 -2"),
            ("BBX 6 8 0 -2", "BBX 6 8 0"),
            ("BBX 6 8 0 -2", "BBX -6 8 0 -2"),
            ("BBX 6 8 0 -2", "BBX 99999999999 8 0 -2"),
            ("BBX 6 8 0 -2", "BBX 16 8 0 -2"),
            ("ENCODING 65", "ENCODING A"),
            ("DWIDTH 6 0", "DWIDTH six"),
            ("FC\nENDCHAR", "FG\nENDCHAR"),
            ("FONT_ASCENT 6", "FONT_ASCENT 600"),
            ("FONT_DESCENT 2", "FONT_DESCENT -6"),
            ("DWIDTH 6 0", "DWIDTH 40 0"),
        ] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good, "{from:?}");
            assert!(BitmapFont::from_bdf("t", &bad).is_err(), "{from:?} -> {to:?}");
        }
        assert!(BitmapFont::from_bdf("t", "").is_err());
        assert!(BitmapFont::from_bdf("t", "0041:FF\n").is_err());
        assert!(BitmapFont::from_bdf("t", "STARTFONT 2.1\nENDFONT\n").is_err());
    }
}
