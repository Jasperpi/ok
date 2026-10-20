//! Counting what has been written: words, characters and sentences.

use crate::doc::{Change, Doc};
use crate::motion::sentence_ranges;
use crate::width::is_combining;

/// Words in `s`: the stretches between whitespace that hold at least one
/// letter or digit. So `well-known` is one word, a lone dash or `#` is none,
/// and markup stuck to a word does not add to the count.
pub fn words(s: &str) -> usize {
    s.split_whitespace().filter(|token| token.chars().any(char::is_alphanumeric)).count()
}

/// Characters as a reader counts them: a combining mark rides on the
/// character before it, and line breaks are not counted.
pub fn clusters(s: &str) -> usize {
    s.chars().filter(|&ch| ch != '\n' && ch != '\r' && !is_combining(ch)).count()
}

/// Sentences in `s`, by the rule the sentence motions use, taken a line at a
/// time: a line with something on it but no terminator still counts as one.
pub fn sentences(s: &str) -> usize {
    s.split(['\n', '\r']).map(|line| sentence_ranges(line).count()).sum()
}

/// A running word count for a whole document.
///
/// It keeps one count per line, so an edit costs a recount of the lines it
/// touched and nothing more.
#[derive(Clone, Debug)]
pub struct Tally {
    lines: Vec<usize>,
    total: usize,
    /// The document's revision when every line was last counted afresh.
    counted: u64,
}

impl Tally {
    /// Counts the whole document.
    pub fn of(doc: &Doc) -> Tally {
        let lines: Vec<usize> = doc.lines().iter().map(|line| words(line)).collect();
        let total = lines.iter().sum();
        Tally { lines, total, counted: doc.revision() }
    }

    /// Brings the count up to date after `change`. `doc` is the document as
    /// the change left it.
    ///
    /// A change that does not fit the table (one made to some other document,
    /// say) is answered with a full recount, so the tally cannot drift.
    ///
    /// The changes of an undo or redo can be handed over one after another
    /// with the document as the last of them left it. That may cost one full
    /// recount but no more: a change that arrives when every line has
    /// already been counted from this very revision of the document has
    /// nothing to add, and is passed over.
    pub fn apply(&mut self, doc: &Doc, change: &Change) {
        if self.counted == doc.revision() && self.lines.len() == doc.line_count() {
            return;
        }
        let fits = change.start <= change.old_end
            && change.start <= change.end
            && change.old_end.line < self.lines.len()
            && change.end.line < doc.line_count();
        if !fits {
            *self = Tally::of(doc);
            return;
        }
        let (first, old, new) = change.lines();
        let fresh: Vec<usize> = (first..first + new).map(|line| words(doc.line(line))).collect();
        let added: usize = fresh.iter().sum();
        let gone: usize = self.lines.splice(first..first + old, fresh).sum();
        self.total = self.total - gone + added;
        if self.lines.len() != doc.line_count() {
            *self = Tally::of(doc);
        }
    }

    /// Words in the whole document.
    pub fn words(&self) -> usize {
        self.total
    }

    /// Words on one line; 0 past the end.
    pub fn line_words(&self, line: usize) -> usize {
        self.lines.get(line).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Pos, Span};

    #[test]
    fn words_are_tokens_with_a_letter_or_digit_in_them() {
        assert_eq!(words(""), 0);
        assert_eq!(words("   \n\t "), 0);
        assert_eq!(words("well-known"), 1);
        assert_eq!(words("-"), 0);
        assert_eq!(words("#"), 0);
        assert_eq!(words("a \u{2014} b"), 2);
        assert_eq!(words("**bold**"), 1);
        assert_eq!(words("[[two words]]"), 2);
        assert_eq!(words("# Heading one\n\n- item 2\n- ...\n"), 4);
        assert_eq!(words("mitsqanaqan\u{0313} \u{4E00}\u{4E8C} caf\u{00E9}"), 3);
        assert_eq!(words("one\u{00A0}two\u{3000}three"), 3);
    }

    #[test]
    fn clusters_skip_marks_and_line_breaks() {
        assert_eq!(clusters(""), 0);
        assert_eq!(clusters("mitsqanaqan\u{0313}"), 11);
        assert_eq!(clusters("a\nb\r\nc"), 3);
        assert_eq!(clusters("\u{2764}\u{FE0F} \u{4E00}"), 3);
        assert_eq!(clusters("\u{0301}"), 0);
    }

    #[test]
    fn sentences_are_counted_line_by_line() {
        assert_eq!(sentences(""), 0);
        assert_eq!(sentences("  \n\n\t"), 0);
        assert_eq!(sentences("No terminator"), 1);
        assert_eq!(sentences("Hello there. How are you? Fine... thanks!"), 3);
        assert_eq!(sentences("See e.g. the docs. Then stop."), 2);
        assert_eq!(sentences("# Title\n\nOne. Two.\r\nThree"), 4);
        assert_eq!(sentences("She said \"Go.\" Then left."), 2);
        assert_eq!(sentences("Trailing space.   "), 1);
    }

