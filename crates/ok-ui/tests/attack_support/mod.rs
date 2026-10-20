//! What the attack tests share: a reference model of the text, the events a
//! hand and a window can send, the invariants checked after every one of
//! them, and a shrinker that cuts a failing run down to its cause.
//!
//! The model is a plain `String` with line breaks as `\n` and two byte
//! offsets, the cursor and where the selection began. For the keys whose
//! meaning DESIGN.md fixes exactly it says what the text and the cursor must
//! be afterwards; for the rest (rows, words, sentences) it says what cannot
//! have happened. It reads nothing from the interface but the text and the
//! cursor before the key.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Once;

use ok_host::{Event, Host, Key, Mods, MouseKind};
use ok_text::{cell_width, is_combining, str_width, Change, Doc, Pos};
use ok_ui::flow::Rest;
use ok_ui::harness::Rig;
use ok_ui::line::Closed;
use ok_ui::page::Page;
use ok_ui::session::{Opened, Options, Session};

// ───────────────────────── a generator that can be run again ─────────────────────────

/// A fixed-seed generator, so a failure can be run again.
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() >> 16) as usize % n.max(1)
    }

    pub fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    pub fn pick<T: Clone>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())].clone()
    }
}

/// How many times over to run the campaigns: `OK_ATTACK_SCALE=20`.
pub fn scale() -> u64 {
    std::env::var("OK_ATTACK_SCALE").ok().and_then(|scale| scale.parse().ok()).unwrap_or(1).max(1)
}

// ───────────────────────── events ─────────────────────────

/// One thing a hand or a window does.
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    Text(char),
    Key(Key, Mods),
    /// Puts this on the clipboard and presses Ctrl V.
    Paste(String),
    /// A click in the middle of a cell of the grid as it then is.
    Click { col: i32, row: i32, shift: bool },
    /// Button down in one cell, a move to another, button up there.
    Drag { from: (i32, i32), to: (i32, i32) },
    Wheel(i32),
    Size(usize, usize),
    Scale(u32),
    Wait(u64),
    Focus(bool),
    Tick,
}

pub const NONE: Mods = Mods::NONE;
pub const CTRL: Mods = Mods::CTRL;
pub const ALT: Mods = Mods::ALT;
pub const SHIFT: Mods = Mods::SHIFT;
pub const CTRL_SHIFT: Mods = Mods { ctrl: true, alt: false, shift: true };
pub const ALT_SHIFT: Mods = Mods { ctrl: false, alt: true, shift: true };
pub const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false };

/// A chord as the script language spells it, for printing a failing run.
pub fn chord_name(key: Key, mods: Mods) -> String {
    let mut name = String::new();
    for (held, word) in [(mods.ctrl, "ctrl+"), (mods.alt, "alt+"), (mods.shift, "shift+")] {
        if held {
            name.push_str(word);
        }
    }
    match key {
        Key::Left => name.push_str("left"),
        Key::Right => name.push_str("right"),
        Key::Up => name.push_str("up"),
        Key::Down => name.push_str("down"),
        Key::Home => name.push_str("home"),
        Key::End => name.push_str("end"),
        Key::PageUp => name.push_str("pageup"),
        Key::PageDown => name.push_str("pagedown"),
        Key::Enter => name.push_str("enter"),
        Key::Tab => name.push_str("tab"),
        Key::Backspace => name.push_str("backspace"),
        Key::Delete => name.push_str("delete"),
        Key::Escape => name.push_str("esc"),
        Key::F(number) => name.push_str(&format!("f{number}")),
        Key::Char(ch) => name.push(ch),
    }
    name
}

/// One event as a line a person can read and type back in.
pub fn describe(ev: &Ev) -> String {
    match ev {
        Ev::Text(ch) => format!("type {:?}", ch.to_string()),
        Ev::Key(key, mods) => format!("press {}", chord_name(*key, *mods)),
        Ev::Paste(text) => format!("paste {text:?}"),
        other => format!("{other:?}"),
    }
}

/// A run of events, one to a line, with consecutive typing joined.
pub fn describe_all(events: &[Ev]) -> String {
    let mut out = String::new();
    let mut typed = String::new();
    let flush = |typed: &mut String, out: &mut String| {
        if !typed.is_empty() {
            out.push_str(&format!("  type {typed:?}\n"));
            typed.clear();
        }
    };
    for ev in events {
        match ev {
            Ev::Text(ch) => typed.push(*ch),
            other => {
                flush(&mut typed, &mut out);
                out.push_str(&format!("  {}\n", describe(other)));
            }
        }
    }
    flush(&mut typed, &mut out);
    out
}

// ───────────────────────── the model's reading of a text ─────────────────────────

/// What a line is, as far as the keys care. Read from DESIGN.md and the
/// doc comments of `ok_text::markup::LineKind`, written again here so that
/// the two can disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Blank,
    Fence,
    Code,
    Item,
    Other,
}

fn is_gap(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b' ' | b'\t'))
}

pub fn is_blank(line: &str) -> bool {
    line.chars().all(char::is_whitespace)
}

fn is_fence(line: &str) -> bool {
    let pad = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line.as_bytes()[pad..];
    pad <= 3 && (rest.starts_with(b"```") || rest.starts_with(b"~~~"))
}

fn is_rule(line: &str) -> bool {
    let mut mark = 0u8;
    let mut count = 0;
    for byte in line.bytes() {
        match byte {
            b' ' | b'\t' => {}
            b'-' | b'*' | b'_' if mark == 0 || mark == byte => {
                mark = byte;
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// The parts of an item's marker: bytes of indentation, the byte after the
/// bullet or number, whether a task box follows, and the byte where the
/// item's own words start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    pub indent: usize,
    pub mark_end: usize,
    pub stop_end: usize,
    pub boxed: bool,
    pub text: usize,
}

pub fn marker(line: &str) -> Option<Marker> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    let (mark_end, stop_end) = match bytes.get(indent)? {
        b'-' | b'*' | b'+' => (indent + 1, indent + 1),
        b'0'..=b'9' => {
            let digits = bytes[indent..].iter().take_while(|b| b.is_ascii_digit()).count();
            if !matches!(bytes.get(indent + digits), Some(b'.' | b')')) {
                return None;
            }
            (indent + digits, indent + digits + 1)
        }
        _ => return None,
    };
    if !is_gap(bytes.get(stop_end)) {
        return None;
    }
    let mut text = stop_end + 1;
    let boxed = matches!(bytes.get(text..text + 4), Some([b'[', b' ' | b'x' | b'X', b']', b' ' | b'\t']));
    if boxed {
        text += 4;
    }
    Some(Marker { indent, mark_end, stop_end, boxed, text })
}

/// The marker Enter writes for the item after this one.
pub fn next_marker(line: &str, m: Marker) -> String {
    let mark = &line[m.indent..m.mark_end];
    let mark = match mark.parse::<u64>() {
        Ok(number) => number.saturating_add(1).to_string(),
        Err(_) => mark.to_string(),
    };
    format!("{}{}{} {}", &line[..m.indent], mark, &line[m.mark_end..m.stop_end], if m.boxed { "[ ] " } else { "" })
}

pub fn kinds(lines: &[&str]) -> Vec<Kind> {
    let mut in_fence = false;
    lines
        .iter()
        .map(|line| {
            if is_fence(line) {
                in_fence = !in_fence;
                Kind::Fence
            } else if in_fence {
                Kind::Code
            } else if is_blank(line) {
                Kind::Blank
            } else if is_rule(line) || line.starts_with('#') || line.trim_start_matches([' ', '\t']).starts_with('>') {
                Kind::Other
            } else if marker(line).is_some() {
                Kind::Item
            } else {
                Kind::Other
            }
        })
        .collect()
}

/// The byte where each line of a flat text starts.
pub fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1)).collect()
}

fn line_of(starts: &[usize], at: usize) -> usize {
    starts.partition_point(|&start| start <= at) - 1
}

