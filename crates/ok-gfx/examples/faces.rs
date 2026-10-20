//! Renders a specimen of the three built-in faces: prose, the test string
//! every face and scale is checked against, marks placed on letters, and the
//! hex box that stands in for a character no face has.
//!
//! Usage: cargo run -p ok-gfx --example faces --release --offline -- shots/faces.png
//!
//! Beside the specimen it writes `faces-marks.png`, the mark placements
//! enlarged four times, which is the picture to judge them by.

use ok_gfx::{clusters, encode_png, gallant, spleen, terminus, BitmapFont, FaceSet, Frame, Pen, Rgb};

const PAPER: Rgb = Rgb(0xEE, 0xE3, 0xC8);
const INK: Rgb = Rgb(0x24, 0x1E, 0x19);

const PROSE: [&str; 5] = [
    "The orchard was eleven acres when my",
    "grandfather bought it and eleven acres",
    "when he stopped farming it. \u{201C}You take",
    "off two of every three,\u{201D} he said, \u{2018}so the",
    "one you leave has room\u{2019} \u{2014} and I did.",
];

/// The string from the fonts verification, section 13: the Chumash word
/// that started this, prose punctuation, box drawing, marks typed as
/// separate characters, the invisible characters, wide characters, an emoji
/// above U+FFFF, a real U+FFFD and the two apostrophes side by side.
const TEST: [&str; 5] = [
    "mitsqanaqan\u{0313} \u{2014} \u{201C}double\u{201D} \u{2018}single\u{2019} \u{2026} \u{2022} \u{00A7} \u{00B6}",
    "\u{00E9} \u{00F1} \u{014D} \u{0161} \u{0152} \u{0153} \u{0294} \u{A78C} \u{2190} \u{2192}  l\u{0313} k\u{0313} t\u{0313} i\u{0313}  e\u{0302}\u{0301}",
    "\u{250C}\u{2500}\u{252C}\u{2500}\u{2510}  a\u{00A0}b a\u{200B}b a\u{00AD}b a\tb  \u{2614}\u{30F3} \u{1F600} \u{FFFD} '\u{2019}",
    "\u{2502}\u{2588}\u{2502}\u{2591}\u{2502}",
    "\u{2514}\u{2500}\u{2534}\u{2500}\u{2518}",
];

/// Marks on letters. Every space starts a new run of clusters, so a mark
/// that follows a space here stands alone, with no letter under it.
const MARKS: [&str; 6] = [
    // The face's own accented letters, then a comma above on every shape of letter.
    "\u{013A}\u{1E31}\u{0165}\u{017E} n\u{0313} l\u{0313} k\u{0313} t\u{0313} i\u{0313} j\u{0313} p\u{0313} q\u{0313} m\u{0313} w\u{0313} y\u{0313} x\u{0313} c\u{0313} s\u{0313}",
    // Letters that carry an accent already, tall letters, capitals.
    "\u{0161}\u{0313} \u{010D}\u{0313} s\u{030C}\u{0313} f\u{0313} h\u{0313} b\u{0313} d\u{0313} K\u{0313} S\u{0313} L\u{0313} Q\u{0301} \u{00E9}\u{0313} e\u{0313}\u{0301}",
    // Accented capitals under a second mark, and three marks deep.
    "\u{0160} \u{0160}\u{0313} \u{010C}\u{0313} \u{017D}\u{0313} S\u{030C}\u{0301} \u{00C9}\u{0301} \u{00D1}\u{0313} \u{0160}\u{0313}\u{0301} n\u{0313}\u{0313} l\u{0313}\u{0301} \u{0131}\u{0313}\u{0301}",
    // Marks below, on letters with tails and without; an ogonek; two overlays.
    "\u{1EF5} g\u{0323} j\u{0323} q\u{0323} y\u{0331} v\u{0331} u\u{0331} p\u{0331} g\u{0331} j\u{0331} y\u{0330} u\u{0330} g\u{0332} y\u{0332} o\u{0328} l\u{0335} o\u{0338}",
    // Marks on a line, on U+FFFD, alone, on a no-break space; a mark no face
    // has, on a letter and alone; marks on characters no face has.
    "\u{2502}\u{0301} \u{FFFD}\u{0301} \u{0313} \u{0301}\u{0323} \u{00A0}\u{0301} x\u{1DC4} \u{1DC4} \u{0E01}\u{0E34} \u{0E01}\u{0301} \u{0E01}\u{0323} \u{0007}\u{0313}",
    // Letters typed in parts beside the same letters in one piece.
    "i\u{0301} i\u{0303} j\u{0302} i\u{0323} n\u{0303} a\u{0308} e\u{0302}\u{0301} a\u{0323}\u{0302} a\u{0302}\u{0323} \u{00F1} \u{00E4} \u{1EBF} \u{1EAD}",
];

/// Characters no face has, side by side as a run of them would be, then a
/// zero-width space and a private-use character each on its own.
const BOXES: &str = "\u{0E01}\u{0E02}\u{4E00}\u{1F600}\u{0007}\u{10FFFF} \u{200B} \u{E000} \u{0B6D}\u{0D8B}";

