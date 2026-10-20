use std::borrow::Cow;

/// A place in a document: the line, and a byte offset into that line.
///
/// `col` always sits on a character boundary. Ordering is document order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }
}

/// A stretch of a document from `start` up to but not including `end`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    /// Builds a span from two positions in either order.
    pub fn new(a: Pos, b: Pos) -> Span {
        if a <= b {
            Span { start: a, end: b }
        } else {
            Span { start: b, end: a }
        }
    }

    pub fn at(p: Pos) -> Span {
        Span { start: p, end: p }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, p: Pos) -> bool {
        self.start <= p && p < self.end
    }
}

/// What one call to [`Doc::replace`] did.
///
/// Before the change, the text from `start` to `old_end` read `removed`.
/// After it, the text from `start` to `end` reads `inserted`. Line breaks in
/// both strings are a single `\n`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Change {
    pub start: Pos,
    pub old_end: Pos,
    pub end: Pos,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    pub fn is_noop(&self) -> bool {
        self.removed.is_empty() && self.inserted.is_empty()
    }

    /// The lines this change touched: the first line, how many lines there
    /// were from it before the change, and how many there are after. A table
    /// holding one entry per line stays in step by replacing `old` entries
    /// starting at `first` with `new` fresh ones; see [`Change::splice`].
    pub fn lines(&self) -> (usize, usize, usize) {
        (self.start.line, self.old_end.line - self.start.line + 1, self.end.line - self.start.line + 1)
    }

    /// Keeps a one-entry-per-line table in step with the document.
    pub fn splice<T>(&self, table: &mut Vec<T>, mut fresh: impl FnMut() -> T) {
        let (first, old, new) = self.lines();
        let first = first.min(table.len());
        let stop = (first + old).min(table.len());
        table.splice(first..stop, (0..new).map(|_| fresh()));
    }
}

/// Turns `\r\n` and lone `\r` into `\n`. Borrowed when nothing needs changing.
pub fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// The text being written: one `String` per line, never fewer than one line,
/// and no line ever contains `\n` or `\r`.
#[derive(Clone, Debug)]
pub struct Doc {
    lines: Vec<String>,
    revision: u64,
}

impl Default for Doc {
    fn default() -> Doc {
        Doc::new()
    }
}

impl Doc {
    /// An empty document: a single empty line.
    pub fn new() -> Doc {
        Doc { lines: vec![String::new()], revision: 0 }
    }

    /// Reads text with any mix of `\n`, `\r\n` and `\r` line breaks. A leading
    /// byte-order mark is dropped.
    pub fn from_text(text: &str) -> Doc {
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let text = normalize_newlines(text);
        Doc { lines: text.split('\n').map(str::to_string).collect(), revision: 0 }
    }

