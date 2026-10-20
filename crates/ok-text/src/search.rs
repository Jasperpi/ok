//! Finding text. The search is literal and works a line at a time, so a
//! match never runs across a line break.

use crate::doc::{Doc, Pos, Span};

/// What is being looked for, made ready once so every line can reuse it.
enum Needle<'a> {
    Exact(&'a str),
    Folded(Folded),
}

impl<'a> Needle<'a> {
    /// `None` for a needle that can match nothing: an empty one, or one with
    /// a line break in it.
    fn new(needle: &'a str, case_sensitive: bool) -> Option<Needle<'a>> {
        if needle.is_empty() || needle.contains(['\n', '\r']) {
            return None;
        }
        Some(if case_sensitive { Needle::Exact(needle) } else { Needle::Folded(Folded::new(needle)) })
    }

    /// The first match in `line` that starts at or after byte `from`, as a
    /// byte range of the line as written.
    fn find(&mut self, line: &str, from: usize) -> Option<(usize, usize)> {
        let rest = line.get(from..)?;
        match self {
            // `str::find` reads the whole needle before it looks at the
            // text, which a long needle would pay again on every short line.
            Needle::Exact(needle) if needle.len() > rest.len() => None,
            Needle::Exact(needle) => rest.find(*needle).map(|i| (from + i, from + i + needle.len())),
            Needle::Folded(folded) => folded.find(rest).map(|(start, end)| (from + start, from + end)),
        }
    }

    /// Every match in `line`, left to right, each starting where the one
    /// before it ended or later.
    fn matches<'s>(&'s mut self, line: &'s str) -> impl Iterator<Item = (usize, usize)> + use<'s, 'a> {
        let mut from = 0;
        std::iter::from_fn(move || {
            let (start, end) = self.find(line, from)?;
            from = end;
            Some((start, end))
        })
    }
}

/// A needle for searching without case.
///
/// Lowercasing can turn one character into several, and of a different size
/// in bytes, so the text is never lowercased as a whole: a search walks it as
/// written, lowercasing a character at a time, and the range it gives is
/// always measured in the original. A match has to use up whole characters:
/// `i` alone does not match U+0130, which lowercases to `i` and a combining
/// dot.
struct Folded {
    /// The needle lowercased one character at a time.
    want: Vec<char>,
    /// For each length of `want` matched so far, the longest shorter start
    /// of `want` that the text just read also ends with. A mismatch falls
    /// back to that instead of going back over the text, so a search is one
    /// pass of the line however much the text and the needle repeat
    /// themselves.
    fallback: Vec<usize>,
    /// Working space for a search. For each of the last `want.len()`
    /// lowercased characters read since the text last matched nothing: the
    /// byte where the character it came from begins, or `usize::MAX` when it
    /// is not the first to come from that character, which no match can
    /// begin with.
    heads: Vec<usize>,
}

impl Folded {
    fn new(needle: &str) -> Folded {
        let want: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
        let mut fallback = vec![0; want.len()];
        let mut matched = 0;
        for at in 1..want.len() {
            while matched > 0 && want[at] != want[matched] {
                matched = fallback[matched - 1];
            }
            if want[at] == want[matched] {
                matched += 1;
            }
            fallback[at] = matched;
        }
        let heads = vec![usize::MAX; want.len()];
        Folded { want, fallback, heads }
    }