/// Three rows of ruled boxes, dashed lines and blocks. Terminus has no
/// dashed lines and not every block, so those it gets drawn in code, in its
/// own weight.
const LINES: [&str; 3] = [
    "\u{2554}\u{2550}\u{2564}\u{2550}\u{2557}\u{250F}\u{2501}\u{2533}\u{2501}\u{2513}\u{256D}\u{2500}\u{256E} \u{2504}\u{2504}\u{2504} \u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588} \u{2594}\u{2594}\u{2595}",
    "\u{255F}\u{2500}\u{253C}\u{2500}\u{2562}\u{2523}\u{2501}\u{254B}\u{2501}\u{252B}\u{2502} \u{2502} \u{254C}\u{254C}\u{254C} \u{2591}\u{2592}\u{2593}\u{2588}\u{258C}\u{2590}\u{2580}\u{2584} \u{2506}\u{254E}\u{2595}",
    "\u{255A}\u{2550}\u{2567}\u{2550}\u{255D}\u{2517}\u{2501}\u{253B}\u{2501}\u{251B}\u{2570}\u{2500}\u{256F} \u{2505}\u{2505}\u{2505} \u{2596}\u{2597}\u{2598}\u{2599}\u{259A}\u{259B}\u{259C}\u{259D} \u{2571}\u{2572}\u{2573}",
];

/// A stand-in for the page's width table, enough for the strings above.
fn cells(cluster: &str) -> u32 {
    match cluster.chars().next() {
        Some('\u{2614}' | '\u{30F3}' | '\u{4E00}' | '\u{1F600}') => 2,
        _ => 1,
    }
}

/// Draws one line the way the page will: cluster by cluster, each given the
/// cells the width table says. A space is an empty cell and ends the run of
/// clusters before it, so that a mark typed after a space is drawn alone.
/// Returns how many cells the line took.
fn line(frame: &mut Frame, set: &FaceSet, x: i32, y: i32, text: &str, pen: Pen) -> i32 {
    let step = set.cell().0 as i32 * pen.scale as i32;
    let mut col = 0;
    for (i, word) in text.split(' ').enumerate() {
        col += (i > 0) as i32;
        for c in clusters(word) {
            let n = cells(c);
            set.draw_cluster_in(frame, c, x + col * step, y, n, pen);
            col += n as i32;
        }
    }
    col
}

fn specimen(frame: &mut Frame, face: &'static BitmapFont, x0: i32) {
    let set = FaceSet::new(face);
    let (cw, ch) = (set.cell().0 as i32, set.cell().1 as i32);
    let ink = Pen::new(INK);
    let half = Pen::new(PAPER.mix(INK, 166));
    let left = x0 + 2 * cw;
    let caption = format!("{} {} by {}, {} glyphs, ascent {}", face.name(), cw, ch, face.len(), face.ascent());
    let mut page: Vec<(&str, Pen)> = vec![(&caption, half), ("", ink)];
    page.extend(PROSE.map(|text| (text, ink)));
    page.push(("", ink));
    page.extend(TEST.map(|text| (text, ink)));
    page.push(("", ink));
    page.extend(MARKS.map(|text| (text, ink)));
    page.push(("", ink));
    page.extend([("No face has these, so each is a box:", half), (BOXES, ink), (BOXES, half), ("", ink)]);
    page.extend(LINES.map(|text| (text, ink)));
    page.push(("", ink));
    let mut row = 1;
    for (text, pen) in page {
        line(frame, &set, left, row * ch, text, pen);
        row += 1;
    }

    let y = row * ch;
    let n = line(frame, &set, left, y, "underscored", ink);
    set.underline(frame, left, y, n as u32, ink);
    let x = left + (n + 2) * cw;
    let n = line(frame, &set, x, y, "struck", ink);
    set.strike(frame, x, y, n as u32, ink);
    let x = x + (n + 2) * cw;
    line(frame, &set, x, y, "double struck", ink.bold(true));
    row += 2;

    line(frame, &set, left, row * ch, "mitsqanaqan\u{0313} l\u{0313}k\u{0313}t\u{0313} \u{0E01}", ink.scale(2));
    row += 3;
    line(frame, &set, left, row * ch, "ok n\u{0313} \u{1F600}", ink.scale(3));
}

fn marks_sheet(faces: [&'static BitmapFont; 3]) -> Frame {
    let scale = 4;
    let tallest = faces.iter().map(|face| face.cell().1 as usize + 2).max().unwrap_or(0);
    let mut frame = Frame::new(1800, faces.len() * (MARKS.len() * tallest * scale as usize + 8) + 24);
    frame.clear(PAPER);
    let mut y = 16;
    for face in faces {
        let set = FaceSet::new(face);
        for text in MARKS {
            line(&mut frame, &set, 16, y, text, Pen::new(INK).scale(scale));
            y += (set.cell().1 as i32 + 2) * scale as i32;
        }
        y += 8;
    }
    frame
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: faces out.png");
    let faces = [gallant(), spleen(), terminus()];

    let mut frame = Frame::new(1920, 1060);
    frame.clear(PAPER);
    for (i, face) in faces.into_iter().enumerate() {
        specimen(&mut frame, face, i as i32 * 640);
    }
    std::fs::write(&out, encode_png(frame.width(), frame.height(), frame.pixels())).expect("write png");

    let sheet = marks_sheet(faces);
    let beside = out.strip_suffix(".png").unwrap_or(&out).to_string() + "-marks.png";
    std::fs::write(&beside, encode_png(sheet.width(), sheet.height(), sheet.pixels())).expect("write png");

    for face in faces {
        println!("{}: {} glyphs, {} double-width", face.name(), face.len(), face.wide_len());
    }
    println!("{out}\n{beside}");
}
