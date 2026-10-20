//! Driving the interface with no window.
//!
//! A [`Rig`] is an [`App`] with a stand-in for the shell round it: a host
//! whose clock moves only when told, whose clipboard is a string and whose
//! folders are temporary ones under the build directory. The rig sends the
//! events the shell would send, asks what the program wants after each as
//! the shell does, delivers the ticks it asks for while time passes, and
//! can hand back the screen as text or write it out as a picture.
//!
//! # The script language
//!
//! [`run`] reads a script, one command to a line. Blank lines and lines
//! starting with `#` are passed over. Paths are relative to the current
//! directory.
//!
//! ```text
//! file PATH             the text to open        (these three must come
//! folder PATH           the writing folder       before any other
//! look NAME             the look to start in     command)
//!
//! size W H              the client area in pixels
//! look NAME             change the look
//! scale N               1, 2 or 3
//! measure N             40 to 100
//! platen on|off         hold the typing row at the platen, or not
//! scroll smooth|jump    the paper rolls in four frames, or jumps
//! motion on|off         off is Windows with animations turned off
//! time Y-M-D H:M        set the wall clock
//! pace MS               time that passes after each typed character
//!
//! type TEXT             type the rest of the line; \n is Enter, \t is
//!                       Tab, \s a space, \\ a backslash, \u{e9} a
//!                       character by number
//! key CHORD [TIMES]     press a key: left, ctrl+shift+left, enter,
//!                       shift+tab, ctrl+z, alt+up, f11, pageup, esc ...
//! hold MODS             ctrl, alt, shift or several joined by +
//! release               let go of whatever is held
//! click X Y             press and let go of the left button at a pixel
//! click cell COL ROW    the same, in the middle of a cell
//! press X Y             the button down, for a drag
//! move X Y              the pointer moves
//! lift X Y              the button up
//! wheel ROWS            positive is away from the user
//! wait MS               let time pass, delivering the ticks asked for
//! tick                  one tick, now
//! focus on|off          the window gains or loses the keyboard
//! cursor LINE BYTE      put the cursor in the text, both counted from 0,
//!                       and bring its row to the platen
//! select L B L B        select from one such place to another
//! clipboard TEXT        put text on the clipboard
//! say TEXT             put a sentence on the say row, as the session
//!                       or the line would
//! open PATH             a second launch hands this one a file
//! close                 the window is closed
//!
//! shot PATH.png         draw the screen and write it as a PNG
//! print                 draw the screen and print it as text
//! ```

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ok_gfx::{encode_png, AlphaGlyph, Frame, Rgb};
use ok_host::{Event, FaceCell, Host, Key, LocalTime, Mods, MouseKind, Program, Waker, Wants};
use ok_text::{Pos, Span};

use crate::app::{App, FRAME_MS};
use crate::canvas::Screen;
use crate::flow::Flow;
use crate::line::Line;
use crate::session::{Options, Session};

/// The number of the day a date is, counted from 1 January 1970.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// The date of a day counted from 1 January 1970.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// Tells one rig's folders from another's within a test run.
static RIGS: AtomicUsize = AtomicUsize::new(0);

/// A host with nothing behind it: no window, no system clipboard, no
/// installed faces, and a clock that stands still until it is moved.
pub struct TestHost {
    /// Milliseconds on the clock that never goes backwards.
    pub now: u64,
    /// The wall clock, in seconds since 1970 as the person at the keyboard
    /// would count them, at the moment `now` was 0.
    wall: i64,
    pub clipboard: Option<String>,
    /// True stands for Windows with animations turned off.
    pub reduced_motion: bool,
    /// Every address handed to the browser.
    pub browsed: Vec<String>,
    /// Every character an outline face was asked for. None is ever
    /// supplied: this host has no faces.
    pub rasterized: Vec<char>,
    /// How many times a waker from this host has been called.
    pub woken: Arc<AtomicUsize>,
    /// The folder this host's folders are under, made when first asked for
    /// and removed when the host goes.
    root: OnceCell<PathBuf>,
}

