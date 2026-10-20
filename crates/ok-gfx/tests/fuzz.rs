//! Damaged and meaningless input to the three face readers.
//!
//! A font file dropped into the `faces` folder can be anything: cut short by
//! a failed download, a gzip file with the wrong ending, another format, or
//! noise. Every case here must come back as an error or, where the damage
//! happens to leave a whole file, as a face that can be drawn with. None may
//! panic: a panic in a reader would stop the program before the page showed.
//!
//! The cases are driven by a small fixed-seed generator, so a failure names
//! a case number and that case fails again on the next run.

use std::panic::{catch_unwind, UnwindSafe};

use ok_gfx::{BitmapFont, FaceSet, FontError, Frame, Pen, Rgb};

const GALLANT_HEX: &str = include_str!("../../../assets/fonts/gallant/gallant.hex");
const SPLEEN_BDF: &str = include_str!("../../../assets/fonts/spleen/spleen-12x24.bdf");
const TERMINUS_BDF: &str = include_str!("../../../assets/fonts/terminus/ter-u22n.bdf");

/// Marsaglia's xorshift, 64 bits of state.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number from 0 up to but not including `n` (0 when `n` is 0).
    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 11) % n.max(1) as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 29) as u8
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len())]
    }

    /// Fewer than `limit` bytes of noise.
    fn noise(&mut self, limit: usize) -> Vec<u8> {
        let len = self.below(limit);
        (0..len).map(|_| self.byte()).collect()
    }
}

/// Runs a reader and turns a panic inside it into a failure that says which
/// case it was.
fn read<T>(what: &str, case: usize, reader: impl FnOnce() -> T + UnwindSafe) -> T {
    catch_unwind(reader).unwrap_or_else(|_| panic!("{what}, case {case}: the reader panicked"))
}

/// A face that loaded, however odd the file, must be sound enough to draw
/// with: sizes inside the limits, measurements inside the cell, every glyph
/// the size of one or two cells with nothing outside its width.
fn exercise(what: &str, case: usize, font: BitmapFont) {
    let (w, h) = font.cell();
    assert!((1..=32).contains(&w) && (1..=64).contains(&h), "{what}, case {case}: cell {w} by {h}");
    assert!(!font.is_empty(), "{what}, case {case}: a face with no glyphs was let through");
    assert!(font.ascent() >= 1 && font.ascent() <= h && font.x_top() < font.ascent(), "{what}, case {case}: metrics");
    for (row, thick) in [font.underline_rows(), font.strike_rows()] {
        assert!(thick >= 1 && row + thick <= h, "{what}, case {case}: a rule at {row} of thickness {thick}");
    }
    let points = font.code_points();
    assert_eq!(points.len(), font.len(), "{what}, case {case}");
    for &cp in &points {
        let g = font.raw_glyph(cp).unwrap_or_else(|| panic!("{what}, case {case}: U+{cp:04X} is listed and has no glyph"));
        assert!(g.width == w || g.width == 2 * w, "{what}, case {case}: U+{cp:04X} is {} wide", g.width);
        assert_eq!(g.rows.len(), h as usize, "{what}, case {case}");
        let outside = if g.width >= 32 { 0 } else { u32::MAX >> g.width };
        assert!(g.rows.iter().all(|r| r & outside == 0), "{what}, case {case}: U+{cp:04X} has ink outside its width");
    }

    let set = FaceSet::new(font);
    let mut frame = Frame::new(48, 40);
    let pen = Pen::new(Rgb(255, 255, 255));
    for (i, text) in ["An\u{0313} x_\u{2014}", "i\u{0301}\u{0323}g\u{0331}\u{0301}", "\u{4E00}\u{1F600}\u{FFFD}\t\u{0007}"].into_iter().enumerate() {
        set.draw_text(&mut frame, text, -3 + 9 * i as i32, -5 + 11 * i as i32, pen.scale(1 + i as u32).bold(i == 1));
    }
    for &cp in points.iter().take(40) {
        if let Some(ch) = char::from_u32(cp) {
            set.draw_cluster(&mut frame, &format!("{ch}\u{0301}"), 30, 25, pen);
        }
    }
    set.underline(&mut frame, 2, 2, 3, pen);
    set.strike(&mut frame, 2, 2, 3, pen);
}