/// One cluster back in a flat text: over any marks and the character they
/// sit on, never past the start of the line except by one line break.
pub fn left(text: &str, at: usize) -> usize {
    let before = &text[..at];
    let mut chars = before.char_indices().rev();
    match chars.next() {
        None => 0,
        Some((i, '\n')) => i,
        Some((i, ch)) => {
            let mut to = i;
            if is_combining(ch) {
                for (i, ch) in chars {
                    if ch == '\n' {
                        break;
                    }
                    to = i;
                    if !is_combining(ch) {
                        break;
                    }
                }
            }
            to
        }
    }
}

/// One cluster on in a flat text.
pub fn right(text: &str, at: usize) -> usize {
    let mut chars = text[at..].char_indices();
    match chars.next() {
        None => at,
        Some((_, '\n')) => at + 1,
        Some(_) => at + chars.find(|&(_, ch)| !is_combining(ch)).map_or(text.len() - at, |(i, _)| i),
    }
}

/// True when a cursor at `at` does not stand between a character and a
/// mark that sits on it.
pub fn on_cluster_boundary(text: &str, at: usize) -> bool {
    if !text.is_char_boundary(at) {
        return false;
    }
    let before = text[..at].chars().next_back();
    let after = text[at..].chars().next();
    match (before, after) {
        (None, _) | (Some('\n'), _) | (_, None) => true,
        (_, Some(after)) => !is_combining(after),
    }
}

/// What Ctrl V puts in: the clipboard less the control characters no key
/// could have typed, line breaks of every kind as one `\n`.
pub fn pasted(clip: &str) -> String {
    let kept: String = clip.chars().filter(|&ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t')).collect();
    kept.replace("\r\n", "\n").replace('\r', "\n")
}

// ───────────────────────── a session that keeps its own copy ─────────────────────────

/// The text as a journal would have it: built from nothing but the changes
/// the interface hands the session.
#[derive(Default)]
pub struct Mirror {
    pub lines: Vec<String>,
    pub wrong: Option<String>,
    pub changes: usize,
}

impl Mirror {
    fn apply(&mut self, change: &Change) -> Result<(), String> {
        let (start, old_end) = (change.start, change.old_end);
        let head = self.lines.get(start.line).and_then(|line| line.get(..start.col)).ok_or("starts outside the text")?.to_string();
        let tail = self.lines.get(old_end.line).and_then(|line| line.get(old_end.col..)).ok_or("ends outside the text")?.to_string();
        if old_end < start {
            return Err(String::from("ends before it starts"));
        }
        let mut removed = String::new();
        for line in start.line..=old_end.line {
            let text = &self.lines[line];
            let from = if line == start.line { start.col } else { 0 };
            let to = if line == old_end.line { old_end.col } else { text.len() };
            removed.push_str(text.get(from..to).ok_or("cuts a character")?);
            if line < old_end.line {
                removed.push('\n');
            }
        }
        if removed != change.removed {
            return Err(format!("says it removed {:?} where the text read {removed:?}", change.removed));
        }
        let mut fresh: Vec<String> = change.inserted.split('\n').map(str::to_string).collect();
        fresh[0].insert_str(0, &head);
        let last = fresh.len() - 1;
        let end = Pos::new(start.line + last, fresh[last].len());
        fresh[last].push_str(&tail);
        self.lines.splice(start.line..=old_end.line, fresh);
        if end != change.end {
            return Err(format!("says it ends at {:?}, which is {end:?}", change.end));
        }
        Ok(())
    }
}

struct Journal {
    first: Opened,
    mirror: Rc<RefCell<Mirror>>,
}

impl Session for Journal {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        self.first.clone()
    }

    fn changed(&mut self, _doc: &Doc, change: &Change, _host: &mut dyn Host) {
        let mut mirror = self.mirror.borrow_mut();
        mirror.changes += 1;
        if mirror.wrong.is_none() {
            if let Err(why) = mirror.apply(change) {
                mirror.wrong = Some(format!("change {change:?} {why}"));
            }
        }
    }
}

// ───────────────────────── a run ─────────────────────────

/// How a run starts.
#[derive(Clone, Debug)]
pub struct Setup {
    pub text: String,
    /// Where the cursor starts, as a line and a byte; the end when `None`.
    pub cursor: Option<(usize, usize)>,
    pub size: (usize, usize),
    pub smooth: bool,
    pub platen: bool,
}

impl Setup {
    pub fn new(text: &str) -> Setup {
        Setup { text: text.to_string(), cursor: None, size: (960, 528), smooth: false, platen: true }
    }

    pub fn sized(mut self, width: usize, height: usize) -> Setup {
        self.size = (width, height);
        self
    }
}

/// What went wrong: a short name for the kind of failure, which is what the
/// shrinker holds on to, and the particulars.
#[derive(Clone, Debug)]
pub struct Fail {
    pub kind: String,
    pub msg: String,
}

fn fail<T>(kind: &str, msg: String) -> Result<T, Fail> {
    Err(Fail { kind: kind.to_string(), msg })
}

/// Failures already reduced to a named test in `attack_found.rs`, which the
/// campaigns pass over so that they can go on to find the next thing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Known {
    /// A word or sentence key leaves the cursor between a character and a
    /// mark on it.
    pub inside_cluster: bool,
    /// Enter on an empty item with a selection reaching into later lines
    /// takes the rest of the last line with it.
    pub enter_empty_item_selection: bool,
    /// The screen leaves out or misplaces a character the layout has on a
    /// visible row.
    pub screen_rows: bool,
    /// What is drawn in the cursor's own cells is not the character of the
    /// text that belongs there: a mark with no picture where the letter
    /// after it should show, or the `#` of a title whose row is left off.
    pub cursor_cell: bool,
    /// The cursor is not drawn inside the sheet.
    pub cursor_off_sheet: bool,
    /// Typing at the platen moved the typing row.
    pub platen_moves: bool,
}

/// The text and the cursor, flat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snap {
    pub text: String,
    pub head: usize,
    pub anchor: usize,
}

impl Snap {
    pub fn of(rig: &Rig) -> Snap {
        let page = rig.app.page();
        let doc = page.doc();
        let flat = |pos: Pos| doc.lines()[..pos.line.min(doc.line_count())].iter().map(|line| line.len() + 1).sum::<usize>() + pos.col;
        Snap { text: rig.text(), head: flat(page.cursor.head), anchor: flat(page.cursor.anchor) }
    }

    pub fn span(&self) -> (usize, usize) {
        (self.head.min(self.anchor), self.head.max(self.anchor))
    }

    /// The text with `|` where the cursor is and `^` where a selection
    /// began, for messages.
    pub fn show(&self) -> String {
        let mut marks = vec![(self.head, '|')];
        if self.anchor != self.head {
            marks.push((self.anchor, '^'));
        }
        marks.sort();
        let mut out = String::new();
        let mut from = 0;
        for (at, mark) in marks {
            let at = at.min(self.text.len());
            out.push_str(self.text.get(from..at).unwrap_or("?"));
            out.push(mark);
            from = at;
        }
        out.push_str(self.text.get(from..).unwrap_or("?"));
        format!("{out:?}")
    }
}

/// A rig with a journal behind it, and what a run remembers between events.
pub struct Run {
    pub rig: Rig,
    pub mirror: Rc<RefCell<Mirror>>,
    pub known: Known,
    /// Every text there has been, hashed, so that an undo can be held to
    /// texts that existed.
    seen: HashSet<String>,
    /// The cursor may be off the sheet: the wheel moved the paper, or
    /// everything was selected.
    away: bool,
    /// Draw and read the screen back after every event.
    pub draw: bool,
    events: usize,
}

impl Run {
    pub fn start(setup: &Setup, known: Known) -> Run {
        let mirror = Rc::new(RefCell::new(Mirror::default()));
        let first = Opened { text: setup.text.clone(), cursor: setup.cursor.map(|(line, col)| Pos::new(line, col)), ..Opened::default() };
        let session = Journal { first, mirror: mirror.clone() };
        let mut rig = Rig::with_seams(Options::default(), Box::new(session), Box::new(Closed), Box::new(Rest));
        rig.app.prefs_mut().smooth = setup.smooth;
        rig.app.prefs_mut().platen = setup.platen;
        rig.size(setup.size.0, setup.size.1);
        mirror.borrow_mut().lines = rig.app.page().doc().lines().to_vec();
        let mut seen = HashSet::new();
        seen.insert(rig.text());
        Run { rig, mirror, known, seen, away: false, draw: true, events: 0 }
    }