impl Default for TestHost {
    fn default() -> TestHost {
        let mut host = TestHost { now: 0, wall: 0, clipboard: None, reduced_motion: false, browsed: Vec::new(), rasterized: Vec::new(), woken: Arc::default(), root: OnceCell::new() };
        host.set_time(2026, 10, 11, 14, 2);
        host
    }
}

impl TestHost {
    /// Sets the wall clock. It moves on from there as `now` does.
    pub fn set_time(&mut self, year: i32, month: u8, day: u8, hour: u8, minute: u8) {
        let days = days_from_civil(year as i64, month.clamp(1, 12) as i64, day.clamp(1, 31) as i64);
        self.wall = days * 86_400 + hour.min(23) as i64 * 3600 + minute.min(59) as i64 * 60 - (self.now / 1000) as i64;
    }

    fn folder(&self, name: &str) -> PathBuf {
        let root = self.root.get_or_init(|| {
            let number = RIGS.fetch_add(1, Ordering::Relaxed);
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/harness").join(format!("{}-{number}", std::process::id()))
        });
        let folder = root.join(name);
        // A folder that cannot be made is still a path; whoever uses it
        // finds out when it writes there.
        let _ = std::fs::create_dir_all(&folder);
        folder
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        if let Some(root) = self.root.get() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl Host for TestHost {
    fn now_ms(&self) -> u64 {
        self.now
    }

    fn local_time(&self) -> LocalTime {
        let seconds = self.wall + (self.now / 1000) as i64;
        let (days, of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        LocalTime {
            year: year as i32,
            month: month as u8,
            day: day as u8,
            hour: (of_day / 3600) as u8,
            minute: (of_day % 3600 / 60) as u8,
            second: (of_day % 60) as u8,
            // 1 January 1970 was a Thursday.
            weekday: (days + 3).rem_euclid(7) as u8,
            unix: seconds,
        }
    }

    fn waker(&self) -> Waker {
        let woken = self.woken.clone();
        Arc::new(move || {
            woken.fetch_add(1, Ordering::Relaxed);
        })
    }

    fn clipboard_get(&mut self) -> Option<String> {
        self.clipboard.clone()
    }

    fn clipboard_set(&mut self, text: &str) {
        self.clipboard = Some(text.to_string());
    }

    fn documents_dir(&self) -> PathBuf {
        self.folder("Documents")
    }

    fn local_data_dir(&self) -> PathBuf {
        self.folder("LocalAppData/ok")
    }

    fn roaming_data_dir(&self) -> PathBuf {
        self.folder("AppData/ok")
    }

    fn system_faces(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn load_face_file(&mut self, _path: &Path) -> Option<String> {
        None
    }

    fn face_cell(&mut self, _family: &str, _height: u32) -> Option<FaceCell> {
        None
    }

    fn rasterize(&mut self, _family: &str, _height: u32, ch: char) -> Option<AlphaGlyph> {
        self.rasterized.push(ch);
        None
    }

    fn open_in_browser(&mut self, url: &str) {
        self.browsed.push(url.to_string());
    }

    fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }
}

/// Reads a chord such as `ctrl+shift+left` into its key and modifiers.
pub fn chord(name: &str) -> Result<(Key, Mods), String> {
    let mut mods = Mods::NONE;
    let mut key = None;
    for part in name.split('+').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            _ if key.is_some() => return Err(format!("{name}: more than one key")),
            "left" => key = Some(Key::Left),
            "right" => key = Some(Key::Right),
            "up" => key = Some(Key::Up),
            "down" => key = Some(Key::Down),
            "home" => key = Some(Key::Home),
            "end" => key = Some(Key::End),
            "pageup" | "pgup" => key = Some(Key::PageUp),
            "pagedown" | "pgdn" => key = Some(Key::PageDown),
            "enter" | "return" => key = Some(Key::Enter),
            "tab" => key = Some(Key::Tab),
            "backspace" => key = Some(Key::Backspace),
            "delete" | "del" => key = Some(Key::Delete),
            "esc" | "escape" => key = Some(Key::Escape),
            "plus" => key = Some(Key::Char('=')),
            "minus" => key = Some(Key::Char('-')),
            other => {
                let function = other.strip_prefix('f').and_then(|digits| digits.parse::<u8>().ok()).filter(|n| (1..=12).contains(n));
                let mut chars = other.chars();
                key = Some(match (function, chars.next(), chars.next()) {
                    (Some(number), _, _) => Key::F(number),
                    (None, Some(ch), None) => Key::Char(ch),
                    _ => return Err(format!("{name}: no key called {part}")),
                });
            }
        }
    }
    key.map(|key| (key, mods)).ok_or_else(|| format!("{name}: no key named"))
}

/// Reads `ctrl`, `alt`, `shift` or several joined by `+`.
fn mods_named(name: &str) -> Result<Mods, String> {
    let mut mods = Mods::NONE;
    for part in name.split('+').map(|part| part.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            other => return Err(format!("{other} is not ctrl, alt or shift")),
        }
    }
    Ok(mods)
}

