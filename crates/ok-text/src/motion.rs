//! Moving through prose by the units a writer thinks in.
//!
//! A cluster is a character with the combining marks on it, and no motion
//! here ever stops inside one: DESIGN.md has every cell a character of the
//! file, "so arrow keys never lie". A word is a run of letters, digits and
//! underscores with the marks that sit on them, and an apostrophe counts when
//! it has word characters on both sides, so "don't" is one word. A mark is
//! whatever the character it sits on is.
//! A sentence ends at one or more of `.` `!` `?` `…`, then any closing quotes
//! or brackets, when whitespace or the end of the line follows; a period
//! before whitespace and a lowercase letter is taken for an abbreviation and
//! passed over, and the end of a line always ends a sentence. A sentence
//! starts at the first character after that which is neither whitespace nor
//! a mark sitting on whitespace. A paragraph is a run of lines with something
//! on them.
//!
//! Every function clamps the position it is given and hands back one that is
//! inside the document and on a character boundary, so none of them can fail.

use std::str::CharIndices;

use crate::doc::{Doc, Pos, Span};
use crate::width::is_combining;

/// The text of `p`'s line on each side of `p`.
fn split(doc: &Doc, p: Pos) -> (&str, &str) {
    let line = doc.line(p.line);
    match (line.get(..p.col), line.get(p.col..)) {
        (Some(before), Some(after)) => (before, after),
        _ => (line, ""),
    }
}

/// The end of the line above `p`, or `p` itself on the first line.
fn end_of_line_above(doc: &Doc, p: Pos) -> Pos {
    match p.line.checked_sub(1) {
        Some(line) => Pos::new(line, doc.line(line).len()),
        None => p,
    }
}

/// Column 0 of the line below `p`, or `p` itself on the last line.
fn start_of_line_below(doc: &Doc, p: Pos) -> Pos {
    if p.line + 1 < doc.line_count() {
        Pos::new(p.line + 1, 0)
    } else {
        p
    }
}

fn span_on(line: usize, start: usize, end: usize) -> Span {
    Span { start: Pos::new(line, start), end: Pos::new(line, end) }
}

/// One cluster back: over any combining marks and the character they sit on.
/// From column 0 it goes to the end of the line above.
pub fn left(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    if p.col == 0 {
        return end_of_line_above(doc, p);
    }
    let (before, _) = split(doc, p);
    let mut col = p.col;
    for (i, ch) in before.char_indices().rev() {
        col = i;
        if !is_combining(ch) {
            break;
        }
    }
    Pos::new(p.line, col)
}

/// One cluster forward: over a character and the combining marks on it. From
/// the end of a line it goes to column 0 of the line below.
pub fn right(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let (_, after) = split(doc, p);
    let mut chars = after.char_indices();
    if chars.next().is_none() {
        return start_of_line_below(doc, p);
    }
    let len = chars.find(|&(_, ch)| !is_combining(ch)).map_or(after.len(), |(i, _)| i);
    Pos::new(p.line, p.col + len)
}

/// Letters, digits, the underscore and combining marks: what may stand next
/// to an apostrophe inside a word. Whether a mark is itself part of a word is
/// decided by what it sits on; see [`WordRanges`].
fn is_wordy(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || is_combining(ch)
}

fn is_apostrophe(ch: char) -> bool {
    ch == '\'' || ch == '\u{2019}'
}

/// The words of one line as byte ranges, in order.
///
/// An apostrophe is part of a word only with a word character straight
/// before it and a letter, digit, underscore or mark straight after it, which
/// keeps "don't" whole without swallowing quotation marks. One apostrophe
/// never counts as the word character beside another, so a doubled
/// apostrophe joins nothing.
///
/// A combining mark belongs to what it sits on: it is in a word when the
/// character before it is and out of one when that is not, so no word starts
/// or ends between a character and a mark on it (the selector after a heart,
/// the joiner between two emoji, a direction mark after a space). With
/// nothing to sit on, at the start of a line, a mark is a word character.
struct WordRanges<'a> {
    chars: CharIndices<'a>,
    len: usize,
    /// Whether the character just passed was a word character.
    in_word: bool,
}

fn word_ranges(line: &str) -> WordRanges<'_> {
    WordRanges { chars: line.char_indices(), len: line.len(), in_word: false }
}

