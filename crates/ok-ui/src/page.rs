//! The page: one text with its cursor, its undo history and everything that
//! is kept in step with it line by line (what each line is, how it wraps,
//! how many words it holds, the day it was last changed).
//!
//! Every change to the text goes through [`Page::replace`], which makes the
//! change, brings the tables up to date, records it for undo and leaves it
//! in an outbox for whoever must be told. The keys of DESIGN.md are methods
//! here, written in the sizes the design gives them: letter, word, sentence,
//! row, paragraph.

use ok_text::count::{self, Tally};
use ok_text::history::{Cursor, EditKind, History, Replay};
use ok_text::markup::{self, LineKind};
use ok_text::wrap::{Layout, Shape};
use ok_text::{is_combining, motion, str_width, Change, Doc, Pos, Span};

/// A writing day, numbered so that later days are larger. Days begin at
/// 04:00; the session says which one a moment falls in. 0 is "before any
/// day this sitting knows of".
pub type Day = u32;

/// The moment an edit is made: for grouping undo steps and for stamping the
/// lines it touches.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Clock {
    pub now_ms: u64,
    pub today: Day,
}

/// The sizes the keys work in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Letter,
    Word,
    Sentence,
    /// One wrapped row up or down.
    Row,
    Paragraph,
    /// Home and End: the ends of the wrapped row.
    RowEnd,
    /// Ctrl Home and Ctrl End: the ends of the text.
    Text,
    /// Page Up and Page Down: this many rows.
    Rows(usize),
}

/// Which way a key goes: back toward the start of the text, or on toward
/// its end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Way {
    Back,
    On,
}

/// How far a quote is set in from each edge of the measure.
pub const QUOTE_INSET: usize = 4;

/// The widest mark that hangs beside the text: the three columns of the
/// gutter. `####` and deeper do not fit there, and since no character of the
/// file is ever hidden or moved off the sheet they are shown as typed at the
/// head of their line, at half strength like any mark.
const HANGS: usize = 3;

/// How a line of this kind is set on the page: what hangs in the gutter,
/// whether it is double size, and how its rows are indented.
pub fn shape_of(kind: LineKind, line: &str, measure: usize) -> Shape {
    match kind {
        LineKind::Heading(level) if level as usize <= HANGS => {
            let marks = level as usize;
            let gap = matches!(line.as_bytes().get(marks), Some(b' ' | b'\t'));
            Shape { hung: with_riders(line, marks + usize::from(gap)), scale: if level == 1 { 2 } else { 1 }, hang: 0, inset: 0 }
        }
        LineKind::Quote => {
            // A list inside a quote keeps its hang: its rows turn over under
            // the item's first letter, as every other list's do.
            let hung = with_riders(line, quote_mark(line));
            let hang = line.get(hung..).map_or(0, markup::hang_of).min(measure / 4);
            Shape { hung, scale: 1, hang, inset: QUOTE_INSET }
        }
        LineKind::Item => Shape::hanging(markup::hang_of(line).min(measure / 2)),
        _ => Shape::default(),
    }
}

/// The bytes of a hung mark and of whatever rides on its last character: a
/// combining mark typed straight after `# ` or `>` sits on that character,
/// and hangs with it, so that the first row of the line never starts between
/// a character and a mark on it.
fn with_riders(line: &str, hung: usize) -> usize {
    let riders: usize = line.get(hung..).unwrap_or("").chars().take_while(|&ch| is_combining(ch)).map(char::len_utf8).sum();
    if hung == 0 {
        0
    } else {
        hung + riders
    }
}

/// Bytes of a quote line that are its mark: any indentation, the `>`, and
/// one space after it.
fn quote_mark(line: &str) -> usize {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    if bytes.get(indent) != Some(&b'>') {
        return 0;
    }
    indent + 1 + usize::from(matches!(bytes.get(indent + 1), Some(b' ' | b'\t')))
}

/// Every `TK` standing alone in capitals on a line, as byte ranges: the
/// newsroom's mark for "to come". It stands alone when no letter or digit
/// touches it, and nothing rides on it: a combining mark after the K makes
/// some other letter of it, and one before the T belongs to whatever it
/// sits on.
pub fn tks(line: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = line.get(from..).and_then(|rest| rest.find("TK")) {
        let start = from + at;
        let end = start + 2;
        let before = line.get(..start).and_then(|head| head.chars().rev().find(|&ch| !is_combining(ch)));
        let after = line.get(end..).and_then(|tail| tail.chars().next());
        let alone = !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(|ch| ch.is_alphanumeric() || is_combining(ch));
        if alone {
            found.push((start, end));
        }
        from = end;
    }
    found
}