/// What `type` reads from a script line: characters to type and keys to
/// press, in order.
fn typed(text: &str) -> Result<Vec<Event>, String> {
    let press = |key: Key| Event::Key { key, mods: Mods::NONE, repeat: false };
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(Event::Text(ch));
            continue;
        }
        match chars.next() {
            Some('n') => out.push(press(Key::Enter)),
            Some('t') => out.push(press(Key::Tab)),
            Some('s') => out.push(Event::Text(' ')),
            Some('\\') => out.push(Event::Text('\\')),
            Some('u') => {
                let digits: String = chars.by_ref().skip_while(|&c| c == '{').take_while(|&c| c != '}').collect();
                let named = u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32);
                out.push(Event::Text(named.ok_or_else(|| format!("\\u{{{digits}}} is not a character"))?));
            }
            other => return Err(format!("\\{} is not an escape", other.map(String::from).unwrap_or_default())),
        }
    }
    Ok(out)
}

fn number<T: std::str::FromStr>(word: Option<&str>, what: &str) -> Result<T, String> {
    word.and_then(|word| word.parse().ok()).ok_or_else(|| format!("{what} wanted"))
}

fn on_off(word: Option<&str>, on: &str, off: &str) -> Result<bool, String> {
    match word {
        Some(word) if word.eq_ignore_ascii_case(on) => Ok(true),
        Some(word) if word.eq_ignore_ascii_case(off) => Ok(false),
        _ => Err(format!("{on} or {off} wanted")),
    }
}

/// The interface with a stand-in for the shell round it.
pub struct Rig {
    pub host: TestHost,
    pub app: App,
    frame: Frame,
    size: (usize, usize),
    /// What the program asked for after the last event.
    wants: Wants,
    /// The modifiers held down between commands.
    held: Mods,
    /// Milliseconds that pass after each typed character.
    pub pace: u64,
}

impl Rig {
    /// Starts the program with these options. The window has no size until
    /// [`Rig::size`] gives it one.
    pub fn new(options: Options) -> Rig {
        let mut host = TestHost::default();
        let mut app = App::new(&mut host, options);
        let wants = app.wants(&host);
        Rig { host, app, frame: Frame::new(0, 0), size: (0, 0), wants, held: Mods::NONE, pace: 0 }
    }

    /// Starts the program with a session, a line and a flow of the caller's
    /// choosing, for testing one seam with stand-ins for the others.
    pub fn with_seams(options: Options, session: Box<dyn Session>, line: Box<dyn Line>, flow: Box<dyn Flow>) -> Rig {
        let mut host = TestHost::default();
        let mut app = App::with_seams(&mut host, options, session, line, flow);
        let wants = app.wants(&host);
        Rig { host, app, frame: Frame::new(0, 0), size: (0, 0), wants, held: Mods::NONE, pace: 0 }
    }

    /// An empty text in a window of this many pixels.
    pub fn sized(width: usize, height: usize) -> Rig {
        let mut rig = Rig::new(Options::default());
        rig.size(width, height);
        rig
    }

    /// A window of this many pixels holding `text`, the cursor at its end.
    pub fn with_text(text: &str, width: usize, height: usize) -> Rig {
        let mut rig = Rig::sized(width, height);
        rig.app.open_text(crate::session::Opened { text: text.to_string(), ..Default::default() });
        rig.wants = rig.app.wants(&rig.host);
        rig
    }