impl Iterator for WordRanges<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        let mut start = None;
        while let Some((i, ch)) = self.chars.next() {
            self.in_word = if is_apostrophe(ch) {
                self.in_word && self.chars.clone().next().is_some_and(|(_, after)| is_wordy(after))
            } else if is_combining(ch) {
                self.in_word || i == 0
            } else {
                is_wordy(ch)
            };
            match (start, self.in_word) {
                (None, true) => start = Some(i),
                (Some(from), false) => return Some((from, i)),
                _ => {}
            }
        }
        start.map(|from| (from, self.len))
    }
}

/// Ctrl+Left: the start of the word before `p`, or column 0 when there is no
/// word before it on the line. From column 0 it goes to the end of the line
/// above.
pub fn word_left(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    if p.col == 0 {
        return end_of_line_above(doc, p);
    }
    let col =
        word_ranges(doc.line(p.line)).map(|(start, _)| start).take_while(|&start| start < p.col).last().unwrap_or(0);
    Pos::new(p.line, col)
}

/// Ctrl+Right: past the rest of the current word and whatever separates it
/// from the next, to the start of the next word on the line, or to the end of
/// the line when there is none. From the end of a line it goes to column 0 of
/// the line below.
pub fn word_right(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let line = doc.line(p.line);
    if p.col >= line.len() {
        return start_of_line_below(doc, p);
    }
    let col = word_ranges(line).map(|(start, _)| start).find(|&start| start > p.col).unwrap_or(line.len());
    Pos::new(p.line, col)
}

/// The end of the word `p` is in, or of the next word when `p` is between
/// words or already at the end of one. Never leaves the line: with no word
/// ahead it gives the end of the line.
pub fn word_end(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let line = doc.line(p.line);
    let col = word_ranges(line).map(|(_, end)| end).find(|&end| end > p.col).unwrap_or(line.len());
    Pos::new(p.line, col)
}

/// The word under `p`, or the word ending at `p`; an empty span at `p` if
/// there is neither.
pub fn word_at(doc: &Doc, p: Pos) -> Span {
    let p = doc.clamp(p);
    match word_ranges(doc.line(p.line)).find(|&(_, end)| end >= p.col) {
        Some((start, end)) if start <= p.col => span_on(p.line, start, end),
        _ => Span::at(p),
    }
}

fn is_terminator(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | '\u{2026}')
}

fn is_closer(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '\u{201D}' | '\u{2019}' | ')' | ']')
}

/// True for a line that is empty or holds only whitespace.
fn is_blank(line: &str) -> bool {
    line.chars().all(char::is_whitespace)
}

/// Where the first character that is not whitespace sits, if there is one.
/// A combining mark that follows whitespace sits on it and is passed over
/// with it, so the answer is never between a space and a mark on that space
/// (the direction mark that opens a Hebrew or Arabic sentence, say).
fn first_non_space(line: &str) -> Option<usize> {
    let mut on_space = false;
    line.char_indices()
        .find(|&(_, ch)| {
            on_space = ch.is_whitespace() || (on_space && is_combining(ch));
            !on_space
        })
        .map(|(at, _)| at)
}

/// Where the sentence starting at byte `start` of `line` ends: just after its
/// terminator and any closing quotes or brackets, or, when it has no
/// terminator, after the last character on the line that is not whitespace.
///
/// A terminator only counts when whitespace or the end of the line follows.
/// A period straight before whitespace and a lowercase letter is taken for an
/// abbreviation and passed over.
fn sentence_end(line: &str, start: usize) -> usize {
    let tail = line.get(start..).unwrap_or("");
    let mut chars = tail.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if !is_terminator(ch) {
            continue;
        }
        let mut last = ch;
        let mut end = start + i + ch.len_utf8();
        while let Some((j, more)) = chars.next_if(|&(_, next)| is_terminator(next)) {
            last = more;
            end = start + j + more.len_utf8();
        }
        let bare = end;
        while let Some((j, closer)) = chars.next_if(|&(_, next)| is_closer(next)) {
            end = start + j + closer.len_utf8();
        }
        match chars.peek() {
            None => return end,
            Some(&(_, next)) if next.is_whitespace() => {
                let onward = line.get(end..).unwrap_or("").chars().find(|c| !c.is_whitespace());
                let abbreviation = last == '.' && end == bare && onward.is_some_and(char::is_lowercase);
                if !abbreviation {
                    return end;
                }
            }
            Some(_) => {}
        }
    }
    line.trim_end().len()
}