    /// Sends one event the way the shell would.
    pub fn send(&mut self, ev: &Ev) {
        let rig = &mut self.rig;
        match ev {
            Ev::Text(ch) => rig.send(Event::Text(*ch)),
            Ev::Key(key, mods) => rig.key(*key, *mods),
            Ev::Paste(text) => {
                rig.host.clipboard = Some(text.clone());
                rig.key(Key::Char('v'), CTRL);
            }
            Ev::Click { col, row, shift } => {
                let (x, y) = rig.cell_centre(*col, *row);
                let mods = if *shift { SHIFT } else { NONE };
                rig.send(Event::Mouse { kind: MouseKind::Down, x, y, mods });
                rig.send(Event::Mouse { kind: MouseKind::Up, x, y, mods });
            }
            Ev::Drag { from, to } => {
                let (x, y) = rig.cell_centre(from.0, from.1);
                rig.send(Event::Mouse { kind: MouseKind::Down, x, y, mods: NONE });
                let (x, y) = rig.cell_centre(to.0, to.1);
                rig.send(Event::Mouse { kind: MouseKind::Move, x, y, mods: NONE });
                rig.send(Event::Mouse { kind: MouseKind::Up, x, y, mods: NONE });
            }
            Ev::Wheel(rows) => rig.wheel(*rows),
            Ev::Size(width, height) => rig.size(*width, *height),
            Ev::Scale(scale) => rig.app.set_scale(*scale),
            Ev::Wait(ms) => rig.wait(*ms),
            Ev::Focus(focused) => rig.send(Event::Focus(*focused)),
            Ev::Tick => rig.send(Event::Tick),
        }
    }

    /// Sends one event and checks everything that must be true after it.
    pub fn step(&mut self, ev: &Ev) -> Result<(), Fail> {
        let before = Snap::of(&self.rig);
        let clip_before = self.rig.host.clipboard.clone();
        let (row_before, platen_before) = (self.cursor_row(), self.rig.app.grid().platen as i64);
        let blank_before = self.rig.app.grid().is_blank();
        self.send(ev);
        self.events += 1;
        let after = Snap::of(&self.rig);
        let told = |what: Fail| Fail { kind: what.kind, msg: format!("{}\n  event:  {}\n  before: {}\n  after:  {}", what.msg, describe(ev), before.show(), after.show()) };
        check_step(&before, ev, &after, &clip_before, &self.rig.host.clipboard, &self.seen, self.known).map_err(told)?;
        self.seen.insert(after.text.clone());
        self.check_state(&after).map_err(told)?;
        // A movement never leaves the cursor between a character and a mark
        // on it. (An edit can: joining a line that opens with a mark onto
        // the line above puts the mark on the last letter there, and the
        // cursor is where the model says in any case.)
        // Nor is a movement to blame for a place an edit had already left
        // one end of the selection in: Left and Right without Shift fold a
        // selection up to one of its ends, wherever those are.
        if !self.known.inside_cluster && before.text == after.text {
            for (name, was, at) in [("cursor", before.head, after.head), ("start of the selection", before.anchor, after.anchor)] {
                let new = at != before.head && at != before.anchor;
                if was != at && new && !on_cluster_boundary(&after.text, at) {
                    return Err(told(Fail { kind: String::from("inside cluster"), msg: format!("the {name} was moved to stand between a character and a mark on it") }));
                }
            }
        }

        // Typing, and Enter, at the platen: the typing row stays where it was.
        let typing = match ev {
            Ev::Text(ch) => !ch.is_control(),
            Ev::Key(Key::Enter, mods) => !mods.ctrl && !mods.alt,
            _ => false,
        };
        let grid = self.rig.app.grid();
        if typing && self.rig.app.prefs().platen && !blank_before && !grid.is_blank() && row_before == platen_before && !self.known.platen_moves {
            let row = self.cursor_row();
            let lowest = if grid.text_rows > 2 { -1 } else { 0 };
            let nowhere_to_go = self.rig.app.view().top() == lowest && row < platen_before;
            if row != platen_before && !nowhere_to_go {
                return Err(told(Fail { kind: String::from("platen"), msg: format!("the typing row was the platen, row {platen_before}, and is now row {row} (top {})", self.rig.app.view().top()) }));
            }
        }

        // The paper rolls for four frames and then rests.
        if matches!(ev, Ev::Wait(ms) if *ms >= 100) && (self.rig.app.view().rolling() || self.rig.app.view().shift() != 0 || self.rig.wants().animating) {
            return Err(told(Fail { kind: String::from("roll"), msg: String::from("the paper is still rolling a tenth of a second after the last key") }));
        }

        // Where the cursor is allowed to be off the sheet.
        match ev {
            Ev::Wheel(_) => self.away = true,
            // A click on the lower half of a title whose upper half is above
            // the sheet puts the cursor on a row that is partly off it.
            Ev::Click { .. } | Ev::Drag { .. } => self.away = true,
            Ev::Key(Key::Char('a'), mods) if mods.ctrl && !mods.alt && !mods.shift => self.away = true,
            Ev::Size(..) | Ev::Scale(_) => self.away = false,
            Ev::Key(Key::Char('='), mods) if mods.ctrl && !mods.alt => self.away = false,
            Ev::Key(Key::Char('-'), mods) if mods.ctrl && !mods.alt && !mods.shift => self.away = false,
            _ if before != after => self.away = false,
            _ => {}
        }
        self.check_cursor_on_sheet().map_err(told)?;
        if self.draw {
            self.check_screen().map_err(told)?;
        }
        Ok(())
    }

    /// The row of the sheet the cursor is on.
    pub fn cursor_row(&self) -> i64 {
        self.rig.app.page().caret().row as i64 - self.rig.app.view().top()
    }

