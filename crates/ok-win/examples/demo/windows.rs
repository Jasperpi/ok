//! The demo itself; it needs the Win32 shell.

use std::path::PathBuf;

use ok_gfx::{BitmapFont, Frame, Pen, Rect, Rgb};
use ok_host::{Event, Host, Key, Mods, MouseKind, Program, Wants};
use ok_win::{Icon, Options, SelfTest, Step};

const GALLANT: &str = include_str!("../../../../assets/fonts/gallant/gallant.hex");

pub const PAPER: Rgb = Rgb(0xEE, 0xE3, 0xC8);
pub const INK: Rgb = Rgb(0x24, 0x1E, 0x19);
pub const DESK: Rgb = Rgb(0x35, 0x25, 0x1B);
pub const DESK_INK: Rgb = Rgb(0xE6, 0xD9, 0xBA);

/// Gallant's cell.
pub const CELL: (i32, i32) = (12, 22);
/// The smallest window the demo is any use in, in cells.
pub const MIN_CELLS: (u32, u32) = (24, 9);
/// How long the marker runs after a key.
pub const RUN_MS: u64 = 1000;
/// How long after the last character `ok` is printed. Longer than the
/// marker's run, so the wake arrives while nothing else is ticking.
pub const OK_AFTER_MS: u64 = 1500;

const WORDS_FILE: &str = "demo-words.txt";

/// Where everything is for a frame of a given size, in cells. Rows from the
/// top: one of desk (the strip that drags the window), the sheet, one of
/// desk, the marker's track, and the row that names the last event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub cols: i32,
    pub rows: i32,
    /// The pixel of cell (0, 0): leftover pixels are split evenly.
    pub x0: i32,
    pub y0: i32,
    pub sheet_col: i32,
    pub sheet_cols: i32,
    pub sheet_row: i32,
    pub sheet_rows: i32,
    /// Where the typed text starts, and how much of it fits.
    pub text_col: i32,
    pub text_row: i32,
    pub measure: i32,
    pub text_rows: i32,
    /// The sheet row `ok` is printed on.
    pub machine_row: i32,
    pub track_row: i32,
    pub status_row: i32,
}

impl Layout {
    pub fn new(width: usize, height: usize) -> Layout {
        let (width, height) = (width.min(1 << 20) as i32, height.min(1 << 20) as i32);
        let (cols, rows) = (width / CELL.0, height / CELL.1);
        let measure = (cols - 8).clamp(1, 40);
        let sheet_cols = measure + 4;
        let sheet_col = (cols - sheet_cols) / 2;
        Layout {
            cols,
            rows,
            x0: (width - cols * CELL.0) / 2,
            y0: (height - rows * CELL.1) / 2,
            sheet_col,
            sheet_cols,
            sheet_row: 1,
            sheet_rows: rows - 4,
            text_col: sheet_col + 2,
            text_row: 2,
            measure,
            text_rows: rows - 8,
            machine_row: rows - 5,
            track_row: rows - 2,
            status_row: rows - 1,
        }
    }

    /// The top-left pixel of a cell.
    pub fn at(&self, col: i32, row: i32) -> (i32, i32) {
        (self.x0 + col * CELL.0, self.y0 + row * CELL.1)
    }

    pub fn sheet(&self) -> Rect {
        let (x, y) = self.at(self.sheet_col, self.sheet_row);
        Rect::new(x, y, self.sheet_cols * CELL.0, self.sheet_rows * CELL.1)
    }
}

pub struct Demo {
    face: BitmapFont,
    size: (usize, usize),
    text: Vec<char>,
    last: String,
    ticks: u64,
    dirty: bool,
    quit: bool,
    fullscreen: bool,
    pointer_hidden: bool,
    /// The marker is on the desk until this moment.
    running: bool,
    run_until: u64,
    /// When `ok` is due, while it is not showing.
    ok_at: Option<u64>,
    ok: bool,
}