/// The sentences of one line as byte ranges, in order. Each runs from its
/// first character through its terminator and closers; the whitespace between
/// sentences belongs to none of them. A blank line has no sentences.
pub(crate) struct SentenceRanges<'a> {
    line: &'a str,
    at: usize,
}

pub(crate) fn sentence_ranges(line: &str) -> SentenceRanges<'_> {
    SentenceRanges { line, at: 0 }
}

impl Iterator for SentenceRanges<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        let start = self.at + first_non_space(self.line.get(self.at..)?)?;
        let end = sentence_end(self.line, start);
        self.at = end;
        Some((start, end))
    }
}

/// The start of the next sentence after `p`, crossing lines and skipping
/// blank ones; the end of the document from the last sentence.
pub fn sentence_next(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let ahead = sentence_ranges(doc.line(p.line)).map(|(start, _)| start).find(|&start| start > p.col);
    if let Some(start) = ahead {
        return Pos::new(p.line, start);
    }
    for line in p.line + 1..doc.line_count() {
        if let Some(start) = first_non_space(doc.line(line)) {
            return Pos::new(line, start);
        }
    }
    doc.end()
}

/// The start of the sentence `p` is in, or of the sentence before when `p` is
/// already at a start. Crosses lines and skips blank ones; the start of the
/// document from the first sentence.
pub fn sentence_prev(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let behind = sentence_ranges(doc.line(p.line)).map(|(start, _)| start).take_while(|&start| start < p.col).last();
    if let Some(start) = behind {
        return Pos::new(p.line, start);
    }
    for line in (0..p.line).rev() {
        if let Some((start, _)) = sentence_ranges(doc.line(line)).last() {
            return Pos::new(line, start);
        }
    }
    Pos::new(0, 0)
}

/// The sentence at `p`: from its start through its terminator and closers,
/// without the whitespace after. Whitespace after a sentence counts as that
/// sentence's, whitespace before the first one on a line as the first's. On a
/// blank line the span is empty.
pub fn sentence_at(doc: &Doc, p: Pos) -> Span {
    let p = doc.clamp(p);
    let mut found = None;
    for (start, end) in sentence_ranges(doc.line(p.line)) {
        if found.is_some() && start > p.col {
            break;
        }
        found = Some((start, end));
    }
    match found {
        Some((start, end)) => span_on(p.line, start, end),
        None => Span::at(p),
    }
}

fn blank_at(doc: &Doc, line: usize) -> bool {
    is_blank(doc.line(line))
}

/// The first line of the paragraph that the non-blank line `line` is in.
fn paragraph_first(doc: &Doc, mut line: usize) -> usize {
    while line > 0 && !blank_at(doc, line - 1) {
        line -= 1;
    }
    line
}

/// The last line of the paragraph that the non-blank line `line` is in.
fn paragraph_last(doc: &Doc, mut line: usize) -> usize {
    while line + 1 < doc.line_count() && !blank_at(doc, line + 1) {
        line += 1;
    }
    line
}

/// Column 0 of the first line of the next paragraph, a paragraph being a run
/// of lines with something on them. The end of the document if there is none.
pub fn paragraph_next(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let count = doc.line_count();
    let mut line = p.line;
    while line < count && !blank_at(doc, line) {
        line += 1;
    }
    while line < count && blank_at(doc, line) {
        line += 1;
    }
    if line < count {
        Pos::new(line, 0)
    } else {
        doc.end()
    }
}

/// Column 0 of the first line of the paragraph `p` is in. From there, or from
/// a blank line, the start of the paragraph before; the start of the document
/// if there is none.
pub fn paragraph_prev(doc: &Doc, p: Pos) -> Pos {
    let p = doc.clamp(p);
    let mut line = p.line;
    if !blank_at(doc, line) {
        let own = Pos::new(paragraph_first(doc, line), 0);
        if p != own {
            return own;
        }
        line = own.line;
    }
    while line > 0 {
        line -= 1;
        if !blank_at(doc, line) {
            return Pos::new(paragraph_first(doc, line), 0);
        }
    }
    Pos::new(0, 0)
}

/// The paragraph at `p`, from column 0 of its first line to the end of its
/// last. On a blank line the span is empty.
pub fn paragraph_at(doc: &Doc, p: Pos) -> Span {
    let p = doc.clamp(p);
    if blank_at(doc, p.line) {
        return Span::at(p);
    }
    let last = paragraph_last(doc, p.line);
    Span { start: Pos::new(paragraph_first(doc, p.line), 0), end: Pos::new(last, doc.line(last).len()) }
}