    /// Sends one event and asks, as the shell does after every event, what
    /// the program wants now.
    pub fn send(&mut self, event: Event) {
        self.app.event(event, &mut self.host);
        self.wants = self.app.wants(&self.host);
    }

    /// What the program asked for after the last event.
    pub fn wants(&self) -> &Wants {
        &self.wants
    }

    pub fn size(&mut self, width: usize, height: usize) {
        self.size = (width, height);
        self.send(Event::Resize { width, height });
    }

    /// Types text a character at a time. A line break in it is Enter and a
    /// tab is the Tab key, since neither ever arrives as a character.
    pub fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\n' => self.send(Event::Key { key: Key::Enter, mods: Mods::NONE, repeat: false }),
                '\t' => self.send(Event::Key { key: Key::Tab, mods: Mods::NONE, repeat: false }),
                ch => self.send(Event::Text(ch)),
            }
            if self.pace > 0 {
                self.wait(self.pace);
            }
        }
    }

    /// Presses one key, with the modifiers its chord names going down
    /// before it and coming up after it as they would under a hand.
    pub fn key(&mut self, key: Key, mods: Mods) {
        let down = Mods { ctrl: mods.ctrl || self.held.ctrl, alt: mods.alt || self.held.alt, shift: mods.shift || self.held.shift };
        if down != self.held {
            self.send(Event::Mods(down));
        }
        self.send(Event::Key { key, mods: down, repeat: false });
        if down != self.held {
            self.send(Event::Mods(self.held));
        }
    }

    /// Presses the key a chord names, such as `ctrl+shift+left`.
    ///
    /// # Panics
    /// When the name is not a chord; this is for tests, which should not
    /// carry on past a misspelt key.
    pub fn press(&mut self, name: &str) {
        match chord(name) {
            Ok((key, mods)) => self.key(key, mods),
            Err(why) => panic!("{why}"),
        }
    }

    /// Holds modifiers down until [`Rig::release`].
    pub fn hold(&mut self, mods: Mods) {
        self.held = mods;
        self.send(Event::Mods(mods));
    }

    pub fn release(&mut self) {
        self.hold(Mods::NONE);
    }

    /// Presses and lets go of the left button at a pixel.
    pub fn click(&mut self, x: i32, y: i32) {
        self.send(Event::Mouse { kind: MouseKind::Down, x, y, mods: self.held });
        self.send(Event::Mouse { kind: MouseKind::Up, x, y, mods: self.held });
    }

    /// The pixel in the middle of a cell.
    pub fn cell_centre(&self, col: i32, row: i32) -> (i32, i32) {
        let cell = self.app.grid().cells(col, row, 1, 1);
        (cell.x + cell.w / 2, cell.y + cell.h / 2)
    }

    /// Clicks in the middle of a cell.
    pub fn click_cell(&mut self, col: i32, row: i32) {
        let (x, y) = self.cell_centre(col, row);
        self.click(x, y);
    }

    pub fn wheel(&mut self, rows: i32) {
        self.send(Event::Wheel { x: 0, y: 0, rows });
    }

    /// Lets `ms` milliseconds pass, delivering a tick every frame while the
    /// program is animating and one at each moment it asked to be woken.
    pub fn wait(&mut self, ms: u64) {
        let end = self.host.now.saturating_add(ms);
        loop {
            let next = match (self.wants.animating, self.wants.wake_in_ms) {
                (true, _) => Some(self.host.now.saturating_add(FRAME_MS)),
                (false, Some(wake)) => Some(self.host.now.saturating_add(wake.max(1))),
                (false, None) => None,
            };
            match next {
                Some(at) if at <= end => {
                    self.host.now = at;
                    self.send(Event::Tick);
                }
                _ => break,
            }
        }
        self.host.now = end;
        self.wants = self.app.wants(&self.host);
    }

    /// Draws the screen and hands back the picture.
    pub fn draw(&mut self) -> &Frame {
        if (self.frame.width(), self.frame.height()) != self.size {
            self.frame.resize(self.size.0, self.size.1);
        }
        self.app.render(&mut self.frame, &mut self.host);
        self.wants = self.app.wants(&self.host);
        &self.frame
    }

    /// Draws the screen and hands it back as text: what was put in every
    /// cell of the grid.
    pub fn screen(&mut self) -> &Screen {
        self.draw();
        self.app.screen()
    }

    /// Draws the screen and gives the colour of one pixel.
    pub fn pixel(&mut self, x: i32, y: i32) -> Option<Rgb> {
        self.draw().get(x, y)
    }

    /// The whole text, lines joined by `\n`.
    pub fn text(&self) -> String {
        self.app.page().doc().to_text("\n")
    }

    /// His own text, even while another is shown on the page in its place.
    pub fn own_text(&self) -> String {
        self.app.text().doc().to_text("\n")
    }

    /// Draws the screen and writes it as a PNG.
    pub fn shot(&mut self, path: &Path) -> std::io::Result<()> {
        let frame = self.draw();
        std::fs::write(path, encode_png(frame.width(), frame.height(), frame.pixels()))
    }

    /// Runs one command of the script language.
    pub fn command(&mut self, line: &str) -> Result<(), String> {
        let line = line.trim_start();
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let mut words = rest.split_whitespace();
        match word {
            "size" => {
                let (width, height) = (number(words.next(), "a width")?, number(words.next(), "a height")?);
                self.size(width, height);
            }
            "look" => {
                if !self.app.set_look(rest.trim()) {
                    return Err(format!("no look called {}", rest.trim()));
                }
            }
            "scale" => self.app.set_scale(number(words.next(), "a scale")?),
            "measure" => self.app.set_measure(number(words.next(), "a measure")?),
            "platen" => self.app.prefs_mut().platen = on_off(words.next(), "on", "off")?,
            "scroll" => self.app.prefs_mut().smooth = on_off(words.next(), "smooth", "jump")?,
            "motion" => self.host.reduced_motion = !on_off(words.next(), "on", "off")?,
            "pace" => self.pace = number(words.next(), "milliseconds")?,
            "time" => {
                let mut parts = rest.split(|ch: char| !ch.is_ascii_digit()).filter(|part| !part.is_empty());
                let year = number(parts.next(), "a year")?;
                let (month, day) = (number(parts.next(), "a month")?, number(parts.next(), "a day")?);
                let (hour, minute) = (number(parts.next(), "an hour")?, number(parts.next(), "a minute")?);
                self.host.set_time(year, month, day, hour, minute);
            }
            "type" => {
                for event in typed(rest)? {
                    self.send(event);
                    if self.pace > 0 {
                        self.wait(self.pace);
                    }
                }
            }
            "key" => {
                let (key, mods) = chord(words.next().ok_or("a key wanted")?)?;
                let times: usize = words.next().map_or(Ok(1), |word| number(Some(word), "a number of times"))?;
                for _ in 0..times {
                    self.key(key, mods);
                }
            }
            "hold" => self.hold(mods_named(rest.trim())?),
            "release" => self.release(),
            "click" | "press" | "move" | "lift" => {
                let (x, y) = match words.next() {
                    Some("cell") => self.cell_centre(number(words.next(), "a column")?, number(words.next(), "a row")?),
                    first => (number(first, "x")?, number(words.next(), "y")?),
                };
                let mouse = |kind| Event::Mouse { kind, x, y, mods: self.held };
                match word {
                    "click" => self.click(x, y),
                    "press" => self.send(mouse(MouseKind::Down)),
                    "move" => self.send(mouse(MouseKind::Move)),
                    _ => self.send(mouse(MouseKind::Up)),
                }
            }
            "wheel" => self.wheel(number(words.next(), "rows")?),
            "wait" => self.wait(number(words.next(), "milliseconds")?),
            "tick" => self.send(Event::Tick),
            "focus" => {
                let focused = on_off(words.next(), "on", "off")?;
                self.send(Event::Focus(focused));
            }
            "cursor" => {
                let at = Pos::new(number(words.next(), "a line")?, number(words.next(), "a byte")?);
                self.app.go(at);
                self.send(Event::Tick);
            }
            "select" => {
                let start = Pos::new(number(words.next(), "a line")?, number(words.next(), "a byte")?);
                let end = Pos::new(number(words.next(), "a line")?, number(words.next(), "a byte")?);
                self.app.select(Span { start, end });
                self.send(Event::Tick);
            }
            "clipboard" => self.host.clipboard = Some(rest.to_string()),
            "say" => self.app.say(rest.trim()),
            "open" => self.send(Event::Open(PathBuf::from(rest.trim()))),
            "close" => self.send(Event::Close),
            "shot" => {
                let path = Path::new(rest.trim());
                self.shot(path).map_err(|why| format!("{}: {why}", path.display()))?;
            }
            "print" => {
                for row in self.screen().lines() {
                    println!("{}", row.trim_end());
                }
            }
            other => return Err(format!("no command called {other}")),
        }
        Ok(())
    }
}