    /// What must be true of the page whatever was just done.
    pub fn check_state(&self, now: &Snap) -> Result<(), Fail> {
        let page = self.rig.app.page();
        let doc = page.doc();
        let cursor = page.cursor;
        if (doc.clamp(cursor.head), doc.clamp(cursor.anchor)) != (cursor.head, cursor.anchor) {
            return fail("cursor outside", format!("the cursor {cursor:?} is not a place the text has"));
        }
        // The selection is in order and reads what lies between its ends.
        let span = cursor.span();
        let (from, to) = now.span();
        if span.start > span.end {
            return fail("selection order", format!("the selection {span:?} ends before it starts"));
        }
        let selected = page.selection().unwrap_or_default();
        if selected != now.text[from..to] {
            return fail("selection text", format!("the selection reads {selected:?} and the text between its ends {:?}", &now.text[from..to]));
        }
        // What the session was told adds up to the text on the page.
        let mirror = self.mirror.borrow();
        if let Some(wrong) = &mirror.wrong {
            return fail("journal", format!("a change handed to the session does not fit the text it was made to: {wrong}"));
        }
        if mirror.lines.join("\n") != now.text {
            return fail("journal", format!("the changes handed to the session add up to {:?}", mirror.lines.join("\n")));
        }
        // The rows of every line, read without trusting how they were made:
        // they follow one another with nothing left out, none is cut inside
        // a character and its marks, none is wider than the measure unless
        // it is a single character that could not be cut, and each line
        // starts on the row after the last row of the line above.
        if now.text.len() < 20_000 {
            let layout = page.layout();
            let measure = page.measure();
            let mut next_row = 0;
            for (line, text) in doc.lines().iter().enumerate() {
                let (rows, shape) = (layout.rows(line), layout.shape(line));
                let wrong = |why: String| fail("rows", format!("line {line} {:?} at measure {measure}, shape {shape:?}, rows {rows:?}: {why}", text.chars().take(60).collect::<String>()));
                if layout.first_row(line) != next_row {
                    return wrong(format!("starts on row {} and the line above ends on row {next_row}", layout.first_row(line)));
                }
                if rows.is_empty() || rows[0].start != shape.hung || rows[rows.len() - 1].end != text.len() {
                    return wrong(String::from("its rows do not run from the end of its hung mark to its end"));
                }
                let room = (measure / shape.scale.max(1)).max(1);
                for (at, row) in rows.iter().enumerate() {
                    if at > 0 && rows[at - 1].end != row.start {
                        return wrong(format!("row {at} does not start where the row before it ends"));
                    }
                    let Some(shown) = text.get(row.start..row.end) else {
                        return wrong(format!("row {at} is cut inside a character"));
                    };
                    if row.start < text.len() && is_combining(text[row.start..].chars().next().unwrap_or(' ')) && row.start > shape.hung {
                        return wrong(format!("row {at} starts with a mark that belongs to the character before it"));
                    }
                    if shown.is_empty() && text.len() > shape.hung {
                        return wrong(format!("row {at} is empty"));
                    }
                    // Spaces and tabs at the end of a row hang past the
                    // measure, with whatever marks sit on them.
                    let (mut wide, mut reach, mut letters) = (row.indent, row.indent, 0);
                    for ch in shown.chars().filter(|ch| !is_combining(*ch)) {
                        reach += cell_width(ch);
                        if ch != ' ' && ch != '\t' {
                            wide = reach;
                            letters += 1;
                        }
                    }
                    let one = letters <= 1;
                    if wide > room && !one {
                        return wrong(format!("row {at} is {wide} cells wide in a measure of {room}"));
                    }
                }
                next_row += rows.len() * shape.scale;
            }
            if layout.total_rows() != next_row.max(1) {
                return fail("rows", format!("the text has {next_row} rows and the layout counts {}", layout.total_rows()));
            }
        }
        // The page is wrapped at the measure the screen is laid out for.
        let grid = self.rig.app.grid();
        if !grid.is_blank() && page.measure() as i32 != grid.measure {
            return fail("measure", format!("the page is wrapped at {} and the grid's measure is {}", page.measure(), grid.measure));
        }
        // Every table kept beside the text is the one a fresh page would have.
        if now.text.len() < 4000 || self.events % 16 == 0 {
            let fresh = Page::new(&format!("\u{FEFF}{}", now.text), page.measure());
            for line in 0..doc.line_count() {
                let (kept, made) = ((page.kind(line), page.layout().shape(line), page.layout().rows(line)), (fresh.kind(line), fresh.layout().shape(line), fresh.layout().rows(line)));
                if kept != made {
                    return fail("tables", format!("line {line} {:?} is kept as {kept:?} and a fresh page makes it {made:?}", doc.line(line)));
                }
            }
            if fresh.layout() != page.layout() {
                return fail("tables", String::from("the rows have drifted from the text"));
            }
            if fresh.words() != page.words() {
                return fail("tables", format!("the page counts {} words and a fresh count is {}", page.words(), fresh.words()));
            }
        }
        Ok(())
    }

    /// The cursor is on the sheet unless the wheel took the paper away.
    pub fn check_cursor_on_sheet(&self) -> Result<(), Fail> {
        let grid = self.rig.app.grid();
        if grid.is_blank() || self.away || self.known.cursor_off_sheet {
            return Ok(());
        }
        let caret = self.rig.app.page().caret();
        let row = self.cursor_row();
        if caret.size == 2 && grid.text_rows < 4 {
            // On a sheet of three rows or fewer the platen is the last row,
            // and a double-size line typed there has no room below it.
            return Ok(());
        }
        if row < 0 || row + caret.size as i64 > grid.text_rows as i64 {
            return fail("cursor off sheet", format!("the cursor is on row {row} of a sheet of {} rows (top {}, caret {caret:?})", grid.text_rows, self.rig.app.view().top()));
        }
        let col = grid.text.first + caret.cell;
        if col < grid.sheet.first || col + caret.size as i32 - 1 > grid.sheet.last {
            return fail("cursor off sheet", format!("the cursor is in column {col} and the sheet is columns {} to {}", grid.sheet.first, grid.sheet.last));
        }
        Ok(())
    }

    /// Draws the screen and reads it back against the text: every visible
    /// row holds the characters of its row of the text, each in its cell,
    /// and the cursor is drawn where the page says it is.
    pub fn check_screen(&mut self) -> Result<(), Fail> {
        self.rig.draw();
        let app = &self.rig.app;
        let (grid, screen, page, view) = (app.grid(), app.screen(), app.page(), app.view());
        if (screen.cols(), screen.rows()) != (grid.cols, grid.rows) {
            return fail("screen size", format!("the screen is {} by {} and the grid {} by {}", screen.cols(), screen.rows(), grid.cols, grid.rows));
        }
        if grid.is_blank() || view.rolling() {
            return Ok(());
        }
        let (doc, layout) = (page.doc(), page.layout());
        let top = view.top();
        let total = layout.total_rows() as i64;
        let rows = grid.text_rows as i64;
        let wide = (grid.sheet.last - grid.sheet.first + 1).max(0) as usize;
        let mut shown_in_order = String::new();
        let caret = page.caret();
        let (caret_row, caret_col) = (caret.row as i64 - top, grid.text.first + caret.cell);
        // The two things known to be wrong in the cursor's own cells: the
        // cursor stands on a mark, which is drawn alone in its block in
        // place of the letter it is in front of; or it stands on a title
        // that is half off the sheet and so not drawn, and shows the one
        // character it is on with no title round it.
        let head = doc.clamp(page.cursor.head);
        let on_a_mark = doc.line(head.line).get(head.col..).and_then(|rest| rest.chars().next()).is_some_and(is_combining);
        let title_top = if caret.cell < 0 { layout.first_row(head.line) as i64 } else { caret.row as i64 };
        let title_cut = title_top < top || title_top + layout.shape(head.line).scale as i64 > top + rows;
        let cursor_cell_known = self.known.cursor_cell && (on_a_mark || title_cut);
        for r in 0..rows {
            let at = top + r;
            let mut want = vec![' '; wide];
            let mut put = |col: i32, ch: char| {
                if col >= grid.sheet.first && col <= grid.sheet.last {
                    want[(col - grid.sheet.first) as usize] = if ch == '\t' { ' ' } else { ch };
                }
            };
            if at >= 0 && at < total {
                let (line, within) = layout.line_of_row(at as usize);
                let shape = layout.shape(line);
                let unit = shape.scale as i64;
                let row_top = layout.first_row(line) as i64 + within as i64 * unit;
                // Half a double-size row at the edge of the sheet is left off.
                let whole = row_top >= top && row_top + unit <= top + rows;
                let text = doc.line(line);
                if whole && at == row_top {
                    if let Some(row) = layout.rows(line).get(within) {
                        let mut x = row.indent as i64;
                        for ch in text.get(row.start..row.end).unwrap_or("").chars() {
                            if is_combining(ch) {
                                continue;
                            }
                            put(grid.text.first + (x * unit) as i32, ch);
                            x += cell_width(ch) as i64;
                        }
                    }
                }
                if whole && within == 0 && shape.hung > 0 && at == row_top + unit - 1 {
                    let mark = text.get(..shape.hung).unwrap_or("").trim_end();
                    let mut col = grid.gutter.last + 1 - str_width(mark) as i32;
                    // A mark riding on the hung mark hangs with it and, as
                    // in the rows, takes no cell.
                    for ch in mark.chars().filter(|ch| !is_combining(*ch)) {
                        put(col, ch);
                        col += cell_width(ch) as i32;
                    }
                }
            }
            // A cursor resting on a mark that has no letter under it shows
            // the mark in its block; in the text the mark takes no cell.
            let got: Vec<char> = (grid.sheet.first..=grid.sheet.last).map(|col| screen.at(col, r as i32).map_or(' ', |cell| cell.ch)).map(|ch| if is_combining(ch) { ' ' } else { ch }).collect();
            // Columns the window cuts off are not on the screen at all.
            let seen = (grid.cols - grid.sheet.first).clamp(0, wide as i32) as usize;
            let mut got = got;
            if cursor_cell_known && r >= caret_row && r < caret_row + caret.size as i64 {
                for col in caret_col..caret_col + 2 * caret.size as i32 {
                    if col >= grid.sheet.first && col <= grid.sheet.last {
                        got[(col - grid.sheet.first) as usize] = want[(col - grid.sheet.first) as usize];
                    }
                }
            }
            if !self.known.screen_rows && got[..seen] != want[..seen] {
                let (got, want): (String, String) = (got[..seen].iter().collect(), want[..seen].iter().collect());
                return fail("screen row", format!("row {r} of the sheet (row {at} of the text, top {top}) reads\n    {:?}\n  and the text has\n    {:?}", got.trim_end(), want.trim_end()));
            }
            let text_from = (grid.text.first - grid.sheet.first).clamp(0, seen as i32) as usize;
            shown_in_order.extend(got[text_from..seen].iter().filter(|ch| !ch.is_whitespace()));
        }
        // Read without the layout: what is on the sheet, row after row, is
        // in the text in that order.
        let mut rest = doc.lines().iter().flat_map(|line| line.chars()).filter(|ch| !ch.is_whitespace() && !is_combining(*ch));
        for ch in shown_in_order.chars() {
            if !rest.any(|there| there == ch) {
                return fail("screen order", format!("the sheet shows {ch:?} where the text does not have it in that order"));
            }
        }

        // The cursor: a block when nothing is selected and the window has
        // the keyboard, drawn where the page says.
        let row = caret_row;
        let on_sheet = row >= 0 && row + caret.size as i64 <= rows;
        let partly = !on_sheet && row + caret.size as i64 > 0 && row < rows;
        let wanted = !page.cursor.has_selection() && on_sheet;
        if caret.size == 2 && grid.text_rows < 4 {
            return Ok(());
        }
        let drawn = screen.cursor.or(screen.kept);
        if self.known.cursor_off_sheet {
            return Ok(());
        }
        match (wanted, drawn) {
            (true, None) => fail("cursor not drawn", format!("no cursor was drawn; the page has it on row {row} (caret {caret:?})")),
            (false, Some(_)) if partly && !page.cursor.has_selection() => Ok(()),
            (false, Some(found)) => fail("cursor drawn", format!("a cursor was drawn at {found:?} with a selection or off the sheet")),
            (true, Some(found)) => {
                let col = grid.text.first + caret.cell;
                if (found.col, found.row as i64) != (col, row) {
                    return fail("cursor misplaced", format!("the cursor was drawn at column {} row {} and the page has it at column {col} row {row}", found.col, found.row));
                }
                let inside = found.col >= grid.sheet.first && found.col + found.wide - 1 <= grid.sheet.last && found.row >= 0 && found.row + found.tall <= grid.text_rows;
                if !inside && !self.away {
                    return fail("cursor off sheet", format!("the cursor was drawn at {found:?}, outside the sheet's columns {} to {} or its {} rows", grid.sheet.first, grid.sheet.last, grid.text_rows));
                }
                Ok(())
            }
            (false, None) => Ok(()),
        }
    }