impl Demo {
    pub fn new(host: &mut dyn Host) -> Demo {
        let face = BitmapFont::from_hex("Gallant", GALLANT).expect("the built-in face is well formed");
        let kept = std::fs::read_to_string(host.local_data_dir().join(WORDS_FILE)).unwrap_or_default();
        Demo {
            face,
            size: (0, 0),
            last: if kept.is_empty() { String::from("Nothing yet") } else { format!("Brought back {} characters", kept.chars().count()) },
            text: kept.chars().collect(),
            ticks: 0,
            dirty: true,
            quit: false,
            fullscreen: false,
            pointer_hidden: false,
            running: false,
            run_until: 0,
            ok_at: None,
            ok: true,
        }
    }

    fn typed(&self) -> String {
        self.text.iter().collect()
    }

    /// Puts what is typed on disk. Called for a close, which is also what the
    /// shell sends after a panic.
    fn keep(&self, host: &mut dyn Host) {
        let dir = host.local_data_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(dir.join(WORDS_FILE), self.typed());
        }
    }

    fn changed(&mut self, now: u64) {
        self.ok = false;
        self.ok_at = Some(now + OK_AFTER_MS);
    }

    /// Lays the text out in the measure and reports each character's cell,
    /// then the cell the cursor rests in.
    fn cells(&self, layout: &Layout, mut each: impl FnMut(char, i32, i32)) -> (i32, i32) {
        let (mut col, mut row) = (0, 0);
        for &ch in &self.text {
            if ch == '\n' {
                (col, row) = (0, row + 1);
                continue;
            }
            let wide = self.face.glyph(ch).is_some_and(|g| g.width > CELL.0 as u32);
            let width = if wide { 2 } else { 1 };
            if col + width > layout.measure {
                (col, row) = (0, row + 1);
            }
            each(ch, col, row);
            col += width;
        }
        if col >= layout.measure {
            (col, row) = (0, row + 1);
        }
        (col, row)
    }

    fn cursor(&self, layout: &Layout) -> Rect {
        let (col, row) = self.cells(layout, |_, _, _| {});
        let (x, y) = layout.at(layout.text_col + col, layout.text_row + row);
        Rect::new(x, y, CELL.0, CELL.1)
    }

    fn write(&self, frame: &mut Frame, layout: &Layout, col: i32, row: i32, words: &str, color: Rgb) {
        let (mut x, y) = layout.at(col, row);
        for ch in words.chars() {
            x += self.face.draw(frame, ch, x, y, Pen::new(color));
        }
    }

    fn key(&mut self, key: Key, mods: Mods, now: u64, host: &mut dyn Host) {
        match (key, mods.ctrl, mods.alt) {
            (Key::Char('q'), true, false) => self.quit = true,
            (Key::Char('c'), true, false) => host.clipboard_set(&self.typed()),
            (Key::Char('v'), true, false) => {
                if let Some(pasted) = host.clipboard_get() {
                    self.text.extend(pasted.chars().filter(|c| *c == '\n' || !c.is_control()));
                    self.changed(now);
                }
            }
            (Key::Enter, true, false) => host.open_in_browser(self.typed().trim()),
            (Key::Enter, false, false) => {
                self.text.push('\n');
                self.changed(now);
            }
            (Key::Backspace, _, false) => {
                if self.text.pop().is_some() {
                    self.changed(now);
                }
            }
            (Key::Escape, false, false) => {
                if !self.text.is_empty() {
                    self.text.clear();
                    self.changed(now);
                }
            }
            (Key::F(11), false, false) => self.fullscreen = !self.fullscreen,
            (Key::F(9), false, false) => panic!("the demo was asked to panic with F9"),
            _ => {}
        }
    }
}

/// A key the way the design writes one: `Ctrl S`, `Shift Tab`, `Page Up`.
fn key_words(key: Key, mods: Mods) -> String {
    let name = match key {
        Key::Left => "Left".to_string(),
        Key::Right => "Right".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "Page Up".to_string(),
        Key::PageDown => "Page Down".to_string(),
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Escape => "Esc".to_string(),
        Key::F(n) => format!("F{n}"),
        Key::Char(' ') => "Space".to_string(),
        Key::Char(c) => c.to_ascii_uppercase().to_string(),
    };
    format!("{}{name}", held_words(mods))
}