/// Column 0 of `p`'s line. This is the document line, which may be drawn as
/// several wrapped rows.
pub fn line_start(doc: &Doc, p: Pos) -> Pos {
    Pos::new(doc.clamp(p).line, 0)
}

/// The end of `p`'s line; see [`line_start`].
pub fn line_end(doc: &Doc, p: Pos) -> Pos {
    let line = doc.clamp(p).line;
    Pos::new(line, doc.line(line).len())
}

pub fn doc_start(_doc: &Doc, _p: Pos) -> Pos {
    Pos::new(0, 0)
}

pub fn doc_end(doc: &Doc, _p: Pos) -> Pos {
    doc.end()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Motion = fn(&Doc, Pos) -> Pos;

    fn p(line: usize, col: usize) -> Pos {
        Pos::new(line, col)
    }

    /// Reads a picture of a document in which each `|` marks a position and
    /// `<` and `>` mark the two ends of a span: gives the text without the
    /// marks, where the `|`s were, and the span.
    fn pictured(picture: &str) -> (Doc, Vec<Pos>, Span) {
        let mut text = String::new();
        let mut marks = Vec::new();
        let mut span = Span::default();
        let (mut line, mut col) = (0, 0);
        for ch in picture.chars() {
            match ch {
                '|' => marks.push(Pos::new(line, col)),
                '<' => span.start = Pos::new(line, col),
                '>' => span.end = Pos::new(line, col),
                '\n' => {
                    line += 1;
                    col = 0;
                    text.push(ch);
                }
                _ => {
                    col += ch.len_utf8();
                    text.push(ch);
                }
            }
        }
        (Doc::from_text(&text), marks, span)
    }

    fn marked(picture: &str) -> (Doc, Vec<Pos>) {
        let (d, marks, _) = pictured(picture);
        (d, marks)
    }

    /// Checks that `forward` steps from each mark to the next and `backward`
    /// from each mark to the one before.
    fn walks(picture: &str, forward: Motion, backward: Motion) {
        let (d, marks) = marked(picture);
        assert!(marks.len() > 1, "{picture:?} needs at least two marks");
        for pair in marks.windows(2) {
            assert_eq!(forward(&d, pair[0]), pair[1], "forward from {:?} in {picture:?}", pair[0]);
            assert_eq!(backward(&d, pair[1]), pair[0], "backward from {:?} in {picture:?}", pair[1]);
        }
    }

    /// Checks that `forward` steps from each mark to the next.
    fn steps(picture: &str, forward: Motion) {
        let (d, marks) = marked(picture);
        for pair in marks.windows(2) {
            assert_eq!(forward(&d, pair[0]), pair[1], "from {:?} in {picture:?}", pair[0]);
        }
    }

    /// Checks that `at` gives the span between `<` and `>` from every mark.
    fn selects(picture: &str, at: fn(&Doc, Pos) -> Span) {
        let (d, marks, want) = pictured(picture);
        assert!(!marks.is_empty(), "{picture:?} needs a mark");
        for from in marks {
            assert_eq!(at(&d, from), want, "at {from:?} in {picture:?}");
        }
    }

    #[test]
    fn left_and_right_move_by_whole_clusters_and_cross_lines() {
        walks("|a|n\u{0313}\u{0301}|b|\n|\u{4E00}|\n|\n|c|", right, left);
        let d = Doc::from_text("an\u{0313}\u{0301}b");
        // Starting inside a cluster still lands on its edges.
        assert_eq!(right(&d, p(0, 2)), p(0, 6));
        assert_eq!(left(&d, p(0, 4)), p(0, 1));
        // A line that opens with a mark has nothing for it to sit on.
        walks("|\u{0301}\u{0301}|x|", right, left);
    }

    #[test]
    fn left_and_right_stop_at_the_ends_of_the_document() {
        let d = Doc::from_text("ab\ncd");
        assert_eq!(left(&d, p(0, 0)), p(0, 0));
        assert_eq!(right(&d, p(1, 2)), p(1, 2));
        assert_eq!(right(&d, p(40, 40)), p(1, 2));
        let empty = Doc::new();
        assert_eq!(left(&empty, p(0, 0)), p(0, 0));
        assert_eq!(right(&empty, p(0, 0)), p(0, 0));
    }

    #[test]
    fn word_motions_follow_ctrl_arrow() {
        walks("|Don't |stop, (|mitsqanaqan\u{0313}) -- |now|\n|next_one  |x|", word_right, word_left);
        // From inside a word, right goes to the next word and left to the
        // start of this one.
        let (d, at) = marked("alpha b|et|a  gamma|  ");
        assert_eq!(word_right(&d, at[0]), p(0, 12));
        assert_eq!(word_left(&d, at[1]), p(0, 6));
        // Nothing but spaces ahead: the end of the line, and there it stays
        // when there is no line below.
        assert_eq!(word_right(&d, at[2]), p(0, 19));
        assert_eq!(word_right(&d, p(0, 19)), p(0, 19));
        // Nothing but spaces behind: column 0, then the line above.
        let (d, at) = marked("one\n   |two");
        assert_eq!(word_left(&d, at[0]), p(1, 0));
        assert_eq!(word_left(&d, p(1, 0)), p(0, 3));
        assert_eq!(word_left(&d, p(0, 0)), p(0, 0));
    }

    #[test]
    fn apostrophes_join_a_word_only_from_the_inside() {
        steps("'|quoted' |rock'n'roll |dogs' |it\u{2019}s |a''|b \u{2018}|so\u{2019}|", word_right);
        selects("say <|do|n'|t|> now", word_at);
        selects("'<|quoted|>'", word_at);
        // A doubled apostrophe joins nothing: `a` and `b` are two words.
        selects("<|a|>''b", word_at);
    }

    #[test]
    fn a_mark_belongs_to_the_character_it_sits_on() {
        // On a letter, or at the start of a line with nothing to sit on, it
        // is part of the word around it.
        selects("<|n|\u{0313}|a|>", word_at);
        selects("<|\u{0301}|a|> b", word_at);
        // On something that is not a word character it is not one either,
        // so nothing stops between the two: the selector after a heart, the
        // joiner between two emoji, a direction mark after a space.
        steps("|I \u{2764}\u{FE0F} |you|", word_right);
        steps("|you| \u{2764}\u{FE0F} I|", word_end);
        steps("|\u{1F468}\u{200D}\u{1F469} |x|", word_right);
        walks("|so. \u{200F}|x|", word_right, word_left);
        let (d, at) = marked("\u{2764}|\u{FE0F}|");
        assert_eq!(word_at(&d, at[0]), Span::at(at[0]));
        assert_eq!(word_left(&d, at[1]), p(0, 0));
        // Not even a letter that is also a mark, here on a dotted circle.
        let (d, at) = marked("\u{25CC}|\u{05B8} x");
        assert_eq!(word_at(&d, at[0]), Span::at(at[0]));
        assert_eq!(word_right(&d, p(0, 0)), p(0, 6));
        // A mark straight after an apostrophe is the word character on that
        // side of it.
        selects("<|don\u{2019}\u{FE0E}|t|> y", word_at);
        selects("<|x|\u{2019}|\u{FE0E}|> y", word_at);
        selects("<|x|>\u{2019} \u{FE0E}", word_at);
    }

    #[test]
    fn a_sentence_never_starts_between_a_space_and_a_mark_on_it() {
        // The direction mark that opens the second sentence rides on the
        // space before it.
        walks("|\u{05E9}\u{05DC}\u{05D5}\u{05DD}. \u{200F}|\u{05E9}\u{05DC}\u{05D5}\u{05DD}.|", sentence_next, sentence_prev);
        walks("|One.  \u{0301}\u{0301}|Two.|", sentence_next, sentence_prev);
        // A mark that opens a line has nothing to sit on and starts it.
        walks("|\u{0301}One. |Two.|", sentence_next, sentence_prev);
        // A line of nothing but spaces and the marks on them has no sentence.
        assert_eq!(sentence_ranges(" \u{200F} \u{0301}").count(), 0);
        assert_eq!(sentence_ranges("\u{0301}").collect::<Vec<_>>(), [(0, 2)]);
    }

    #[test]
    fn word_end_goes_to_the_end_of_this_word_or_the_next() {
        steps("|Don't| stop|, mitsqanaqan\u{0313}| now|", word_end);
        let (d, at) = marked("al|pha  |beta|  ");
        assert_eq!(word_end(&d, at[0]), p(0, 5));
        assert_eq!(word_end(&d, at[1]), p(0, 11));
        // No word ahead: the end of the line, and no further.
        assert_eq!(word_end(&d, at[2]), p(0, 13));
        assert_eq!(word_end(&d, p(0, 13)), p(0, 13));
        let two = Doc::from_text("a\nb");
        assert_eq!(word_end(&two, p(0, 1)), p(0, 1));
    }

    #[test]
    fn word_at_takes_the_word_under_or_just_before() {
        selects("one <|t|w|o|> three", word_at);
        let (d, at) = marked("one, | two|");
        assert_eq!(word_at(&d, at[0]), Span::at(at[0]));
        assert_eq!(word_at(&d, at[1]), Span { start: p(0, 6), end: p(0, 9) });
        assert_eq!(word_at(&Doc::new(), p(3, 3)), Span::at(p(0, 0)));
    }

    #[test]
    fn sentences_end_at_terminators_followed_by_space() {
        walks("|Hello there. |How are you?! |Fine\u{2026} |Thanks!|", sentence_next, sentence_prev);
        // Dots inside numbers and addresses are not followed by whitespace.
        walks("|Pi is 3.14 or so. |See example.com today.|", sentence_next, sentence_prev);
        // Closing quotes and brackets stay with the sentence they close.
        walks("|She said \"Go.\" |Then (quietly.) |\u{201C}Why?\u{201D} |I left.|", sentence_next, sentence_prev);
        walks("|He said 'no.' |[See note.] |It\u{2019}s \u{2018}fine.\u{2019} |End.|", sentence_next, sentence_prev);
        selects("One. <|She said \"Go|.\"|> Then left.", sentence_at);
    }

    #[test]
    fn a_period_before_a_lowercase_letter_is_an_abbreviation() {
        walks("|See e.g. the docs, approx. five. |Then stop.|", sentence_next, sentence_prev);
        walks("|Wait... what? |Yes.|", sentence_next, sentence_prev);
        // The guard is for periods only.
        walks("|\"Why?\" |she asked.|", sentence_next, sentence_prev);
    }

    #[test]
    fn sentence_motions_cross_lines_and_skip_blank_ones() {
        walks("|One. |Two\n\n  \t\n   |Three.   \n|four|", sentence_next, sentence_prev);
        // Blank lines at either end: the motions run on to the document's ends.
        let d = Doc::from_text("\n  One.\n\n");
        assert_eq!(sentence_next(&d, p(0, 0)), p(1, 2));
        assert_eq!(sentence_next(&d, p(1, 2)), p(3, 0));
        assert_eq!(sentence_prev(&d, p(3, 0)), p(1, 2));
        assert_eq!(sentence_prev(&d, p(1, 2)), p(0, 0));
        // From inside a sentence, back goes to its own start.
        let (d, at) = marked("One. Tw|o.");
        assert_eq!(sentence_prev(&d, at[0]), p(0, 5));
    }

    #[test]
    fn sentence_at_stops_before_the_space_after() {
        selects("<|One|.|>  Two.", sentence_at);
        // The space after a sentence counts as that sentence's.
        selects("<One.> |  Two.", sentence_at);
        selects("One.  <|Tw|o.\"|>", sentence_at);
        // The space before the first sentence on a line counts as the first's.
        selects(" | <|No stop here|> |  ", sentence_at);
        let d = Doc::from_text("a\n  \nb");
        assert_eq!(sentence_at(&d, p(1, 1)), Span::at(p(1, 1)));
    }

    #[test]
    fn paragraph_motions_stop_at_first_lines() {
        walks("|a\nb\n\n \n|c\nd\n\n|e|", paragraph_next, paragraph_prev);
        let d = Doc::from_text("a\nb\n\n \nc\nd\n\ne");
        assert_eq!(paragraph_next(&d, p(1, 1)), p(4, 0));
        assert_eq!(paragraph_next(&d, p(2, 0)), p(4, 0));
        assert_eq!(paragraph_next(&d, p(7, 1)), p(7, 1));
        // From inside a paragraph, back goes to its own first line.
        assert_eq!(paragraph_prev(&d, p(5, 1)), p(4, 0));
        assert_eq!(paragraph_prev(&d, p(4, 1)), p(4, 0));
        // From a blank line, to the paragraph above.
        assert_eq!(paragraph_prev(&d, p(6, 0)), p(4, 0));
        assert_eq!(paragraph_prev(&d, p(3, 1)), p(0, 0));
        assert_eq!(paragraph_prev(&d, p(0, 0)), p(0, 0));
        // Leading blank lines: the first paragraph does not start the document.
        let d = Doc::from_text("\n\nfirst\n");
        assert_eq!(paragraph_prev(&d, p(2, 3)), p(2, 0));
        assert_eq!(paragraph_prev(&d, p(2, 0)), p(0, 0));
        assert_eq!(paragraph_next(&d, p(0, 0)), p(2, 0));
        assert_eq!(paragraph_next(&d, p(2, 0)), p(3, 0));
    }

    #[test]
    fn paragraph_at_spans_the_whole_run_of_lines() {
        let d = Doc::from_text("a\nb\n\n \nc\ndd\n\ne");
        assert_eq!(paragraph_at(&d, p(1, 0)), Span { start: p(0, 0), end: p(1, 1) });
        assert_eq!(paragraph_at(&d, p(4, 1)), Span { start: p(4, 0), end: p(5, 2) });
        assert_eq!(paragraph_at(&d, p(3, 1)), Span::at(p(3, 1)));
        assert_eq!(paragraph_at(&d, p(7, 0)), Span { start: p(7, 0), end: p(7, 1) });
    }

    #[test]
    fn line_and_document_ends() {
        let d = Doc::from_text("caf\u{00E9}\n\nlast");
        assert_eq!(line_start(&d, p(0, 3)), p(0, 0));
        assert_eq!(line_end(&d, p(0, 1)), p(0, 5));
        assert_eq!(line_end(&d, p(1, 0)), p(1, 0));
        assert_eq!(line_start(&d, p(9, 9)), p(2, 0));
        assert_eq!(doc_start(&d, p(2, 2)), p(0, 0));
        assert_eq!(doc_end(&d, p(0, 0)), p(2, 4));
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

    const PIECES: &[&str] = &[
        "",
        " ",
        "  ",
        "word",
        "don't",
        "it\u{2019}s",
        "'",
        "\u{2019}",
        ".",
        ". ",
        "!",
        "? ",
        "\u{2026}",
        "\"",
        ")",
        "]",
        "\n",
        "\n\n",
        " \n",
        "n\u{0313}",
        "\u{0301}",
        "\u{4E00}",
        "\u{00E9}",
        "A",
        "b",
        "_",
        "-",
        "\t",
        "\u{1F600}",
        "\u{FE0F}",
        "e.g. ",
        "3.14",
        "\u{00A0}",
    ];

    fn random_doc(rng: &mut Rng) -> Doc {
        let mut text = String::new();
        for _ in 0..rng.below(40) {
            text.push_str(PIECES[rng.below(PIECES.len())]);
        }
        Doc::from_text(&text)
    }

    fn on_cluster_edge(d: &Doc, q: Pos) -> bool {
        d.line(q.line).get(q.col..).and_then(|rest| rest.chars().next()).is_none_or(|ch| !is_combining(ch))
            || q.col == 0
    }

    /// Every motion, from any position at all, must land inside the document
    /// on a character boundary, move the way its name says, and get somewhere.
    #[test]
    fn random_positions_never_escape_and_always_make_progress() {
        let forward: [(&str, Motion); 4] = [
            ("right", right),
            ("word_right", word_right),
            ("sentence_next", sentence_next),
            ("paragraph_next", paragraph_next),
        ];
        let backward: [(&str, Motion); 4] = [
            ("left", left),
            ("word_left", word_left),
            ("sentence_prev", sentence_prev),
            ("paragraph_prev", paragraph_prev),
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..400 {
            let d = random_doc(&mut rng);
            let (first, last) = (Pos::new(0, 0), d.end());
            for _ in 0..40 {
                let line = rng.below(d.line_count() + 2);
                let q = Pos::new(line, rng.below(d.line(line).len() + 3));
                let c = d.clamp(q);
                for (name, motion) in forward {
                    let to = motion(&d, q);
                    assert_eq!(d.clamp(to), to, "{name} from {q:?} in {:?}", d.lines());
                    assert!(to > c || (to == c && c == last), "{name} from {q:?} in {:?}", d.lines());
                }
                for (name, motion) in backward {
                    let to = motion(&d, q);
                    assert_eq!(d.clamp(to), to, "{name} from {q:?} in {:?}", d.lines());
                    assert!(to < c || (to == c && c == first), "{name} from {q:?} in {:?}", d.lines());
                }

                let end = word_end(&d, q);
                assert_eq!((d.clamp(end), end.line), (end, c.line));
                assert!(end > c || end == line_end(&d, c));
                assert_eq!(line_start(&d, q), Pos::new(c.line, 0));
                assert_eq!(line_end(&d, q), Pos::new(c.line, d.line(c.line).len()));
                assert_eq!((doc_start(&d, q), doc_end(&d, q)), (first, last));

                for (name, at) in [("word_at", word_at as fn(&Doc, Pos) -> Span), ("sentence_at", sentence_at)] {
                    let span = at(&d, q);
                    assert!(span.start <= span.end, "{name} at {q:?} in {:?}", d.lines());
                    assert_eq!((d.clamp(span.start), d.clamp(span.end)), (span.start, span.end));
                    assert_eq!((span.start.line, span.end.line), (c.line, c.line));
                }
                let word = word_at(&d, q);
                assert!(word.start <= c && c <= word.end);
                let paragraph = paragraph_at(&d, q);
                assert!(paragraph.start <= c && c <= paragraph.end);
                assert_eq!(paragraph.is_empty(), is_blank(d.line(c.line)));
                assert!(!d.slice(paragraph).split('\n').any(is_blank) || paragraph.is_empty());

                // Clusters: a step one way and back is a round trip.
                if on_cluster_edge(&d, c) {
                    if c != last {
                        assert_eq!(left(&d, right(&d, c)), c, "from {c:?} in {:?}", d.lines());
                    }
                    if c != first {
                        assert_eq!(right(&d, left(&d, c)), c, "from {c:?} in {:?}", d.lines());
                    }
                }
                // A word is a word from wherever in it the question is asked.
                if !word.is_empty() {
                    assert_eq!((word_at(&d, word.start), word_at(&d, word.end)), (word, word));
                    assert_eq!((word_end(&d, word.start), word_left(&d, word.end)), (word.end, word.start));
                }
            }
        }
    }

    /// Stepping by any unit from one end of the document reaches the other,
    /// and going forward then back by sentences or paragraphs revisits the
    /// same stops.
    #[test]
    fn random_walks_cover_the_document_both_ways() {
        let pairs: [(&str, Motion, Motion); 4] = [
            ("cluster", right, left),
            ("word", word_right, word_left),
            ("sentence", sentence_next, sentence_prev),
            ("paragraph", paragraph_next, paragraph_prev),
        ];
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        for _ in 0..300 {
            let d = random_doc(&mut rng);
            let limit = d.len_bytes() + d.line_count() + 2;
            for (name, forward, backward) in pairs {
                let mut stops = vec![Pos::new(0, 0)];
                while stops[stops.len() - 1] != d.end() {
                    assert!(stops.len() <= limit, "{name} forward stalls in {:?}", d.lines());
                    stops.push(forward(&d, stops[stops.len() - 1]));
                }
                let mut back = vec![d.end()];
                while back[back.len() - 1] != Pos::new(0, 0) {
                    assert!(back.len() <= limit, "{name} backward stalls in {:?}", d.lines());
                    back.push(backward(&d, back[back.len() - 1]));
                }
                if name == "cluster" {
                    back.reverse();
                    assert_eq!(stops, back, "{name} in {:?}", d.lines());
                }
            }
        }
    }

    #[test]
    fn sentence_ranges_tile_each_line_in_order() {
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..400 {
            let d = random_doc(&mut rng);
            for text in d.lines() {
                let mut at = 0;
                for (start, end) in sentence_ranges(text) {
                    assert!(at <= start && start < end && end <= text.len(), "{text:?}");
                    assert!(text[at..start].chars().all(|ch| ch.is_whitespace() || is_combining(ch)), "{text:?}");
                    assert!(start == 0 || !is_combining(text[start..].chars().next().unwrap_or(' ')), "{text:?}");
                    assert!(!text[start..end].starts_with(char::is_whitespace), "{text:?}");
                    assert!(!text[start..end].ends_with(char::is_whitespace), "{text:?}");
                    at = end;
                }
                assert!(text[at..].chars().all(|ch| ch.is_whitespace() || is_combining(ch)), "{text:?}");
                assert_eq!(sentence_ranges(text).count() == 0, first_non_space(text).is_none());
                assert!(!is_blank(text) || first_non_space(text).is_none());
            }
        }
    }
}