/// What every case does with a reader's answer.
fn settle(what: &str, case: usize, answer: Result<BitmapFont, FontError>) -> bool {
    match answer {
        Ok(font) => {
            exercise(what, case, font);
            true
        }
        Err(e) => {
            assert!(!e.what.is_empty(), "{what}, case {case}: an error with nothing to say");
            false
        }
    }
}

/// How many cases loaded and how many were refused.
#[derive(Default)]
struct Tally {
    loaded: usize,
    refused: usize,
}

impl Tally {
    fn add(&mut self, loaded: bool) {
        if loaded {
            self.loaded += 1;
        } else {
            self.refused += 1;
        }
    }
}

/// Replaces, removes, doubles or cuts short a few characters of `text`.
fn scramble(rng: &mut Rng, text: &str, alphabet: &[u8]) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            break;
        }
        let at = rng.below(bytes.len());
        match rng.below(5) {
            0 => bytes[at] = *rng.pick(alphabet),
            1 => {
                bytes.remove(at);
            }
            2 => bytes.insert(at, *rng.pick(alphabet)),
            3 => {
                let len = rng.below(40).min(bytes.len() - at);
                let piece = bytes[at..at + len].to_vec();
                bytes.splice(at..at, piece);
            }
            _ => {
                let len = rng.below(40).min(bytes.len() - at);
                bytes.drain(at..at + len);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Numbers a reader has to survive wherever it expects a number.
const NUMBERS: [&str; 34] = [
    "0", "1", "-1", "2", "3", "7", "8", "9", "12", "16", "24", "31", "32", "33", "64", "65", "128", "255", "256", "4095", "4096", "4097",
    "65535", "65536", "2147483647", "2147483648", "-2147483648", "-2147483649", "4294967295", "4294967296", "99999999999999999999", "-0",
    "0x10", "",
];

/// Swaps a few of the whole numbers in `text` for numbers from [`NUMBERS`],
/// and sometimes drops, doubles or swaps a line.
fn renumber(rng: &mut Rng, text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for _ in 0..1 + rng.below(3) {
        if lines.is_empty() {
            break;
        }
        let at = rng.below(lines.len());
        match rng.below(8) {
            0 => {
                lines.remove(at);
            }
            1 => {
                let copy = lines[at].clone();
                lines.insert(at, copy);
            }
            2 => {
                let other = rng.below(lines.len());
                lines.swap(at, other);
            }
            3 => {
                let keep = rng.below(lines[at].len() + 1);
                lines[at].truncate(keep);
            }
            _ => {
                let words: Vec<&str> = lines[at].split(' ').collect();
                let numeric: Vec<usize> = (0..words.len()).filter(|&i| words[i].parse::<i64>().is_ok()).collect();
                if numeric.is_empty() {
                    continue;
                }
                let swap = *rng.pick(&numeric);
                let new = *rng.pick(&NUMBERS);
                let rebuilt: Vec<&str> = words.iter().enumerate().map(|(i, &w)| if i == swap { new } else { w }).collect();
                lines[at] = rebuilt.join(" ");
            }
        }
    }
    lines.join("\n") + "\n"
}

/// The glyph lines of a small `.hex` face, 12 by 4: three narrow glyphs, an
/// accent and a wide glyph.
const SMALL_HEX_GLYPHS: &str = "0048:9090F0F090909090
0078:0000909060609090
005f:000000000000FFF0
0301:0C00180000000000
4E00:FFFFFF800001800001FFFFFF
";

/// A whole `.hex` face.
fn small_hex() -> String {
    format!("# Width: 12
# Height: 4
{SMALL_HEX_GLYPHS}")
}

/// True when a `.hex` text cut off at some byte still reads as a whole
/// file: at least one finished glyph line, and a last line that is either
/// empty or itself finished. (A wide glyph cut at exactly the length of a
/// narrow one is such a line; the format has no way to tell.)
fn hex_is_whole(text: &str, rows: usize) -> bool {
    let finished = |line: &str| {
        let line = line.trim_end_matches('\r');
        line.split_once(':').is_some_and(|(cp, data)| !cp.is_empty() && (data.len() == rows * 4 || data.len() == rows * 6))
    };
    let glyphs = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect::<Vec<_>>();
    !glyphs.is_empty() && glyphs.iter().all(|l| finished(l))
}

#[test]
fn a_hex_file_cut_short_is_an_error_unless_the_cut_leaves_a_whole_file() {
    let small = small_hex();
    assert!(settle("small hex", 0, BitmapFont::from_hex("t", &small)));
    let mut whole = 0;
    for cut in 0..small.len() {
        let piece = &small[..cut];
        let ok = settle("small hex cut", cut, read("small hex cut", cut, || BitmapFont::from_hex("t", piece)));
        assert_eq!(ok, hex_is_whole(piece, 4), "cut at {cut}: {piece:?}");
        whole += ok as usize;
    }
    assert!(whole > 0 && whole < small.len() / 4);

    // Gallant itself: every cut inside the header and the first glyphs, then
    // cuts scattered through the half megabyte.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let cuts: Vec<usize> = (0..420).chain((0..160).map(|_| rng.below(GALLANT_HEX.len()))).collect();
    for (case, cut) in cuts.into_iter().enumerate() {
        let piece = &GALLANT_HEX[..cut];
        let answer = read("gallant.hex cut", case, || BitmapFont::from_hex("Gallant", piece));
        assert_eq!(answer.is_ok(), hex_is_whole(piece, 22), "cut at {cut}");
        if let Ok(font) = answer {
            assert!(font.len() < 5320 && font.cell() == (12, 22));
        }
    }
}

#[test]
fn garbage_given_to_the_hex_reader_is_an_error_and_never_a_panic() {
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let small = small_hex();
    let alphabet = b"0123456789abcdefABCDEF:::##\n\n\r\t WidthHeig-+xg\xC3\xA9";
    let mut tally = Tally::default();

    for case in 0..3000 {
        // Noise, read as text the way a file of unknown kind is.
        let noise = rng.noise(700);
        let text = String::from_utf8_lossy(&noise).into_owned();
        let answer = read("hex noise", case, || BitmapFont::from_hex("t", &text));
        assert!(answer.is_err(), "hex noise, case {case} loaded as a face");
        let _ = read("bytes noise", case, || BitmapFont::from_bytes("t", &noise)).map(|f| exercise("bytes noise", case, f));

        // Lines built from the characters a .hex file is made of.
        let mut lines = String::new();
        for _ in 0..rng.below(12) {
            for _ in 0..rng.below(90) {
                lines.push(*rng.pick(alphabet) as char);
            }
            lines.push('\n');
        }
        tally.add(settle("hex lines", case, read("hex lines", case, || BitmapFont::from_hex("t", &lines))));

        // A whole file with a few characters knocked about.
        let damaged = scramble(&mut rng, &small, alphabet);
        tally.add(settle("hex damaged", case, read("hex damaged", case, || BitmapFont::from_hex("t", &damaged))));

        // A whole file under a header that claims some other cell.
        let (w, h) = (*rng.pick(&NUMBERS), *rng.pick(&NUMBERS));
        let resized = format!("# Width: {w}\n# Height: {h}\n{SMALL_HEX_GLYPHS}");
        tally.add(settle("hex resized", case, read("hex resized", case, || BitmapFont::from_hex("t", &resized))));
    }
    // Most damage is caught; some leaves a file that is still whole.
    assert!(tally.refused > 6000 && tally.loaded > 0, "{} refused, {} loaded", tally.refused, tally.loaded);

    // Lines far longer than any glyph, and a cell as large as is allowed.
    for (case, text) in [
        format!("0041:{}\n", "F".repeat(200_000)),
        format!("{}:00\n", "0".repeat(200_000)),
        format!("# Width: {}\n0041:00\n", "9".repeat(5000)),
        "#".repeat(100_000),
        ":".repeat(100_000),
        "\n".repeat(100_000),
        format!("# Width: 32\n# Height: 64\n10FFFF:{}\n", "FFFFFFFF".repeat(64)),
        format!("# Width: 16\n# Height: 64\n0041:{}\n3042:{}\n", "FFFF".repeat(64), "FFFFFFFF".repeat(64)),
    ]
    .into_iter()
    .enumerate()
    {
        let loaded = settle("hex extremes", case, read("hex extremes", case, || BitmapFont::from_hex("t", &text)));
        assert_eq!(loaded, case >= 6, "hex extremes, case {case}");
    }
}

/// A whole BDF face in the shape of the real ones: a full-cell letter, tight
/// boxes placed by their offsets, an accent above, an underscore below the
/// baseline, a glyph with no code point and a double-width glyph that is
/// wider than the font's bounding box, as in Gallant's own BDF.
fn small_bdf() -> String {
    let mut s = String::from(
        "STARTFONT 2.1\nFONT -ok-test-medium-r-normal--8-80-75-75-C-60-ISO10646-1\nSIZE 8 75 75\nFONTBOUNDINGBOX 6 8 0 -2\n\
         STARTPROPERTIES 3\nFONT_ASCENT 6\nFONT_DESCENT 2\nDEFAULT_CHAR 32\nENDPROPERTIES\nCHARS 6\n",
    );
    let glyphs: [(&str, i64, &str, &[&str]); 6] = [
        ("H", 72, "6 8 0 -2", &["00", "88", "88", "F8", "88", "88", "00", "00"]),
        ("x", 120, "5 3 0 0", &["88", "70", "88"]),
        ("underscore", 95, "6 1 0 -2", &["FC"]),
        ("acute", 769, "2 2 2 4", &["40", "80"]),
        ("unencoded", -1, "6 8 0 -2", &["FC", "FC", "FC", "FC", "FC", "FC", "FC", "FC"]),
        ("wide", 0x4E00, "12 8 0 -2", &["FFF0", "8010", "8010", "8010", "8010", "FFF0", "0000", "0000"]),
    ];
    for (name, cp, bbx, rows) in glyphs {
        let advance = if name == "wide" { 12 } else { 6 };
        s.push_str(&format!("STARTCHAR {name}\nENCODING {cp}\nSWIDTH 500 0\nDWIDTH {advance} 0\nBBX {bbx}\nBITMAP\n"));
        for row in rows {
            s.push_str(row);
            s.push('\n');
        }
        s.push_str("ENDCHAR\n");
    }
    s.push_str("ENDFONT\n");
    s
}

#[test]
fn the_small_bdf_face_reads_as_written() {
    let f = BitmapFont::from_bdf("t", &small_bdf()).expect("a whole face");
    assert_eq!((f.cell(), f.ascent(), f.len(), f.wide_len()), ((6, 8), 6, 5, 1));
    assert_eq!(f.code_points(), vec![72, 95, 120, 769, 0x4E00]);
    assert_eq!(f.glyph('x').unwrap().rows, &[0, 0, 0, 0x8800_0000, 0x7000_0000, 0x8800_0000, 0, 0]);
    assert_eq!(f.glyph('\u{0301}').unwrap().rows, &[0x1000_0000, 0x2000_0000, 0, 0, 0, 0, 0, 0]);
    assert_eq!(f.glyph('\u{4E00}').unwrap().width, 12);
    assert_eq!((f.x_top(), f.underline_rows()), (3, (7, 1)));
    exercise("small bdf", 0, f);
}

#[test]
fn a_bdf_file_cut_short_is_always_an_error() {
    let small = small_bdf();
    let end = small.rfind("ENDFONT").expect("the last line") + "ENDFONT".len();
    for cut in 0..small.len() {
        let piece = &small[..cut];
        let answer = read("small bdf cut", cut, || BitmapFont::from_bdf("t", piece));
        assert_eq!(answer.is_ok(), cut >= end, "cut at {cut}");
        let bytes = read("small bdf cut, as bytes", cut, || BitmapFont::from_bytes("t", piece.as_bytes()));
        assert_eq!(bytes.is_ok(), cut >= end, "cut at {cut}");
    }

    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for (name, text) in [("Spleen", SPLEEN_BDF), ("Terminus", TERMINUS_BDF)] {
        let end = text.rfind("ENDFONT").expect("the last line") + "ENDFONT".len();
        let cuts: Vec<usize> = (0..60).map(|_| rng.below(end)).chain([0, 1, end - 1, end - 8]).collect();
        for (case, cut) in cuts.into_iter().enumerate() {
            let piece = text.get(..cut).unwrap_or("");
            let answer = read(name, case, || BitmapFont::from_bdf(name, piece));
            assert!(answer.is_err(), "{name} cut at {cut} loaded");
        }
    }
}

#[test]
fn garbage_given_to_the_bdf_reader_is_an_error_and_never_a_panic() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let small = small_bdf();
    let alphabet = b"0123456789ABCDEFabcdef \n\n-STRCHENDBIMPXW\r\t\xE2\x82\xAC";
    let keywords = [
        "STARTFONT", "FONT", "SIZE", "FONTBOUNDINGBOX", "STARTPROPERTIES", "FONT_ASCENT", "FONT_DESCENT", "ENDPROPERTIES", "CHARS",
        "STARTCHAR", "ENCODING", "SWIDTH", "DWIDTH", "BBX", "BITMAP", "ENDCHAR", "ENDFONT", "COMMENT", "FF", "0000", "8010",
    ];
    let mut tally = Tally::default();

    for case in 0..3000 {
        let noise = rng.noise(700);
        let text = String::from_utf8_lossy(&noise).into_owned();
        assert!(read("bdf noise", case, || BitmapFont::from_bdf("t", &text)).is_err(), "bdf noise, case {case} loaded");
        let headed = format!("STARTFONT 2.1\n{text}\nENDFONT\n");
        let _ = settle("bdf noise with a first line", case, read("bdf noise with a first line", case, || BitmapFont::from_bdf("t", &headed)));

        // The format's own words in any order, each followed by a few numbers.
        let mut words = String::from("STARTFONT 2.1\n");
        for _ in 0..rng.below(40) {
            let word: &&str = rng.pick(&keywords);
            words.push_str(word);
            for _ in 0..rng.below(5) {
                words.push(' ');
                let number: &&str = rng.pick(&NUMBERS);
                words.push_str(number);
            }
            words.push('\n');
        }
        tally.add(settle("bdf words", case, read("bdf words", case, || BitmapFont::from_bdf("t", &words))));

        let damaged = scramble(&mut rng, &small, alphabet);
        tally.add(settle("bdf damaged", case, read("bdf damaged", case, || BitmapFont::from_bdf("t", &damaged))));

        let renumbered = renumber(&mut rng, &small);
        tally.add(settle("bdf renumbered", case, read("bdf renumbered", case, || BitmapFont::from_bdf("t", &renumbered))));
    }
    assert!(tally.refused > 3000 && tally.loaded > 100, "{} refused, {} loaded", tally.refused, tally.loaded);

    // The real files with numbers swapped: whatever still loads must draw.
    for case in 0..40 {
        let text = if case % 2 == 0 { SPLEEN_BDF } else { TERMINUS_BDF };
        let renumbered = renumber(&mut rng, text);
        let _ = settle("a real bdf renumbered", case, read("a real bdf renumbered", case, || BitmapFont::from_bdf("t", &renumbered)));
    }

    // Sizes at and past every limit.
    let giant_rows = format!("{}\n", "F".repeat(1024)).repeat(64);
    for (case, (from, to)) in [
        ("BBX 12 8 0 -2", "BBX 4096 4096 -2000 -2000".to_string()),
        ("BBX 12 8 0 -2", "BBX 4097 8 0 -2".to_string()),
        ("BBX 12 8 0 -2", "BBX 12 8 2147483647 -2147483648".to_string()),
        ("BBX 12 8 0 -2", "BBX 12 8 -2147483648 2147483647".to_string()),
        ("FONT_ASCENT 6", "FONT_ASCENT 2147483647".to_string()),
        ("FONT_ASCENT 6", "FONT_ASCENT -2147483648".to_string()),
        ("FONT_DESCENT 2", "FONT_DESCENT 2147483647".to_string()),
        ("FONTBOUNDINGBOX 6 8 0 -2", "FONTBOUNDINGBOX 2147483647 2147483647 -2147483648 -2147483648".to_string()),
        ("DWIDTH 12 0", "DWIDTH 2147483647 0".to_string()),
        ("ENCODING 19968", "ENCODING 2147483647".to_string()),
        ("FFF0\n8010\n8010\n8010\n8010\nFFF0\n0000\n0000\n", giant_rows),
    ]
    .into_iter()
    .enumerate()
    {
        let text = small.replacen(from, &to, 1);
        assert_ne!(text, small, "bdf extremes, case {case}");
        let _ = settle("bdf extremes", case, read("bdf extremes", case, || BitmapFont::from_bdf("t", &text)));
    }
}

/// A whole PSF2 file: `count` glyphs of `width` by `height`, each a
/// different picture, with a Unicode table when `table` is set.
fn psf2(count: u32, width: u32, height: u32, table: bool) -> Vec<u8> {
    let stride = width.div_ceil(8);
    let mut f = vec![0x72, 0xB5, 0x4A, 0x86];
    for v in [0, 32, table as u32, count, height * stride, height, width] {
        f.extend(v.to_le_bytes());
    }
    for i in 0..count {
        for r in 0..height * stride {
            f.push((i * 37 + r * 11 + 1) as u8);
        }
    }
    if table {
        for i in 0..count {
            // The glyph's own character, sometimes a second one, sometimes a sequence.
            let ch = char::from_u32(0x41 + i).unwrap_or('?');
            let mut entry = ch.to_string();
            if i % 3 == 0 {
                entry.push(char::from_u32(0x400 + i).unwrap_or('?'));
            }
            f.extend(entry.as_bytes());
            if i % 4 == 1 {
                f.push(0xFE);
                f.extend("e\u{0301}".as_bytes());
            }
            f.push(0xFF);
        }
    }
    f
}

/// A whole PSF1 file: 256 glyphs, or 512 with bit 0 of `mode`, and a table
/// when bit 1 or bit 2 is set.
fn psf1(mode: u8, height: u8) -> Vec<u8> {
    let count: usize = if mode & 1 != 0 { 512 } else { 256 };
    let mut f = vec![0x36, 0x04, mode, height];
    for i in 0..count {
        for r in 0..height as usize {
            f.push((i * 7 + r * 13 + 1) as u8);
        }
    }
    if mode & 0x06 != 0 {
        for i in 0..count as u16 {
            f.extend((0x20 + i).to_le_bytes());
            if i % 5 == 0 {
                f.extend(0xFFFEu16.to_le_bytes());
                f.extend(0x65u16.to_le_bytes());
                f.extend(0x301u16.to_le_bytes());
            }
            f.extend(0xFFFFu16.to_le_bytes());
        }
    }
    f
}

#[test]
fn a_psf_file_cut_short_is_always_an_error() {
    let files = [psf2(9, 12, 22, true), psf2(300, 8, 16, false), psf2(3, 32, 64, true), psf1(0, 16), psf1(2, 8), psf1(3, 14), psf1(5, 1)];
    for (which, file) in files.iter().enumerate() {
        let what = format!("psf file {which}");
        assert!(settle(&what, 0, BitmapFont::from_psf("p", file)), "{what} is whole and did not load");
        assert!(BitmapFont::from_bytes("p", file).is_ok(), "{what}");
        // Every cut of a small file; a spread of cuts through a large one.
        let step = (file.len() / 1500).max(1);
        for cut in (0..file.len()).step_by(step).chain(file.len().saturating_sub(40)..file.len()) {
            let piece = &file[..cut];
            assert!(read(&what, cut, || BitmapFont::from_psf("p", piece)).is_err(), "{what} cut at {cut} loaded");
            assert!(read(&what, cut, || BitmapFont::from_bytes("p", piece)).is_err(), "{what} cut at {cut} loaded as bytes");
        }
    }
}

#[test]
fn garbage_given_to_the_psf_reader_is_an_error_and_never_a_panic() {
    let mut rng = Rng(0x6A09_E667_F3BC_C908);
    let whole = [psf2(9, 12, 22, true), psf2(40, 8, 16, false), psf1(2, 8), psf1(1, 16)];
    let fields: [u32; 24] =
        [0, 1, 2, 3, 4, 8, 9, 12, 16, 22, 31, 32, 33, 40, 44, 64, 65, 255, 256, 512, 0x4000_0000, 0x7FFF_FFFF, 0x8000_0000, u32::MAX];
    let mut tally = Tally::default();

    for case in 0..3000 {
        // Noise with no magic number in front is not a PSF file.
        let noise = rng.noise(600);
        let is_psf = noise.starts_with(&[0x36, 0x04]) || noise.starts_with(&[0x72, 0xB5, 0x4A, 0x86]);
        let answer = read("psf noise", case, || BitmapFont::from_psf("p", &noise));
        assert!(answer.is_err() || is_psf, "psf noise, case {case} loaded");

        // Noise behind each magic number.
        let mut one = vec![0x36, 0x04];
        one.extend(&noise);
        if case % 4 == 0 {
            // Long enough to hold 256 or 512 glyphs, so the table is reached.
            one.truncate(4);
            one.extend(rng.noise(9000));
        }
        tally.add(settle("psf1 noise", case, read("psf1 noise", case, || BitmapFont::from_psf("p", &one))));
        let mut two = vec![0x72, 0xB5, 0x4A, 0x86];
        two.extend(&noise);
        let _ = settle("psf2 noise", case, read("psf2 noise", case, || BitmapFont::from_psf("p", &two)));
        let mut zipped = vec![0x1F, 0x8B];
        zipped.extend(&noise);
        let e = read("gzip noise", case, || BitmapFont::from_psf("p", &zipped)).expect_err("gzip is refused");
        assert!(e.what.contains("gzip"), "{e}");

        // A PSF2 header built from numbers that have broken a reader
        // somewhere, in front of a body of any length. Three times in four
        // the size of a glyph agrees with the cell, and half the time the
        // header is its usual length, so the later checks are reached too.
        let (width, height) = (*rng.pick(&fields), *rng.pick(&fields));
        let fitting = height.wrapping_mul(width.wrapping_add(7) / 8);
        let glyph_size = if rng.below(4) != 0 { fitting } else { *rng.pick(&fields) };
        let header_size = if rng.below(2) == 0 { 32 } else { *rng.pick(&fields) };
        let mut built = vec![0x72, 0xB5, 0x4A, 0x86];
        for v in [*rng.pick(&fields), header_size, *rng.pick(&fields), *rng.pick(&fields), glyph_size, height, width] {
            built.extend(v.to_le_bytes());
        }
        built.extend(rng.noise(3000));
        tally.add(settle("psf2 fields", case, read("psf2 fields", case, || BitmapFont::from_psf("p", &built))));

        // A whole file with a few bytes changed, most often in the header
        // or the table, where a wrong number does the most harm.
        let mut damaged = rng.pick(&whole).clone();
        for _ in 0..1 + rng.below(4) {
            let at = match rng.below(3) {
                0 => rng.below(32.min(damaged.len())),
                1 => damaged.len() - 1 - rng.below(60.min(damaged.len())),
                _ => rng.below(damaged.len()),
            };
            damaged[at] = match rng.below(4) {
                0 => 0xFF,
                1 => 0xFE,
                2 => 0,
                _ => rng.byte(),
            };
        }
        tally.add(settle("psf damaged", case, read("psf damaged", case, || BitmapFont::from_psf("p", &damaged))));
        let _ = read("psf damaged, as bytes", case, || BitmapFont::from_bytes("p", &damaged));
    }
    assert!(tally.refused > 3000 && tally.loaded > 100, "{} refused, {} loaded", tally.refused, tally.loaded);
}

#[test]
fn a_file_of_one_kind_given_to_another_reader_is_an_error() {
    let hex = small_hex();
    let bdf = small_bdf();
    let psf = psf2(9, 12, 22, true);
    assert!(BitmapFont::from_bdf("t", &hex).is_err());
    assert!(BitmapFont::from_hex("t", &bdf).is_err());
    assert!(BitmapFont::from_psf("t", hex.as_bytes()).is_err());
    assert!(BitmapFont::from_psf("t", bdf.as_bytes()).is_err());
    assert!(BitmapFont::from_hex("t", &String::from_utf8_lossy(&psf)).is_err());
    assert!(BitmapFont::from_bdf("t", &String::from_utf8_lossy(&psf)).is_err());
    // The reader that picks by content sends each to the right place.
    assert_eq!(BitmapFont::from_bytes("t", hex.as_bytes()).unwrap().len(), 5);
    assert_eq!(BitmapFont::from_bytes("t", bdf.as_bytes()).unwrap().len(), 5);
    assert_eq!(BitmapFont::from_bytes("t", &psf).unwrap().len(), 12);
    // A byte-order mark in front of a text face is passed over.
    assert_eq!(BitmapFont::from_bytes("t", format!("\u{FEFF}{hex}").as_bytes()).unwrap().len(), 5);
    assert_eq!(BitmapFont::from_bytes("t", format!("\u{FEFF}{bdf}").as_bytes()).unwrap().len(), 5);
    assert!(BitmapFont::from_bytes("t", &[]).is_err());
}