fn held_words(mods: Mods) -> String {
    let mut words = String::new();
    for (held, name) in [(mods.ctrl, "Ctrl "), (mods.alt, "Alt "), (mods.shift, "Shift ")] {
        if held {
            words.push_str(name);
        }
    }
    words
}

/// An event in words, for the bottom row.
fn event_words(event: &Event) -> Option<String> {
    Some(match event {
        Event::Key { key, mods, repeat } => format!("Key {}{}", key_words(*key, *mods), if *repeat { ", repeating" } else { "" }),
        Event::Text(c) if c.is_ascii_graphic() => format!("Text {c}"),
        Event::Text(c) => format!("Text U+{:04X}", *c as u32),
        Event::Mods(mods) if mods.any() => format!("Holding {}", held_words(*mods).trim_end()),
        Event::Mods(_) => String::from("Holding nothing"),
        Event::Mouse { kind, x, y, mods } => {
            let what = match kind {
                MouseKind::Down => "Mouse down at",
                MouseKind::Up => "Mouse up at",
                MouseKind::Move => "Mouse at",
            };
            format!("{what} {x}, {y}{}", if mods.any() { format!(" with {}", held_words(*mods).trim_end()) } else { String::new() })
        }
        Event::Wheel { x, y, rows } => format!("Wheel {} rows {} at {x}, {y}", rows.abs(), if *rows > 0 { "up" } else { "down" }),
        Event::Resize { width, height } => format!("Resize to {width} by {height}"),
        Event::Focus(true) => String::from("Focus gained"),
        Event::Focus(false) => String::from("Focus lost"),
        Event::Close => String::from("Close"),
        Event::Open(path) => format!("Open {}", path.display()),
        Event::Tick => return None,
    })
}

impl Program for Demo {
    fn event(&mut self, event: Event, host: &mut dyn Host) {
        let now = host.now_ms();
        if let Some(words) = event_words(&event) {
            self.last = words;
            self.dirty = true;
        }
        match event {
            Event::Key { key, mods, .. } => {
                self.key(key, mods, now, host);
                (self.running, self.run_until) = (true, now + RUN_MS);
            }
            Event::Text(ch) => {
                self.text.push(ch);
                self.changed(now);
                self.pointer_hidden = true;
                (self.running, self.run_until) = (true, now + RUN_MS);
            }
            Event::Mouse { .. } => self.pointer_hidden = false,
            Event::Resize { width, height } => self.size = (width, height),
            Event::Tick => {
                self.ticks += 1;
                self.dirty = true;
                if self.ok_at.is_some_and(|at| now >= at) {
                    (self.ok, self.ok_at) = (true, None);
                }
            }
            Event::Close => self.keep(host),
            Event::Open(_) | Event::Focus(_) | Event::Wheel { .. } | Event::Mods(_) => {}
        }
    }

    fn wants(&mut self, host: &dyn Host) -> Wants {
        let now = host.now_ms();
        // The marker's second is over: one more frame takes it off the desk.
        if self.running && now >= self.run_until {
            self.running = false;
            self.dirty = true;
        }
        let count = self.text.len();
        Wants {
            redraw: self.dirty,
            animating: self.running,
            wake_in_ms: self.ok_at.map(|at| at.saturating_sub(now)),
            quit: self.quit,
            fullscreen: self.fullscreen,
            pointer_hidden: self.pointer_hidden,
            caret: Some(self.cursor(&Layout::new(self.size.0, self.size.1))),
            title: format!("ok demo, {count} character{}", if count == 1 { "" } else { "s" }),
            cell: (CELL.0 as u32, CELL.1 as u32),
            min_cells: MIN_CELLS,
            caption_height: CELL.1 as u32,
            ground: DESK,
        }
    }

