//! A still proof of the one screen at 160 by 49, drawn by hand before the
//! interface exists, so the layout and the looks can be judged on real pixels.
//! Usage: cargo run -p ok-gfx --example proof -- out-dir

use ok_gfx::{encode_png, BitmapFont, Frame, Pen, Rect, Rgb};

const GALLANT: &str = include_str!("../../../assets/fonts/gallant/gallant.hex");
const CW: i32 = 12;
const CH: i32 = 22;
const TOP: i32 = 1;

struct Look {
    name: &'static str,
    paper: Rgb,
    ink: Rgb,
    desk: Rgb,
    desk_ink: Rgb,
}

struct Screen<'a> {
    f: Frame,
    font: &'a BitmapFont,
}

impl Screen<'_> {
    fn text(&mut self, col: i32, row: i32, s: &str, pen: Pen) {
        let step = CW * pen.scale as i32;
        let mut x = col * CW;
        for ch in s.chars() {
            if ('\u{0300}'..='\u{036F}').contains(&ch) {
                self.font.draw(&mut self.f, ch, x - step, TOP + row * CH, pen);
            } else {
                x += self.font.draw(&mut self.f, ch, x, TOP + row * CH, pen);
            }
        }
    }

    fn right(&mut self, end_col: i32, row: i32, s: &str, pen: Pen) {
        self.text(end_col + 1 - s.chars().count() as i32, row, s, pen);
    }

    fn cells(&mut self, col: i32, row: i32, w: i32, h: i32, c: Rgb) {
        self.f.fill(Rect::new(col * CW, TOP + row * CH, w * CW, h * CH), c);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Title,
    Body,
    Blank,
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: proof out-dir");
    let font = BitmapFont::from_hex("Gallant", GALLANT).expect("gallant.hex");

    // Provisional values. The real tokens are named and sourced in the looks.
    let looks = [
        Look { name: "manila", paper: Rgb(0xEE, 0xE3, 0xC8), ink: Rgb(0x24, 0x1E, 0x19), desk: Rgb(0x35, 0x25, 0x1B), desk_ink: Rgb(0xE6, 0xD9, 0xBA) },
        Look { name: "sun", paper: Rgb(0xFF, 0xFF, 0xFF), ink: Rgb(0x00, 0x00, 0x00), desk: Rgb(0xFF, 0xFF, 0xFF), desk_ink: Rgb(0x00, 0x00, 0x00) },
        Look { name: "amber", paper: Rgb(0x19, 0x13, 0x10), ink: Rgb(0xE1, 0x82, 0x00), desk: Rgb(0x19, 0x13, 0x10), desk_ink: Rgb(0xE1, 0x82, 0x00) },
    ];

    // The text, already broken at a 66-column measure. `true` marks rows written today.
    let doc: Vec<(&str, Kind, bool)> = vec![
        ("The orchard", Kind::Title, false),
        ("", Kind::Blank, false),
        ("The orchard was eleven acres when my grandfather bought it and", Kind::Body, false),
        ("eleven acres when he stopped farming it, and in between he never", Kind::Body, false),
        ("once owned a tape measure longer than his arm. He kept the trees", Kind::Body, false),
        ("in rows he paced out in boots, one long step and one short.", Kind::Body, false),
        ("", Kind::Blank, false),
        ("I spent the summer I was fifteen on a ladder in those rows. The", Kind::Body, false),
        ("job was thinning: you take off two of every three apricots while", Kind::Body, false),
        ("they are still green and hard, so that the one you leave has room", Kind::Body, false),
        ("to become something. It felt like *waste*. I filled buckets with", Kind::Body, false),
        ("fruit that would never ripen and carried them to the compost, and", Kind::Body, false),
        ("I remember thinking that a better farmer would have found a way to", Kind::Body, false),
        ("keep all three.", Kind::Body, false),
        ("", Kind::Blank, false),
        ("He let me think that for about a week. Then he walked me to the", Kind::Body, true),
        ("one tree he had left alone on purpose, TK the year he planted it,", Kind::Body, true),
        ("and showed me what keeping all three looks like. The branches had", Kind::Body, true),
        ("split under the weight. The fruit was the size of walnuts and", Kind::Body, true),
        ("tasted of nothing. He did not say anything about it, which I have", Kind::Body, true),
        ("come to understand as a kind of teaching, the kind I got again in", Kind::Body, true),
        ("[[Grandfather interview]] when I asked what he would do", Kind::Body, true),
        ("differently and he said he would have **thinned harder**.", Kind::Body, true),
        ("", Kind::Blank, true),
        ("I think about that tree whenever I am asked to add one more thing", Kind::Body, true),
        ("to a week that is already full. For a long time I believed the", Kind::Body, true),
        ("answer was to keep all three and", Kind::Body, true),
    ];

    let tree: &[(i32, &str, &str, bool)] = &[
        (0, "College", "", false),
        (1, "The orchard", "412", true),
        (1, "Essay plan", "210", false),
        (1, "Grandfather interview", "630", false),
        (1, "Leadership essay", "348", false),
        (1, "Orchard notes", "1,204", false),
        (1, "Why engineering", "351", false),
        (0, "Mock trial", "", false),
        (1, "Closing argument", "1,105", false),
        (1, "Cross of the detective", "1,430", false),
        (1, "Opening statement", "890", false),
        (0, "Student government", "", false),
        (1, "Budget speech", "540", false),
        (1, "Homecoming run of show", "760", false),
    ];

    for look in &looks {
        let mut s = Screen { f: Frame::new(1920, 1080), font: &font };
        let flat = look.desk == look.paper;
        let half = look.ink.mix(look.paper, 90);
        let faint = look.ink.mix(look.paper, 166);
        let d_half = look.desk_ink.mix(look.desk, 90);
        let d_faint = look.desk_ink.mix(look.desk, 166);

        s.f.clear(look.desk);
        s.f.fill(Rect::new(41 * CW, 0, 78 * CW, 1080), look.paper);

        // Contents, typed on the desk: names, leader dots, word counts.
        for (i, &(depth, name, words, open)) in tree.iter().enumerate() {
            let row = 1 + i as i32;
            let col = 2 + depth * 2;
            let pen = Pen::new(if open { look.desk_ink } else { d_half });
            s.text(col, row, name, pen);
            if !words.is_empty() {
                let from = col + name.chars().count() as i32 + 1;
                let to = 36 - words.chars().count() as i32 - 1;
                let dots: String = (from..=to).map(|_| '.').collect();
                s.text(from, row, &dots, Pen::new(d_faint));
                s.right(36, row, words, pen);
            }
        }

        // The page. Row 28 is the platen: the line being written never moves.
        let platen = 28;
        let first = platen - doc.len() as i32;
        let mut row = first;
        let mut cursor = (0, 0);
        for &(line, kind, today) in &doc {
            match kind {
                Kind::Title => {
                    s.text(44, row + 1, "#", Pen::new(half));
                    s.text(47, row, line, Pen::new(look.ink).scale(2));
                    row += 2;
                    continue;
                }
                Kind::Blank => {}
                Kind::Body => {
                    let mut col = 47;
                    let mut rest = line;
                    // Marks stay visible at half strength; the words take the attribute.
                    while !rest.is_empty() {
                        let (piece, pen, under, inverse, used) = if let Some(r) = rest.strip_prefix("**") {
                            let end = r.find("**").unwrap();
                            s.text(col, row, "**", Pen::new(half));
                            col += 2;
                            (&r[..end], Pen::new(look.ink).bold(true), false, false, end + 4)
                        } else if let Some(r) = rest.strip_prefix('*') {
                            let end = r.find('*').unwrap();
                            s.text(col, row, "*", Pen::new(half));
                            col += 1;
                            (&r[..end], Pen::new(look.ink), true, false, end + 2)
                        } else if let Some(r) = rest.strip_prefix("[[") {
                            let end = r.find("]]").unwrap();
                            s.text(col, row, "[[", Pen::new(half));
                            col += 2;
                            (&r[..end], Pen::new(look.ink), false, false, end + 4)
                        } else if rest.starts_with("TK ") {
                            ("TK", Pen::new(look.paper), false, true, 2)
                        } else {
                            let stop = ["**", "*", "[[", "TK "].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
                            (&rest[..stop.max(1)], Pen::new(look.ink), false, false, stop.max(1))
                        };
                        let n = piece.chars().count() as i32;
                        if inverse {
                            s.cells(col, row, n, 1, look.ink);
                        }
                        s.text(col, row, piece, pen);
                        if under {
                            let line: String = (0..n).map(|_| '_').collect();
                            s.text(col, row, &line, Pen::new(look.ink));
                        }
                        col += n;
                        let closing = used - piece.len() - if used > piece.len() { (used - piece.len()) / 2 } else { 0 };
                        if closing > 0 && !inverse {
                            let mark = &rest[used - closing..used];
                            s.text(col, row, mark, Pen::new(half));
                            col += closing as i32;
                        }
                        rest = &rest[used..];
                    }
                    cursor = (col, row);
                }
            }
            if today {
                // Change bar: a two-pixel stroke beside every row written today.
                s.f.fill(Rect::new(45 * CW + 5, TOP + row * CH, 2, CH), look.ink);
            }
            row += 1;
        }
        s.cells(cursor.0, cursor.1, 1, 1, look.ink);

        // The machine line, and the row above it where the machine speaks.
        s.text(47, 46, "Last here Tuesday at 23:41. That sitting added 388 words.", Pen::new(faint));
        s.text(47, 47, "ok", Pen::new(look.ink));
        s.right(112, 47, "The orchard    412 of 650 words    163 today", Pen::new(half));

        // The map: the whole text at two pixels a letter, four pixels a line.
        // On a desk it is a small sheet of the same paper that grows as he writes.
        let (mx, my) = (121 * CW, TOP + CH);
        let mut y = my + 6;
        let rows: Vec<(&str, Kind, bool)> = doc.clone();
        let map_h = rows.iter().map(|r| if r.1 == Kind::Title { 8 } else { 4 }).sum::<i32>() + 12;
        if !flat {
            s.f.fill(Rect::new(mx - 4, my, 11 * CW + 8, map_h), look.paper);
        }
        let mut notch_y = y;
        for (i, &(line, kind, today)) in rows.iter().enumerate() {
            let c = if today { look.ink } else { half };
            let unit = if kind == Kind::Title { 4 } else { 2 };
            for (n, ch) in line.chars().enumerate() {
                if ch != ' ' {
                    s.f.fill(Rect::new(mx + n as i32 * unit, y, unit, unit), c);
                }
            }
            if i == rows.len() - 1 {
                notch_y = y;
            }
            y += unit * 2;
        }
        s.f.fill(Rect::new(120 * CW + 2, notch_y, 6, 2), look.desk_ink);
        s.text(134, 1, "The orchard", Pen::new(d_half));
        s.text(134, 4, "Page 2", Pen::new(d_faint));
        s.text(134, 40, "See also", Pen::new(d_faint));
        s.text(136, 41, "Grandfather interview", Pen::new(d_half));
        s.text(134, 43, "Linked from", Pen::new(d_faint));
        s.text(136, 44, "Essay plan", Pen::new(d_half));

        let out = format!("{dir}/proof-{}.png", look.name);
        std::fs::write(&out, encode_png(s.f.width(), s.f.height(), s.f.pixels())).expect("write png");
        println!("{out}");
    }
}