    /// The first stretch of whole characters in `text` that lowercases to
    /// exactly `want`, as a byte range of `text`.
    fn find(&mut self, text: &str) -> Option<(usize, usize)> {
        let size = self.want.len();
        let first = *self.want.first()?;
        // `matched` is how much of `want` the lowercased text read so far
        // ends with, and is below `size` wherever it is used as an index;
        // `slot` is where in `heads` the next lowercased character goes.
        let (mut matched, mut slot) = (0, 0);
        for (i, ch) in text.char_indices() {
            if matched == 0 {
                // Most characters come while nothing is matched and cannot
                // begin a match, which for ASCII needs no table to find out.
                if ch.is_ascii() && ch.to_ascii_lowercase() != first {
                    continue;
                }
                // Whatever matches from here on began no earlier than this.
                slot = 0;
            }
            let mut head = i;
            for lower in ch.to_lowercase() {
                if matched == size {
                    // A match that began or ended part of the way through
                    // a character and so was no match: carry on from what
                    // is left of it.
                    matched = self.fallback[size - 1];
                }
                while matched > 0 && self.want[matched] != lower {
                    matched = self.fallback[matched - 1];
                }
                if self.want[matched] == lower {
                    matched += 1;
                }
                self.heads[slot] = head;
                head = usize::MAX;
                slot = if slot + 1 == size { 0 } else { slot + 1 };
            }
            // `slot` has come round to the lowercased character `size` back,
            // the one a match ending here began with.
            if matched == size && self.heads[slot] != usize::MAX {
                return Some((self.heads[slot], i + ch.len_utf8()));
            }
        }
        None
    }
}

fn span_on(line: usize, (start, end): (usize, usize)) -> Span {
    Span { start: Pos::new(line, start), end: Pos::new(line, end) }
}

/// Every match of `needle`, in document order. Matches do not overlap: the
/// search for the next one starts where the last one ended.
///
/// An empty needle matches nothing, and neither does one containing a line
/// break. Without `case_sensitive`, two stretches of text match when they are
/// equal after [`char::to_lowercase`] on each character.
pub fn find_all(doc: &Doc, needle: &str, case_sensitive: bool) -> Vec<Span> {
    let Some(mut needle) = Needle::new(needle, case_sensitive) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (line, text) in doc.lines().iter().enumerate() {
        found.extend(needle.matches(text).map(|range| span_on(line, range)));
    }
    found
}

/// The first match starting at or after `from`, wrapping round to the top of
/// the document when there is none below. Always one of the matches
/// [`find_all`] gives.
pub fn find_next(doc: &Doc, needle: &str, from: Pos, case_sensitive: bool) -> Option<Span> {
    let mut needle = Needle::new(needle, case_sensitive)?;
    let from = doc.clamp(from);
    let lines = doc.lines();
    let mut on_line = |line: usize, least: usize| {
        let text = lines.get(line)?;
        needle.matches(text).find(|&(start, _)| start >= least).map(|range| span_on(line, range))
    };
    on_line(from.line, from.col)
        .or_else(|| (from.line + 1..lines.len()).find_map(|line| on_line(line, 0)))
        .or_else(|| (0..=from.line).find_map(|line| on_line(line, 0)))
}

