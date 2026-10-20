//! What the shell's tests share: the demo program, a way to run it unseen,
//! and a way to read text back out of a frame's pixels.
//!
//! Reading back is exact, not fuzzy. Every cell of the frame is compared,
//! pixel for pixel, with what `ok_gfx` draws for each printable character of
//! Gallant, so a test that reads `Sphinx` has seen those six glyphs.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use ok_gfx::{BitmapFont, Frame, Pen, Rgb};
use ok_win::{Options, Report, SelfTest, Step};

#[path = "../../examples/demo.rs"]
pub mod demo;

use demo::{Demo, Layout, CELL, DESK_INK, INK};

/// The solid cell the cursor is drawn as.
pub const BLOCK: char = '\u{2588}';
/// The outlined box drawn for a character the face lacks.
pub const BOX: char = '\u{25A1}';
/// A cell that is none of the glyphs known here.
pub const UNKNOWN: char = '\u{FFFD}';

/// One cell's pixels: a row of bits per pixel row, leftmost pixel highest.
type Mask = Vec<u16>;

struct Glyphs {
    by_mask: HashMap<Mask, char>,
}

fn glyphs() -> &'static Glyphs {
    static GLYPHS: OnceLock<Glyphs> = OnceLock::new();
    GLYPHS.get_or_init(|| {
        let source = include_str!("../../../../assets/fonts/gallant/gallant.hex");
        let face = BitmapFont::from_hex("Gallant", source).expect("Gallant parses");
        let white = Rgb(255, 255, 255);
        let mut by_mask = HashMap::new();
        let mut learn = |shown: char, drawn: char| {
            let mut cell = Frame::new(CELL.0 as usize, CELL.1 as usize);
            cell.clear(Rgb(0, 0, 0));
            face.draw(&mut cell, drawn, 0, 0, Pen::new(white));
            by_mask.entry(mask(&cell, 0, 0, white)).or_insert(shown);
        };
        for ch in ' '..='~' {
            learn(ch, ch);
        }
        // A character Gallant has no glyph for is drawn as this box.
        learn(BOX, '\u{1F600}');
        by_mask.insert(vec![0xFFF0; CELL.1 as usize], BLOCK);
        Glyphs { by_mask }
    })
}

fn mask(frame: &Frame, x: i32, y: i32, ink: Rgb) -> Mask {
    (0..CELL.1)
        .map(|row| {
            (0..CELL.0).fold(0u16, |bits, col| {
                let lit = frame.get(x + col, y + row) == Some(ink);
                bits | ((lit as u16) << (15 - col))
            })
        })
        .collect()
}

/// A report's last frame, readable as text.
pub struct Screen {
    frame: Frame,
    pub layout: Layout,
}

impl Screen {
    pub fn of(report: &Report) -> Screen {
        let mut frame = Frame::new(report.width, report.height);
        frame.pixels_mut().copy_from_slice(&report.pixels);
        Screen { frame, layout: Layout::new(report.width, report.height) }
    }

    /// Reads `count` cells starting at a cell, taking pixels of exactly `ink` as lit.
    pub fn read(&self, col: i32, row: i32, count: i32, ink: Rgb) -> String {
        (0..count)
            .map(|i| {
                let (x, y) = self.layout.at(col + i, row);
                glyphs().by_mask.get(&mask(&self.frame, x, y, ink)).copied().unwrap_or(UNKNOWN)
            })
            .collect()
    }

    /// One row of the typed text, cursor block included, trailing blanks removed.
    pub fn text_row(&self, row: i32) -> String {
        let l = &self.layout;
        self.read(l.text_col, l.text_row + row, l.measure, INK).trim_end().to_string()
    }

    /// The first row of typed text without the cursor.
    pub fn typed(&self) -> String {
        self.text_row(0).trim_end_matches(BLOCK).to_string()
    }

    /// The bottom row's words: the last event the demo was given.
    pub fn last_event(&self) -> String {
        let l = &self.layout;
        let row = self.read(2, l.status_row, l.cols - 2, DESK_INK);
        // The tick count sits at the right, at least two blank cells away.
        row.split("  ").next().unwrap_or_default().trim_end().to_string()
    }

    /// What is printed on the sheet's machine line.
    pub fn machine_line(&self) -> String {
        let l = &self.layout;
        self.read(l.text_col, l.machine_row, l.measure, INK).trim_end().to_string()
    }

    pub fn pixel(&self, x: i32, y: i32) -> Option<Rgb> {
        self.frame.get(x, y)
    }

    /// True when any pixel of the marker's track is the marker's colour.
    pub fn marker_showing(&self) -> bool {
        let l = &self.layout;
        let (_, y) = l.at(0, l.track_row);
        (0..self.frame.width() as i32).any(|x| self.frame.get(x, y + CELL.1 / 2) == Some(DESK_INK))
    }
}

/// An empty folder for one test, under the build's own temporary directory.
pub fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ok-win").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a test folder can be made");
    dir
}

pub fn options() -> Options {
    Options { app_id: String::from("JasperMeller.ok.test"), name: String::from("ok-test"), icons: Vec::new(), open: None }
}

/// Runs the demo unseen with a script, in a fresh folder named for the test.
pub fn run(name: &str, script: Vec<Step>) -> Report {
    run_with(SelfTest::new(folder(name), script))
}

/// Runs the demo unseen and checks what every run must hold: the window was
/// cloaked for its whole life, never came to the front, and the test ended
/// because its script did.
pub fn run_with(test: SelfTest) -> Report {
    let report = ok_win::selftest(options(), test, |host| Box::new(Demo::new(host)));
    assert!(report.cloaked, "the window was not cloaked");
    assert_eq!(report.on_glass, Some(true), "what the window holds is not the frame the program drew");
    assert!(!report.was_foreground, "the window came to the front");
    assert!(!report.timed_out, "the script did not finish");
    report
}
