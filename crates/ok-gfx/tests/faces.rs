//! The three faces inside the binary, and the strings the fonts
//! verification says every face and scale must survive.

use ok_gfx::{clusters, face_notices, gallant, spleen, terminus, AlphaGlyph, BitmapFont, FaceSet, Frame, OutlineFace, Pen, Rgb};

const GALLANT_HEX: &str = include_str!("../../../assets/fonts/gallant/gallant.hex");
const SPLEEN_BDF: &str = include_str!("../../../assets/fonts/spleen/spleen-12x24.bdf");
const TERMINUS_BDF: &str = include_str!("../../../assets/fonts/terminus/ter-u22n.bdf");

const INK: Rgb = Rgb(255, 255, 255);
const GROUND: Rgb = Rgb(0, 0, 0);

/// Section 13 of the verification: the research's string, then marks typed
/// as separate characters, a two-mark letter, a no-break space, a
/// zero-width space, a soft hyphen, a tab, two wide characters, an emoji
/// above U+FFFF, a real U+FFFD, and a straight and a curly apostrophe.
const TEST_STRING: &str = "mitsqanaqan\u{0313} \u{2014} \u{201C}double\u{201D} \u{2018}single\u{2019} \u{2026} \u{2022} \u{00A7} \u{00B6} \
    \u{00E9} \u{00F1} \u{014D} \u{0161} \u{0152} \u{0153} \u{0294} \u{A78C} \u{2190} \u{2192} \
    \u{250C}\u{2500}\u{252C}\u{2500}\u{2510} \u{2502}\u{2588}\u{2502}\u{2591}\u{2502} \u{2514}\u{2500}\u{2534}\u{2500}\u{2518} \
    l\u{0313} k\u{0313} t\u{0313} i\u{0313} e\u{0302}\u{0301} a\u{00A0}b a\u{200B}b a\u{00AD}b a\tb \
    \u{2614}\u{30F3} \u{1F600} \u{FFFD} '\u{2019}";

