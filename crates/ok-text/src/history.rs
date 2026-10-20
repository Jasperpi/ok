//! Undo and redo.
//!
//! The editor records every [`Change`] after making it. Changes are gathered
//! into steps, and one press of undo takes back one step: typing comes back
//! roughly a word at a time, a run of deleting all at once, and whatever was
//! recorded inside a group as a single step however many changes it took.

use std::collections::VecDeque;

use crate::doc::{Change, Doc, Pos, Span};

/// A change recorded longer than this after the one before starts a new step.
const MERGE_WINDOW_MS: u64 = 1000;

/// How many steps [`History::new`] keeps.
const DEFAULT_LIMIT: usize = 10_000;

/// Where the caret is and where the selection began.
///
/// The selection runs between `anchor` and `head`; they are equal when
/// nothing is selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cursor {
    pub head: Pos,
    pub anchor: Pos,
}

impl Cursor {
    /// A cursor at `p` with nothing selected.
    pub fn at(p: Pos) -> Cursor {
        Cursor { head: p, anchor: p }
    }

    /// What is selected, in document order whichever way it was dragged.
    pub fn span(&self) -> Span {
        Span::new(self.anchor, self.head)
    }

    pub fn has_selection(&self) -> bool {
        self.head != self.anchor
    }
}

/// What sort of edit a change was. Only a run of `Insert`s or a run of
/// `Delete`s can share an undo step; an `Other` always stands alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Insert,
    Delete,
    Other,
}

/// What an undo or redo did: where the cursor belongs afterwards, and the
/// changes exactly as [`Doc::replace`] returned them while replaying, in the
/// order they were applied. Applying them in that order keeps per-line tables
/// and the journal in step with the document.
///
/// By the time the caller sees the changes the document has had them all. A
/// table kept with [`Change::splice`] does not mind, and neither does a
/// [`Tally`](crate::count::Tally): hand it each change in turn with the
/// document as it now stands.
#[derive(Clone, Debug)]
pub struct Replay {
    pub cursor: Cursor,
    pub changes: Vec<Change>,
}

/// One press of undo: the changes it takes back, oldest first, and the cursor
/// on either side of them.
#[derive(Clone, Debug)]
struct Step {
    changes: Vec<Change>,
    before: Cursor,
    after: Cursor,
    /// What a run of backspaces has taken from just in front of the last
    /// change, back to front. It belongs at the start of that change's
    /// `removed`, but putting each key's text there would move the whole run
    /// on every key, so it gathers here until [`Step::settle`] puts it in
    /// place.
    backspaced: String,
}

impl Step {
    fn new(change: Change, before: Cursor, after: Cursor) -> Step {
        Step { changes: vec![change], before, after, backspaced: String::new() }
    }

    /// Adds a change that stands on its own after the ones already here.
    fn push(&mut self, change: Change) {
        self.settle();
        self.changes.push(change);
    }

    /// Takes in a change that continues this step's run of typing or
    /// deleting. Where the two make a single replace it is folded into the
    /// change before it, so undoing a word replays one change, not one per
    /// key.
    fn absorb(&mut self, change: Change) {
        if !self.fold(&change) {
            self.push(change);
        }
    }

    /// Folds `next` into the last change when, made one after the other on a
    /// single line, the two come to the same thing as one replace: typing on
    /// from the end of what was typed, or deleting on from either side of
    /// what was deleted. Returns false, changing nothing, for any other pair.
    ///
    /// Neither change holds a line break: [`Mark::continues`] has turned
    /// away any that does before it gets here. So the run gathered so far is
    /// never read through again, and a key costs the same however long the
    /// run has grown.
    fn fold(&mut self, next: &Change) -> bool {
        let Some(prev) = self.changes.last_mut() else {
            return false;
        };
        if next.removed.is_empty() && next.start == prev.end {
            prev.inserted.push_str(&next.inserted);
            prev.end = next.end;
            return true;
        }
        if !next.inserted.is_empty() || !prev.inserted.is_empty() {
            return false;
        }
        if next.old_end == prev.start {
            // Backspace: the new text went from just before the old.
            self.backspaced.extend(next.removed.chars().rev());
            prev.start = next.start;
            prev.end = next.end;
            return true;
        }
        if next.start == prev.start {
            // Forward delete: the new text went from just after the old.
            prev.removed.push_str(&next.removed);
            prev.old_end.col = prev.old_end.col.saturating_add(next.removed.len());
            return true;
        }
        false
    }