    fn render(&mut self, frame: &mut Frame, host: &mut dyn Host) {
        self.dirty = false;
        let layout = Layout::new(frame.width(), frame.height());
        frame.clear(DESK);
        frame.fill(layout.sheet(), PAPER);

        // The words, kept inside the sheet however small it gets.
        let old = frame.set_clip(layout.sheet());
        let mut letters = Vec::with_capacity(self.text.len());
        let (cursor_col, cursor_row) = self.cells(&layout, |ch, col, row| letters.push((ch, col, row)));
        for (ch, col, row) in letters {
            if row < layout.text_rows {
                let (x, y) = layout.at(layout.text_col + col, layout.text_row + row);
                self.face.draw(frame, ch, x, y, Pen::new(INK));
            }
        }
        if cursor_row < layout.text_rows {
            let (x, y) = layout.at(layout.text_col + cursor_col, layout.text_row + cursor_row);
            frame.fill(Rect::new(x, y, CELL.0, CELL.1), INK);
        }
        if self.ok {
            self.write(frame, &layout, layout.text_col, layout.machine_row, "ok", INK);
        }
        frame.set_clip(old);

        if self.running {
            let left = self.run_until.saturating_sub(host.now_ms()).min(RUN_MS) as i32;
            let span = (layout.cols - 5).max(0) * CELL.0;
            let (x, y) = layout.at(2, layout.track_row);
            let travelled = span - span * left / RUN_MS as i32;
            frame.fill(Rect::new(x + travelled, y, CELL.0, CELL.1), DESK_INK);
        }
        self.write(frame, &layout, 2, layout.status_row, &self.last, DESK_INK);
        let ticks = format!("{} ticks", self.ticks);
        let right = layout.cols - 2 - ticks.len() as i32;
        if right > 4 + self.last.chars().count() as i32 {
            self.write(frame, &layout, right, layout.status_row, &ticks, DESK_INK);
        }
    }
}

/// The demo's icon at one size: a sheet on a desk with a block cursor.
fn icon(side: u32) -> Icon {
    let px = |c: Rgb| 0xFF00_0000 | c.px();
    let (inset, block) = (side / 8, side / 4);
    let mut argb = vec![px(DESK); (side * side) as usize];
    for y in inset..side - inset {
        for x in inset * 2..side - inset * 2 {
            let on_block = x >= side / 2 - block / 2 && x < side / 2 + block / 2 && y >= side / 2 - block && y < side / 2 + block / 2;
            argb[(y * side + x) as usize] = px(if on_block { INK } else { PAPER });
        }
    }
    Icon { width: side, height: side, argb }
}

fn options() -> Options {
    Options {
        app_id: String::from("JasperMeller.ok.demo"),
        name: String::from("ok-demo"),
        icons: vec![icon(16), icon(32), icon(48)],
        open: None,
    }
}

pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, png] = args.as_slice() {
        if flag == "--selftest" {
            let png = PathBuf::from(png);
            let data = png.with_extension("selftest-data");
            // Words kept by an earlier run would be brought back and make
            // this frame differ from the last one.
            let _ = std::fs::remove_file(data.join("Local").join(WORDS_FILE));
            let script = vec![
                Step::Type(String::from("The line you are writing is held still.")),
                Step::Press(Key::Enter),
                Step::Type(String::from("ok")),
                Step::Wait(OK_AFTER_MS + 200),
            ];
            let test = SelfTest { png: Some(png), ..SelfTest::new(data, script) };
            let report = ok_win::selftest(options(), test, |host| Box::new(Demo::new(host)));
            println!(
                "{} by {} pixels, {} ticks, drawn {} times, exit code {}",
                report.width,
                report.height,
                report.ticks.len(),
                report.renders,
                report.exit_code
            );
            std::process::exit(report.exit_code);
        }
    }
    let open = args.get(1).filter(|a| !a.starts_with("--")).map(PathBuf::from);
    std::process::exit(ok_win::run(Options { open, ..options() }, |host| Box::new(Demo::new(host))));
}