    /// Undo all the way is the text the run began with, and redo all the
    /// way is the text it had reached. `bound` is at least the number of
    /// steps there can be to undo.
    pub fn check_undo_all(&mut self, original: &str, bound: usize) -> Result<(), Fail> {
        // Whatever had been undone when the run ended is redone first, so
        // that the text is the last one the history holds.
        for _ in 0..bound {
            self.rig.key(Key::Char('y'), CTRL);
        }
        let reached = self.rig.text();
        for _ in 0..bound {
            self.rig.key(Key::Char('z'), CTRL);
        }
        let back = self.rig.text();
        if back != original {
            return fail("undo all", format!("undo all the way gives {back:?}, and the text began as {original:?}"));
        }
        self.check_state(&Snap::of(&self.rig)).map_err(|what| Fail { kind: what.kind, msg: format!("after undoing everything: {}", what.msg) })?;
        for _ in 0..bound {
            self.rig.key(Key::Char('y'), CTRL);
        }
        let again = self.rig.text();
        if again != reached {
            return fail("redo all", format!("redo all the way gives {again:?}, and the text had reached {reached:?}"));
        }
        self.check_state(&Snap::of(&self.rig)).map_err(|what| Fail { kind: what.kind, msg: format!("after redoing everything: {}", what.msg) })?;
        self.away = true;
        Ok(())
    }
}

// ───────────────────────── what one event may do ─────────────────────────

fn splice(text: &str, from: usize, to: usize, with: &str) -> String {
    format!("{}{}{}", &text[..from], with, &text[to..])
}

fn exact(what: &str, after: &Snap, text: String, head: usize, anchor: usize) -> Result<(), Fail> {
    let want = Snap { text, head, anchor };
    if *after != want {
        return fail(what, format!("{what}: the model has {}", want.show()));
    }
    Ok(())
}

fn unchanged(what: &str, before: &Snap, after: &Snap) -> Result<(), Fail> {
    if before != after {
        return fail(what, format!("{what}: neither the text nor the cursor should have changed"));
    }
    Ok(())
}

/// The text is as it was; with Shift the selection still begins where it
/// did, and without it nothing is selected.
fn moved(what: &str, before: &Snap, after: &Snap, shift: bool) -> Result<(), Fail> {
    if before.text != after.text {
        return fail(what, format!("{what}: a movement changed the text"));
    }
    if shift && after.anchor != before.anchor {
        return fail(what, format!("{what}: Shift moved the start of the selection"));
    }
    if !shift && after.anchor != after.head {
        return fail(what, format!("{what}: a movement without Shift left a selection"));
    }
    Ok(())
}