/// Section 11 of the verification, as clusters: a mark with nothing before
/// it, fifty marks on one letter, a mark after a box-drawing bar and after
/// U+FFFD, a wide character, a character above U+FFFF. Then what a caller
/// should never pass and might.
fn crash_list() -> Vec<String> {
    let mut list: Vec<String> = [
        "\u{0313}",
        "\u{0301}\u{0313}\u{0323}",
        "\u{2502}\u{0301}",
        "\u{2588}\u{0313}",
        "\u{FFFD}\u{0301}",
        "\u{2614}",
        "\u{2614}\u{0313}",
        "\u{30F3}",
        "\u{1F600}",
        "\u{1F600}\u{0301}\u{FE0F}",
        "\u{10FFFF}",
        "S\u{030C}\u{0301}",
        "\u{00C9}\u{0301}\u{0302}\u{0303}\u{0304}",
        "",
        "\t",
        "\n",
        "\r",
        "\r\n",
        "\0",
        "\u{7F}",
        "\u{85}",
        "\u{200B}",
        "\u{200B}\u{200B}\u{200B}",
        "\u{FEFF}",
        "\u{00A0}\u{0313}",
        "ab",
        "a\tb\n",
        "g\u{0323}\u{0323}\u{0323}\u{0323}",
        "\u{0E01}\u{0E34}",
        // Marks on characters that are drawn as boxes, on every side at once.
        "\u{0E01}\u{0301}\u{0323}\u{0335}\u{0E34}",
        "\u{0007}\u{0313}\u{0323}",
        "\u{200B}\u{0301}",
        "\u{1F600}\u{0313}\u{0301}\u{0323}\u{0331}",
        // A mark no face has, alone and under another.
        "\u{1DC4}",
        "\u{1DC4}\u{0301}",
        // Marks below on a tail, on a block and on a line; marks piled on an accented capital.
        "y\u{0331}\u{0323}\u{0330}\u{0332}",
        "\u{2588}\u{0323}\u{0331}",
        "\u{2500}\u{0332}\u{0323}",
        "\u{0160}\u{0313}\u{0301}\u{0302}\u{0303}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    list.push(format!("n{}", "\u{0313}".repeat(50)));
    list.push(format!("l{}", "\u{0301}\u{0323}\u{0335}\u{0313}".repeat(13)));
    list.push("\u{0313}".repeat(50));
    list.push("x".repeat(5000));
    list
}

/// An outline face holding Gallant's letters as blurred coverage, so the
/// outline paths see what a rasterizer gives: soft edges and offsets.
fn soft_outline(text: &str) -> FaceSet {
    let mut set = FaceSet::new(OutlineFace::new("soft", 22, (13, 24), 18));
    for ch in set.missing(text) {
        let glyph = gallant().glyph(ch).map(|g| {
            let (w, h) = (g.width as usize + 2, g.rows.len() + 2);
            let mut coverage = vec![0u8; w * h];
            for (r, &row) in g.rows.iter().enumerate() {
                for c in 0..g.width as usize {
                    if row & (0x8000_0000 >> c) != 0 {
                        coverage[(r + 1) * w + c + 1] = 255;
                        for (dr, dc) in [(0, 1), (2, 1), (1, 0), (1, 2)] {
                            let at = (r + dr) * w + c + dc;
                            coverage[at] = coverage[at].max(60);
                        }
                    }
                }
            }
            AlphaGlyph { width: w as u32, height: h as u32, left: -1, top: 0, coverage }
        });
        set.active_mut().as_outline_mut().expect("an outline face").insert(ch, glyph);
    }
    set
}

fn top_ink_row(frame: &Frame) -> Option<usize> {
    let w = frame.width();
    frame.pixels().chunks(w).position(|row| row.iter().any(|&p| p != GROUND.px()))
}

#[test]
fn gallant_loads_with_the_verified_counts() {
    let f = gallant();
    assert_eq!(f.name(), "Gallant");
    assert_eq!(f.cell(), (12, 22));
    assert_eq!(f.len(), 5320);
    assert_eq!(f.wide_len(), 88);
    assert_eq!((f.ascent(), f.descent(), f.x_top()), (17, 5, 7));
    assert_eq!(f.underline_rows(), (18, 2));
    assert_eq!(f.strike_rows(), (11, 2));

    let all = f.code_points();
    assert_eq!(all.len(), 5320);
    assert!(all.windows(2).all(|p| p[0] < p[1]));
    assert_eq!((all[0], all[all.len() - 1]), (0x0000, 0xFFFF));
    let wide: Vec<u32> = all.iter().copied().filter(|&cp| f.raw_glyph(cp).is_some_and(|g| g.width == 24)).collect();
    assert_eq!(wide.len(), 88);
    assert_eq!((wide[0], wide[87]), (0x231A, 0x30F3));
    assert!(wide.contains(&0x2614) && wide.contains(&0x2B50) && wide.contains(&0x2B55));
}

#[test]
fn gallant_packed_by_the_build_is_gallant_hex_glyph_for_glyph() {
    let from_text = BitmapFont::from_hex("Gallant", GALLANT_HEX).expect("gallant.hex");
    let packed = gallant();
    assert_eq!(from_text.code_points(), packed.code_points());
    for cp in packed.code_points() {
        let (a, b) = (from_text.raw_glyph(cp).unwrap(), packed.raw_glyph(cp).unwrap());
        assert_eq!((a.width, a.rows), (b.width, b.rows), "U+{cp:04X}");
    }
    assert_eq!((from_text.ascent(), from_text.x_top(), from_text.underline_rows()), (17, 7, (18, 2)));
    assert_eq!((from_text.len(), from_text.wide_len()), (5320, 88));
    assert_eq!(BitmapFont::from_bytes("Gallant", GALLANT_HEX.as_bytes()).unwrap().len(), 5320);
}

#[test]
fn gallant_glyphs_match_the_rows_read_off_the_source() {
    let f = gallant();
    // Capital A, from the worked example in the research.
    let a: Vec<u32> = f.glyph('A').unwrap().rows.iter().map(|r| r >> 16).collect();
    let want = [0, 0, 0, 0x0600, 0x0600, 0x0b00, 0x0b00, 0x0900, 0x1180, 0x1180, 0x1080, 0x3fc0, 0x20c0, 0x2040, 0x4060, 0x4060, 0xe0f0, 0, 0, 0, 0, 0];
    assert_eq!(a, want);
    // The acute and the comma above, at capital height.
    let rows = |ch: char| -> Vec<u32> { f.glyph(ch).unwrap().rows.iter().map(|r| r >> 16).collect() };
    assert_eq!(&rows('\u{0301}')[..4], &[0x0300, 0x0600, 0x0c00, 0]);
    assert_eq!(&rows('\u{0313}')[..5], &[0x0f00, 0x0f00, 0x0300, 0x0e00, 0]);
    // Box drawing reaches the cell edges, so lines join.
    assert_eq!(&rows('\u{2500}')[10..12], &[0xfff0, 0xfff0]);
    assert!(rows('\u{2502}').iter().all(|&r| r == 0x0600));
    assert_eq!(f.glyph('\u{30F3}').unwrap().width, 24);

    // What the face has and lacks, as the gap scan found.
    for has in ['\u{0131}', '\u{0237}', '\u{02BC}', '\u{0294}', '\u{2019}', '\u{2026}', '\u{25CC}', '\u{A78B}', '\u{A78C}', '\u{FFFD}'] {
        assert!(f.has(has), "U+{:04X}", has as u32);
    }
    assert!((0x0300..=0x036F).all(|cp| f.raw_glyph(cp).is_some()));
    assert!((0x2500..=0x259F).all(|cp| f.raw_glyph(cp).is_some()));
    for lacks in [0x00A0, 0x0080, 0x009F, 0x1DC0, 0x2065, 0xFE00, 0xFEFF, 0x1F600] {
        assert!(f.raw_glyph(lacks).is_none(), "U+{lacks:04X}");
    }

    // The no-break space is a blank cell all the same, and the control
    // slots hold pictures that text never sees.
    assert!(f.glyph('\u{00A0}').unwrap().rows.iter().all(|&r| r == 0));
    for cp in (0x00..=0x1Fu32).chain([0x7F]) {
        let picture = f.raw_glyph(cp).expect("a stored picture");
        assert!(picture.rows.iter().any(|&r| r != 0), "U+{cp:04X}");
        assert!(f.glyph(char::from_u32(cp).unwrap()).is_none());
    }
}

#[test]
fn spleen_loads_with_the_verified_counts() {
    let f = spleen();
    assert_eq!((f.name(), f.cell(), f.len(), f.wide_len()), ("Spleen", (12, 24), 950, 0));
    assert_eq!((f.ascent(), f.descent()), (19, 5));
    assert_eq!(BitmapFont::from_bdf("s", SPLEEN_BDF).unwrap().code_points(), f.code_points());
    // No arrows, no combining marks, no U+FFFD; the em dash is one row.
    for lacks in ['\u{2190}', '\u{2192}', '\u{0301}', '\u{0313}', '\u{FFFD}'] {
        assert!(!f.has(lacks), "U+{:04X}", lacks as u32);
    }
    let dash = f.glyph('\u{2014}').unwrap();
    assert_eq!(dash.ink_rows(), Some((12, 12)));
    assert_eq!(dash.rows[12] >> 16, 0x3FC0);
    assert!(f.has('\u{E0A0}') && f.has('\u{00E9}') && f.has('\u{2588}'));
    // DEFAULT_CHAR is a blank in this file; a missing character must not become one.
    assert!(f.glyph('\u{4E00}').is_none());
}

#[test]
fn terminus_loads_with_the_verified_counts() {
    let f = terminus();
    assert_eq!((f.name(), f.cell(), f.len(), f.wide_len()), ("Terminus", (11, 22), 1356, 0));
    assert_eq!((f.ascent(), f.descent()), (17, 5));
    assert_eq!(BitmapFont::from_bdf("t", TERMINUS_BDF).unwrap().code_points(), f.code_points());
    assert_eq!(BitmapFont::from_bytes("t", TERMINUS_BDF.as_bytes()).unwrap().len(), 1356);
    assert!(f.has('\u{0301}') && !f.has('\u{0313}') && f.has('\u{FFFD}'));
    // Capital A from the research: 1E00 on row 3, the bar 7F80 on row 10.
    let a = f.glyph('A').unwrap();
    assert_eq!((a.width, a.rows[3] >> 16, a.rows[10] >> 16), (11, 0x1E00, 0x7F80));
    assert_eq!(a.ink_rows(), Some((3, 16)));
    // The file stores a picture at code point 0; text does not see it.
    assert!(f.raw_glyph(0).is_some() && f.glyph('\0').is_none());
}

#[test]
fn every_built_in_face_brings_its_licence() {
    let notices = face_notices();
    assert_eq!(notices.len(), 5);
    assert!(notices.iter().all(|(heading, text)| !heading.is_empty() && text.len() > 1000));
}

#[test]
fn the_test_string_and_the_crash_list_draw_at_every_scale_and_position() {
    let mut everything: Vec<String> = clusters(TEST_STRING).map(|c| c.to_string()).collect();
    everything.extend(crash_list());
    let text: String = everything.concat();

    let tiny = BitmapFont::from_hex("tiny", "# Width: 5\n# Height: 7\n0041:70888888F88888\n0301:10200000000000\n").unwrap();
    let wide_only = BitmapFont::from_hex("w", "# Width: 16\n# Height: 3\n4E00:FFFFFFFF0000FFFFFFFFFFFF\n").unwrap();
    let sets = [
        FaceSet::new(gallant()),
        FaceSet::new(spleen()),
        FaceSet::new(terminus()),
        FaceSet::new(tiny.clone()),
        FaceSet::new(wide_only),
        FaceSet::with_fallback(gallant(), tiny),
        soft_outline(&text),
        FaceSet::new(OutlineFace::new("empty", 0, (0, 0), 0)),
        FaceSet::new(OutlineFace::new("big", 132, (80, 160), 120)),
    ];
    let spots = [
        (0, 0),
        (-5, -9),
        (-11, 3),
        (95, 70),
        (99, 79),
        (40, -21),
        (-1000, -1000),
        (5000, 5000),
        (i32::MIN, i32::MIN),
        (i32::MAX, i32::MAX),
        (i32::MAX - 3, 0),
        (0, i32::MIN + 3),
    ];

    for set in &sets {
        for scale in 1..=3u32 {
            let mut frame = Frame::new(100, 80);
            frame.clear(GROUND);
            for (i, cluster) in everything.iter().enumerate() {
                let (x, y) = spots[i % spots.len()];
                let pen = Pen::new(INK).scale(scale).bold(i % 3 == 0).alpha(if i % 5 == 0 { 140 } else { 255 });
                let n = set.draw_cluster(&mut frame, cluster, x, y, pen);
                assert!(n <= 2 && (n > 0) != cluster.is_empty(), "{cluster:?}");
                assert_eq!(n, set.cluster_cells(cluster));
                for cells in [0, 1, 2, 7] {
                    assert_eq!(set.draw_cluster_in(&mut frame, cluster, y, x, cells, pen), n);
                }
            }
            for &(x, y) in &spots {
                set.draw_text(&mut frame, TEST_STRING, x, y, Pen::new(INK).scale(scale));
                set.underline(&mut frame, x, y, 40, Pen::new(INK).scale(scale));
                set.strike(&mut frame, x, y, 40, Pen::new(INK).scale(scale));
                set.draw_hex_box(&mut frame, 0x10FFFF, x, y, 2, Pen::new(INK).scale(scale));
            }
        }
        // Scales no setting allows, and frames with no room at all.
        for scale in [0, 4, 64, 65, u32::MAX] {
            let mut frame = Frame::new(30, 30);
            set.draw_text(&mut frame, "n\u{0313}\u{2614}\u{1F600}", -3, -3, Pen::new(INK).scale(scale).bold(true));
        }
        for (w, h) in [(0, 0), (1, 1), (0, 50), (50, 0)] {
            let mut frame = Frame::new(w, h);
            set.draw_text(&mut frame, TEST_STRING, 0, 0, Pen::new(INK));
        }
    }
}

#[test]
fn the_test_string_leaves_no_cell_empty_that_holds_a_character() {
    for face in [gallant(), spleen(), terminus()] {
        let set = FaceSet::new(face);
        let (cw, ch) = (set.cell().0 as usize, set.cell().1 as usize);
        for cluster in clusters(TEST_STRING) {
            let first = cluster.chars().next().unwrap();
            let mut frame = Frame::new(cw * 2, ch);
            frame.clear(GROUND);
            set.draw_cluster(&mut frame, cluster, 0, 0, Pen::new(INK));
            let inked = frame.pixels().iter().any(|&p| p != GROUND.px());
            let quiet = matches!(first, ' ' | '\t' | '\u{00A0}');
            assert_eq!(inked, !quiet, "{} draws {cluster:?} wrongly", face.name());
        }
    }
}

#[test]
fn a_comma_above_puts_ink_above_the_letter_in_every_face_at_every_scale() {
    for face in [gallant(), spleen(), terminus()] {
        let set = FaceSet::new(face);
        let (cw, ch) = (set.cell().0 as usize, set.cell().1 as usize);
        for letter in ['n', 'l', 'k', 't', 'i'] {
            for scale in 1..=3usize {
                let pen = Pen::new(INK).scale(scale as u32);
                let mut plain = Frame::new(cw * scale, ch * scale);
                let mut marked = Frame::new(cw * scale, ch * scale);
                plain.clear(GROUND);
                marked.clear(GROUND);
                set.draw_cluster(&mut plain, &letter.to_string(), 0, 0, pen);
                set.draw_cluster(&mut marked, &format!("{letter}\u{0313}"), 0, 0, pen);
                let (letter_top, mark_top) = (top_ink_row(&plain).unwrap(), top_ink_row(&marked).unwrap());
                assert!(mark_top < letter_top, "{} {letter}: mark at {mark_top}, letter at {letter_top}", face.name());
                assert!(mark_top % scale == 0);
                // The mark is thirteen pixels of ink that the bare letter did not have up there.
                let above = |f: &Frame| f.pixels()[..face.x_top() as usize * scale * cw * scale].iter().filter(|&&p| p != GROUND.px()).count();
                assert!(above(&marked) >= 13 * scale * scale, "{} {letter}", face.name());
            }
        }
    }
}

#[test]
fn mitsqanaqan_ends_in_an_n_with_its_comma() {
    for face in [gallant(), spleen(), terminus()] {
        let set = FaceSet::new(face);
        let (cw, ch) = (set.cell().0 as usize, set.cell().1 as usize);
        let mut frame = Frame::new(cw * 12, ch);
        frame.clear(GROUND);
        let end = set.draw_text(&mut frame, "mitsqanaqan\u{0313}", 0, 0, Pen::new(INK));
        assert_eq!(end as usize, cw * 11, "eleven cells: the comma takes none");

        let x_top = face.x_top() as usize;
        let above = |cell: usize| -> usize {
            (0..x_top.saturating_sub(1)).map(|y| (cell * cw..(cell + 1) * cw).filter(|&x| frame.get(x as i32, y as i32) == Some(INK)).count()).sum()
        };
        // Cells 5 and 6 are a plain a and a plain n; cell 10 is the n with the comma.
        assert_eq!(above(5), 0);
        assert_eq!(above(6), 0);
        assert_eq!(above(10), 13);
        // A clear row parts the comma from the letter.
        assert!((10 * cw..11 * cw).all(|x| frame.get(x as i32, x_top as i32 - 1) == Some(GROUND)));
        // From the x-height down the two n's are the same picture.
        for y in x_top..ch {
            for x in 0..cw {
                assert_eq!(frame.get((6 * cw + x) as i32, y as i32), frame.get((10 * cw + x) as i32, y as i32));
            }
        }
    }
}

/// `gallant.hex` rewritten as BDF in the shape of the project's own
/// `gallant.bdf`, which is not vendored here: the font's bounding box is the
/// narrow cell, and the 88 wide glyphs carry `DWIDTH 24 0` and a `BBX` twice
/// as wide as that box. With `tight`, every glyph's box is shrunk to its ink
/// and placed by its offsets instead of filling the cell.
fn gallant_as_bdf(tight: bool) -> String {
    let face = gallant();
    let mut bdf = String::from(
        "STARTFONT 2.1\nFONT -sun-gallant-medium-r-normal--22-220-75-75-C-120-ISO10646-1\nSIZE 22 75 75\nFONTBOUNDINGBOX 12 22 0 -5\n\
         STARTPROPERTIES 3\nFONT_ASCENT 17\nFONT_DESCENT 5\nDEFAULT_CHAR 65533\nENDPROPERTIES\nCHARS 5320\n",
    );
    for cp in face.code_points() {
        let g = face.raw_glyph(cp).expect("a listed glyph");
        let inked: Vec<usize> = (0..g.rows.len()).filter(|&r| g.rows[r] != 0).collect();
        let all = g.rows.iter().fold(0u32, |a, &r| a | r);
        // The box: first column, width, first row, height.
        let (c0, bw, r0, bh) = match (tight, inked.first(), inked.last()) {
            (false, _, _) => (0, g.width, 0, g.rows.len()),
            (true, Some(&first), Some(&last)) => {
                let c0 = all.leading_zeros();
                (c0, 32 - all.trailing_zeros() - c0, first, last - first + 1)
            }
            (true, _, _) => (0, 0, 0, 0),
        };
        let below = 17 - (r0 + bh) as i32;
        bdf.push_str(&format!("STARTCHAR U{cp:04X}\nENCODING {cp}\nSWIDTH 500 0\nDWIDTH {} 0\nBBX {bw} {bh} {c0} {below}\nBITMAP\n", g.width));
        let digits = 2 * bw.div_ceil(8) as usize;
        for &row in &g.rows[r0..r0 + bh] {
            let hex = format!("{:08X}", row << c0);
            bdf.push_str(&hex[..digits]);
            bdf.push('\n');
        }
        bdf.push_str("ENDCHAR\n");
    }
    bdf.push_str("ENDFONT\n");
    bdf
}

#[test]
fn a_bdf_with_wide_glyphs_outside_its_font_box_loads_whole() {
    for tight in [false, true] {
        let bdf = gallant_as_bdf(tight);
        assert_eq!(bdf.matches("\nDWIDTH 24 0\n").count(), 88);
        assert_eq!(bdf.contains("\nBBX 24 22 0 -5\n"), !tight);
        let read = BitmapFont::from_bdf("Gallant", &bdf).expect("a whole face");
        assert_eq!((read.cell(), read.ascent(), read.len(), read.wide_len()), ((12, 22), 17, 5320, 88));
        assert_eq!((read.x_top(), read.underline_rows(), read.strike_rows()), (7, (18, 2), (11, 2)));
        for cp in gallant().code_points() {
            let (a, b) = (gallant().raw_glyph(cp).unwrap(), read.raw_glyph(cp).expect("the same glyphs"));
            assert_eq!((a.width, a.rows), (b.width, b.rows), "U+{cp:04X}, tight {tight}");
        }
    }
}

#[test]
fn gallant_survives_a_trip_through_psf2() {
    // Every single-width glyph in code point order, so glyph 65 is the A,
    // then a Unicode table naming each glyph's character.
    let face = gallant();
    let narrow: Vec<u32> = face.code_points().into_iter().filter(|&cp| face.raw_glyph(cp).is_some_and(|g| g.width == 12)).collect();
    assert_eq!(narrow.len(), 5232);
    let mut file = vec![0x72, 0xB5, 0x4A, 0x86];
    for v in [0u32, 32, 1, narrow.len() as u32, 44, 22, 12] {
        file.extend(v.to_le_bytes());
    }
    for &cp in &narrow {
        for &row in face.raw_glyph(cp).unwrap().rows {
            file.extend(&row.to_be_bytes()[..2]);
        }
    }
    // The worked example in the research: the A is 44 bytes at 32 + 65 * 44.
    let a = [
        0, 0, 0, 0, 0, 0, 0x06, 0, 0x06, 0, 0x0B, 0, 0x0B, 0, 0x09, 0, 0x11, 0x80, 0x11, 0x80, 0x10, 0x80, 0x3F, 0xC0, 0x20, 0xC0, 0x20, 0x40,
        0x40, 0x60, 0x40, 0x60, 0xE0, 0xF0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(&file[2892..2892 + 44], &a);
    for &cp in &narrow {
        let ch = char::from_u32(cp).expect("a character");
        file.extend(ch.encode_utf8(&mut [0; 4]).as_bytes());
        file.push(0xFF);
    }

    let read = BitmapFont::from_psf("Gallant", &file).expect("a whole face");
    assert_eq!((read.cell(), read.len(), read.wide_len()), ((12, 22), 5232, 0));
    assert_eq!(read.code_points(), narrow);
    for &cp in &narrow {
        assert_eq!(read.raw_glyph(cp).unwrap().rows, face.raw_glyph(cp).unwrap().rows, "U+{cp:04X}");
    }
    // PSF says nothing of the baseline; it is read off the letters.
    assert_eq!((read.ascent(), read.x_top(), read.underline_rows()), (17, 7, (18, 2)));
    assert!(read.glyph('\t').is_none() && read.raw_glyph(9).is_some());
    assert_eq!(BitmapFont::from_bytes("Gallant", &file).unwrap().len(), 5232);
}

#[test]
fn a_wide_character_in_the_last_column_is_cut_at_the_edge_and_never_spilled() {
    for face in [gallant(), spleen(), terminus()] {
        let set = FaceSet::new(face);
        for scale in 1..=3i32 {
            let (cw, ch) = (set.cell().0 as i32 * scale, set.cell().1 as i32 * scale);
            let pen = Pen::new(INK).scale(scale as u32);
            let lit = |frame: &Frame, x0: i32, x1: i32| -> usize {
                (0..ch).map(|y| (x0..x1).filter(|&x| frame.get(x, y) == Some(INK)).count()).sum()
            };
            // Five columns; the character starts in the last. Given two
            // cells, half of it is past the edge of the frame.
            let mut frame = Frame::new(5 * cw as usize, ch as usize);
            frame.clear(GROUND);
            set.draw_cluster_in(&mut frame, "\u{2614}", 4 * cw, 0, 2, pen);
            assert!(lit(&frame, 4 * cw, 5 * cw) > 0, "{} x{scale}", face.name());
            assert_eq!(lit(&frame, 0, 4 * cw), 0, "{} x{scale}", face.name());
            // Given the one cell that is left, it is the box, whole.
            let mut one = Frame::new(5 * cw as usize, ch as usize);
            let mut boxed = Frame::new(5 * cw as usize, ch as usize);
            one.clear(GROUND);
            boxed.clear(GROUND);
            set.draw_cluster_in(&mut one, "\u{2614}", 4 * cw, 0, 1, pen);
            set.draw_hex_box(&mut boxed, 0x2614, 4 * cw, 0, 1, pen);
            assert!(one.pixels() == boxed.pixels(), "{} x{scale}", face.name());
            assert_eq!(lit(&one, 0, 4 * cw), 0);
            // Marks on it, an emoji above U+FFFF, and a clip narrower than the frame.
            for cluster in ["\u{2614}\u{0313}", "\u{30F3}\u{0301}\u{0323}", "\u{1F600}", "\u{1F600}\u{FE0F}"] {
                let mut clipped = Frame::new(5 * cw as usize, ch as usize);
                clipped.clear(GROUND);
                clipped.set_clip(ok_gfx::Rect::new(0, 0, 4 * cw + cw / 2, ch));
                set.draw_cluster_in(&mut clipped, cluster, 4 * cw, 0, 2, pen);
                assert_eq!(lit(&clipped, 0, 4 * cw) + lit(&clipped, 4 * cw + cw / 2, 5 * cw), 0, "{cluster:?}");
                assert!(lit(&clipped, 4 * cw, 4 * cw + cw / 2) > 0, "{} {cluster:?} x{scale}", face.name());
            }
        }
    }
}

#[test]
fn soft_edges_do_not_change_where_an_outline_faces_marks_are_placed() {
    // The soft face is Gallant one row lower in a 13 by 24 cell, each stroke
    // with a faint fringe. The hard face is the same letters in the same
    // cell as a bitmap. Where the soft face's full-strength pixels fall must
    // be where the bitmap's pixels fall: the fringe is not ink to be avoided.
    let list = [
        "n\u{0313}", "l\u{0313}", "k\u{0313}", "t\u{0313}", "i\u{0313}", "j\u{0313}", "f\u{0313}", "h\u{0313}", "b\u{0313}", "d\u{0313}",
        "K\u{0313}", "e\u{0302}\u{0301}", "s\u{030C}\u{0313}", "n\u{0313}\u{0301}", "y\u{0331}", "p\u{0331}", "g\u{0323}", "l\u{0335}", "\u{0268}\u{0301}",
    ];
    let soft = soft_outline(&list.concat());
    let mut lowered = String::from("# Width: 13\n# Height: 24\n");
    for cp in gallant().code_points() {
        let g = gallant().raw_glyph(cp).expect("a listed glyph");
        if g.width == 12 {
            let rows: String = g.rows.iter().map(|r| format!("{:04X}", r >> 16)).collect();
            lowered.push_str(&format!("{cp:04X}:0000{rows}0000\n"));
        }
    }
    let hard = FaceSet::new(BitmapFont::from_hex("Gallant, lowered", &lowered).expect("a whole face"));
    for set in [&soft, &hard] {
        assert_eq!((set.cell(), set.ascent(), set.active().x_top()), ((13, 24), 18, 8));
    }
    for cluster in list {
        let (mut a, mut b) = (Frame::new(13, 24), Frame::new(13, 24));
        a.clear(GROUND);
        b.clear(GROUND);
        assert_eq!(soft.draw_cluster(&mut a, cluster, 0, 0, Pen::new(INK)), 1);
        assert_eq!(hard.draw_cluster(&mut b, cluster, 0, 0, Pen::new(INK)), 1);
        for y in 0..24 {
            for x in 0..13 {
                assert_eq!(a.get(x, y) == Some(INK), b.get(x, y) == Some(INK), "{cluster:?} at {x},{y}");
            }
        }
        // And there is a fringe: something drawn that is neither ink nor ground.
        assert!(a.pixels().iter().any(|&p| p != INK.px() && p != GROUND.px()), "{cluster:?}");
    }
}