    #[test]
    fn tally_counts_lines_and_follows_edits() {
        let mut d = Doc::from_text("one two\n\n- three\n# \nfour");
        let mut tally = Tally::of(&d);
        assert_eq!(tally.words(), 4);
        assert_eq!((tally.line_words(0), tally.line_words(1), tally.line_words(2)), (2, 0, 1));
        assert_eq!(tally.line_words(99), 0);

        // Splitting a word in two, then joining three lines into one.
        let change = d.replace(Span::at(Pos::new(0, 2)), " ");
        tally.apply(&d, &change);
        assert_eq!((tally.words(), tally.line_words(0)), (5, 3));
        let change = d.replace(Span::new(Pos::new(0, 8), Pos::new(2, 2)), "");
        tally.apply(&d, &change);
        assert_eq!(d.line(0), "on e twothree");
        assert_eq!((tally.words(), tally.line_words(0), tally.line_words(1)), (4, 3, 0));

        let undone = d.revert(&change);
        tally.apply(&d, &undone);
        assert_eq!((tally.words(), tally.line_words(2)), (5, 1));
    }

    #[test]
    fn a_change_that_does_not_fit_gets_a_full_recount() {
        let d = Doc::from_text("one\ntwo three");
        let mut tally = Tally::of(&Doc::new());
        let stray = Change {
            start: Pos::new(7, 0),
            old_end: Pos::new(3, 0),
            end: Pos::new(usize::MAX, 0),
            removed: String::from("x"),
            inserted: String::new(),
        };
        tally.apply(&d, &stray);
        assert_eq!((tally.words(), tally.line_words(1)), (3, 2));
        // In range, but not the change that was made: the line counts disagree.
        let mut d = d;
        d.replace(Span::at(Pos::new(0, 3)), " and");
        let other = Change { end: Pos::new(1, 0), inserted: String::from("\n"), ..Change::default() };
        tally.apply(&d, &other);
        assert_eq!((tally.words(), tally.line_words(0), tally.line_words(2)), (4, 2, 0));
    }

    #[test]
    fn the_changes_of_a_replay_can_all_be_applied_to_the_document_as_it_ends_up() {
        let mut d = Doc::from_text("one two\nthree\nfour five six");
        let mut tally = Tally::of(&d);
        // Three changes before the tally hears of any, as an undo makes
        // them: two lines joined, a line split in three, a word added.
        let changes = [
            d.replace(Span::new(Pos::new(0, 7), Pos::new(1, 0)), " "),
            d.replace(Span::at(Pos::new(1, 4)), "\n\n"),
            d.replace(Span::at(Pos::new(0, 0)), "zero "),
        ];
        assert_eq!(d.lines(), &["zero one two three", "four", "", " five six"]);
        for change in &changes {
            tally.apply(&d, change);
        }
        assert_eq!(tally.words(), 7);
        for (i, text) in d.lines().iter().enumerate() {
            assert_eq!(tally.line_words(i), words(text));
        }
        // The first change brought on a full recount; the others found it
        // already made from this revision and had nothing to add.
        assert_eq!(tally.counted, d.revision());
        // The next real change is followed line by line as usual.
        let change = d.replace(Span::new(Pos::new(3, 0), Pos::new(3, 5)), "");
        tally.apply(&d, &change);
        assert_eq!((tally.words(), tally.line_words(3)), (6, 1));
        assert_ne!(tally.counted, d.revision());
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

    /// After any run of edits and reverts the running count equals a count of
    /// the whole text, line by line as well as in total.
    #[test]
    fn random_edits_keep_the_tally_equal_to_a_full_count() {
        let pieces = [
            "",
            "a",
            " ",
            "word ",
            "\n",
            "\n\n",
            "x\ny",
            "-",
            "# ",
            "**bold**",
            "well-known",
            "\u{00E9}",
            "n\u{0313}",
            "\u{4E00}",
            "\t",
            "\u{00A0}",
            "1",
            " - ",
            "[[two words]]\n",
        ];
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let mut d = Doc::new();
        let mut tally = Tally::of(&d);
        let mut made: Vec<Change> = Vec::new();
        for turn in 0..4000 {
            let change = if turn % 7 == 6 && !made.is_empty() {
                let last = made.pop().unwrap_or_default();
                d.revert(&last)
            } else {
                made.clear();
                let a = rng.below(d.len_bytes() + 1);
                let b = a + rng.below(9);
                let change = d.replace(Span::new(d.pos_at(a), d.pos_at(b)), pieces[rng.below(pieces.len())]);
                made.push(change.clone());
                change
            };
            tally.apply(&d, &change);
            assert_eq!(tally.words(), words(&d.to_text("\n")));
            for (i, text) in d.lines().iter().enumerate() {
                assert_eq!(tally.line_words(i), words(text));
            }
            assert_eq!(tally.line_words(d.line_count()), 0);
        }
    }
}