/// Checks one event against the model: `before` and `after` are the text
/// and the cursor on either side of it.
pub fn check_step(before: &Snap, ev: &Ev, after: &Snap, clip_before: &Option<String>, clip_after: &Option<String>, seen: &HashSet<String>, known: Known) -> Result<(), Fail> {
    let b = before;
    let (s, e) = b.span();
    let selected = s != e;
    let text = b.text.as_str();
    let starts = line_starts(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let line_no = |at: usize| line_of(&starts, at);
    let line_start = |at: usize| starts[line_of(&starts, at)];
    let line_end = |at: usize| starts[line_of(&starts, at)] + lines[line_of(&starts, at)].len();
    match ev {
        Ev::Text(ch) if ch.is_control() => unchanged("a control character", b, after),
        Ev::Text(ch) => {
            let typed = ch.to_string();
            exact("typing", after, splice(text, s, e, &typed), s + typed.len(), s + typed.len())
        }
        Ev::Paste(clip) => {
            let put = pasted(clip);
            if clip_after.as_deref() != Some(clip.as_str()) {
                return fail("paste", String::from("pasting changed the clipboard"));
            }
            if put.is_empty() {
                return unchanged("pasting nothing", b, after);
            }
            exact("paste", after, splice(text, s, e, &put), s + put.len(), s + put.len())
        }
        // A click outside the rows of text lands nowhere and changes nothing.
        Ev::Click { .. } if b == after => Ok(()),
        // A click on the map goes there, with or without Shift.
        Ev::Click { .. } if b.text == after.text && after.head == after.anchor => Ok(()),
        Ev::Click { shift, .. } => moved("a click", b, after, *shift),
        Ev::Drag { .. } => {
            if b.text != after.text {
                return fail("a drag", String::from("a drag changed the text"));
            }
            Ok(())
        }
        Ev::Wheel(_) | Ev::Size(..) | Ev::Scale(_) | Ev::Wait(_) | Ev::Focus(_) | Ev::Tick => unchanged("no key was pressed", b, after),
        Ev::Key(_, mods) if mods.ctrl && mods.alt => unchanged("Ctrl with Alt is never bound", b, after),
        Ev::Key(key, mods) => {
            let (ctrl, alt, shift) = (mods.ctrl, mods.alt, mods.shift);
            let plain = !ctrl && !alt;
            match key {
                Key::Left | Key::Right if plain => {
                    let on = *key == Key::Right;
                    let to = match (selected && !shift, on) {
                        (true, false) => s,
                        (true, true) => e,
                        (false, false) => left(text, b.head),
                        (false, true) => right(text, b.head),
                    };
                    exact("a letter left or right", after, b.text.clone(), to, if shift { b.anchor } else { to })
                }
                Key::Left | Key::Right => {
                    // By word or by sentence.
                    let what = if ctrl { "a word left or right" } else { "a sentence left or right" };
                    moved(what, b, after, shift)?;
                    let on = *key == Key::Right;
                    let (h, to) = (b.head, after.head);
                    let stuck = if on { h == text.len() } else { h == 0 };
                    let went = if on { to > h } else { to < h };
                    if stuck != (to == h) || (!stuck && !went) {
                        return fail(what, format!("{what}: went from {h} to {to}"));
                    }
                    if ctrl && !stuck {
                        // A word key never leaves the line except from its edge.
                        let edge = if on { h == line_end(h) } else { h == line_start(h) };
                        let want = if on { h + 1 } else { h - 1 };
                        if edge && to != want {
                            return fail(what, format!("{what}: from the edge of a line it went to {to}, not across one line break"));
                        }
                        if !edge && line_no(to) != line_no(h) {
                            return fail(what, format!("{what}: left the line from the middle of it"));
                        }
                    }
                    Ok(())
                }
                Key::Up | Key::Down if alt => check_carry(b, after, *key == Key::Down),
                Key::Up | Key::Down => {
                    let what = if ctrl { "a paragraph up or down" } else { "a row up or down" };
                    moved(what, b, after, shift)?;
                    let on = *key == Key::Down;
                    let (h, to) = (b.head, after.head);
                    let stuck = if on { h == text.len() } else { h == 0 };
                    let went = if on { to > h } else { to < h };
                    if stuck != (to == h) || (!stuck && !went) {
                        return fail(what, format!("{what}: went from {h} to {to}"));
                    }
                    if ctrl && to != 0 && to != text.len() {
                        // Lands on column 0 of the first line of a paragraph.
                        let line = line_no(to);
                        let first_of_paragraph = to == starts[line] && !is_blank(lines[line]) && (line == 0 || is_blank(lines[line - 1]));
                        if !first_of_paragraph {
                            return fail(what, format!("{what}: landed at {to}, which is not the start of a paragraph"));
                        }
                    }
                    Ok(())
                }
                Key::Home | Key::End if ctrl => {
                    let to = if *key == Key::Home { 0 } else { text.len() };
                    exact("the ends of the text", after, b.text.clone(), to, if shift { b.anchor } else { to })
                }
                Key::Home | Key::End if alt => unchanged("Alt Home and Alt End are not bound", b, after),
                Key::Home | Key::End => {
                    moved("the ends of the row", b, after, shift)?;
                    if after.head < line_start(b.head) || after.head > line_end(b.head) {
                        return fail("the ends of the row", String::from("Home or End left the line"));
                    }
                    Ok(())
                }
                Key::PageUp | Key::PageDown if !plain => unchanged("the page keys take no Ctrl or Alt", b, after),
                Key::PageUp | Key::PageDown => {
                    moved("a screen up or down", b, after, shift)?;
                    let on = *key == Key::PageDown;
                    if (on && after.head < b.head) || (!on && after.head > b.head) {
                        return fail("a screen up or down", String::from("a page key went the wrong way"));
                    }
                    Ok(())
                }
                Key::Backspace | Key::Delete => check_erase(b, after, *key == Key::Delete, ctrl, alt, known),
                Key::Enter if plain => check_enter(b, after, shift, known),
                Key::Enter => unchanged("Enter with Ctrl or Alt changes nothing here", b, after),
                Key::Tab if plain => check_nest(b, after, shift),
                Key::Tab => unchanged("Tab with Ctrl or Alt is not bound", b, after),
                Key::Escape | Key::F(_) => unchanged("a key of the line or the window", b, after),
                Key::Char(ch) if ctrl => match (ch.to_ascii_lowercase(), shift) {
                    ('z', _) | ('y', false) => {
                        if !seen.contains(&after.text) {
                            return fail("undo invents", format!("undo or redo gave a text that never was: {:?}", after.text));
                        }
                        Ok(())
                    }
                    ('x', false) | ('c', false) => {
                        let cut = ch.eq_ignore_ascii_case(&'x');
                        if !selected {
                            if clip_after != clip_before {
                                return fail("clipboard", String::from("cut or copy with nothing selected changed the clipboard"));
                            }
                            return unchanged("cut or copy with nothing selected", b, after);
                        }
                        let want = text[s..e].replace('\n', "\r\n");
                        if clip_after.as_deref() != Some(want.as_str()) {
                            return fail("clipboard", format!("the clipboard holds {clip_after:?} and the selection was {want:?}"));
                        }
                        if cut {
                            exact("cut", after, splice(text, s, e, ""), s, s)
                        } else {
                            unchanged("copy", b, after)
                        }
                    }
                    // Ev::Paste is how a paste is sent; a bare Ctrl V pastes
                    // whatever the clipboard holds.
                    ('v', false) => {
                        let put = pasted(clip_before.as_deref().unwrap_or(""));
                        if put.is_empty() {
                            return unchanged("pasting nothing", b, after);
                        }
                        exact("paste", after, splice(text, s, e, &put), s + put.len(), s + put.len())
                    }
                    ('a', false) => exact("select all", after, b.text.clone(), text.len(), 0),
                    ('t', false) => {
                        if b.text != after.text {
                            return fail("Ctrl T", String::from("Ctrl T changed the text"));
                        }
                        let (from, to) = after.span();
                        if after != b && (&after.text[from..to] != "TK" || after.head != to) {
                            return fail("Ctrl T", String::from("Ctrl T selected something that is not a TK"));
                        }
                        Ok(())
                    }
                    _ => unchanged("a chord that does not touch the text", b, after),
                },
                Key::Char(_) => unchanged("a chord that is not bound", b, after),
            }
        }
    }
}

/// Backspace and Delete at every size.
fn check_erase(b: &Snap, after: &Snap, forward: bool, ctrl: bool, alt: bool, known: Known) -> Result<(), Fail> {
    let (s, e) = b.span();
    let text = b.text.as_str();
    if s != e {
        return exact("erasing a selection", after, splice(text, s, e, ""), s, s);
    }
    let starts = line_starts(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let kinds = kinds(&lines);
    let h = b.head;
    let line = line_of(&starts, h);
    let col = h - starts[line];
    let blank = |line: usize| kinds[line] == Kind::Blank;
    if !forward && col == 0 {
        // The start of a line: the break before it, and with it the blank
        // line Enter wrote when that is what lies above. Enter writes two
        // breaks wherever it is pressed, at the very start of a paragraph
        // too, where both go in above it; DESIGN.md has Backspace at the
        // start of a paragraph undo what Enter did. So the breaks between
        // this line and the last line above with anything on it (or the top
        // of the text) are counted: whole pairs give the last pair back
        // together, and one left over goes first by itself. (The model used
        // to ask only for one blank line under a paragraph, which is the
        // failure `attack_found.rs` names: Enter at the start of a
        // paragraph could not be taken back with one Backspace.)
        let blanks = (0..line).rev().take_while(|&above| blank(above)).count();
        let breaks = blanks + usize::from(blanks < line);
        let from = if breaks >= 2 && breaks % 2 == 0 { starts[line - 1] - 1 } else { h.saturating_sub(1) };
        return exact("Backspace at the start of a line", after, splice(text, from, h, ""), from, from);
    }
    if forward && col == lines[line].len() {
        let count = lines.len();
        let to = if line + 2 < count && !blank(line) && blank(line + 1) && !blank(line + 2) {
            starts[line + 2]
        } else if line + 1 < count && kinds[line] == Kind::Item && kinds[line + 1] == Kind::Item {
            starts[line + 1] + marker(lines[line + 1]).map_or(0, |m| m.text)
        } else {
            (h + 1).min(text.len())
        };
        return exact("Delete at the end of a line", after, splice(text, h, to, ""), h, h);
    }
    if !ctrl && !alt {
        if !forward {
            // Just after an item's marker, under another item: the break
            // and the marker go together.
            let joins = line > 0 && kinds[line] == Kind::Item && kinds[line - 1] == Kind::Item && marker(lines[line]).is_some_and(|m| m.text == col);
            let from = if joins { starts[line] - 1 } else { left(text, h) };
            return exact("Backspace", after, splice(text, from, h, ""), from, from);
        }
        let to = right(text, h);
        return exact("Delete", after, splice(text, h, to, ""), h, h);
    }
    // A word or a sentence: something next to the cursor on its own line,
    // and nothing else.
    let what = if ctrl { "erasing a word" } else { "erasing a sentence" };
    let gone = text.len().checked_sub(after.text.len()).filter(|&gone| gone > 0);
    let Some(gone) = gone else {
        return fail(what, format!("{what}: nothing was taken from the middle of a line"));
    };
    let (from, to) = if forward { (h, h + gone) } else { (h.saturating_sub(gone), h) };
    if to > text.len() || to - from != gone || !text.is_char_boundary(from) || !text.is_char_boundary(to) || splice(text, from, to, "") != after.text {
        return fail(what, format!("{what}: what went was not one stretch beside the cursor"));
    }
    if text[from..to].contains('\n') {
        return fail(what, format!("{what}: it reached across a line break from the middle of a line"));
    }
    if !known.inside_cluster && on_cluster_boundary(text, h) && !(on_cluster_boundary(text, from) && on_cluster_boundary(text, to)) {
        return fail("erase cuts cluster", format!("{what}: it took part of a character and its marks and left the rest"));
    }
    if (after.head, after.anchor) != (from, from) {
        return fail(what, format!("{what}: the cursor is not where the stretch was"));
    }
    Ok(())
}

/// Enter and Shift Enter.
fn check_enter(b: &Snap, after: &Snap, soft: bool, known: Known) -> Result<(), Fail> {
    let (s, e) = b.span();
    let text = b.text.as_str();
    let starts = line_starts(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let kinds = kinds(&lines);
    let line = line_of(&starts, s);
    let col = s - starts[line];
    if soft || matches!(kinds[line], Kind::Blank | Kind::Code | Kind::Fence) {
        return exact("a line break", after, splice(text, s, e, "\n"), s + 1, s + 1);
    }
    if kinds[line] == Kind::Item {
        let Some(m) = marker(lines[line]) else {
            return fail("model", String::from("the model calls a line an item and finds no marker on it"));
        };
        if col >= m.text && lines[line][m.text..].trim().is_empty() {
            // An empty item ends the list: its marker goes, and a blank
            // line stands where it was. Whatever is selected goes too, and
            // nothing else does.
            let end = e.max(starts[line] + lines[line].len());
            if known.enter_empty_item_selection && line_of(&starts, e) != line {
                return Ok(());
            }
            return exact("Enter on an empty item", after, splice(text, starts[line], end, "\n"), starts[line] + 1, starts[line] + 1);
        }
        if col >= m.text {
            let next = format!("\n{}", next_marker(lines[line], m));
            return exact("Enter in a list", after, splice(text, s, e, &next), s + next.len(), s + next.len());
        }
        return exact("Enter inside a marker", after, splice(text, s, e, "\n"), s + 1, s + 1);
    }
    exact("Enter", after, splice(text, s, e, "\n\n"), s + 2, s + 2)
}

/// Tab and Shift Tab: every item the selection touches moves in by two
/// spaces or back by up to two, and nothing else changes at all.
fn check_nest(b: &Snap, after: &Snap, back: bool) -> Result<(), Fail> {
    let (s, e) = b.span();
    let text = b.text.as_str();
    let starts = line_starts(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let kinds = kinds(&lines);
    let (first, last) = (line_of(&starts, s), line_of(&starts, e));
    let mut fresh: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    let mut shift = vec![0isize; lines.len()];
    for line in first..=last {
        if kinds[line] != Kind::Item {
            continue;
        }
        if back {
            let spaces = lines[line].bytes().take(2).take_while(|&b| b == b' ').count();
            fresh[line].replace_range(..spaces, "");
            shift[line] = -(spaces as isize);
        } else {
            fresh[line].insert_str(0, "  ");
            shift[line] = 2;
        }
    }
    let want_text = fresh.join("\n");
    if want_text == b.text {
        return unchanged("Tab where there is nothing to nest", b, after);
    }
    let fresh_starts = line_starts(&want_text);
    let place = |at: usize| {
        let line = line_of(&starts, at);
        fresh_starts[line] + (at - starts[line]).saturating_add_signed(shift[line])
    };
    exact("Tab in a list", after, want_text.clone(), place(b.head), place(b.anchor))
}

/// Alt Up and Alt Down: the paragraph the cursor is in changes places with
/// the one above or below it, the gap between them staying where it is.
fn check_carry(b: &Snap, after: &Snap, down: bool) -> Result<(), Fail> {
    let text = b.text.as_str();
    let starts = line_starts(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let at = line_of(&starts, b.head);
    let col = b.head - starts[at];
    if is_blank(lines[at]) {
        return unchanged("carrying from a blank line", b, after);
    }
    let run = |line: usize| {
        let (mut first, mut last) = (line, line);
        while first > 0 && !is_blank(lines[first - 1]) {
            first -= 1;
        }
        while last + 1 < lines.len() && !is_blank(lines[last + 1]) {
            last += 1;
        }
        (first, last)
    };
    let (own_first, own_last) = run(at);
    let other = if down { (own_last + 1..lines.len()).find(|&line| !is_blank(lines[line])).map(run) } else { (0..own_first).rev().find(|&line| !is_blank(lines[line])).map(run) };
    let Some((other_first, other_last)) = other else {
        return unchanged("carrying with nothing to pass", b, after);
    };
    let (upper, lower) = if down { ((own_first, own_last), (other_first, other_last)) } else { ((other_first, other_last), (own_first, own_last)) };
    let mut fresh: Vec<&str> = Vec::with_capacity(lines.len());
    fresh.extend(&lines[..upper.0]);
    fresh.extend(&lines[lower.0..=lower.1]);
    fresh.extend(&lines[upper.1 + 1..lower.0]);
    fresh.extend(&lines[upper.0..=upper.1]);
    fresh.extend(&lines[lower.1 + 1..]);
    let want_text = fresh.join("\n");
    let landed = if down { upper.0 + (lower.1 - lower.0 + 1) + (lower.0 - upper.1 - 1) } else { upper.0 };
    let fresh_starts = line_starts(&want_text);
    let to = fresh_starts[landed + (at - own_first)] + col;
    exact("carrying the paragraph", after, want_text, to, to)
}

// ───────────────────────── running and shrinking ─────────────────────────

thread_local! {
    /// While set, a panic on this thread is remembered and not printed.
    static QUIET: Cell<bool> = const { Cell::new(false) };
    static PANICKED: RefCell<String> = const { RefCell::new(String::new()) };
}

fn quiet_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let usual = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if QUIET.with(Cell::get) {
                PANICKED.with(|slot| *slot.borrow_mut() = info.to_string());
            } else {
                usual(info);
            }
        }));
    });
}