/// Whether the first and the last of the lines a change touched read the
/// same after it as before. `doc` is the text as that change left it.
fn ends_untouched(doc: &Doc, change: &Change) -> (bool, bool) {
    let (removed, inserted) = (change.removed.as_str(), change.inserted.as_str());
    let (removed_broke, inserted_broke) = (removed.contains('\n'), inserted.contains('\n'));
    fn first_of(text: &str) -> &str {
        text.split('\n').next().unwrap_or("")
    }
    fn last_of(text: &str) -> &str {
        text.rsplit('\n').next().unwrap_or("")
    }
    // What stood before the change on its first line, and after it on its
    // last, is the same on both sides of the change.
    let head = doc.line(change.start.line).get(..change.start.col).unwrap_or("");
    let tail = doc.line(change.end.line).get(change.end.col..).unwrap_or("");
    let alike = |a: (&str, &str), b: (&str, &str)| a.0.len() + a.1.len() == b.0.len() + b.1.len() && a.0.bytes().chain(a.1.bytes()).eq(b.0.bytes().chain(b.1.bytes()));
    let first_same = alike((first_of(inserted), if inserted_broke { "" } else { tail }), (first_of(removed), if removed_broke { "" } else { tail }));
    let last_same = alike((if inserted_broke { "" } else { head }, last_of(inserted)), (if removed_broke { "" } else { head }, last_of(removed)));
    (first_same, last_same)
}

/// The parts of a list item's marker.
struct Item<'a> {
    indent: &'a str,
    /// `-`, `*`, `+`, or the digits of a numbered item.
    mark: &'a str,
    /// `.` or `)` after a number; empty after a bullet.
    stop: &'a str,
    boxed: bool,
    /// The byte where the item's own words start.
    text: usize,
}

impl Item<'_> {
    /// Reads the marker of a line the markup calls an item.
    fn of(line: &str) -> Option<Item<'_>> {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
        let digits = bytes.get(indent..)?.iter().take_while(|b| b.is_ascii_digit()).count();
        let (mark_end, stop_end) = match bytes.get(indent)? {
            b'-' | b'*' | b'+' => (indent + 1, indent + 1),
            _ if digits > 0 && matches!(bytes.get(indent + digits), Some(b'.' | b')')) => (indent + digits, indent + digits + 1),
            _ => return None,
        };
        if !matches!(bytes.get(stop_end), Some(b' ' | b'\t')) {
            return None;
        }
        let mut text = stop_end + 1;
        let boxed = matches!(bytes.get(text..text + 4), Some([b'[', b' ' | b'x' | b'X', b']', b' ' | b'\t']));
        if boxed {
            text += 4;
        }
        Some(Item { indent: line.get(..indent)?, mark: line.get(indent..mark_end)?, stop: line.get(mark_end..stop_end)?, boxed, text })
    }

    /// The marker of the item that follows this one: the same bullet, or the
    /// next number, and an empty box if this one had a box.
    fn next(&self) -> String {
        let mark = match self.mark.parse::<u64>() {
            Ok(number) => number.saturating_add(1).to_string(),
            Err(_) => self.mark.to_string(),
        };
        format!("{}{}{} {}", self.indent, mark, self.stop, if self.boxed { "[ ] " } else { "" })
    }
}

/// Where the cursor is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caret {
    pub line: usize,
    /// The row of the whole text it is on, counted in grid rows.
    pub row: usize,
    /// Grid columns from the first column of the text. Negative for a
    /// cursor on a mark hanging in the gutter.
    pub cell: i32,
    /// 2 on a double-size line, where the cursor is two cells each way.
    pub size: usize,
}

/// One text being written.
pub struct Page {
    doc: Doc,
    pub cursor: Cursor,
    history: History,
    kinds: Vec<LineKind>,
    layout: Layout,
    tally: Tally,
    /// For every line, the day it was last changed.
    stamps: Vec<Day>,
    measure: usize,
    /// The grid column Up and Down are making for, kept from one press to
    /// the next so a short row on the way does not pull the cursor left.
    goal: Option<usize>,
    /// Changes made and not yet handed on.
    outbox: Vec<Change>,
    /// A text open for reading only takes no edits.
    pub read_only: bool,
    /// The word limit, and where the first word past it starts when the
    /// text has one. Kept in step with the text.
    limit: Option<usize>,
    over: Option<Pos>,
}