    /// Puts what the backspaces took at the front of the last change's
    /// `removed`, the right way round. The changes are not fit to replay
    /// until this has been done.
    fn settle(&mut self) {
        if self.backspaced.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.backspaced);
        if let Some(last) = self.changes.last_mut() {
            let mut removed = String::with_capacity(held.len() + last.removed.len());
            removed.extend(held.chars().rev());
            removed.push_str(&last.removed);
            last.removed = removed;
        }
    }
}

fn has_break(change: &Change) -> bool {
    change.removed.contains('\n') || change.inserted.contains('\n')
}

/// What the merging rules need to know about a recorded change.
#[derive(Clone, Copy, Debug)]
struct Mark {
    kind: EditKind,
    at_ms: u64,
    start: Pos,
    old_end: Pos,
    end: Pos,
    has_break: bool,
    /// The inserted text starts with something other than whitespace.
    opens_word: bool,
    /// The inserted text ends in whitespace.
    ends_in_space: bool,
}

impl Mark {
    fn of(change: &Change, kind: EditKind, at_ms: u64) -> Mark {
        Mark {
            kind,
            at_ms,
            start: change.start,
            old_end: change.old_end,
            end: change.end,
            has_break: has_break(change),
            opens_word: change.inserted.chars().next().is_some_and(|ch| !ch.is_whitespace()),
            ends_in_space: change.inserted.chars().next_back().is_some_and(char::is_whitespace),
        }
    }

    /// Whether this change carries on from `prev` closely enough to be undone
    /// with it; the rules are set out on [`History::record`].
    fn continues(&self, prev: &Mark) -> bool {
        if self.kind != prev.kind
            || self.has_break
            || prev.has_break
            || self.at_ms.abs_diff(prev.at_ms) > MERGE_WINDOW_MS
        {
            return false;
        }
        match self.kind {
            EditKind::Insert => self.start == prev.end && !(prev.ends_in_space && self.opens_word),
            EditKind::Delete => self.old_end == prev.start || self.start == prev.start,
            EditKind::Other => false,
        }
    }
}

/// The undo and redo stacks for one document.
#[derive(Clone, Debug)]
pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    limit: usize,
    /// True while the newest undo step may still take in more changes.
    open: bool,
    /// How many groups have been begun and not yet ended.
    depth: usize,
    last: Option<Mark>,
}

impl Default for History {
    fn default() -> History {
        History::new()
    }
}

impl History {
    /// A history that keeps the last 10,000 steps.
    pub fn new() -> History {
        History::with_limit(DEFAULT_LIMIT)
    }

    /// A history that keeps at most `steps` steps, forgetting the oldest when
    /// a new one would go over.
    pub fn with_limit(steps: usize) -> History {
        History { undo: VecDeque::new(), redo: Vec::new(), limit: steps, open: false, depth: 0, last: None }
    }

    /// Records a change that has just been made to the document, with the
    /// cursor as it was before and as it is after.
    ///
    /// Inside a group the change always joins the group's step. Outside one
    /// it joins the open step only as the next key in a run of typing or of
    /// deleting: the same kind as the change before it, `Insert` or `Delete`;
    /// no more than a second later; neither of the two holding a line break;
    /// and right next to it. For typing that means starting where the last
    /// insertion ended, and not starting a new word after a space, so that
    /// undo peels off about a word and its trailing space at a time. For
    /// deleting it means ending where the last deletion started (backspace)
    /// or starting where it started (forward delete). Anything else starts a
    /// new step.
    ///
    /// Recording throws away what could have been redone. A change that did
    /// nothing is ignored altogether and leaves redo alone.
    pub fn record(&mut self, change: Change, kind: EditKind, before: Cursor, after: Cursor, now_ms: u64) {
        if change.is_noop() {
            return;
        }
        self.redo.clear();
        let mark = Mark::of(&change, kind, now_ms);
        let grouped = self.depth > 0;
        let joins = self.open && (grouped || self.last.is_some_and(|prev| mark.continues(&prev)));
        self.last = Some(mark);
        match self.undo.back_mut() {
            Some(step) if joins => {
                step.after = after;
                if grouped {
                    step.push(change);
                } else {
                    step.absorb(change);
                }
            }
            _ => {
                self.undo.push_back(Step::new(change, before, after));
                self.open = true;
                while self.undo.len() > self.limit {
                    self.undo.pop_front();
                }
            }
        }
    }