/// How a run is checked.
#[derive(Clone, Copy, Debug)]
pub struct Checks {
    pub known: Known,
    /// Read the screen back after every event.
    pub draw: bool,
    /// Undo everything and redo everything when the events are done.
    pub undo_all: bool,
}

impl Default for Checks {
    fn default() -> Checks {
        Checks { known: Known::default(), draw: true, undo_all: true }
    }
}

/// Runs events from a start and says what went wrong first, and at which
/// event. A panic anywhere in the interface is a failure like any other.
pub fn run(setup: &Setup, events: &[Ev], checks: Checks) -> Result<(), (usize, Fail)> {
    quiet_panics();
    let at = Cell::new(0usize);
    QUIET.with(|quiet| quiet.set(true));
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), (usize, Fail)> {
        let mut run = Run::start(setup, checks.known);
        run.draw = checks.draw;
        let original = run.rig.text();
        run.check_state(&Snap::of(&run.rig)).map_err(|what| (0, what))?;
        if checks.draw {
            run.check_screen().map_err(|what| (0, what))?;
        }
        for (number, ev) in events.iter().enumerate() {
            at.set(number);
            run.step(ev).map_err(|what| (number, what))?;
        }
        if checks.undo_all {
            at.set(events.len());
            // No event makes more than one step, and a paste or a drag is
            // one event.
            run.check_undo_all(&original, events.len() + 2).map_err(|what| (events.len(), what))?;
        }
        Ok(())
    }));
    QUIET.with(|quiet| quiet.set(false));
    match outcome {
        Ok(result) => result,
        Err(_) => {
            let said = PANICKED.with(|slot| slot.borrow().clone());
            // The place a panic came from names it; the particulars vary.
            let place = said.lines().next().unwrap_or("").to_string();
            Err((at.get(), Fail { kind: format!("panic {place}"), msg: said }))
        }
    }
}