    /// The whole text with lines joined by `eol`.
    pub fn to_text(&self, eol: &str) -> String {
        let mut out = String::with_capacity(self.len_bytes() + self.lines.len() * eol.len().saturating_sub(1));
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push_str(eol);
            }
            out.push_str(line);
        }
        out
    }

    /// Always at least 1.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The text of line `i`, or the empty string past the end.
    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map(String::as_str).unwrap_or("")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Size in bytes with each line break counted as one byte.
    pub fn len_bytes(&self) -> usize {
        self.lines.iter().map(String::len).sum::<usize>() + self.lines.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Goes up by one each time the text really changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The position after the last character.
    pub fn end(&self) -> Pos {
        let line = self.lines.len() - 1;
        Pos { line, col: self.lines[line].len() }
    }

    /// Brings any position into the document and back onto a character boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        if p.line >= self.lines.len() {
            return self.end();
        }
        let text = &self.lines[p.line];
        let mut col = p.col.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        Pos { line: p.line, col }
    }

    /// The text of `span`, with line breaks as `\n`.
    pub fn slice(&self, span: Span) -> String {
        let span = Span::new(self.clamp(span.start), self.clamp(span.end));
        let (a, b) = (span.start, span.end);
        if a.line == b.line {
            return self.lines[a.line][a.col..b.col].to_string();
        }
        let mut out = String::from(&self.lines[a.line][a.col..]);
        for line in &self.lines[a.line + 1..b.line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.lines[b.line][..b.col]);
        out
    }

    /// The one editing primitive: replaces `span` with `text`.
    ///
    /// Typing is a replace of an empty span; deleting is a replace with empty
    /// text. Positions out of range are clamped first. Line breaks in `text`
    /// may be `\n`, `\r\n` or `\r`.
    pub fn replace(&mut self, span: Span, text: &str) -> Change {
        let span = Span::new(self.clamp(span.start), self.clamp(span.end));
        let (start, old_end) = (span.start, span.end);
        let removed = self.slice(span);
        let inserted = normalize_newlines(text).into_owned();

        let tail = self.lines[old_end.line][old_end.col..].to_string();
        self.lines[start.line].truncate(start.col);
        self.lines.drain(start.line + 1..=old_end.line);

        let mut parts = inserted.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[start.line].push_str(first);
        let mut end = Pos { line: start.line, col: start.col + first.len() };
        let added: Vec<String> = parts.map(str::to_string).collect();
        if let Some(last) = added.last() {
            end = Pos { line: start.line + added.len(), col: last.len() };
        }
        self.lines.splice(start.line + 1..start.line + 1, added);
        self.lines[end.line].push_str(&tail);

        if !removed.is_empty() || !inserted.is_empty() {
            self.revision += 1;
        }
        Change { start, old_end, end, removed, inserted }
    }

    /// Puts back what `change` took out. The document must be in the state
    /// the change left it in.
    pub fn revert(&mut self, change: &Change) -> Change {
        self.replace(Span { start: change.start, end: change.end }, &change.removed)
    }

    /// Makes `change` again after a [`Doc::revert`].
    pub fn reapply(&mut self, change: &Change) -> Change {
        self.replace(Span { start: change.start, end: change.old_end }, &change.inserted)
    }

    /// Distance of `p` from the start of the document in bytes, counting each
    /// line break as one. Walks the lines, so keep it out of per-key paths.
    pub fn offset_of(&self, p: Pos) -> usize {
        let p = self.clamp(p);
        self.lines[..p.line].iter().map(|l| l.len() + 1).sum::<usize>() + p.col
    }

    /// The inverse of [`Doc::offset_of`]. Offsets past the end give the end.
    pub fn pos_at(&self, mut offset: usize) -> Pos {
        for (line, text) in self.lines.iter().enumerate() {
            if offset <= text.len() {
                return self.clamp(Pos { line, col: offset });
            }
            offset -= text.len() + 1;
        }
        self.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn reads_every_kind_of_line_break_and_drops_the_bom() {
        let d = Doc::from_text("\u{FEFF}one\r\ntwo\rthree\nfour");
        assert_eq!(d.lines(), &["one", "two", "three", "four"]);
        assert_eq!(d.to_text("\n"), "one\ntwo\nthree\nfour");
        assert_eq!(d.to_text("\r\n"), "one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(Doc::from_text("").line_count(), 1);
        assert_eq!(Doc::from_text("a\n").lines(), &["a", ""]);
        assert!(Doc::new().is_empty());
    }

    #[test]
    fn typing_and_deleting_within_a_line() {
        let mut d = Doc::from_text("helo");
        let c = d.replace(Span::at(p(0, 3)), "l");
        assert_eq!(d.to_text("\n"), "hello");
        assert_eq!((c.start, c.old_end, c.end), (p(0, 3), p(0, 3), p(0, 4)));
        assert_eq!(c.lines(), (0, 1, 1));
        let c = d.replace(Span::new(p(0, 5), p(0, 1)), "");
        assert_eq!(d.to_text("\n"), "h");
        assert_eq!(c.removed, "ello");
        assert_eq!(d.revision(), 2);
    }

    #[test]
    fn splitting_and_joining_lines() {
        let mut d = Doc::from_text("alpha beta");
        let c = d.replace(Span::at(p(0, 5)), "\n");
        assert_eq!(d.lines(), &["alpha", " beta"]);
        assert_eq!(c.end, p(1, 0));
        assert_eq!(c.lines(), (0, 1, 2));
        let c = d.replace(Span::new(p(0, 5), p(1, 1)), " ");
        assert_eq!(d.lines(), &["alpha beta"]);
        assert_eq!(c.removed, "\n ");
        assert_eq!(c.lines(), (0, 2, 1));
    }

    #[test]
    fn replacing_across_lines_with_several_lines() {
        let mut d = Doc::from_text("one\ntwo\nthree\nfour");
        let c = d.replace(Span::new(p(0, 2), p(2, 3)), "X\r\nY\rZ");
        assert_eq!(d.lines(), &["onX", "Y", "Zee", "four"]);
        assert_eq!(c.removed, "e\ntwo\nthr");
        assert_eq!(c.inserted, "X\nY\nZ");
        assert_eq!(c.end, p(2, 1));
        assert_eq!(c.lines(), (0, 3, 3));
    }

    #[test]
    fn revert_and_reapply_are_exact() {
        let before = "The quick\nbrown fox\njumps";
        let mut d = Doc::from_text(before);
        let c = d.replace(Span::new(p(0, 4), p(1, 5)), "slow\n\ngrey");
        let after = d.to_text("\n");
        d.revert(&c);
        assert_eq!(d.to_text("\n"), before);
        d.reapply(&c);
        assert_eq!(d.to_text("\n"), after);
    }

    #[test]
    fn positions_are_clamped_onto_character_boundaries() {
        let mut d = Doc::from_text("caf\u{00E9}\nni\u{00F1}o");
        assert_eq!(d.clamp(p(0, 4)), p(0, 3)); // inside the two-byte é
        assert_eq!(d.clamp(p(0, 99)), p(0, 5));
        assert_eq!(d.clamp(p(9, 0)), p(1, 5));
        // A replace aimed inside a character must not split it.
        d.replace(Span::new(p(1, 3), p(1, 3)), "-");
        assert_eq!(d.line(1), "ni-\u{00F1}o");
        assert_eq!(d.line(7), "");
    }

    #[test]
    fn noop_replace_does_not_bump_the_revision() {
        let mut d = Doc::from_text("abc");
        let c = d.replace(Span::at(p(0, 1)), "");
        assert!(c.is_noop());
        assert_eq!(d.revision(), 0);
    }

    #[test]
    fn offsets_round_trip() {
        let d = Doc::from_text("ab\n\ncd\u{00E9}");
        assert_eq!(d.len_bytes(), 8);
        for off in 0..=d.len_bytes() {
            let pos = d.pos_at(off);
            if d.to_text("\n").is_char_boundary(off) {
                assert_eq!(d.offset_of(pos), off);
            }
        }
        assert_eq!(d.pos_at(999), d.end());
        assert_eq!(d.slice(Span::new(p(0, 1), p(2, 1))), "b\n\nc");
    }

    #[test]
    fn splice_keeps_a_side_table_in_step() {
        let mut d = Doc::from_text("a\nb\nc\nd");
        let mut table: Vec<u32> = vec![10, 20, 30, 40];
        let c = d.replace(Span::new(p(1, 0), p(2, 1)), "x\ny\nz");
        c.splice(&mut table, || 0);
        assert_eq!(d.lines(), &["a", "x", "y", "z", "d"]);
        assert_eq!(table, vec![10, 0, 0, 0, 40]);
        assert_eq!(table.len(), d.line_count());
    }

    /// Random edits checked against a plain `String` doing the same thing.
    #[test]
    fn random_edits_match_a_reference_string() {
        let mut seed = 0x2545_F491u32;
        let mut rnd = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize % n.max(1)
        };
        let pieces = ["", "a", "\u{00E9}", "word ", "\n", "\n\n", "x\ny", "n\u{0313}", "\u{4E00}", "end\n"];
        let mut d = Doc::new();
        let mut reference = String::new();
        let mut undo: Vec<Change> = Vec::new();
        for _ in 0..3000 {
            let len = reference.len();
            let mut a = rnd(len + 1);
            let mut b = (a + rnd(6)).min(len);
            while !reference.is_char_boundary(a) {
                a -= 1;
            }
            while !reference.is_char_boundary(b) {
                b -= 1;
            }
            let text = pieces[rnd(pieces.len())];
            let c = d.replace(Span::new(d.pos_at(a), d.pos_at(b)), text);
            assert_eq!(c.removed, &reference[a..b]);
            reference.replace_range(a..b, text);
            assert_eq!(d.to_text("\n"), reference);
            assert_eq!(d.slice(Span { start: c.start, end: c.end }), c.inserted);
            assert_eq!(d.len_bytes(), reference.len());
            undo.push(c);
        }
        for c in undo.iter().rev() {
            d.revert(c);
        }
        assert!(d.is_empty());
    }
}