impl Page {
    /// A page holding `text`, wrapped at `measure`, with the cursor at its
    /// end and nothing to undo.
    pub fn new(text: &str, measure: usize) -> Page {
        let doc = Doc::from_text(text);
        let kinds = markup::classify(doc.lines());
        let layout = Layout::new(&doc, measure, &|at, line| shape_of(kinds.get(at).copied().unwrap_or(LineKind::Text), line, measure));
        let tally = Tally::of(&doc);
        let stamps = vec![0; doc.line_count()];
        let cursor = Cursor::at(doc.end());
        Page { doc, cursor, history: History::new(), kinds, layout, tally, stamps, measure, goal: None, outbox: Vec::new(), read_only: false, limit: None, over: None }
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn measure(&self) -> usize {
        self.measure
    }

    /// What a line is. A paragraph past the end.
    pub fn kind(&self, line: usize) -> LineKind {
        self.kinds.get(line).copied().unwrap_or(LineKind::Text)
    }

    /// The day a line was last changed.
    pub fn stamp(&self, line: usize) -> Day {
        self.stamps.get(line).copied().unwrap_or(0)
    }

    /// Sets the day each line was last changed, as remembered from earlier
    /// sittings. Lines the list does not reach count as older than any day.
    pub fn set_stamps(&mut self, stamps: &[Day]) {
        self.stamps = (0..self.doc.line_count()).map(|line| stamps.get(line).copied().unwrap_or(0)).collect();
    }

    /// Words in the whole text.
    pub fn words(&self) -> usize {
        self.tally.words()
    }

    /// Words in the selection, when there is one.
    pub fn selected_words(&self) -> Option<usize> {
        self.cursor.has_selection().then(|| count::words(&self.doc.slice(self.cursor.span())))
    }

    /// The word limit of this text, if it has one.
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Sets the word limit, or takes it away.
    pub fn set_limit(&mut self, limit: Option<usize>) {
        if limit != self.limit {
            self.limit = limit;
            self.find_over();
        }
    }

    /// Where the first word past the limit starts, when the text runs over
    /// its limit. Everything from here on is past it.
    pub fn over(&self) -> Option<Pos> {
        self.over
    }

    /// Finds the first word past the limit afresh: the line the running
    /// count crosses the limit on, and the word on it that does the
    /// crossing. Words are counted as the facts count them.
    fn find_over(&mut self) {
        self.over = None;
        let Some(limit) = self.limit.filter(|&limit| self.tally.words() > limit) else { return };
        let mut before = 0;
        for line in 0..self.doc.line_count() {
            let here = self.tally.line_words(line);
            if before + here <= limit {
                before += here;
                continue;
            }
            let text = self.doc.line(line);
            let mut counted = before;
            let mut start = None;
            for (at, ch) in text.char_indices() {
                if ch.is_whitespace() {
                    start = None;
                    continue;
                }
                let from = *start.get_or_insert(at);
                // A stretch counts as a word from its first letter or digit.
                if ch.is_alphanumeric() && text.get(from..at).is_some_and(|head| !head.chars().any(char::is_alphanumeric)) {
                    counted += 1;
                    if counted > limit {
                        self.over = Some(Pos::new(line, from));
                        return;
                    }
                }
            }
            return;
        }
    }

    /// The limit and the row of the text its rule is set above: the row the
    /// first word past the limit is on. `None` when the text is within its
    /// limit or has none.
    pub fn rule(&self) -> Option<(usize, usize)> {
        let over = self.over?;
        Some((self.limit?, self.layout.locate(&self.doc, over).0))
    }

    /// The row of the paper a row of the text is on. The paper has every
    /// row of the text and, when the text runs past its limit, one more for
    /// the rule, which pushes the rows from there on down by one.
    pub fn paper_row(&self, text_row: usize) -> i64 {
        text_row as i64 + i64::from(self.rule().is_some_and(|(_, at)| text_row >= at))
    }

    /// The row of the text on a row of the paper. `None` on the rule's own
    /// row and off either end.
    pub fn text_row(&self, paper_row: i64) -> Option<usize> {
        let row = match self.rule() {
            Some((_, at)) if paper_row == at as i64 => return None,
            Some((_, at)) if paper_row > at as i64 => paper_row - 1,
            _ => paper_row,
        };
        (0 <= row && row < self.layout.total_rows() as i64).then_some(row as usize)
    }

    /// Rows of the paper: the rows of the text, and one for the rule.
    pub fn paper_rows(&self) -> usize {
        self.layout.total_rows() + usize::from(self.rule().is_some())
    }

    /// The changes made since this was last asked, oldest first.
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.outbox)
    }

    /// Wraps the text again at a new measure.
    pub fn set_measure(&mut self, measure: usize) {
        if measure != self.measure {
            self.measure = measure;
            self.rewrap();
        }
    }

    fn rewrap(&mut self) {
        let (kinds, measure) = (&self.kinds, self.measure);
        self.layout.rewrap(&self.doc, measure, &|at, line| shape_of(kinds.get(at).copied().unwrap_or(LineKind::Text), line, measure));
    }

    /// Brings what follows from the text as a whole in step with it.
    fn settle(&mut self) {
        if self.limit.is_some() {
            self.find_over();
        }
    }

    /// Reads every table afresh from the text.
    fn rebuild(&mut self) {
        self.kinds = markup::classify(self.doc.lines());
        self.rewrap();
        self.tally = Tally::of(&self.doc);
        self.stamps.resize(self.doc.line_count(), 0);
        self.settle();
    }

    /// Brings the tables in step with one change the document has just had.
    fn sync(&mut self, change: &Change, today: Day) {
        let (first, old, new) = change.lines();
        let fits = first + old <= self.kinds.len() && first + new <= self.doc.line_count();
        if !fits {
            self.rebuild();
            return;
        }
        let is_fence = |kind: &&LineKind| **kind == LineKind::Fence;
        let mut in_fence = self.kinds[..first].iter().filter(is_fence).count() % 2 == 1;
        let fences_gone = self.kinds[first..first + old].iter().filter(is_fence).count();
        let fresh: Vec<LineKind> = (first..first + new)
            .map(|line| {
                let kind = markup::classify_line(self.doc.line(line), in_fence);
                in_fence ^= kind == LineKind::Fence;
                kind
            })
            .collect();
        let fences_come = fresh.iter().filter(is_fence).count();
        self.kinds.splice(first..first + old, fresh);
        if (fences_gone + fences_come) % 2 == 1 {
            // A fence opened or closed: every line below changes sides.
            self.kinds = markup::classify(self.doc.lines());
            self.rewrap();
        } else {
            let (kinds, measure) = (&self.kinds, self.measure);
            self.layout.apply(&self.doc, change, &|at, line| shape_of(kinds.get(at).copied().unwrap_or(LineKind::Text), line, measure));
        }
        self.tally.apply(&self.doc, change);
        self.stamp_lines(change, today);
        self.settle();
    }

    /// Stamps the lines a change wrote with today. A change counts as
    /// touching every line from where it starts to where it ends, but the
    /// lines at either end may read exactly as they did: Enter at the end of
    /// a paragraph leaves that paragraph alone, and Enter at its start
    /// leaves it alone one line further down. Those keep the day they had.
    fn stamp_lines(&mut self, change: &Change, today: Day) {
        let (first, old, new) = change.lines();
        if old == 0 || first + old > self.stamps.len() || first + new > self.doc.line_count() {
            self.stamps.resize(self.doc.line_count(), today);
            return;
        }
        let (was_first, was_last) = (self.stamps[first], self.stamps[first + old - 1]);
        let (first_same, last_same) = ends_untouched(&self.doc, change);
        self.stamps.splice(first..first + old, std::iter::repeat_n(today, new));
        // When a line was split in two that read alike, the upper one is the
        // line that was there and the lower one is new.
        if last_same && !(first_same && new > old) {
            self.stamps[first + new - 1] = was_last;
        }
        if first_same {
            self.stamps[first] = was_first;
        }
    }

    /// The one way the text changes: replaces `span` with `text`, records it
    /// for undo as `kind`, and leaves the cursor at `after`, or just past
    /// the new text when that is `None`. Returns false when nothing changed.
    pub fn replace(&mut self, span: Span, text: &str, kind: EditKind, after: Option<Cursor>, clock: Clock) -> bool {
        if self.read_only {
            return false;
        }
        let before = self.cursor;
        let change = self.doc.replace(span, text);
        if change.is_noop() {
            return false;
        }
        self.sync(&change, clock.today);
        self.cursor = match after {
            Some(cursor) => Cursor { head: self.doc.clamp(cursor.head), anchor: self.doc.clamp(cursor.anchor) },
            None => Cursor::at(change.end),
        };
        self.history.record(change.clone(), kind, before, self.cursor, clock.now_ms);
        self.outbox.push(change);
        self.goal = None;
        true
    }

    /// Puts the tables and the cursor right after an undo or a redo.
    fn replayed(&mut self, replay: Replay, clock: Clock) {
        match replay.changes.as_slice() {
            [only] => self.sync(only, clock.today),
            many => {
                // The document has had them all, so the tables that read it
                // are read afresh; the stamps only need each change's lines.
                for change in many {
                    change.splice(&mut self.stamps, || clock.today);
                }
                self.rebuild();
            }
        }
        self.cursor = Cursor { head: self.doc.clamp(replay.cursor.head), anchor: self.doc.clamp(replay.cursor.anchor) };
        self.outbox.extend(replay.changes);
        self.goal = None;
    }

    /// Takes back the last step. False when there is nothing to take back.
    pub fn undo(&mut self, clock: Clock) -> bool {
        if self.read_only {
            return false;
        }
        match self.history.undo(&mut self.doc) {
            Some(replay) => {
                self.replayed(replay, clock);
                true
            }
            None => false,
        }
    }

    /// Makes the step undone last again.
    pub fn redo(&mut self, clock: Clock) -> bool {
        if self.read_only {
            return false;
        }
        match self.history.redo(&mut self.doc) {
            Some(replay) => {
                self.replayed(replay, clock);
                true
            }
            None => false,
        }
    }

    /// Puts the cursor at `pos`; with `select`, stretching the selection to
    /// it from where the selection began.
    pub fn place(&mut self, pos: Pos, select: bool) {
        let head = self.doc.clamp(pos);
        self.cursor = Cursor { head, anchor: if select { self.cursor.anchor } else { head } };
        self.history.seal();
        self.goal = None;
    }

    /// Selects a stretch, the cursor at its end.
    pub fn select(&mut self, span: Span) {
        self.cursor = Cursor { head: self.doc.clamp(span.end), anchor: self.doc.clamp(span.start) };
        self.history.seal();
        self.goal = None;
    }

    pub fn select_all(&mut self) {
        self.select(Span { start: Pos::default(), end: self.doc.end() });
    }

    /// The text selected, with line breaks as `\n`.
    pub fn selection(&self) -> Option<String> {
        self.cursor.has_selection().then(|| self.doc.slice(self.cursor.span()))
    }

    /// Moves the cursor by one unit, selecting on the way with `select`.
    ///
    /// Left and Right without Shift fold a selection up to the end they
    /// point at. Up and Down keep making for the column they started from.
    pub fn travel(&mut self, unit: Unit, way: Way, select: bool) {
        let head = self.cursor.head;
        let back = way == Way::Back;
        let mut goal = None;
        let to = match unit {
            Unit::Letter if self.cursor.has_selection() && !select => {
                let span = self.cursor.span();
                if back {
                    span.start
                } else {
                    span.end
                }
            }
            Unit::Letter if back => motion::left(&self.doc, head),
            Unit::Letter => motion::right(&self.doc, head),
            Unit::Word if back => motion::word_left(&self.doc, head),
            Unit::Word => motion::word_right(&self.doc, head),
            Unit::Sentence if back => motion::sentence_prev(&self.doc, head),
            Unit::Sentence => motion::sentence_next(&self.doc, head),
            Unit::Paragraph if back => motion::paragraph_prev(&self.doc, head),
            Unit::Paragraph => motion::paragraph_next(&self.doc, head),
            Unit::RowEnd if back => self.layout.row_start(&self.doc, head),
            Unit::RowEnd => self.layout.row_end(&self.doc, head),
            Unit::Text if back => Pos::default(),
            Unit::Text => self.doc.end(),
            Unit::Row | Unit::Rows(_) => {
                let cell = self.goal.unwrap_or_else(|| self.layout.locate(&self.doc, head).1);
                goal = Some(cell);
                let rows = if let Unit::Rows(rows) = unit { rows.max(1) } else { 1 };
                let mut at = head;
                for _ in 0..rows {
                    let next = if back { self.layout.up(&self.doc, at, cell) } else { self.layout.down(&self.doc, at, cell) };
                    if next == at {
                        break;
                    }
                    at = next;
                }
                at
            }
        };
        self.place(to, select);
        self.goal = goal;
    }

    /// Backspace and Delete at each size. A selection is simply removed.
    ///
    /// At the start of a paragraph, Backspace undoes what Enter did: the
    /// blank line and the break go together, and an item joins the item
    /// above it. Delete at the end of a paragraph is the same act seen from
    /// the other side. Word and sentence never reach across a line break;
    /// at the edge of a line they do what the letter does there.
    pub fn erase(&mut self, unit: Unit, way: Way, clock: Clock) -> bool {
        if self.cursor.has_selection() {
            return self.replace(self.cursor.span(), "", EditKind::Delete, None, clock);
        }
        let p = self.doc.clamp(self.cursor.head);
        let line_len = self.doc.line(p.line).len();
        let span = match (unit, way) {
            (_, Way::Back) if p.col == 0 => self.break_before(p),
            (_, Way::On) if p.col >= line_len => self.break_after(p),
            (Unit::Word, Way::Back) => Span::new(motion::word_left(&self.doc, p), p),
            (Unit::Word, Way::On) => Span::new(p, motion::word_right(&self.doc, p)),
            (Unit::Sentence, Way::Back) => {
                let start = motion::sentence_prev(&self.doc, p);
                Span::new(if start.line == p.line { start } else { Pos::new(p.line, 0) }, p)
            }
            (Unit::Sentence, Way::On) => {
                let next = motion::sentence_next(&self.doc, p);
                Span::new(p, if next.line == p.line && next > p { next } else { Pos::new(p.line, line_len) })
            }
            (_, Way::Back) => match self.item_start(p) {
                Some(span) => span,
                None => Span::new(motion::left(&self.doc, p), p),
            },
            (_, Way::On) => Span::new(p, motion::right(&self.doc, p)),
        };
        self.replace(span, "", EditKind::Delete, None, clock)
    }

    /// What Backspace takes at the start of a line: the break before it, and
    /// with it the blank line Enter wrote, when that is what lies above.
    ///
    /// Enter writes its breaks two at a time, wherever it is pressed: at
    /// the end of a paragraph, in the middle of one, or at its very start,
    /// where both go in above it. So the breaks between this line and the
    /// last line above with anything on it (or the top of the text) are
    /// counted. When they come to whole pairs, the last pair is Enter's and
    /// goes together; when one is left over, a single break was put there
    /// some other way, and it goes first by itself.
    fn break_before(&self, p: Pos) -> Span {
        let blank = |line: usize| self.kind(line) == LineKind::Blank;
        let blanks = (0..p.line).rev().take_while(|&line| blank(line)).count();
        // One break more than the blank lines when a line with something
        // on it stands above them, and as many as them at the top.
        let breaks = blanks + usize::from(blanks < p.line);
        if breaks >= 2 && breaks % 2 == 0 {
            return Span::new(Pos::new(p.line - 2, self.doc.line(p.line - 2).len()), p);
        }
        Span::new(motion::left(&self.doc, p), p)
    }

    /// What Delete takes at the end of a line: the break after it, with the
    /// blank line between two paragraphs, or with the marker of the item
    /// below when both lines are items.
    fn break_after(&self, p: Pos) -> Span {
        let blank = |line: usize| self.kind(line) == LineKind::Blank;
        let lines = self.doc.line_count();
        if p.line + 2 < lines && !blank(p.line) && blank(p.line + 1) && !blank(p.line + 2) {
            return Span::new(p, Pos::new(p.line + 2, 0));
        }
        if p.line + 1 < lines && self.kind(p.line) == LineKind::Item && self.kind(p.line + 1) == LineKind::Item {
            if let Some(item) = Item::of(self.doc.line(p.line + 1)) {
                return Span::new(p, Pos::new(p.line + 1, item.text));
            }
        }
        Span::new(p, motion::right(&self.doc, p))
    }

    /// What Backspace takes just after an item's marker when the line above
    /// is an item too: the break and the marker, which is what Enter wrote.
    fn item_start(&self, p: Pos) -> Option<Span> {
        if p.line == 0 || self.kind(p.line) != LineKind::Item || self.kind(p.line - 1) != LineKind::Item {
            return None;
        }
        let item = Item::of(self.doc.line(p.line))?;
        (item.text == p.col).then(|| Span::new(Pos::new(p.line - 1, self.doc.line(p.line - 1).len()), p))
    }

    /// Puts `text` in at the cursor, over the selection if there is one.
    /// One typed letter joins the run of typing before it for undo; anything
    /// longer, and anything typed over a selection, is a step of its own.
    pub fn insert(&mut self, text: &str, clock: Clock) -> bool {
        let typing = !self.cursor.has_selection() && text.chars().count() == 1 && !text.contains('\n');
        let kind = if typing { EditKind::Insert } else { EditKind::Other };
        self.replace(self.cursor.span(), text, kind, None, clock)
    }

    /// Enter. With `soft` (Shift Enter) a line break inside the paragraph.
    /// Otherwise it ends the paragraph and writes the blank line between
    /// paragraphs; in a list it starts the next item, and on an empty item
    /// ends the list; on a blank line or in fenced code it is one line
    /// break.
    pub fn enter(&mut self, soft: bool, clock: Clock) -> bool {
        let span = self.cursor.span();
        let p = span.start;
        let line = self.doc.line(p.line);
        let kind = self.kind(p.line);
        if soft || matches!(kind, LineKind::Blank | LineKind::Code | LineKind::Fence) {
            return self.replace(span, "\n", EditKind::Other, None, clock);
        }
        if kind == LineKind::Item {
            return match Item::of(line) {
                Some(item) if p.col >= item.text && line.get(item.text..).is_some_and(|rest| rest.trim().is_empty()) => {
                    // An empty item: the list is over. Its marker goes and
                    // the blank line that ends a paragraph takes its place.
                    // A selection reaching into later lines goes with it, to
                    // where it ends and no further.
                    let whole = Span::new(Pos::new(p.line, 0), span.end.max(Pos::new(p.line, line.len())));
                    self.replace(whole, "\n", EditKind::Other, None, clock)
                }
                Some(item) if p.col >= item.text => {
                    let next = format!("\n{}", item.next());
                    self.replace(span, &next, EditKind::Other, None, clock)
                }
                _ => self.replace(span, "\n", EditKind::Other, None, clock),
            };
        }
        self.replace(span, "\n\n", EditKind::Other, None, clock)
    }

    /// Tab and Shift Tab in a list: moves every item the selection touches
    /// in by two spaces, or back out by up to two. False, and nothing done,
    /// when no line it touches is an item.
    pub fn nest(&mut self, way: Way, clock: Clock) -> bool {
        let span = self.cursor.span();
        let lines: Vec<usize> = (span.start.line..=span.end.line).filter(|&line| self.kind(line) == LineKind::Item).collect();
        if lines.is_empty() || self.read_only {
            return false;
        }
        let mut cursor = self.cursor;
        let mut done = false;
        self.history.begin_group();
        for line in lines {
            let shift = |pos: &mut Pos, by: isize| {
                if pos.line == line {
                    pos.col = pos.col.saturating_add_signed(by);
                }
            };
            let start = Pos::new(line, 0);
            let (edit, by) = match way {
                Way::On => (Span::at(start), 2),
                Way::Back => {
                    let spaces = self.doc.line(line).bytes().take(2).take_while(|&b| b == b' ').count();
                    (Span::new(start, Pos::new(line, spaces)), -(spaces as isize))
                }
            };
            shift(&mut cursor.head, by);
            shift(&mut cursor.anchor, by);
            done |= self.replace(edit, if way == Way::On { "  " } else { "" }, EditKind::Other, Some(cursor), clock);
        }
        self.history.end_group();
        if done {
            self.cursor = Cursor { head: self.doc.clamp(cursor.head), anchor: self.doc.clamp(cursor.anchor) };
        }
        done
    }

    /// Alt Up and Alt Down: carries the paragraph the cursor is in past the
    /// paragraph above or below it, the cursor going with it. False when
    /// there is no paragraph here or none to pass.
    pub fn carry(&mut self, way: Way, clock: Clock) -> bool {
        let head = self.doc.clamp(self.cursor.head);
        let own = motion::paragraph_at(&self.doc, head);
        let other = match way {
            Way::Back => motion::paragraph_at(&self.doc, motion::paragraph_prev(&self.doc, own.start)),
            Way::On => motion::paragraph_at(&self.doc, motion::paragraph_next(&self.doc, own.end)),
        };
        // A blank line is no paragraph, and neither is the cursor's own.
        let apart = if way == Way::Back { other.end.line < own.start.line } else { other.start.line > own.end.line };
        if own.is_empty() || other.is_empty() || !apart {
            return false;
        }
        let (upper, lower) = if way == Way::Back { (other, own) } else { (own, other) };
        let between = Span::new(upper.end, lower.start);
        let whole = Span::new(upper.start, lower.end);
        let (upper_text, gap, lower_text) = (self.doc.slice(upper), self.doc.slice(between), self.doc.slice(lower));
        let swapped = format!("{lower_text}{gap}{upper_text}");

        // The paragraph that moved is changed today; the one it passed is
        // only somewhere else, and keeps the days it had.
        let lower_lines = lower.end.line - lower.start.line + 1;
        let gap_lines = lower.start.line - upper.end.line - 1;
        let passed: Vec<Day> = match way {
            Way::Back => (upper.start.line..=upper.end.line).map(|line| self.stamp(line)).collect(),
            Way::On => (lower.start.line..=lower.end.line).map(|line| self.stamp(line)).collect(),
        };
        let landed = match way {
            Way::Back => upper.start.line,
            Way::On => upper.start.line + lower_lines + gap_lines,
        };
        let cursor = Cursor::at(Pos::new(landed + (head.line - own.start.line), head.col));
        if !self.replace(whole, &swapped, EditKind::Other, Some(cursor), clock) {
            return false;
        }
        let passed_at = match way {
            Way::Back => upper.start.line + lower_lines + gap_lines,
            Way::On => upper.start.line,
        };
        for (offset, day) in passed.into_iter().enumerate() {
            if let Some(stamp) = self.stamps.get_mut(passed_at + offset) {
                *stamp = day;
            }
        }
        true
    }

    /// Ctrl T: selects the next `TK` after the cursor, going round to the
    /// top after the last. False when the text holds none.
    pub fn next_tk(&mut self) -> bool {
        let head = self.doc.clamp(self.cursor.head);
        let lines = self.doc.line_count();
        let on = |line: usize| -> Vec<(usize, usize)> {
            match self.kind(line) {
                LineKind::Code | LineKind::Fence => Vec::new(),
                _ => tks(self.doc.line(line)),
            }
        };
        let ahead = on(head.line).into_iter().find(|&(start, _)| start >= head.col).map(|found| (head.line, found));
        let found = ahead
            .or_else(|| (head.line + 1..lines).find_map(|line| on(line).first().map(|&found| (line, found))))
            .or_else(|| (0..=head.line).find_map(|line| on(line).first().map(|&found| (line, found))));
        match found {
            Some((line, (start, end))) => {
                self.select(Span { start: Pos::new(line, start), end: Pos::new(line, end) });
                true
            }
            None => false,
        }
    }

    /// How many `TK`s the text still holds.
    pub fn tk_count(&self) -> usize {
        (0..self.doc.line_count()).filter(|&line| !matches!(self.kind(line), LineKind::Code | LineKind::Fence)).map(|line| tks(self.doc.line(line)).len()).sum()
    }

    /// The first and last line of the paragraph the cursor is in. On a
    /// blank line, that line alone.
    pub fn own_paragraph(&self) -> (usize, usize) {
        let own = motion::paragraph_at(&self.doc, self.cursor.head);
        (own.start.line, own.end.line)
    }

    /// The mark of a line that hangs in the gutter, without the space after
    /// it. Empty when nothing hangs.
    pub fn hung_mark(&self, line: usize) -> &str {
        let hung = self.layout.shape(line).hung;
        self.doc.line(line).get(..hung).unwrap_or("").trim_end()
    }

    /// Where the cursor is drawn.
    pub fn caret(&self) -> Caret {
        let head = self.doc.clamp(self.cursor.head);
        let shape = self.layout.shape(head.line);
        if head.col < shape.hung {
            // On the mark in the gutter, which is single size and sits on
            // the last grid row of the line's first row.
            let text = self.doc.line(head.line);
            let mark_end = self.hung_mark(head.line).len();
            let after = text.get(head.col..mark_end.max(head.col)).map_or(0, str_width) as i32;
            // The mark ends three cells before the text. The space after it
            // has no cell of its own there, since the column beside the mark
            // is the change bars'; the cursor on it rests in the empty column
            // before the text. No cursor goes into the sheet's edge column.
            let cell = if after == 0 { -1 } else { (-2 - after).max(-5) };
            return Caret { line: head.line, row: self.layout.first_row(head.line) + shape.scale - 1, cell, size: 1 };
        }
        let (row, cell) = self.layout.locate(&self.doc, head);
        // Whitespace may hang past the measure; the cursor rests one cell
        // past it and no further.
        Caret { line: head.line, row, cell: cell.min(self.measure) as i32, size: shape.scale }
    }

    /// The position under a cell of the page: `row` a row of the paper,
    /// `cell` a grid column counted from the first column of the text
    /// (negative in the gutter). The rule of a word limit holds no text; a
    /// cell of it answers as the row under it.
    pub fn pos_at(&self, row: usize, cell: i32) -> Pos {
        let row = match self.rule() {
            Some((_, at)) if row > at => row - 1,
            _ => row,
        };
        self.pos_at_text_row(row, cell)
    }

    /// The position under a cell of a row of the text, whatever the paper
    /// has between its rows.
    pub fn pos_at_text_row(&self, row: usize, cell: i32) -> Pos {
        let row = row.min(self.layout.total_rows().saturating_sub(1));
        self.layout.pos_at(&self.doc, row, cell.max(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tk_stands_alone_in_capitals() {
        assert_eq!(tks("on purpose, TK the year"), [(12, 14)]);
        assert_eq!(tks("TK"), [(0, 2)]);
        assert_eq!(tks("(TK) and TK."), [(1, 3), (9, 11)]);
        assert!(tks("ATK TKs TKTK tk Tk 4TK TK9").is_empty());
        assert_eq!(tks("\u{00E9}TK TK\u{00E9} TK-2"), [(10, 12)]);
        // A mark on the K, or a marked letter before the T, and it is not alone.
        assert!(tks("TK\u{0301} e\u{0301}TK").is_empty());
        assert_eq!(tks(" \u{200F}TK"), [(4, 6)]);
    }

    #[test]
    fn a_line_is_shaped_by_what_it_is() {
        let shape = |line: &str| shape_of(markup::classify_line(line, false), line, 66);
        assert_eq!(shape("# The orchard"), Shape { hung: 2, scale: 2, hang: 0, inset: 0 });
        assert_eq!(shape("#"), Shape { hung: 1, scale: 2, hang: 0, inset: 0 });
        assert_eq!(shape("## Where it is weak"), Shape { hung: 3, scale: 1, hang: 0, inset: 0 });
        assert_eq!(shape("### Third"), Shape { hung: 4, scale: 1, hang: 0, inset: 0 });
        assert_eq!(shape("#### Deep"), Shape::default());
        assert_eq!(shape("###### Deepest"), Shape::default());
        assert_eq!(shape("> a quote"), Shape { hung: 2, scale: 1, hang: 0, inset: 4 });
        assert_eq!(shape(">bare"), Shape { hung: 1, scale: 1, hang: 0, inset: 4 });
        // A mark typed straight after the hung mark rides on it and hangs with it.
        assert_eq!(shape(">\u{200D}x"), Shape { hung: 4, scale: 1, hang: 0, inset: 4 });
        assert_eq!(shape("# \u{0301}e"), Shape { hung: 4, scale: 2, hang: 0, inset: 0 });
        assert_eq!(shape("> - a list in a quote"), Shape { hung: 2, scale: 1, hang: 2, inset: 4 });
        assert_eq!(shape("> 12. numbered"), Shape { hung: 2, scale: 1, hang: 4, inset: 4 });
        assert_eq!(shape("- item"), Shape::hanging(2));
        assert_eq!(shape("  12. item"), Shape::hanging(6));
        assert_eq!(shape("- [ ] task"), Shape::hanging(6));
        assert_eq!(shape("plain"), Shape::default());
        // A list buried in indentation still leaves half the measure.
        let deep = format!("{}- x", " ".repeat(80));
        assert_eq!(shape(&deep), Shape::hanging(33));
    }

    #[test]
    fn the_marker_of_the_next_item_follows_from_this_one() {
        let next = |line: &str| Item::of(line).map(|item| item.next());
        assert_eq!(next("- one").as_deref(), Some("- "));
        assert_eq!(next("  * one").as_deref(), Some("  * "));
        assert_eq!(next("9. nine").as_deref(), Some("10. "));
        assert_eq!(next("3) three").as_deref(), Some("4) "));
        assert_eq!(next("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(next("18446744073709551615. last").as_deref(), Some("18446744073709551615. "));
        assert_eq!(next("99999999999999999999999. big").as_deref(), Some("99999999999999999999999. "));
        assert!(next("-no space").is_none());
        assert!(next("plain").is_none());
        assert_eq!(Item::of("- [ ] task").map(|item| item.text), Some(6));
    }
}