/// Runs a whole script and hands back the rig as the script left it. An
/// error names the line it came from.
pub fn run(script: &str) -> Result<Rig, String> {
    let mut options = Options::default();
    let mut rig: Option<Rig> = None;
    for (number, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (word, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
        let fail = |why: String| format!("line {}: {why}", number + 1);
        match (word, &mut rig) {
            ("file", None) => options.file = Some(PathBuf::from(rest.trim())),
            ("folder", None) => options.folder = Some(PathBuf::from(rest.trim())),
            ("look", None) => options.look = Some(rest.trim().to_string()),
            ("file" | "folder", Some(_)) => return Err(fail(format!("{word} must come before the first command that acts"))),
            (_, started) => started.get_or_insert_with(|| Rig::new(options.clone())).command(trimmed).map_err(fail)?,
        }
    }
    Ok(rig.unwrap_or_else(|| Rig::new(options)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_dates_go_both_ways() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 11), 20_737);
        for days in [-1, 0, 59, 60, 365, 11_016, 20_737, 20_738, 100_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn the_wall_clock_moves_with_the_other_clock() {
        let mut host = TestHost::default();
        host.set_time(2026, 10, 11, 23, 59);
        let now = host.local_time();
        assert_eq!((now.year, now.month, now.day, now.hour, now.minute, now.weekday), (2026, 10, 11, 23, 59, 6));
        host.now += 61_000;
        let now = host.local_time();
        assert_eq!((now.month, now.day, now.hour, now.minute, now.second, now.weekday), (10, 12, 0, 0, 1, 0));
    }

    #[test]
    fn chords_are_read_by_name() {
        assert_eq!(chord("ctrl+shift+left"), Ok((Key::Left, Mods { ctrl: true, alt: false, shift: true })));
        assert_eq!(chord("Enter"), Ok((Key::Enter, Mods::NONE)));
        assert_eq!(chord("alt+up"), Ok((Key::Up, Mods::ALT)));
        assert_eq!(chord("f11"), Ok((Key::F(11), Mods::NONE)));
        assert_eq!(chord("ctrl+f"), Ok((Key::Char('f'), Mods::CTRL)));
        assert_eq!(chord("ctrl+="), Ok((Key::Char('='), Mods::CTRL)));
        assert_eq!(chord("ctrl+minus"), Ok((Key::Char('-'), Mods::CTRL)));
        assert!(chord("ctrl").is_err());
        assert!(chord("left+right").is_err());
        assert!(chord("f13").is_err());
        assert!(chord("banana").is_err());
    }

    #[test]
    fn a_script_names_the_line_it_fails_on() {
        assert_eq!(run("size 800 600\n\n# fine so far\nfrobnicate 3").err().as_deref(), Some("line 4: no command called frobnicate"));
        assert_eq!(run("size 800").err().as_deref(), Some("line 1: a height wanted"));
        assert_eq!(run("type a\\q").err().as_deref(), Some("line 1: \\q is not an escape"));
        assert_eq!(run("size 800 600\nfile x.md").err().as_deref(), Some("line 2: file must come before the first command that acts"));
        assert!(run("").is_ok());
    }

    #[test]
    fn typing_in_a_script_reads_its_escapes() {
        let rig = run("size 960 528\ntype one\\ntwo\\stwo \\\\ caf\\u{e9}").unwrap();
        assert_eq!(rig.text(), "one\n\ntwo two \\ caf\u{e9}");
    }
}