/// Cuts a failing run down: events are taken out, and lines of the starting
/// text, for as long as the same kind of failure is still there.
pub fn shrink(setup: &Setup, events: &[Ev], kind: &str, checks: Checks) -> (Setup, Vec<Ev>) {
    let still = |setup: &Setup, events: &[Ev]| matches!(run(setup, events, checks), Err((_, what)) if what.kind == kind);
    let mut setup = setup.clone();
    let mut events = events.to_vec();
    if let Err((at, _)) = run(&setup, &events, checks) {
        events.truncate((at + 1).min(events.len()));
    }
    loop {
        let size = (events.len(), setup.text.len());
        let mut chunk = events.len().div_ceil(2).max(1);
        loop {
            let mut from = 0;
            while from < events.len() {
                let to = (from + chunk).min(events.len());
                let mut fewer = events.clone();
                fewer.drain(from..to);
                if still(&setup, &fewer) {
                    events = fewer;
                } else {
                    from = to;
                }
            }
            if chunk == 1 {
                break;
            }
            chunk = chunk.div_ceil(2);
        }
        // The starting text: whole lines first, then characters.
        if setup.cursor.is_none() {
            let mut lines: Vec<String> = setup.text.split('\n').map(str::to_string).collect();
            let mut at = 0;
            while at < lines.len() && lines.len() > 1 {
                let mut fewer = lines.clone();
                fewer.remove(at);
                let tried = Setup { text: fewer.join("\n"), ..setup.clone() };
                if still(&tried, &events) {
                    lines = fewer;
                    setup = tried;
                } else {
                    at += 1;
                }
            }
            if setup.text.len() <= 400 {
                let mut chars: Vec<char> = setup.text.chars().collect();
                let mut at = 0;
                while at < chars.len() {
                    let mut fewer = chars.clone();
                    fewer.remove(at);
                    let tried = Setup { text: fewer.iter().collect(), ..setup.clone() };
                    if still(&tried, &events) {
                        chars = fewer;
                        setup = tried;
                    } else {
                        at += 1;
                    }
                }
            }
        }
        if (events.len(), setup.text.len()) == size {
            break;
        }
    }
    (setup, events)
}

/// A failing run cut down and written out so that it can be turned into a
/// named test by hand.
pub fn report(setup: &Setup, events: &[Ev], checks: Checks, seed: u64) -> Option<(String, String)> {
    let (at, what) = run(setup, events, checks).err()?;
    let (small_setup, small) = shrink(setup, events, &what.kind, checks);
    let again = run(&small_setup, &small, checks).err().map(|(_, what)| what.msg).unwrap_or_default();
    let text = format!(
        "[{}] seed {seed}, event {at} of {}\n  start: {:?} at {:?}, window {:?}, smooth {}, platen {}\n{}  {}\n",
        what.kind,
        events.len(),
        small_setup.text,
        small_setup.cursor,
        small_setup.size,
        small_setup.smooth,
        small_setup.platen,
        describe_all(&small),
        again.replace('\n', "\n  ")
    );
    Some((what.kind, text))
}

// ───────────────────────── random events ─────────────────────────

/// What gets typed: prose, every mark of the markup, and text that is hard
/// on a grid of cells.
pub const TYPED: [&str; 46] = [
    "a",
    "e",
    " ",
    " ",
    "the ",
    "orchard ",
    "word. ",
    "Yes? ",
    "No! ",
    "it's ",
    "# ",
    "## ",
    "### ",
    "> ",
    "- ",
    "* ",
    "1. ",
    "9) ",
    "- [ ] ",
    "- [x] ",
    "*",
    "**",
    "_",
    "`",
    "```",
    "~~~",
    "---",
    "[[",
    "]]",
    "TK",
    "e\u{0301}",
    "\u{0301}",
    "n\u{0303}\u{0308}\u{0323}",
    "\u{4E00}\u{4E8C}",
    "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{2764}\u{FE0F}",
    "\u{200D}",
    "\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{200F}",
    "http://a.b/c ",
    "\u{00A0}",
    "\u{2028}",
    "\u{1F1FA}\u{1F1F8}",
    "\u{AD}",
    "\u{10FFFF}",
    "\u{0E01}\u{0E33}\u{0E49}",
    "TK ",
];

/// What gets pasted.
pub const PASTED: [&str; 14] = [
    "\t",
    "a\tb",
    "one\r\ntwo\r\n",
    "\r\n\r\n",
    "\r",
    "x\ry\nz",
    "\u{0}\u{1}\u{7f}\u{1b}[31m",
    "# H\n\n> q\n- i\n- \n```\nTK\n```\n",
    "\n\n\n",
    "\u{FEFF}",
    "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} \u{0301}\u{0301}",
    "- one\n  - two\n    1. three",
    "word ",
    "\u{4E00}\t\u{0301}\r\n",
];

const MOVES: [Key; 8] = [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End, Key::PageUp, Key::PageDown];
const CHORDS: [char; 14] = ['z', 'y', 'x', 'c', 'v', 'a', 's', 't', 'f', '=', '-', 'q', 'w', 'b'];

fn any_mods(rng: &mut Rng) -> Mods {
    match rng.below(12) {
        0..=3 => NONE,
        4 | 5 => SHIFT,
        6 | 7 => CTRL,
        8 => ALT,
        9 => CTRL_SHIFT,
        10 => ALT_SHIFT,
        _ => CTRL_ALT,
    }
}

/// One random thing, as events: a snippet of typing is several.
pub fn random_events(rng: &mut Rng, out: &mut Vec<Ev>, sizes: &[(usize, usize)]) {
    match rng.below(100) {
        0..=37 => out.extend(rng.pick(&TYPED).chars().map(Ev::Text)),
        38..=55 => out.push(Ev::Key(rng.pick(&MOVES), any_mods(rng))),
        56..=63 => {
            let key = if rng.chance(2) { Key::Backspace } else { Key::Delete };
            let mods = match rng.below(8) {
                0..=3 => NONE,
                4 | 5 => CTRL,
                6 => ALT,
                _ => SHIFT,
            };
            out.push(Ev::Key(key, mods));
        }
        64..=70 => {
            let mods = match rng.below(10) {
                0..=5 => NONE,
                6 | 7 => SHIFT,
                8 => CTRL,
                _ => ALT,
            };
            out.push(Ev::Key(Key::Enter, mods));
        }
        71..=74 => out.push(Ev::Key(Key::Tab, if rng.chance(2) { NONE } else { SHIFT })),
        75..=78 => {
            let key = if rng.chance(2) { Key::Up } else { Key::Down };
            out.push(Ev::Key(key, if rng.chance(4) { ALT_SHIFT } else { ALT }));
        }
        79..=85 => {
            let ch = rng.pick(&CHORDS);
            let mods = match rng.below(8) {
                0..=5 => CTRL,
                6 => CTRL_SHIFT,
                _ => ALT,
            };
            out.push(Ev::Key(Key::Char(ch), mods));
        }
        86 | 87 => out.push(Ev::Key(Key::Char('z'), CTRL)),
        88 => out.push(Ev::Key(rng.pick(&[Key::Escape, Key::F(1), Key::F(3), Key::F(4), Key::F(11), Key::F(5)]), any_mods(rng))),
        89..=91 => out.push(Ev::Paste(rng.pick(&PASTED).to_string())),
        92 | 93 => out.push(Ev::Click { col: rng.below(170) as i32 - 5, row: rng.below(52) as i32 - 2, shift: rng.chance(4) }),
        94 => out.push(Ev::Drag { from: (rng.below(120) as i32, rng.below(40) as i32), to: (rng.below(120) as i32, rng.below(40) as i32) }),
        95 => out.push(Ev::Wheel(rng.pick(&[1, -1, 3, -3, 40, -40]))),
        96 => {
            let (width, height) = rng.pick(sizes);
            out.push(Ev::Size(width, height));
        }
        97 => out.push(Ev::Wait(rng.pick(&[1, 16, 300, 1200, 6000]))),
        98 => out.push(Ev::Focus(rng.chance(2))),
        _ => out.push(Ev::Tick),
    }
}