    /// Closes the open step, so the next change recorded starts a new one.
    /// Call it when the cursor moves or anything else breaks a run of typing.
    /// Inside a group it does nothing: a group is one step whatever happens.
    pub fn seal(&mut self) {
        if self.depth == 0 {
            self.open = false;
        }
    }

    /// Starts a group: everything recorded until the matching
    /// [`History::end_group`] is one step. Groups nest, and the step ends
    /// with the outermost.
    ///
    /// An undo, a redo or a clear in the middle of a group ends the step
    /// there, and what the group records afterwards is a step of its own.
    pub fn begin_group(&mut self) {
        if self.depth == 0 {
            self.open = false;
        }
        self.depth += 1;
    }

    /// Ends the group begun last. Without one open it only seals.
    pub fn end_group(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.open = false;
        }
    }

    /// Takes back the newest step, reverting its changes newest first, and
    /// gives the cursor as it was before the step's first change. `None` when
    /// there is nothing to undo.
    ///
    /// The step left on top is closed: a change recorded next starts a new
    /// one, even where it would have carried on a run.
    pub fn undo(&mut self, doc: &mut Doc) -> Option<Replay> {
        let mut step = self.undo.pop_back()?;
        step.settle();
        self.open = false;
        let changes = step.changes.iter().rev().map(|change| doc.revert(change)).collect();
        let cursor = step.before;
        self.redo.push(step);
        Some(Replay { cursor, changes })
    }

    /// Makes the step undone last again, reapplying its changes oldest first,
    /// and gives the cursor as it was after the step's last change. `None`
    /// when there is nothing to redo. The step comes back closed, as after
    /// [`History::undo`].
    pub fn redo(&mut self, doc: &mut Doc) -> Option<Replay> {
        let step = self.redo.pop()?;
        self.open = false;
        let changes = step.changes.iter().map(|change| doc.reapply(change)).collect();
        let cursor = step.after;
        self.undo.push_back(step);
        Some(Replay { cursor, changes })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forgets every step, as after loading a different file. Groups that are
    /// open stay open.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos::new(line, col)
    }

    /// A document with its history and cursor, edited the way the app does
    /// it. `shadow` is a second copy that only ever sees the recorded and
    /// replayed changes, to prove they are enough to follow the document.
    struct Desk {
        doc: Doc,
        shadow: Doc,
        history: History,
        cursor: Cursor,
        now: u64,
    }

    impl Desk {
        fn new(text: &str) -> Desk {
            let doc = Doc::from_text(text);
            let cursor = Cursor::at(doc.end());
            Desk { shadow: doc.clone(), doc, history: History::new(), cursor, now: 0 }
        }

        fn text(&self) -> String {
            self.doc.to_text("\n")
        }

        fn follow(&mut self, change: &Change) {
            let echoed = self.shadow.replace(Span { start: change.start, end: change.old_end }, &change.inserted);
            assert_eq!(&echoed, change);
        }

        fn edit(&mut self, span: Span, text: &str, kind: EditKind) {
            let before = self.cursor;
            let change = self.doc.replace(span, text);
            self.follow(&change);
            self.cursor = Cursor::at(change.end);
            self.history.record(change, kind, before, self.cursor, self.now);
        }

        /// Types `text` a character at a time, 40 ms apart.
        fn type_text(&mut self, text: &str) {
            for ch in text.chars() {
                self.now += 40;
                self.edit(Span::at(self.cursor.head), ch.encode_utf8(&mut [0; 4]), EditKind::Insert);
            }
        }

        fn backspace(&mut self) {
            self.now += 40;
            let head = self.cursor.head;
            let start = self.doc.pos_at(self.doc.offset_of(head).saturating_sub(1));
            self.edit(Span::new(start, head), "", EditKind::Delete);
        }

        fn delete(&mut self) {
            self.now += 40;
            let head = self.cursor.head;
            let offset = self.doc.offset_of(head);
            let end = (1..=4).map(|n| self.doc.pos_at(offset + n)).find(|&q| q != head).unwrap_or(head);
            self.edit(Span::new(head, end), "", EditKind::Delete);
        }

        fn replay(&mut self, replay: Option<Replay>) -> Option<Replay> {
            let replay = replay?;
            for change in &replay.changes {
                self.follow(change);
            }
            assert_eq!(self.shadow.to_text("\n"), self.text());
            self.cursor = replay.cursor;
            Some(replay)
        }

        fn undo(&mut self) -> Option<Replay> {
            let replay = self.history.undo(&mut self.doc);
            self.replay(replay)
        }

        fn redo(&mut self) -> Option<Replay> {
            let replay = self.history.redo(&mut self.doc);
            self.replay(replay)
        }

        /// Undoes until there is nothing left, giving the text after each step.
        fn undo_all(&mut self) -> Vec<String> {
            let mut texts = Vec::new();
            while self.undo().is_some() {
                texts.push(self.text());
            }
            texts
        }
    }

    #[test]
    fn cursor_spans_run_in_document_order() {
        let forward = Cursor { anchor: p(0, 2), head: p(1, 4) };
        let backward = Cursor { anchor: p(1, 4), head: p(0, 2) };
        assert_eq!(forward.span(), Span { start: p(0, 2), end: p(1, 4) });
        assert_eq!(backward.span(), forward.span());
        assert!(forward.has_selection() && backward.has_selection());
        assert!(!Cursor::at(p(3, 1)).has_selection());
        assert!(Cursor::at(p(3, 1)).span().is_empty());
        assert_eq!(Cursor::default(), Cursor::at(p(0, 0)));
    }

    #[test]
    fn typing_is_undone_a_word_and_its_space_at_a_time() {
        let mut desk = Desk::new("");
        desk.type_text("hello brave  world");
        assert_eq!(desk.undo_all(), ["hello brave  ", "hello ", ""]);
        assert!(!desk.history.can_undo() && desk.history.can_redo());
        for text in ["hello ", "hello brave  ", "hello brave  world"] {
            assert!(desk.redo().is_some());
            assert_eq!(desk.text(), text);
        }
        assert!(desk.redo().is_none());
    }

    #[test]
    fn a_run_of_typing_replays_as_one_change() {
        let mut desk = Desk::new("say ");
        desk.type_text("mitsqanaqan\u{0313}");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "say ");
        assert_eq!(replay.cursor, Cursor::at(p(0, 4)));
        assert_eq!(replay.changes.len(), 1);
        assert_eq!(replay.changes[0].removed, "mitsqanaqan\u{0313}");
        let replay = desk.redo().unwrap();
        assert_eq!(desk.text(), "say mitsqanaqan\u{0313}");
        assert_eq!(replay.cursor, Cursor::at(p(0, 17)));
        assert_eq!(replay.changes.len(), 1);
    }

    #[test]
    fn a_pause_starts_a_new_step() {
        let mut desk = Desk::new("");
        desk.type_text("ab");
        desk.now += 960;
        desk.type_text("c");
        desk.now += 961;
        desk.type_text("d");
        assert_eq!(desk.undo_all(), ["abc", ""]);
    }

    #[test]
    fn a_seal_starts_a_new_step() {
        let mut desk = Desk::new("");
        desk.type_text("ab");
        desk.history.seal();
        desk.type_text("cd");
        desk.history.seal();
        desk.history.seal();
        assert_eq!(desk.undo_all(), ["ab", ""]);
    }

    #[test]
    fn line_breaks_stand_alone() {
        let mut desk = Desk::new("");
        desk.type_text("ab\ncd");
        assert_eq!(desk.undo_all(), ["ab\n", "ab", ""]);
    }

    #[test]
    fn typing_somewhere_else_starts_a_new_step() {
        let mut desk = Desk::new("");
        desk.type_text("ab");
        desk.cursor = Cursor::at(p(0, 0));
        desk.type_text("cd");
        assert_eq!(desk.text(), "cdab");
        assert_eq!(desk.undo_all(), ["ab", ""]);
    }

    #[test]
    fn a_run_of_backspaces_is_one_step() {
        let mut desk = Desk::new("abcd\u{00E9}f");
        desk.cursor = Cursor::at(p(0, 6));
        for _ in 0..3 {
            desk.backspace();
        }
        assert_eq!(desk.text(), "abf");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "abcd\u{00E9}f");
        assert_eq!(replay.cursor, Cursor::at(p(0, 6)));
        assert_eq!(replay.changes.len(), 1);
        assert_eq!(replay.changes[0].inserted, "cd\u{00E9}");
        let replay = desk.redo().unwrap();
        assert_eq!(desk.text(), "abf");
        assert_eq!(replay.cursor, Cursor::at(p(0, 2)));
        assert!(desk.undo().is_some());
        assert!(desk.undo().is_none());
    }

    #[test]
    fn a_run_of_forward_deletes_is_one_step() {
        let mut desk = Desk::new("ab\u{00E9}def");
        desk.cursor = Cursor::at(p(0, 1));
        for _ in 0..3 {
            desk.delete();
        }
        assert_eq!(desk.text(), "aef");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "ab\u{00E9}def");
        assert_eq!(replay.changes.len(), 1);
        assert_eq!(replay.changes[0].inserted, "b\u{00E9}d");
        assert!(desk.undo().is_none());
    }

    #[test]
    fn backspace_and_forward_delete_mix_in_one_step() {
        let mut desk = Desk::new("abcdef");
        desk.cursor = Cursor::at(p(0, 3));
        desk.backspace();
        desk.delete();
        desk.backspace();
        assert_eq!(desk.text(), "aef");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "abcdef");
        assert_eq!(replay.changes.len(), 1);
        assert_eq!(replay.changes[0].inserted, "bcd");
        assert!(desk.undo().is_none());
        assert!(desk.redo().is_some());
        assert_eq!(desk.text(), "aef");
    }

    #[test]
    fn a_run_of_backspaces_keeps_its_order_when_another_change_joins_the_step() {
        let mut desk = Desk::new("abcd\u{00E9}n\u{0313}");
        for _ in 0..4 {
            desk.backspace();
        }
        assert_eq!(desk.text(), "abc");
        // A deletion that leaves something behind carries the run on, but
        // is not one replace with it.
        desk.now += 40;
        desk.edit(Span::new(p(0, 2), p(0, 3)), "X", EditKind::Delete);
        assert_eq!(desk.text(), "abX");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "abcd\u{00E9}n\u{0313}");
        assert_eq!(replay.changes.len(), 2);
        assert_eq!(replay.changes[1].inserted, "d\u{00E9}n\u{0313}");
        assert!(desk.undo().is_none());
        assert!(desk.redo().is_some());
        assert_eq!(desk.text(), "abX");
        assert_eq!(desk.undo_all(), ["abcd\u{00E9}n\u{0313}"]);
    }

    #[test]
    fn a_long_run_of_deleting_both_ways_is_one_change_in_the_order_it_stood() {
        let text: String = ('a'..='z').chain('\u{00E0}'..='\u{00FF}').cycle().take(4000).collect();
        let mut desk = Desk::new(&text);
        desk.cursor = Cursor::at(desk.doc.pos_at(3000));
        for _ in 0..900 {
            desk.backspace();
        }
        for _ in 0..500 {
            desk.delete();
        }
        for _ in 0..900 {
            desk.backspace();
        }
        let left = desk.text();
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), text);
        assert_eq!(replay.changes.len(), 1);
        assert_eq!(left.len() + replay.changes[0].inserted.len(), text.len());
        assert!(desk.undo().is_none());
        assert!(desk.redo().is_some());
        assert_eq!(desk.text(), left);
    }

    #[test]
    fn joining_lines_is_its_own_step() {
        let mut desk = Desk::new("ab\ncd");
        desk.cursor = Cursor::at(p(1, 1));
        desk.backspace();
        desk.backspace();
        desk.backspace();
        assert_eq!(desk.text(), "ad");
        assert_eq!(desk.undo_all(), ["abd", "ab\nd", "ab\ncd"]);
    }

    #[test]
    fn typing_and_deleting_do_not_share_a_step() {
        let mut desk = Desk::new("");
        desk.type_text("abc");
        desk.backspace();
        desk.type_text("d");
        assert_eq!(desk.undo_all(), ["ab", "abc", ""]);
    }

    #[test]
    fn other_edits_never_merge() {
        let mut desk = Desk::new("abc");
        desk.edit(Span::at(p(0, 3)), "d", EditKind::Other);
        desk.edit(Span::at(p(0, 4)), "e", EditKind::Other);
        assert_eq!(desk.undo_all(), ["abcd", "abc"]);
    }

    #[test]
    fn a_group_is_one_step() {
        let mut desk = Desk::new("one two three");
        desk.history.begin_group();
        desk.edit(Span::new(p(0, 8), p(0, 13)), "3", EditKind::Other);
        desk.history.seal();
        desk.now += 5000;
        desk.edit(Span::new(p(0, 4), p(0, 7)), "2\n", EditKind::Insert);
        desk.edit(Span::new(p(0, 0), p(0, 3)), "", EditKind::Delete);
        desk.history.end_group();
        assert_eq!(desk.text(), " 2\n 3");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "one two three");
        assert_eq!(replay.changes.len(), 3);
        assert_eq!(replay.cursor, Cursor::at(p(0, 13)));
        assert!(desk.undo().is_none());
        let replay = desk.redo().unwrap();
        assert_eq!(desk.text(), " 2\n 3");
        assert_eq!(replay.changes.len(), 3);
        assert_eq!(replay.cursor, Cursor::at(p(0, 0)));
    }

    #[test]
    fn groups_nest_and_keep_apart_from_their_neighbours() {
        let mut desk = Desk::new("");
        desk.type_text("a");
        desk.history.begin_group();
        desk.type_text("b");
        desk.history.begin_group();
        desk.type_text("c");
        desk.history.end_group();
        desk.type_text(" d");
        desk.history.end_group();
        desk.type_text("e");
        assert_eq!(desk.text(), "abc de");
        assert_eq!(desk.undo_all(), ["abc d", "a", ""]);
        // An empty group leaves nothing behind, and a stray end is harmless.
        desk.history.begin_group();
        desk.history.end_group();
        desk.history.end_group();
        assert!(!desk.history.can_undo());
        desk.type_text("xy");
        assert_eq!(desk.undo_all(), [""]);
    }

    #[test]
    fn undo_and_redo_give_back_the_cursor() {
        let mut desk = Desk::new("hello world");
        let selection = Cursor { anchor: p(0, 0), head: p(0, 5) };
        desk.cursor = selection;
        desk.now = 40;
        desk.edit(selection.span(), "X", EditKind::Insert);
        desk.type_text("Y");
        assert_eq!(desk.text(), "XY world");
        let replay = desk.undo().unwrap();
        assert_eq!(desk.text(), "hello world");
        assert_eq!(replay.cursor, selection);
        let replay = desk.redo().unwrap();
        assert_eq!(replay.cursor, Cursor::at(p(0, 2)));
    }

    #[test]
    fn recording_clears_redo_but_a_noop_does_not() {
        let mut desk = Desk::new("");
        desk.type_text("one two");
        assert!(desk.undo().is_some());
        assert!(desk.history.can_redo());
        // A backspace at the start of the document changes nothing.
        desk.cursor = Cursor::at(p(0, 0));
        desk.backspace();
        assert!(desk.history.can_redo());
        desk.cursor = Cursor::at(p(0, 4));
        desk.type_text("x");
        assert!(!desk.history.can_redo());
        assert!(desk.redo().is_none());
        assert_eq!(desk.undo_all(), ["one ", ""]);
    }

    #[test]
    fn a_noop_does_not_break_a_run() {
        let mut desk = Desk::new("abc");
        desk.cursor = Cursor::at(p(0, 2));
        desk.backspace();
        desk.backspace();
        desk.backspace();
        desk.delete();
        assert_eq!(desk.text(), "");
        assert_eq!(desk.undo_all(), ["abc"]);
    }

    #[test]
    fn undo_closes_the_step_it_leaves_on_top() {
        let mut desk = Desk::new("");
        desk.type_text("ab");
        desk.history.seal();
        desk.type_text("c");
        assert!(desk.undo().is_some());
        desk.type_text("d");
        assert_eq!(desk.undo_all(), ["ab", ""]);
    }

    #[test]
    fn the_oldest_steps_are_dropped_at_the_limit() {
        let mut desk = Desk::new("");
        desk.history = History::with_limit(2);
        for word in ["one ", "two ", "three ", "four"] {
            desk.type_text(word);
        }
        assert_eq!(desk.undo_all(), ["one two three ", "one two "]);
        assert!(desk.redo().is_some() && desk.redo().is_some() && desk.redo().is_none());
        assert_eq!(desk.text(), "one two three four");

        let mut desk = Desk::new("");
        desk.history = History::with_limit(0);
        desk.type_text("gone for good");
        desk.history.begin_group();
        desk.type_text("!");
        desk.history.end_group();
        assert!(!desk.history.can_undo() && desk.undo().is_none());
    }

    #[test]
    fn clear_forgets_everything() {
        let mut desk = Desk::new("");
        desk.type_text("one two");
        assert!(desk.undo().is_some());
        desk.history.clear();
        assert!(!desk.history.can_undo() && !desk.history.can_redo());
        assert!(desk.undo().is_none() && desk.redo().is_none());
        // The next change does not join a step that is no longer there.
        desk.type_text("x");
        assert_eq!(desk.undo_all(), ["one "]);
    }

    #[test]
    fn made_up_changes_cannot_panic() {
        let mut doc = Doc::from_text("caf\u{00E9}\nb");
        let mut history = History::new();
        let wild = Change {
            start: p(9, 9),
            old_end: p(0, 4),
            end: p(usize::MAX, usize::MAX),
            removed: String::from("zz\nz"),
            inserted: String::from("\u{00E9}"),
        };
        let kinds = [EditKind::Insert, EditKind::Delete, EditKind::Other, EditKind::Delete];
        for (n, kind) in kinds.into_iter().enumerate() {
            let mut change = wild.clone();
            change.removed.truncate(n.min(2));
            history.record(change, kind, Cursor::default(), Cursor::at(p(5, 5)), u64::MAX - n as u64);
        }
        // Two deletes that merge, with an end column that cannot be added to.
        for removed in ["x", "y"] {
            let change = Change { old_end: p(0, usize::MAX), removed: removed.into(), ..Change::default() };
            history.record(change, EditKind::Delete, Cursor::default(), Cursor::default(), 0);
        }
        let mut steps = 0;
        while history.undo(&mut doc).is_some() {
            steps += 1;
        }
        assert_eq!(steps, 5);
        while history.redo(&mut doc).is_some() {}
        assert!(history.can_undo() && !history.can_redo());
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as usize % n.max(1)
        }
    }

    fn random_pos(rng: &mut Rng, doc: &Doc) -> Pos {
        doc.pos_at(rng.below(doc.len_bytes() + 1))
    }

    /// Random sessions of typing, deleting, pasting, grouping, sealing,
    /// waiting, moving, undoing and redoing. However the steps fall, undoing
    /// everything must give back the first text, redoing everything the last,
    /// and each redo must land on the very text its undo left from.
    #[test]
    fn random_sessions_undo_to_the_start_and_redo_to_the_end() {
        let pieces =
            ["a", "b", " ", "  ", "\u{00E9}", "n\u{0313}", "\n", "word", "two words ", "x\ny", "\u{4E00}", ". "];
        let kinds = [EditKind::Insert, EditKind::Delete, EditKind::Other];
        let first = "The quick brown fox.\n\nJumps over\nthe lazy dog.";
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..80 {
            let mut desk = Desk::new(first);
            for _ in 0..250 {
                desk.now += [0, 10, 300, 1200][rng.below(4)];
                match rng.below(20) {
                    0 => desk.history.seal(),
                    1 => desk.history.begin_group(),
                    2 => desk.history.end_group(),
                    3 => desk.cursor = Cursor::at(random_pos(&mut rng, &desk.doc)),
                    4 => {
                        desk.undo();
                    }
                    5 => {
                        desk.redo();
                    }
                    6..=8 => desk.backspace(),
                    9 | 10 => desk.delete(),
                    11 | 12 => {
                        let span = Span::new(random_pos(&mut rng, &desk.doc), random_pos(&mut rng, &desk.doc));
                        let (text, kind) = (pieces[rng.below(pieces.len())], kinds[rng.below(kinds.len())]);
                        desk.edit(span, text, kind);
                    }
                    13 => {
                        let head = desk.cursor.head;
                        let (text, kind) = (pieces[rng.below(pieces.len())], kinds[rng.below(kinds.len())]);
                        desk.edit(Span::at(head), text, kind);
                    }
                    _ => desk.type_text(pieces[rng.below(pieces.len())]),
                }
            }
            // The session may have stopped part of the way back through its
            // own undos; the last text is the one with nothing left to redo.
            while desk.redo().is_some() {}
            let last = desk.text();
            let mut texts = vec![last.clone()];
            texts.extend(desk.undo_all());
            assert_eq!(desk.text(), first);
            assert!(!desk.history.can_undo());
            for text in texts.iter().rev().skip(1) {
                assert!(desk.redo().is_some());
                assert_eq!(&desk.text(), text);
            }
            assert!(desk.redo().is_none());
            assert_eq!(desk.text(), last);
            assert_eq!(desk.shadow.to_text("\n"), last);
        }
    }
}