/// The last match starting before `from`, wrapping round to the bottom of the
/// document when there is none above. Always one of the matches [`find_all`]
/// gives.
pub fn find_prev(doc: &Doc, needle: &str, from: Pos, case_sensitive: bool) -> Option<Span> {
    let mut needle = Needle::new(needle, case_sensitive)?;
    let from = doc.clamp(from);
    let lines = doc.lines();
    let mut on_line = |line: usize, below: usize| {
        let text = lines.get(line)?;
        needle.matches(text).take_while(|&(start, _)| start < below).last().map(|range| span_on(line, range))
    };
    on_line(from.line, from.col)
        .or_else(|| (0..from.line).rev().find_map(|line| on_line(line, usize::MAX)))
        .or_else(|| (from.line..lines.len()).rev().find_map(|line| on_line(line, usize::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos::new(line, col)
    }

    fn s(line: usize, start: usize, end: usize) -> Span {
        span_on(line, (start, end))
    }

    #[test]
    fn finds_every_match_in_document_order() {
        let d = Doc::from_text("the other\nthe\n\nbathe");
        assert_eq!(find_all(&d, "the", true), vec![s(0, 0, 3), s(0, 5, 8), s(1, 0, 3), s(3, 2, 5)]);
        assert_eq!(find_all(&d, "x", true), vec![]);
        assert_eq!(find_all(&Doc::new(), "the", false), vec![]);
    }

    #[test]
    fn matches_do_not_overlap() {
        let d = Doc::from_text("aaaaa\nabababa");
        assert_eq!(find_all(&d, "aa", true), vec![s(0, 0, 2), s(0, 2, 4)]);
        assert_eq!(find_all(&d, "ABA", false), vec![s(1, 0, 3), s(1, 4, 7)]);
    }

    #[test]
    fn empty_needles_and_line_breaks_match_nothing() {
        let d = Doc::from_text("ab\ncd\n\n");
        for case_sensitive in [true, false] {
            assert_eq!(find_all(&d, "", case_sensitive), vec![]);
            assert_eq!(find_all(&d, "\n", case_sensitive), vec![]);
            assert_eq!(find_all(&d, "b\nc", case_sensitive), vec![]);
            assert_eq!(find_all(&d, "b\r\nc", case_sensitive), vec![]);
            assert_eq!(find_all(&d, "bc", case_sensitive), vec![]);
            assert_eq!(find_next(&d, "", p(0, 0), case_sensitive), None);
            assert_eq!(find_prev(&d, "b\nc", p(1, 1), case_sensitive), None);
        }
    }

    #[test]
    fn case_can_be_ignored() {
        let d = Doc::from_text("Hello HELLO hello h\u{00C9}llo");
        assert_eq!(find_all(&d, "hello", true), vec![s(0, 12, 17)]);
        assert_eq!(find_all(&d, "hElLo", false), vec![s(0, 0, 5), s(0, 6, 11), s(0, 12, 17)]);
        assert_eq!(find_all(&d, "H\u{00E9}LLO", false), vec![s(0, 18, 24)]);
        assert_eq!(find_all(&d, "H\u{00E9}LLO", true), vec![]);
    }

    #[test]
    fn spans_are_measured_in_the_text_as_written() {
        // U+0130 is two bytes and lowercases to three; U+1E9E is three and
        // lowercases to two; the Kelvin sign is three and lowercases to one.
        let d = Doc::from_text("a\u{0130}b \u{1E9E}b \u{212A}b");
        assert_eq!(find_all(&d, "B", false), vec![s(0, 3, 4), s(0, 8, 9), s(0, 13, 14)]);
        assert_eq!(find_all(&d, "\u{0130}", false), vec![s(0, 1, 3)]);
        assert_eq!(find_all(&d, "i\u{0307}B", false), vec![s(0, 1, 4)]);
        assert_eq!(find_all(&d, "\u{00DF}b", false), vec![s(0, 5, 9)]);
        assert_eq!(find_all(&d, "k", false), vec![s(0, 10, 13)]);
        assert_eq!(find_all(&d, "k", true), vec![]);
        for span in find_all(&d, "b", false) {
            assert_eq!(d.slice(span), "b");
        }
    }

    #[test]
    fn a_match_uses_up_whole_characters() {
        // `i` is only the first half of what U+0130 lowercases to.
        let d = Doc::from_text("\u{0130}x i\u{0307}x");
        assert_eq!(find_all(&d, "i", false), vec![s(0, 4, 5)]);
        assert_eq!(find_all(&d, "\u{0130}X", false), vec![s(0, 0, 3), s(0, 4, 8)]);
        assert_eq!(find_all(&d, "\u{0307}", false), vec![s(0, 5, 7)]);
    }

    #[test]
    fn next_and_previous_wrap_round() {
        let d = Doc::from_text("one fish\ntwo fish\n\nred");
        let (first, second) = (s(0, 4, 8), s(1, 4, 8));
        assert_eq!(find_next(&d, "fish", p(0, 0), true), Some(first));
        assert_eq!(find_next(&d, "fish", p(0, 4), true), Some(first));
        assert_eq!(find_next(&d, "fish", p(0, 5), true), Some(second));
        assert_eq!(find_next(&d, "fish", p(1, 5), true), Some(first));
        assert_eq!(find_next(&d, "fish", p(99, 99), true), Some(first));
        assert_eq!(find_prev(&d, "fish", p(3, 3), true), Some(second));
        assert_eq!(find_prev(&d, "fish", p(1, 5), true), Some(second));
        assert_eq!(find_prev(&d, "fish", p(1, 4), true), Some(first));
        assert_eq!(find_prev(&d, "fish", p(0, 4), true), Some(second));
        assert_eq!(find_prev(&d, "fish", p(0, 0), true), Some(second));
        assert_eq!(find_next(&d, "FISH", p(1, 5), false), Some(first));
        assert_eq!(find_next(&d, "FISH", p(1, 5), true), None);
        assert_eq!(find_prev(&d, "bird", p(1, 5), false), None);
        // The only match is found from anywhere, itself included.
        assert_eq!(find_next(&d, "red", p(3, 1), true), Some(s(3, 0, 3)));
        assert_eq!(find_prev(&d, "red", p(3, 0), true), Some(s(3, 0, 3)));
    }

    #[test]
    fn next_and_previous_step_between_the_matches_find_all_gives() {
        // "aa" in "aaa" also occurs at column 1, but that overlaps the match
        // at column 0 and is not one of the matches shown.
        let d = Doc::from_text("aaa");
        assert_eq!(find_all(&d, "aa", true), vec![s(0, 0, 2)]);
        assert_eq!(find_next(&d, "aa", p(0, 1), true), Some(s(0, 0, 2)));
        assert_eq!(find_prev(&d, "aa", p(0, 3), true), Some(s(0, 0, 2)));
    }

    #[test]
    fn a_needle_that_repeats_itself_is_still_found() {
        // Each of these begins to match, fails part of the way through, and
        // has to carry on from what it had without missing the real match.
        let d = Doc::from_text("aaab abcabcabd ABABAC aabaabaaab");
        for case_sensitive in [true, false] {
            assert_eq!(find_all(&d, "aab", case_sensitive), vec![s(0, 1, 4), s(0, 22, 25), s(0, 25, 28), s(0, 29, 32)]);
            assert_eq!(find_all(&d, "abcabd", case_sensitive), vec![s(0, 8, 14)]);
            assert_eq!(find_all(&d, "aabaaab", case_sensitive), vec![s(0, 25, 32)]);
        }
        assert_eq!(find_all(&d, "ababac", false), vec![s(0, 15, 21)]);
        assert_eq!(find_all(&d, "ababac", true), vec![]);
    }

    #[test]
    fn a_match_cannot_begin_or_end_inside_a_character() {
        // U+0130 lowercases to `i` and a combining dot. The two dots after it
        // match from the second character on, not from inside the first.
        let d = Doc::from_text("\u{0130}\u{0307}\u{0307}");
        assert_eq!(find_all(&d, "\u{0307}\u{0307}", false), vec![s(0, 2, 6)]);
        assert_eq!(find_all(&d, "i\u{0307}\u{0307}", false), vec![s(0, 0, 4)]);
        assert_eq!(find_all(&d, "i", false), vec![]);
        // A run of them, and a needle that fits it only out of step.
        let d = Doc::from_text(&"\u{0130}".repeat(6));
        assert_eq!(find_all(&d, "\u{0307}i", false), vec![]);
        assert_eq!(find_all(&d, "i\u{0307}i\u{0307}", false), vec![s(0, 0, 4), s(0, 4, 8), s(0, 8, 12)]);
        assert_eq!(find_all(&d, "\u{0130}\u{0130}\u{0130}\u{0130}", false), vec![s(0, 0, 8)]);
    }

    #[test]
    fn a_needle_longer_than_a_line_is_not_found_in_it() {
        let d = Doc::from_text("ab\n\nabc\nabcd");
        for case_sensitive in [true, false] {
            assert_eq!(find_all(&d, "abcd", case_sensitive), vec![s(3, 0, 4)]);
            assert_eq!(find_all(&d, "abcde", case_sensitive), vec![]);
            assert_eq!(find_next(&d, "bcd", p(3, 2), case_sensitive), Some(s(3, 1, 4)));
        }
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

    fn fold(text: &str) -> String {
        text.chars().flat_map(char::to_lowercase).collect()
    }

    /// The matches in one line, found the slow and obvious way: try every
    /// start and every end.
    fn plain_scan(text: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
        let same = |a: &str| if case_sensitive { a == needle } else { fold(a) == fold(needle) };
        let edges: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
        let mut found = Vec::new();
        let mut from = 0;
        while from < edges.len() {
            let start = edges[from];
            match edges[from + 1..].iter().position(|&end| same(&text[start..end])) {
                Some(n) => {
                    found.push((start, edges[from + 1 + n]));
                    from += 1 + n;
                }
                None => from += 1,
            }
        }
        found
    }

    /// Random text and needles, with characters whose lowercase forms change
    /// length.
    #[test]
    fn random_searches_agree_with_a_plain_scan() {
        let pieces = [
            "a",
            "A",
            "b",
            "ab",
            "aB",
            "aa",
            " ",
            "\n",
            "\u{0130}",
            "i\u{0307}",
            "i",
            "I",
            "\u{0307}",
            "\u{1E9E}",
            "\u{00DF}",
            "\u{212A}",
            "k",
            "K",
            "\u{00E9}",
            "\u{00C9}",
            "\u{4E00}",
            "\u{03A3}",
            "\u{03C3}",
            "\u{03C2}",
        ];
        let mut rng = Rng(0x1357_9BDF_0246_8ACE);
        for _ in 0..600 {
            let mut text = String::new();
            for _ in 0..rng.below(30) {
                text.push_str(pieces[rng.below(pieces.len())]);
            }
            let d = Doc::from_text(&text);
            let mut needle = String::new();
            for _ in 0..1 + rng.below(3) {
                needle.push_str(pieces[rng.below(pieces.len())]);
            }
            check(&d, &needle, &mut rng);
        }
    }

    /// The same over a handful of characters, so that the text keeps almost
    /// matching and the needles overlap themselves.
    #[test]
    fn random_searches_of_repetitive_text_agree_with_a_plain_scan() {
        let pieces = ["a", "a", "A", "b", "ab", "\u{0130}", "i", "\u{0307}", "i\u{0307}"];
        let mut rng = Rng(0x0F1E_2D3C_4B5A_6978);
        for _ in 0..500 {
            let mut text = String::new();
            for _ in 0..rng.below(40) {
                text.push_str(pieces[rng.below(pieces.len())]);
            }
            let d = Doc::from_text(&text);
            let mut needle = String::new();
            for _ in 0..1 + rng.below(7) {
                needle.push_str(pieces[rng.below(pieces.len())]);
            }
            check(&d, &needle, &mut rng);
        }
    }

    /// Checks one needle against the slow scan: every span must read back as
    /// the needle, the spans must be in order and apart, and next and
    /// previous must agree with the full list.
    fn check(d: &Doc, needle: &str, rng: &mut Rng) {
        for case_sensitive in [true, false] {
            let all = find_all(d, needle, case_sensitive);
            if needle.contains('\n') {
                assert_eq!(all, vec![]);
                assert_eq!(find_next(d, needle, p(0, 0), case_sensitive), None);
                assert_eq!(find_prev(d, needle, d.end(), case_sensitive), None);
                continue;
            }
            let mut want = Vec::new();
            for (line, text) in d.lines().iter().enumerate() {
                want.extend(plain_scan(text, needle, case_sensitive).into_iter().map(|range| span_on(line, range)));
            }
            assert_eq!(all, want, "{needle:?} in {:?}", d.lines());

            for span in &all {
                assert!(span.start < span.end && span.start.line == span.end.line);
                assert_eq!((d.clamp(span.start), d.clamp(span.end)), (span.start, span.end));
                let read = d.slice(*span);
                if case_sensitive {
                    assert_eq!(read, needle);
                } else {
                    assert_eq!(fold(&read), fold(needle));
                }
            }
            for pair in all.windows(2) {
                assert!(pair[0].end <= pair[1].start);
            }

            for _ in 0..8 {
                let line = rng.below(d.line_count() + 1);
                let from = Pos::new(line, rng.below(d.line(line).len() + 2));
                let at = d.clamp(from);
                let next = all.iter().find(|span| span.start >= at).or(all.first()).copied();
                let prev = all.iter().rev().find(|span| span.start < at).or(all.last()).copied();
                assert_eq!(find_next(d, needle, from, case_sensitive), next, "{needle:?} from {from:?}");
                assert_eq!(find_prev(d, needle, from, case_sensitive), prev, "{needle:?} from {from:?}");
            }
        }
    }
}
