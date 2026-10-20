//! Adversarial tests for the text core: history, motion, count and search.
//!
//! Everything here goes through the public API. The property tests are driven
//! by a small xorshift generator and compare the crate against reference
//! models written straight from the contract, as plainly and slowly as
//! possible, over text chosen to be unpleasant: stacked combining marks, wide
//! characters, emoji joined by U+200D, right-to-left text, U+0130, blank
//! lines, lines of only spaces, tabs, very long unbroken words, and positions
//! that are out of range or inside a multi-byte character.
//!
//! Where the contract leaves a choice open the models follow the choice the
//! implementation documents, and the test says so in a comment.
//!
//! Five tests here failed when they were written, and the source was changed
//! to pass them: two for word characters (a combining mark on something that
//! is not a word character, and an alphanumeric character that is also a
//! combining mark), and three under "Cost per key" for work that grew with
//! the length of a run or a needle. The two for word characters have since
//! been turned round with the contract: a mark is whatever it sits on, so
//! that no key stops between a character and a mark on it.
//!
//! For a longer hunt set `OK_TEXT_FUZZ` to a whole number to multiply the
//! rounds of every property test, and `OK_TEXT_SEED` to change every seed.

use ok_text::count::{self, Tally};
use ok_text::doc::normalize_newlines;
use ok_text::history::{Cursor, EditKind, History, Replay};
use ok_text::motion;
use ok_text::search;
use ok_text::{is_combining, Change, Doc, Pos, Span};

// ---------------------------------------------------------------------------
// Shared tools
// ---------------------------------------------------------------------------

fn env_number(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|value| value.trim().parse().ok())
}

/// How many rounds a property test runs: `n`, times the whole number in the
/// environment variable `OK_TEXT_FUZZ` when it is set, for a longer hunt.
/// `OK_TEXT_SEED` changes every seed, for a different one.
fn rounds(n: usize) -> usize {
    n * env_number("OK_TEXT_FUZZ").map_or(1, |scale| scale.max(1) as usize)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let salt = env_number("OK_TEXT_SEED").unwrap_or(0).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        Rng((seed ^ salt).max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            ((self.next() >> 11) % n as u64) as usize
        }
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

fn p(line: usize, col: usize) -> Pos {
    Pos::new(line, col)
}

fn sp(line: usize, start: usize, end: usize) -> Span {
    Span { start: p(line, start), end: p(line, end) }
}

fn text_of(doc: &Doc) -> String {
    doc.to_text("\n")
}

/// Text chosen to be unpleasant.
const HOSTILE: &[&str] = &[
    // Plain words, with apostrophes, hyphens, underscores and digits.
    "a",
    "I",
    "b",
    "word",
    "Word",
    "two words",
    "Don't",
    "it\u{2019}s",
    "rock'n'roll",
    "dogs'",
    "'tis",
    "well-known",
    "x_y",
    "_",
    "42",
    "3.14",
    "e.g.",
    "etc.",
    "Mr.",
    "U.S.",
    // Sentence punctuation and closers.
    ".",
    ". ",
    "!",
    "? ",
    "\u{2026}",
    "...",
    "?!",
    ".\"",
    ".'",
    ".)",
    ".]",
    ".\u{201D}",
    ".\u{2019}",
    "\"",
    "'",
    "''",
    "\u{2018}",
    "\u{2019}",
    "\u{201C}",
    "\u{201D}",
    "(",
    ")",
    "[",
    "]",
    ",",
    ";",
    "-",
    "\u{2014}",
    "#",
    "**",
    "[[",
    "]]",
    // Whitespace of several kinds.
    " ",
    " ",
    " ",
    "  ",
    "\t",
    "\u{00A0}",
    "\u{3000}",
    "\u{2028}",
    "\u{0085}",
    "\u{000B}",
    // Line breaks, blank lines and lines of only whitespace.
    "\n",
    "\n",
    "\n\n",
    " \n",
    "\n   \n",
    "\n\t\n",
    "\r\n",
    "\r",
    // Combining marks: alone, stacked, with nothing to sit on, alphabetic ones.
    "\u{0301}",
    "\u{0313}",
    "n\u{0313}",
    "e\u{0301}\u{0302}\u{0303}\u{0304}",
    "\u{0345}",
    "\u{FE0F}",
    "\u{200D}",
    "\u{200B}",
    "\u{200E}",
    "\u{200F}",
    "\u{202E}",
    "\u{FEFF}",
    "\u{20DD}",
    // Wide characters.
    "\u{4E00}",
    "\u{4E8C}\u{4E09}",
    "\u{3042}",
    "\u{FF21}",
    "\u{AC00}",
    "\u{3002}",
    // Emoji, some joined by U+200D, some with selectors and modifiers.
    "\u{1F600}",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{2764}\u{FE0F}",
    "\u{1F1FA}\u{1F1F8}",
    "\u{1F44D}\u{1F3FD}",
    // Right-to-left text, with points and marks.
    "\u{05E9}\u{05DC}\u{05D5}\u{05DD}",
    "\u{05E9}\u{05B8}",
    "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}",
    "\u{0643}\u{0650}",
    "\u{061F}",
    // Thai, whose vowels are combining.
    "\u{0E01}\u{0E34}",
    "\u{0E31}",
    // Characters whose lowercase form is a different length.
    "\u{0130}",
    "i\u{0307}",
    "\u{0131}",
    "\u{1E9E}",
    "\u{00DF}",
    "\u{212A}",
    "K",
    "k",
    "\u{03A3}",
    "\u{03C3}",
    "\u{03C2}",
    "\u{01C5}",
    "\u{2126}",
    "\u{00C9}",
    "\u{00E9}",
    // Numbers that are not digits.
    "\u{00BD}",
    "\u{0663}",
];

fn random_text(rng: &mut Rng, max_pieces: usize) -> String {
    let mut text = String::new();
    for _ in 0..rng.below(max_pieces + 1) {
        text.push_str(rng.pick(HOSTILE));
    }
    text
}

/// A character from anywhere in Unicode, leaning towards the ranges where the
/// rules have something to decide: marks, joiners and selectors, every kind
/// of space, case pairs, right-to-left scripts, wide characters and emoji.
fn random_char(rng: &mut Rng) -> char {
    const RANGES: &[(u32, u32)] = &[
        (0x20, 0x7E),
        (0x20, 0x7E),
        (0x09, 0x0D),
        (0x80, 0x24F),
        (0x2B0, 0x36F),
        (0x370, 0x3FF),
        (0x590, 0x6FF),
        (0xE00, 0xE7F),
        (0x1680, 0x1680),
        (0x1AB0, 0x1AFF),
        (0x1DC0, 0x1DFF),
        (0x1E00, 0x1FFF),
        (0x2000, 0x206F),
        (0x20D0, 0x20FF),
        (0x2100, 0x218F),
        (0x2460, 0x24FF),
        (0x3000, 0x30FF),
        (0x4E00, 0x4E7F),
        (0xFE00, 0xFE2F),
        (0xFEFF, 0xFEFF),
        (0xFF00, 0xFFEF),
        (0x1F1E6, 0x1F1FF),
        (0x1F300, 0x1F64F),
        (0xE0100, 0xE01EF),
        (0, 0x10FFFF),
        (0, 0x10FFFF),
    ];
    loop {
        let (lo, hi) = RANGES[rng.below(RANGES.len())];
        if let Some(ch) = char::from_u32(lo + rng.below((hi - lo + 1) as usize) as u32) {
            return ch;
        }
    }
}

/// Text of arbitrary characters with enough spaces, stops and line breaks
/// among them for words, sentences and paragraphs to form.
fn random_unicode_text(rng: &mut Rng, max_pieces: usize) -> String {
    let glue: &[&str] = &[" ", " ", " ", ". ", ".", "\n", "\n\n", "'", "\u{2019}", "? ", "\"", ")"];
    let mut text = String::new();
    for _ in 0..rng.below(max_pieces + 1) {
        match rng.below(10) {
            0..=5 => text.push(random_char(rng)),
            6..=8 => text.push_str(rng.pick(glue)),
            _ => text.push_str(rng.pick(HOSTILE)),
        }
    }
    text
}

/// Every position worth trying in a small document: each byte column of each
/// line (so inside multi-byte characters too), one past the end, and a few
/// that are far out of range.
fn positions(doc: &Doc) -> Vec<Pos> {
    let mut out = Vec::new();
    for line in 0..doc.line_count() {
        for col in 0..=doc.line(line).len() + 1 {
            out.push(p(line, col));
        }
        out.push(p(line, usize::MAX));
    }
    out.push(p(doc.line_count(), 0));
    out.push(p(doc.line_count() + 3, 7));
    out.push(p(usize::MAX, 0));
    out.push(p(usize::MAX, usize::MAX));
    out
}

fn random_pos(rng: &mut Rng, doc: &Doc) -> Pos {
    match rng.below(14) {
        0 => p(usize::MAX, usize::MAX),
        1 => p(rng.below(doc.line_count()), usize::MAX),
        2 => p(doc.line_count() + rng.below(3), rng.below(5)),
        _ => {
            let line = rng.below(doc.line_count());
            p(line, rng.below(doc.line(line).len() + 2))
        }
    }
}

/// Reads a picture of a document in which each `|` marks a position.
fn marked(picture: &str) -> (Doc, Vec<Pos>) {
    let mut text = String::new();
    let mut marks = Vec::new();
    let (mut line, mut col) = (0, 0);
    for ch in picture.chars() {
        match ch {
            '|' => marks.push(p(line, col)),
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
    (Doc::from_text(&text), marks)
}

type Motion = fn(&Doc, Pos) -> Pos;

/// `forward` steps from each mark to the next.
fn steps(picture: &str, forward: Motion) {
    let (d, marks) = marked(picture);
    assert!(marks.len() > 1, "{picture:?} needs at least two marks");
    for pair in marks.windows(2) {
        assert_eq!(forward(&d, pair[0]), pair[1], "forward from {:?} in {picture:?}", pair[0]);
    }
}

/// `backward` steps from each mark to the one before.
fn steps_back(picture: &str, backward: Motion) {
    let (d, marks) = marked(picture);
    assert!(marks.len() > 1, "{picture:?} needs at least two marks");
    for pair in marks.windows(2) {
        assert_eq!(backward(&d, pair[1]), pair[0], "backward from {:?} in {picture:?}", pair[1]);
    }
}

fn walks(picture: &str, forward: Motion, backward: Motion) {
    steps(picture, forward);
    steps_back(picture, backward);
}

// ---------------------------------------------------------------------------
// Reference models, written from the contract
// ---------------------------------------------------------------------------

/// Where clusters begin and end on a line: a base character and the
/// combining marks that follow it.
fn ref_cluster_edges(line: &str) -> Vec<usize> {
    let mut edges = vec![0];
    for (i, ch) in line.char_indices() {
        if i > 0 && !is_combining(ch) {
            edges.push(i);
        }
    }
    if !line.is_empty() {
        edges.push(line.len());
    }
    edges
}

fn is_apostrophe(ch: char) -> bool {
    ch == '\'' || ch == '\u{2019}'
}

fn is_plain_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || is_combining(ch)
}

/// Whether each character of a line is a word character, by the letter of
/// the contract: "alphanumeric, an underscore, or an apostrophe with word
/// characters on both sides; a combining mark is whatever the character it
/// sits on is, and a word character when it opens a line."
///
/// The contract's rule for the apostrophe refers to itself; as the
/// implementation documents, the sides are the characters straight before
/// and after, one apostrophe never counts as the word character beside
/// another, and a mark straight after an apostrophe counts on that side.
fn ref_word_classes(line: &str) -> Vec<(usize, bool)> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut out: Vec<(usize, bool)> = Vec::with_capacity(chars.len());
    for (k, &(i, ch)) in chars.iter().enumerate() {
        let wordy = if is_apostrophe(ch) {
            let left = k > 0 && out[k - 1].1;
            let right = chars.get(k + 1).is_some_and(|&(_, c)| is_plain_word_char(c));
            left && right
        } else if is_combining(ch) {
            k == 0 || out[k - 1].1
        } else {
            is_plain_word_char(ch)
        };
        out.push((i, wordy));
    }
    out
}

fn ref_words(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, wordy) in ref_word_classes(line) {
        match (start, wordy) {
            (None, true) => start = Some(i),
            (Some(from), false) => {
                out.push((from, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push((from, line.len()));
    }
    out
}

fn is_terminator(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | '\u{2026}')
}

fn is_closer(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '\u{201D}' | '\u{2019}' | ')' | ']')
}

/// The sentences of one line as byte ranges, by the contract's rule.
///
/// A sentence ends just before character `k` when `k` is the end of the line
/// or whitespace, and what comes before `k` is one or more terminators and
/// then any closers; unless the character before `k` is a period and the
/// first thing after the whitespace is a lowercase letter. A sentence that
/// finds no such end runs to the last character on the line that is not
/// whitespace.
fn ref_sentences(line: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let n = chars.len();
    let off = |k: usize| if k < n { chars[k].0 } else { line.len() };
    let is_end = |k: usize| -> bool {
        if k == 0 || (k < n && !chars[k].1.is_whitespace()) {
            return false;
        }
        let mut j = k;
        while j > 0 && is_closer(chars[j - 1].1) {
            j -= 1;
        }
        if j == 0 || !is_terminator(chars[j - 1].1) {
            return false;
        }
        if chars[k - 1].1 == '.' && k < n {
            let mut m = k;
            while m < n && chars[m].1.is_whitespace() {
                m += 1;
            }
            if m < n && chars[m].1.is_lowercase() {
                return false;
            }
        }
        true
    };
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        // Whitespace, and with it any mark that sits on whitespace.
        let mut on_space = false;
        while i < n {
            on_space = chars[i].1.is_whitespace() || (on_space && is_combining(chars[i].1));
            if !on_space {
                break;
            }
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        let mut end = start + 1;
        while end <= n && !is_end(end) {
            end += 1;
        }
        if end > n {
            end = n;
            while end > start && chars[end - 1].1.is_whitespace() {
                end -= 1;
            }
        }
        out.push((off(start), off(end)));
        i = end;
    }
    out
}

/// Everything the motion models need to know about a document.
struct Reference {
    edges: Vec<Vec<usize>>,
    words: Vec<Vec<(usize, usize)>>,
    sentences: Vec<Vec<(usize, usize)>>,
    blank: Vec<bool>,
}

impl Reference {
    fn of(doc: &Doc) -> Reference {
        let lines = doc.lines();
        Reference {
            edges: lines.iter().map(|l| ref_cluster_edges(l)).collect(),
            words: lines.iter().map(|l| ref_words(l)).collect(),
            sentences: lines.iter().map(|l| ref_sentences(l)).collect(),
            blank: lines.iter().map(|l| l.chars().all(char::is_whitespace)).collect(),
        }
    }

    fn above(doc: &Doc, c: Pos) -> Pos {
        if c.line > 0 {
            p(c.line - 1, doc.line(c.line - 1).len())
        } else {
            c
        }
    }

    fn below(doc: &Doc, c: Pos) -> Pos {
        if c.line + 1 < doc.line_count() {
            p(c.line + 1, 0)
        } else {
            c
        }
    }

    fn right(&self, doc: &Doc, c: Pos) -> Pos {
        if c.col >= doc.line(c.line).len() {
            return Reference::below(doc, c);
        }
        let col = self.edges[c.line].iter().copied().find(|&e| e > c.col).expect("an edge at the line end");
        p(c.line, col)
    }

    fn left(&self, doc: &Doc, c: Pos) -> Pos {
        if c.col == 0 {
            return Reference::above(doc, c);
        }
        let col = self.edges[c.line].iter().rev().copied().find(|&e| e < c.col).expect("an edge at column 0");
        p(c.line, col)
    }

    fn word_right(&self, doc: &Doc, c: Pos) -> Pos {
        let len = doc.line(c.line).len();
        if c.col >= len {
            return Reference::below(doc, c);
        }
        let col = self.words[c.line].iter().map(|w| w.0).find(|&s| s > c.col).unwrap_or(len);
        p(c.line, col)
    }

    /// With no word before `c` on the line the contract does not say where to
    /// stop; the implementation documents column 0, mirroring `word_right`.
    fn word_left(&self, doc: &Doc, c: Pos) -> Pos {
        if c.col == 0 {
            return Reference::above(doc, c);
        }
        let col = self.words[c.line].iter().rev().map(|w| w.0).find(|&s| s < c.col).unwrap_or(0);
        p(c.line, col)
    }

    /// The ends `word_end` may give. From the exact end of a word the
    /// contract can be read either way (stay, or go on to the next word), so
    /// both are allowed there.
    fn word_end(&self, doc: &Doc, c: Pos) -> Vec<Pos> {
        let len = doc.line(c.line).len();
        let onward = self.words[c.line].iter().map(|w| w.1).find(|&e| e > c.col).unwrap_or(len);
        let mut allowed = vec![p(c.line, onward)];
        if self.words[c.line].iter().any(|w| w.1 == c.col) {
            allowed.push(c);
        }
        allowed
    }

    fn word_at(&self, c: Pos) -> Span {
        match self.words[c.line].iter().find(|w| w.0 <= c.col && c.col <= w.1) {
            Some(&(start, end)) => sp(c.line, start, end),
            None => Span::at(c),
        }
    }

    fn sentence_next(&self, doc: &Doc, c: Pos) -> Pos {
        if let Some(&(start, _)) = self.sentences[c.line].iter().find(|s| s.0 > c.col) {
            return p(c.line, start);
        }
        for line in c.line + 1..doc.line_count() {
            if let Some(&(start, _)) = self.sentences[line].first() {
                return p(line, start);
            }
        }
        doc.end()
    }

    fn sentence_prev(&self, c: Pos) -> Pos {
        if let Some(&(start, _)) = self.sentences[c.line].iter().rev().find(|s| s.0 < c.col) {
            return p(c.line, start);
        }
        for line in (0..c.line).rev() {
            if let Some(&(start, _)) = self.sentences[line].last() {
                return p(line, start);
            }
        }
        p(0, 0)
    }

    /// Which sentence whitespace belongs to is the implementation's choice:
    /// the one before it, or the first on the line.
    fn sentence_at(&self, c: Pos) -> Span {
        let list = &self.sentences[c.line];
        match list.iter().rev().find(|s| s.0 <= c.col).or(list.first()) {
            Some(&(start, end)) => sp(c.line, start, end),
            None => Span::at(c),
        }
    }

    fn starts_paragraph(&self, line: usize) -> bool {
        !self.blank[line] && (line == 0 || self.blank[line - 1])
    }

    fn own_paragraph_start(&self, line: usize) -> usize {
        (0..=line).rev().find(|&l| self.starts_paragraph(l)).expect("a non-blank line is in a paragraph")
    }

    fn paragraph_next(&self, doc: &Doc, c: Pos) -> Pos {
        (c.line + 1..doc.line_count()).find(|&l| self.starts_paragraph(l)).map_or(doc.end(), |l| p(l, 0))
    }

    fn paragraph_prev(&self, c: Pos) -> Pos {
        let mut from = c.line;
        if !self.blank[c.line] {
            let own = self.own_paragraph_start(c.line);
            if c != p(own, 0) {
                return p(own, 0);
            }
            from = own;
        }
        (0..from).rev().find(|&l| self.starts_paragraph(l)).map_or(p(0, 0), |l| p(l, 0))
    }

    fn paragraph_at(&self, doc: &Doc, c: Pos) -> Span {
        if self.blank[c.line] {
            return Span::at(c);
        }
        let first = self.own_paragraph_start(c.line);
        let last = (c.line..doc.line_count()).take_while(|&l| !self.blank[l]).last().expect("its own line");
        Span { start: p(first, 0), end: p(last, doc.line(last).len()) }
    }
}

fn ref_count_words(s: &str) -> usize {
    let mut count = 0;
    let mut in_token = false;
    let mut counts = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if in_token && counts {
                count += 1;
            }
            in_token = false;
            counts = false;
        } else {
            in_token = true;
            counts |= ch.is_alphanumeric();
        }
    }
    if in_token && counts {
        count += 1;
    }
    count
}

fn ref_count_clusters(s: &str) -> usize {
    let mut count = 0;
    for ch in s.chars() {
        if ch != '\n' && ch != '\r' && !is_combining(ch) {
            count += 1;
        }
    }
    count
}

fn ref_count_sentences(s: &str) -> usize {
    normalize_newlines(s).split('\n').map(|line| ref_sentences(line).len()).sum()
}

fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// The matches in one line: at each character in turn, grow a candidate one
/// whole character at a time until its (lowercased) text is as long as the
/// needle's, and compare.
fn ref_find_line(text: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let want = if case_sensitive { needle.to_string() } else { fold(needle) };
    let mut out = Vec::new();
    if want.is_empty() {
        return out;
    }
    let mut i = 0;
    while i < text.len() {
        let mut have = String::new();
        let mut end = None;
        for (j, ch) in text[i..].char_indices() {
            if case_sensitive {
                have.push(ch);
            } else {
                have.extend(ch.to_lowercase());
            }
            if have.len() >= want.len() {
                if have == want {
                    end = Some(i + j + ch.len_utf8());
                }
                break;
            }
        }
        match end {
            Some(end) => {
                out.push((i, end));
                i = end;
            }
            None => i += text[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

fn ref_find_all(doc: &Doc, needle: &str, case_sensitive: bool) -> Vec<Span> {
    let mut out = Vec::new();
    if needle.contains('\n') || needle.contains('\r') {
        return out;
    }
    for (line, text) in doc.lines().iter().enumerate() {
        out.extend(ref_find_line(text, needle, case_sensitive).into_iter().map(|(a, b)| sp(line, a, b)));
    }
    out
}

// ---------------------------------------------------------------------------
// motion: properties
// ---------------------------------------------------------------------------

fn check_motions_at(doc: &Doc, r: &Reference, q: Pos) {
    let c = doc.clamp(q);
    let first = p(0, 0);
    let last = doc.end();
    let ctx = || format!("from {q:?} (clamped {c:?}) in {:?}", doc.lines());

    let simple: [(&str, Motion, Pos); 12] = [
        ("left", motion::left, r.left(doc, c)),
        ("right", motion::right, r.right(doc, c)),
        ("word_left", motion::word_left, r.word_left(doc, c)),
        ("word_right", motion::word_right, r.word_right(doc, c)),
        ("sentence_next", motion::sentence_next, r.sentence_next(doc, c)),
        ("sentence_prev", motion::sentence_prev, r.sentence_prev(c)),
        ("paragraph_next", motion::paragraph_next, r.paragraph_next(doc, c)),
        ("paragraph_prev", motion::paragraph_prev, r.paragraph_prev(c)),
        ("line_start", motion::line_start, p(c.line, 0)),
        ("line_end", motion::line_end, p(c.line, doc.line(c.line).len())),
        ("doc_start", motion::doc_start, first),
        ("doc_end", motion::doc_end, last),
    ];
    for (name, f, want) in simple {
        let got = f(doc, q);
        assert_eq!(got, want, "{name} {}", ctx());
        // Total and pure: in the document, on a character boundary, the same twice.
        assert_eq!(doc.clamp(got), got, "{name} left the document {}", ctx());
        assert_eq!(f(doc, q), got, "{name} is not pure {}", ctx());
    }

    let end = motion::word_end(doc, q);
    assert!(r.word_end(doc, c).contains(&end), "word_end gave {end:?} {}", ctx());
    assert_eq!(end.line, c.line, "word_end left the line {}", ctx());

    assert_eq!(motion::word_at(doc, q), r.word_at(c), "word_at {}", ctx());
    assert_eq!(motion::sentence_at(doc, q), r.sentence_at(c), "sentence_at {}", ctx());
    assert_eq!(motion::paragraph_at(doc, q), r.paragraph_at(doc, c), "paragraph_at {}", ctx());

    // Straight from the contract, without the models.
    let sentence = motion::sentence_at(doc, q);
    let said = doc.slice(sentence);
    assert!(!said.starts_with(char::is_whitespace) && !said.ends_with(char::is_whitespace), "sentence_at {}", ctx());
    assert!(!said.contains('\n'), "sentence_at crossed a line {}", ctx());
    let next = motion::sentence_next(doc, q);
    assert!(next > c || (next == c && c == last), "sentence_next did not move {}", ctx());
    if next != last {
        let line = doc.line(next.line);
        assert!(line[next.col..].chars().next().is_some_and(|ch| !ch.is_whitespace()), "sentence_next {}", ctx());
        // Whitespace comes before it, or a mark that sits on whitespace.
        assert!(line[..next.col].chars().next_back().is_none_or(|ch| ch.is_whitespace() || is_combining(ch)), "sentence_next {}", ctx());
        assert!(next.col == 0 || line[next.col..].chars().next().is_some_and(|ch| !is_combining(ch)), "sentence_next stopped inside a cluster {}", ctx());
    }
    let paragraph = motion::paragraph_at(doc, q);
    assert!(paragraph.start <= c && c <= paragraph.end, "paragraph_at {}", ctx());
    for forward in [motion::right as Motion, motion::word_right, motion::paragraph_next] {
        let to = forward(doc, q);
        assert!(to > c || (to == c && c == last), "a forward motion did not move {}", ctx());
    }
    for backward in [motion::left as Motion, motion::word_left, motion::sentence_prev, motion::paragraph_prev] {
        let to = backward(doc, q);
        assert!(to < c || (to == c && c == first), "a backward motion did not move {}", ctx());
    }
}

fn check_motions(doc: &Doc) {
    let r = Reference::of(doc);
    for q in positions(doc) {
        check_motions_at(doc, &r, q);
    }
}

#[test]
fn motions_agree_with_the_reference_models_on_hostile_text() {
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    for _ in 0..rounds(2500) {
        let doc = Doc::from_text(&random_text(&mut rng, 22));
        check_motions(&doc);
    }
}

#[test]
fn motions_agree_with_the_reference_models_on_sentence_heavy_text() {
    // The general mix seldom lines up terminators, closers and capitals; this one does little else.
    #[rustfmt::skip]
    let pieces: &[&str] = &[
        "a", "B", "word", "Word", " ", " ", "  ", "\t", ".", ".", "!", "?", "\u{2026}", "\"", "'", "\u{201D}",
        "\u{2019}", ")", "]", "(", "\u{201C}", "\n", "\n\n", " \n", "e.g.", "3.14", "\u{00E9}", "\u{0301}", "\u{00A0}",
        "\u{0345}", "\u{4E00}", "1",
    ];
    let mut rng = Rng::new(0x0123_4567_89AB_CDEF);
    for _ in 0..rounds(4000) {
        let mut text = String::new();
        for _ in 0..rng.below(26) {
            text.push_str(rng.pick(pieces));
        }
        check_motions(&Doc::from_text(&text));
    }
}

#[test]
fn motions_agree_with_the_reference_models_on_word_heavy_text() {
    #[rustfmt::skip]
    let pieces: &[&str] = &[
        "a", "b", "Z", "9", "_", "'", "'", "\u{2019}", "\u{2019}", "\u{0301}", "\u{0313}", "\u{FE0F}", "\u{200D}",
        " ", " ", "-", ",", "\n", "\u{2764}", "\u{1F468}", "\u{4E00}", "\u{05E9}", "\u{0E31}", "\u{0345}", "\u{00E9}",
    ];
    let mut rng = Rng::new(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..rounds(4000) {
        let mut text = String::new();
        for _ in 0..rng.below(20) {
            text.push_str(rng.pick(pieces));
        }
        check_motions(&Doc::from_text(&text));
    }
}

#[test]
fn motions_agree_with_the_reference_models_on_blank_line_heavy_text() {
    let pieces: &[&str] = &["\n", "\n", "\n", " ", "\t", "a", "b.", "\u{3000}", "\u{200B}", " \n", "x y"];
    let mut rng = Rng::new(0x1357_9BDF_0246_8ACE);
    for _ in 0..rounds(3000) {
        let mut text = String::new();
        for _ in 0..rng.below(24) {
            text.push_str(rng.pick(pieces));
        }
        check_motions(&Doc::from_text(&text));
    }
}

#[test]
fn motions_hold_up_on_very_long_unbroken_words_and_lines() {
    let mut rng = Rng::new(0x5555_AAAA_1234_9876);
    let long_word = "a".repeat(20_000);
    let long_marks = format!("e{}", "\u{0301}".repeat(6_000));
    let long_spaces = " ".repeat(15_000);
    let long_dots = ". ".repeat(8_000);
    let long_quotes = "'".repeat(9_000);
    let long_wide = "\u{4E00}".repeat(7_000);
    let long_emoji = "\u{1F468}\u{200D}".repeat(4_000);
    let parts = [&long_word, &long_marks, &long_spaces, &long_dots, &long_quotes, &long_wide, &long_emoji];
    for round in 0..14 {
        let mut text = String::new();
        for _ in 0..1 + rng.below(4) {
            text.push_str(parts[rng.below(parts.len())]);
            text.push_str(rng.pick(HOSTILE));
        }
        if round % 2 == 0 {
            text.push('\n');
            text.push_str(parts[rng.below(parts.len())]);
        }
        let doc = Doc::from_text(&text);
        let r = Reference::of(&doc);
        for _ in 0..30 {
            let q = random_pos(&mut rng, &doc);
            check_motions_at(&doc, &r, q);
        }
        check_motions_at(&doc, &r, p(0, 0));
        check_motions_at(&doc, &r, doc.end());
    }
}

#[test]
fn stepping_right_then_left_visits_the_same_clusters() {
    let mut rng = Rng::new(0x0F0F_F0F0_1234_4321);
    for _ in 0..rounds(600) {
        let doc = Doc::from_text(&random_text(&mut rng, 25));
        let limit = doc.len_bytes() + doc.line_count() + 2;
        let mut there = vec![p(0, 0)];
        while there[there.len() - 1] != doc.end() {
            assert!(there.len() <= limit, "right stalls in {:?}", doc.lines());
            there.push(motion::right(&doc, there[there.len() - 1]));
        }
        let mut back = vec![doc.end()];
        while back[back.len() - 1] != p(0, 0) {
            assert!(back.len() <= limit, "left stalls in {:?}", doc.lines());
            back.push(motion::left(&doc, back[back.len() - 1]));
        }
        back.reverse();
        assert_eq!(there, back, "in {:?}", doc.lines());
        // Clusters counted by walking are the clusters count.rs counts.
        let walked = there.len() - 1 - (doc.line_count() - 1);
        let orphans = doc.lines().iter().filter(|l| l.chars().next().is_some_and(is_combining)).count();
        assert_eq!(walked, count::clusters(&text_of(&doc)) + orphans, "in {:?}", doc.lines());
    }
}

#[test]
fn every_unit_walks_from_one_end_of_the_document_to_the_other() {
    let pairs: [(&str, Motion, Motion); 3] = [
        ("word", motion::word_right, motion::word_left),
        ("sentence", motion::sentence_next, motion::sentence_prev),
        ("paragraph", motion::paragraph_next, motion::paragraph_prev),
    ];
    let mut rng = Rng::new(0x7777_3333_1111_9999);
    for _ in 0..rounds(500) {
        let doc = Doc::from_text(&random_text(&mut rng, 25));
        let limit = doc.len_bytes() + doc.line_count() + 2;
        for (name, forward, backward) in pairs {
            let mut at = p(0, 0);
            let mut taken = 0;
            while at != doc.end() {
                at = forward(&doc, at);
                taken += 1;
                assert!(taken <= limit, "{name} forward stalls in {:?}", doc.lines());
            }
            taken = 0;
            while at != p(0, 0) {
                at = backward(&doc, at);
                taken += 1;
                assert!(taken <= limit, "{name} backward stalls in {:?}", doc.lines());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// motion: one case for each sentence of the contract
// ---------------------------------------------------------------------------

#[test]
fn left_and_right_move_one_base_character_with_the_marks_that_follow_it() {
    walks("|a|e\u{0301}\u{0302}\u{0303}|n\u{0313}|\u{4E00}|\u{1F600}|z|", motion::right, motion::left);
    // U+200D and U+FE0F are combining by width::is_combining, so they stay with what they follow.
    walks("|\u{1F468}\u{200D}|\u{1F469}\u{200D}|\u{1F467}|\u{2764}\u{FE0F}|", motion::right, motion::left);
    walks("|\u{05E9}\u{05B8}|\u{05DC}\u{200F}|\u{05D5}|", motion::right, motion::left);
}

#[test]
fn left_and_right_cross_line_breaks() {
    walks("|a|\n|\n|b|\n|", motion::right, motion::left);
}

#[test]
fn left_and_right_stop_at_the_ends_of_the_document() {
    let d = Doc::from_text("ab\ncd");
    assert_eq!(motion::left(&d, p(0, 0)), p(0, 0));
    assert_eq!(motion::right(&d, p(1, 2)), p(1, 2));
    let empty = Doc::new();
    assert_eq!(motion::left(&empty, p(0, 0)), p(0, 0));
    assert_eq!(motion::right(&empty, p(0, 0)), p(0, 0));
}

#[test]
fn word_right_skips_the_rest_of_the_word_then_what_is_not_a_word_to_the_next_word_start() {
    steps("|alpha, (|beta) -- |gamma|", motion::word_right);
    let (d, at) = marked("al|pha ...  |beta");
    assert_eq!(motion::word_right(&d, at[0]), at[1]);
}

#[test]
fn word_right_stops_at_the_line_end_when_no_word_follows() {
    steps("one| ...|", motion::word_right);
    steps("|one|", motion::word_right);
    steps("| ?!  |", motion::word_right);
}

#[test]
fn word_right_from_the_end_of_a_line_goes_to_column_0_of_the_next() {
    steps("one ...|\n|   |two|\n|", motion::word_right);
    // On the last line there is nowhere to go.
    let d = Doc::from_text("one\ntwo");
    assert_eq!(motion::word_right(&d, p(1, 3)), p(1, 3));
}

#[test]
fn word_left_stops_at_the_start_of_the_word_before() {
    steps_back("|alpha, (|beta) -- |gamma|", motion::word_left);
    let (d, at) = marked("alpha |be|ta");
    assert_eq!(motion::word_left(&d, at[1]), at[0]);
    let (d, at) = marked("|alpha ... |");
    assert_eq!(motion::word_left(&d, at[1]), at[0]);
}

#[test]
fn word_left_from_column_0_goes_to_the_end_of_the_previous_line() {
    steps_back("one ...|\n|two", motion::word_left);
    steps_back("|\n|\n|x", motion::word_left);
    let d = Doc::from_text("one\ntwo");
    assert_eq!(motion::word_left(&d, p(0, 0)), p(0, 0));
}

#[test]
fn word_end_gives_the_end_of_the_current_word() {
    let (d, at) = marked("|al|ph|a| beta");
    for from in &at[..3] {
        assert_eq!(motion::word_end(&d, *from), at[3], "from {from:?}");
    }
}

#[test]
fn word_end_gives_the_end_of_the_next_word_from_between_words() {
    let (d, at) = marked("alpha,| |(|beta|) x");
    for from in &at[..3] {
        assert_eq!(motion::word_end(&d, *from), at[3], "from {from:?}");
    }
}

#[test]
fn word_end_never_leaves_the_line() {
    let (d, at) = marked("alpha |.|.. |\nbeta");
    for from in &at {
        assert_eq!(motion::word_end(&d, *from), at[2], "from {from:?}");
    }
    let d = Doc::from_text("\nbeta");
    assert_eq!(motion::word_end(&d, p(0, 0)), p(0, 0));
}

#[test]
fn letters_digits_and_underscores_are_word_characters() {
    let (d, at) = marked("- |x_1\u{00E9}\u{4E00}\u{05E9}\u{0663}_| -");
    let whole = Span { start: at[0], end: at[1] };
    assert_eq!(motion::word_at(&d, p(0, 4)), whole);
    assert_eq!(motion::word_end(&d, at[0]), at[1]);
    assert_eq!(motion::word_left(&d, at[1]), at[0]);
}

#[test]
fn a_combining_mark_on_a_word_character_is_part_of_the_word() {
    // The contract's own example: n followed by U+0313.
    let (d, at) = marked("say |mitsqanaqan\u{0313}| now");
    let whole = Span { start: at[0], end: at[1] };
    assert_eq!(motion::word_at(&d, at[0]), whole);
    assert_eq!(motion::word_at(&d, at[1]), whole);
    assert_eq!(motion::word_end(&d, at[0]), at[1]);
    let (d, at) = marked("|n\u{0313}a| |e\u{0301}\u{0302}\u{0303}x|");
    assert_eq!(motion::word_at(&d, at[0]), Span { start: at[0], end: at[1] });
    assert_eq!(motion::word_at(&d, at[3]), Span { start: at[2], end: at[3] });
    assert_eq!(motion::word_right(&d, at[0]), at[2]);
}

#[test]
fn an_apostrophe_with_word_characters_on_both_sides_is_a_word_character() {
    for picture in ["say |don't| now", "say |it\u{2019}s| now", "|rock'n'roll| x", "|o'_| x", "|n\u{0313}'t|"] {
        let (d, at) = marked(picture);
        let whole = Span { start: at[0], end: at[1] };
        assert_eq!(motion::word_at(&d, at[0]), whole, "{picture:?}");
        assert_eq!(motion::word_at(&d, at[1]), whole, "{picture:?}");
        assert_eq!(motion::word_end(&d, at[0]), at[1], "{picture:?}");
    }
}

#[test]
fn an_apostrophe_without_word_characters_on_both_sides_is_not_a_word_character() {
    for picture in ["'|quoted|' x", "\u{2018}|quoted|\u{2019} x", "the |dogs|' bowl", "'|tis| so", "a |b|' 'c"] {
        let (d, at) = marked(picture);
        let whole = Span { start: at[0], end: at[1] };
        assert_eq!(motion::word_at(&d, at[0]), whole, "{picture:?}");
        assert_eq!(motion::word_at(&d, at[1]), whole, "{picture:?}");
    }
    // A lone apostrophe is no word at all.
    let (d, at) = marked("a |'| b");
    assert_eq!(motion::word_at(&d, at[0]), Span::at(at[0]));
    assert_eq!(motion::word_right(&d, p(0, 0)), p(0, 4));
}

/// The contract: "A combining mark belongs to what it sits on." DESIGN.md
/// has it that every cell is a character in the file, "so arrow keys never
/// lie": the selector after a heart shares the heart's cell, so there is
/// nothing between the two for Ctrl+arrow to stop at, and nothing for Ctrl
/// Backspace to cut. (For a while the contract read "a combining mark is a
/// word character" whatever it sat on, which made the selector a word by
/// itself; the attack on the keys found that stop from the screen's side and
/// the wording was changed.)
#[test]
fn a_combining_mark_is_a_word_character_only_on_a_word_character() {
    let (d, at) = marked("I \u{2764}|\u{FE0F}| |you");
    assert_eq!(motion::word_at(&d, at[0]), Span::at(at[0]));
    assert_eq!(motion::word_right(&d, p(0, 0)), at[2]);
    assert_eq!(motion::word_left(&d, at[1]), p(0, 0));
    let (d, at) = marked("|n\u{0313}| .\u{0301} |\u{0301}");
    assert_eq!(motion::word_at(&d, at[1]), Span { start: at[0], end: at[1] });
    assert_eq!(motion::word_right(&d, at[0]), d.end());
    // With nothing to sit on, at the start of a line, it is one.
    let (d, at) = marked("|\u{0301}a| b");
    assert_eq!(motion::word_at(&d, at[0]), Span { start: at[0], end: at[1] });
}

/// 128 characters are both alphanumeric and combining by
/// `width::is_combining`: the Hebrew points, the Arabic vowel signs, the
/// Thai vowels written above and below, U+0345. `count::words` goes by
/// tokens between whitespace and counts one of these on a dotted circle as
/// a word; the word motions go by what the mark sits on, and do not stop
/// between the circle and its point.
#[test]
fn a_mark_that_is_also_a_letter_still_belongs_to_what_it_sits_on() {
    assert!('\u{05B8}'.is_alphanumeric() && is_combining('\u{05B8}'));
    // The Hebrew vowel qamats on a dotted circle, the way a grammar prints it.
    let (d, at) = marked("the vowel \u{25CC}|\u{05B8}| |is long");
    assert_eq!(count::words("\u{25CC}\u{05B8}"), 1);
    assert_eq!(motion::word_at(&d, at[0]), Span::at(at[0]));
    assert_eq!(motion::word_right(&d, p(0, 4)), at[2]);
}

#[test]
fn sentence_next_gives_the_start_of_the_next_sentence_after_p() {
    let (d, at) = marked("|On|e.| |Two.");
    for from in &at[..3] {
        assert_eq!(motion::sentence_next(&d, *from), at[3], "from {from:?}");
    }
}

#[test]
fn sentence_prev_gives_the_start_of_the_sentence_p_is_in() {
    let (d, at) = marked("One. |T|wo|.| Three.");
    for from in &at[1..] {
        assert_eq!(motion::sentence_prev(&d, *from), at[0], "from {from:?}");
    }
}

#[test]
fn sentence_prev_from_a_sentence_start_gives_the_previous_sentence() {
    steps_back("|One. |Two. |Three.", motion::sentence_prev);
}

#[test]
fn a_sentence_ends_at_each_of_the_four_terminators() {
    walks("|One. |Two! |Three? |Four\u{2026} |Five|", motion::sentence_next, motion::sentence_prev);
}

#[test]
fn a_sentence_ends_at_a_run_of_several_terminators() {
    walks("|What?! |Really... |Yes!!! |No\u{2026}. |Ok|", motion::sentence_next, motion::sentence_prev);
}

#[test]
fn closing_quotes_and_brackets_after_the_terminator_stay_with_the_sentence() {
    walks(
        "|\"Go.\" |'Go.' |(Go.) |[Go.] |\u{201C}Go.\u{201D} |\u{2018}Go.\u{2019} |End|",
        motion::sentence_next,
        motion::sentence_prev,
    );
    walks("|(He said \"Go.\") |Then (she said 'No!')] |End|", motion::sentence_next, motion::sentence_prev);
    let (d, at) = marked("One. |(He said \"Go.\")| Then.");
    assert_eq!(motion::sentence_at(&d, at[0]), Span { start: at[0], end: at[1] });
}

#[test]
fn a_terminator_ends_a_sentence_only_before_whitespace_or_the_line_end() {
    walks("|Pi is 3.14 and a.b.c is not. |Next|", motion::sentence_next, motion::sentence_prev);
    walks("|See example.com/x?y=1!z now. |Next|", motion::sentence_next, motion::sentence_prev);
    walks("|He said \"No.\"x and \"No!\"; left. |Next|", motion::sentence_next, motion::sentence_prev);
    // Any whitespace will do.
    walks("|One.\t|Two.\u{00A0}|Three.\u{3000}|Four.|", motion::sentence_next, motion::sentence_prev);
    // The end of the line follows the terminator and its closers.
    let (d, at) = marked("One. |Two.\")|");
    assert_eq!(motion::sentence_at(&d, at[0]), Span { start: at[0], end: at[1] });
}

#[test]
fn a_period_before_whitespace_and_a_lowercase_letter_is_not_an_end() {
    walks("|See approx. five of them. |Next|", motion::sentence_next, motion::sentence_prev);
    walks("|Use e.g. this, i.e. that,   etc.  and more. |Next|", motion::sentence_next, motion::sentence_prev);
    walks("|Wait... what? |Next|", motion::sentence_next, motion::sentence_prev);
    let (d, at) = marked("|approx. five.|");
    assert_eq!(motion::sentence_at(&d, p(0, 3)), Span { start: at[0], end: at[1] });
}

#[test]
fn the_abbreviation_guard_needs_a_period_and_a_lowercase_letter() {
    // A capital after the period: an end.
    walks("|See approx. |Five of them. |Next|", motion::sentence_next, motion::sentence_prev);
    // A digit or a bracket after the period: an end.
    walks("|It was 1999. |2000 came. |(then) more. |Next|", motion::sentence_next, motion::sentence_prev);
    // Not a period: an end, whatever follows.
    walks("|Really? |yes! |ok\u{2026} |fine. |Next|", motion::sentence_next, motion::sentence_prev);
}

#[test]
fn the_end_of_a_line_always_ends_a_sentence() {
    walks("|no terminator here\n|next line|", motion::sentence_next, motion::sentence_prev);
    // Even after what would otherwise be an abbreviation.
    walks("|ends with approx.\n|five|", motion::sentence_next, motion::sentence_prev);
    walks("|trailing space.   \n|five|", motion::sentence_next, motion::sentence_prev);
    assert_eq!(count::sentences("ends with approx.\nfive"), 2);
}

#[test]
fn a_sentence_starts_at_the_first_non_whitespace_character_after_an_end_or_of_a_line() {
    let (d, at) = marked("|  \t|Lead.   |Next.\n   |Third.  ");
    assert_eq!(motion::sentence_next(&d, at[0]), at[1]);
    assert_eq!(motion::sentence_next(&d, at[1]), at[2]);
    assert_eq!(motion::sentence_next(&d, at[2]), at[3]);
    assert_eq!(motion::sentence_prev(&d, at[3]), at[2]);
    assert_eq!(motion::sentence_prev(&d, at[2]), at[1]);
}

#[test]
fn sentence_motions_cross_lines_and_skip_blank_ones() {
    walks("|One. |Two\n\n \t \n|Three.\n\n\n|four|", motion::sentence_next, motion::sentence_prev);
}

#[test]
fn sentence_next_from_the_last_sentence_gives_the_document_end() {
    let d = Doc::from_text("One. Two.\n\n  ");
    assert_eq!(motion::sentence_next(&d, p(0, 5)), d.end());
    assert_eq!(motion::sentence_next(&d, p(0, 7)), d.end());
    assert_eq!(motion::sentence_next(&d, d.end()), d.end());
    assert_eq!(d.end(), p(2, 2));
}

#[test]
fn sentence_prev_from_the_first_sentence_gives_the_document_start() {
    let d = Doc::from_text("\n  One. Two.");
    assert_eq!(motion::sentence_prev(&d, p(1, 2)), p(0, 0));
    assert_eq!(motion::sentence_prev(&d, p(0, 0)), p(0, 0));
    let d = Doc::from_text("One. Two.");
    assert_eq!(motion::sentence_prev(&d, p(0, 0)), p(0, 0));
}

#[test]
fn paragraph_next_gives_column_0_of_the_first_line_of_the_next_paragraph() {
    let d = Doc::from_text("a\nb\n\n \t\nc\nd\n\ne");
    for from in [p(0, 0), p(0, 1), p(1, 1), p(2, 0), p(3, 1)] {
        assert_eq!(motion::paragraph_next(&d, from), p(4, 0), "from {from:?}");
    }
    for from in [p(4, 0), p(5, 1), p(6, 0)] {
        assert_eq!(motion::paragraph_next(&d, from), p(7, 0), "from {from:?}");
    }
}

#[test]
fn paragraph_next_gives_the_document_end_when_there_is_no_next_paragraph() {
    let d = Doc::from_text("a\nb\n\n  ");
    for from in [p(0, 0), p(1, 1), p(2, 0), p(3, 1), p(3, 2)] {
        assert_eq!(motion::paragraph_next(&d, from), p(3, 2), "from {from:?}");
    }
    let d = Doc::from_text("a\nbc");
    assert_eq!(motion::paragraph_next(&d, p(0, 0)), p(1, 2));
}

#[test]
fn a_line_of_only_whitespace_is_blank_and_separates_paragraphs() {
    for gap in ["", " ", "\t", "\u{00A0}\u{3000} "] {
        let d = Doc::from_text(&format!("a\n{gap}\nb"));
        assert_eq!(motion::paragraph_next(&d, p(0, 0)), p(2, 0), "gap {gap:?}");
        assert_eq!(motion::paragraph_at(&d, p(0, 0)), sp(0, 0, 1), "gap {gap:?}");
        assert_eq!(motion::paragraph_at(&d, p(1, 0)), Span::at(p(1, 0)), "gap {gap:?}");
    }
    // Without the blank line it is one paragraph.
    let d = Doc::from_text("a\nb");
    assert_eq!(motion::paragraph_at(&d, p(0, 0)), Span { start: p(0, 0), end: p(1, 1) });
}

#[test]
fn paragraph_prev_gives_column_0_of_the_first_line_of_the_paragraph_p_is_in() {
    let d = Doc::from_text("a\n\nbb\ncc\ndd\n\ne");
    for from in [p(2, 1), p(2, 2), p(3, 0), p(3, 1), p(4, 2)] {
        assert_eq!(motion::paragraph_prev(&d, from), p(2, 0), "from {from:?}");
    }
}

#[test]
fn paragraph_prev_from_a_paragraph_start_or_a_blank_line_gives_the_previous_paragraph() {
    let d = Doc::from_text("a\nb\n\n \nc\nd\n\ne");
    assert_eq!(motion::paragraph_prev(&d, p(4, 0)), p(0, 0));
    assert_eq!(motion::paragraph_prev(&d, p(7, 0)), p(4, 0));
    assert_eq!(motion::paragraph_prev(&d, p(6, 0)), p(4, 0));
    assert_eq!(motion::paragraph_prev(&d, p(3, 1)), p(0, 0));
    assert_eq!(motion::paragraph_prev(&d, p(2, 0)), p(0, 0));
}

#[test]
fn paragraph_prev_gives_the_document_start_when_there_is_no_previous_paragraph() {
    let d = Doc::from_text("\n \nfirst\nmore");
    assert_eq!(motion::paragraph_prev(&d, p(2, 0)), p(0, 0));
    assert_eq!(motion::paragraph_prev(&d, p(1, 1)), p(0, 0));
    assert_eq!(motion::paragraph_prev(&d, p(0, 0)), p(0, 0));
    assert_eq!(motion::paragraph_prev(&d, p(3, 2)), p(2, 0));
}

#[test]
fn line_start_and_line_end_are_of_the_doc_line() {
    let d = Doc::from_text("caf\u{00E9} au lait\n\n  last  ");
    assert_eq!(motion::line_start(&d, p(0, 7)), p(0, 0));
    assert_eq!(motion::line_end(&d, p(0, 1)), p(0, 13));
    assert_eq!(motion::line_start(&d, p(1, 0)), p(1, 0));
    assert_eq!(motion::line_end(&d, p(1, 0)), p(1, 0));
    assert_eq!(motion::line_start(&d, p(2, 4)), p(2, 0));
    assert_eq!(motion::line_end(&d, p(2, 4)), p(2, 8));
}

#[test]
fn doc_start_and_doc_end() {
    let d = Doc::from_text("one\ntwo\n");
    for from in [p(0, 0), p(1, 2), p(2, 0), p(9, 9)] {
        assert_eq!(motion::doc_start(&d, from), p(0, 0));
        assert_eq!(motion::doc_end(&d, from), p(2, 0));
    }
}

#[test]
fn word_at_gives_the_word_under_p_or_the_word_ending_at_p() {
    let (d, at) = marked("one |t|w|o|, three");
    let two = Span { start: at[0], end: at[3] };
    for from in &at {
        assert_eq!(motion::word_at(&d, *from), two, "at {from:?}");
    }
}

#[test]
fn word_at_gives_an_empty_span_at_p_when_there_is_no_word() {
    let (d, at) = marked("one,| |-| two\n|\n  |  ");
    for from in &at {
        assert_eq!(motion::word_at(&d, *from), Span::at(*from), "at {from:?}");
    }
}

#[test]
fn sentence_at_runs_from_the_start_through_terminator_and_closers_without_the_space_after() {
    let (d, at) = marked("One.  |He said \"Go.\")|  Then.  ");
    let want = Span { start: at[0], end: at[1] };
    for col in at[0].col..=at[1].col {
        assert_eq!(motion::sentence_at(&d, p(0, col)), want, "at column {col}");
    }
    let (d, at) = marked("One. |No terminator|   ");
    assert_eq!(motion::sentence_at(&d, p(0, 9)), Span { start: at[0], end: at[1] });
}

#[test]
fn paragraph_at_runs_from_column_0_of_its_first_line_to_the_end_of_its_last() {
    let d = Doc::from_text("x\n\n  a\nbb\nccc  \n\ny");
    let want = Span { start: p(2, 0), end: p(4, 5) };
    for from in [p(2, 0), p(2, 3), p(3, 1), p(4, 0), p(4, 5)] {
        assert_eq!(motion::paragraph_at(&d, from), want, "at {from:?}");
    }
    assert_eq!(motion::paragraph_at(&d, p(6, 0)), sp(6, 0, 1));
}

#[test]
fn paragraph_at_is_an_empty_span_at_p_on_a_blank_line() {
    let d = Doc::from_text("x\n\n \t \ny");
    for from in [p(1, 0), p(2, 0), p(2, 2), p(2, 3)] {
        assert_eq!(motion::paragraph_at(&d, from), Span::at(from), "at {from:?}");
    }
}

#[test]
fn motions_are_total_on_positions_far_out_of_range_and_inside_characters() {
    let d = Doc::from_text("caf\u{00E9} \u{1F468}\u{200D}\u{1F469}.\n\n\u{0130}x");
    let r = Reference::of(&d);
    for line in [0, 1, 2, 3, usize::MAX / 2, usize::MAX - 1, usize::MAX] {
        for col in [0, 1, 4, 5, 6, 7, 8, 9, usize::MAX / 2, usize::MAX - 1, usize::MAX] {
            check_motions_at(&d, &r, p(line, col));
        }
    }
    let empty = Doc::new();
    let r = Reference::of(&empty);
    for q in positions(&empty) {
        check_motions_at(&empty, &r, q);
    }
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

#[test]
fn words_are_whitespace_separated_tokens_holding_an_alphanumeric_character() {
    assert_eq!(count::words("well-known"), 1);
    assert_eq!(count::words("-"), 0);
    assert_eq!(count::words("#"), 0);
    assert_eq!(count::words("**bold**"), 1);
    assert_eq!(count::words("[[two words]]"), 2);
    assert_eq!(count::words(""), 0);
    assert_eq!(count::words(" \t\n "), 0);
    assert_eq!(count::words("one\ttwo\nthree  four"), 4);
    assert_eq!(count::words("... 7 \u{2014} \u{4E00} \u{1F600} _ \u{0301}"), 2);
}

#[test]
fn clusters_are_characters_without_combining_marks_or_line_breaks() {
    assert_eq!(count::clusters(""), 0);
    assert_eq!(count::clusters("abc"), 3);
    assert_eq!(count::clusters("mitsqanaqan\u{0313}"), 11);
    assert_eq!(count::clusters("e\u{0301}\u{0302}\u{0303}"), 1);
    assert_eq!(count::clusters("a\nb\n"), 2);
    assert_eq!(count::clusters("a b\t"), 4);
    assert_eq!(count::clusters("\u{4E00}\u{1F600}"), 2);
}

#[test]
fn sentences_are_counted_line_by_line_by_the_sentence_rule() {
    assert_eq!(count::sentences(""), 0);
    assert_eq!(count::sentences("One. Two! Three? Four\u{2026} Five"), 5);
    assert_eq!(count::sentences("See approx. five. Then stop."), 2);
    assert_eq!(count::sentences("He said \"Go.\" Then left."), 2);
    assert_eq!(count::sentences("One.\nTwo. Three.\n\n  \nFour"), 4);
    assert_eq!(count::sentences("Pi is 3.14 exactly"), 1);
}

#[test]
fn a_non_blank_line_with_no_terminator_counts_as_one_sentence() {
    assert_eq!(count::sentences("no terminator"), 1);
    assert_eq!(count::sentences("# Title\n\nno terminator\n- item"), 3);
    assert_eq!(count::sentences("\n  \n\t"), 0);
    assert_eq!(count::sentences("-"), 1);
}

#[test]
fn counts_agree_with_the_reference_models_on_hostile_text() {
    let mut rng = Rng::new(0x2545_F491_4F6C_DD1D);
    for _ in 0..rounds(6000) {
        let text = random_text(&mut rng, 30);
        assert_eq!(count::words(&text), ref_count_words(&text), "words of {text:?}");
        assert_eq!(count::clusters(&text), ref_count_clusters(&text), "clusters of {text:?}");
        assert_eq!(count::sentences(&text), ref_count_sentences(&text), "sentences of {text:?}");
    }
}

#[test]
fn sentence_count_matches_the_stops_of_sentence_next() {
    // count.rs and motion.rs must agree on what a sentence is.
    let mut rng = Rng::new(0x6A09_E667_F3BC_C908);
    for _ in 0..rounds(800) {
        let doc = Doc::from_text(&random_text(&mut rng, 25));
        let mut starts = 0;
        let first_line = doc.line(0);
        if first_line.chars().next().is_some_and(|ch| !ch.is_whitespace()) {
            starts += 1;
        }
        let mut at = p(0, 0);
        loop {
            let next = motion::sentence_next(&doc, at);
            if next == at {
                break;
            }
            let lands_on_text = doc.line(next.line)[next.col..].chars().next().is_some_and(|ch| !ch.is_whitespace());
            if lands_on_text {
                starts += 1;
            } else {
                assert_eq!(next, doc.end());
            }
            at = next;
        }
        assert_eq!(starts, count::sentences(&text_of(&doc)), "in {:?}", doc.lines());
    }
}

#[test]
fn tally_of_counts_every_line() {
    let d = Doc::from_text("one two\n\n- three\n# \nfour \u{2014} five");
    let tally = Tally::of(&d);
    assert_eq!(tally.words(), 5);
    let per_line: Vec<usize> = (0..5).map(|i| tally.line_words(i)).collect();
    assert_eq!(per_line, [2, 0, 1, 0, 2]);
}

#[test]
fn tally_line_words_is_zero_past_the_end() {
    let d = Doc::from_text("one two\nthree");
    let tally = Tally::of(&d);
    assert_eq!(tally.line_words(1), 1);
    assert_eq!(tally.line_words(2), 0);
    assert_eq!(tally.line_words(usize::MAX), 0);
    assert_eq!(Tally::of(&Doc::new()).words(), 0);
    assert_eq!(Tally::of(&Doc::new()).line_words(0), 0);
}

fn assert_tally(tally: &Tally, doc: &Doc, why: &str) {
    let text = text_of(doc);
    assert_eq!(tally.words(), ref_count_words(&text), "{why}: total in {:?}", doc.lines());
    assert_eq!(tally.words(), count::words(&text), "{why}: total in {:?}", doc.lines());
    for (i, line) in doc.lines().iter().enumerate() {
        assert_eq!(tally.line_words(i), ref_count_words(line), "{why}: line {i} of {:?}", doc.lines());
    }
    assert_eq!(tally.line_words(doc.line_count()), 0, "{why}");
}

#[test]
fn tally_follows_single_edits_of_every_shape() {
    let mut d = Doc::from_text("one two\nthree four five\n\nsix");
    let mut tally = Tally::of(&d);
    let edits: [(Span, &str); 12] = [
        (Span::at(p(0, 3)), ""),                          // nothing
        (Span::at(p(0, 2)), " "),                         // split a word
        (Span::new(p(0, 2), p(0, 3)), ""),                // and join it again
        (Span::at(p(1, 5)), "\n"),                        // split a line
        (Span::new(p(1, 5), p(2, 0)), ""),                // join two lines
        (Span::new(p(0, 4), p(3, 1)), ""),                // join across several lines
        (Span::at(p(0, 0)), "a b\nc\n\nd e f\n"),         // paste several lines at the top
        (Span::at(p(99, 99)), "\nlast line here"),        // out of range: the end
        (Span::new(p(0, 0), p(99, 0)), "all gone"),       // the whole document
        (Span::at(p(0, 3)), "\r\n\r\n"),                  // other kinds of line break
        (Span::new(p(1, 0), p(2, 2)), "- # ** \u{2014}"), // words to non-words
        (Span::new(p(0, 0), p(9, 9)), ""),                // empty
    ];
    for (span, text) in edits {
        let change = d.replace(span, text);
        tally.apply(&d, &change);
        assert_tally(&tally, &d, &format!("after {change:?}"));
        let undone = d.revert(&change);
        tally.apply(&d, &undone);
        assert_tally(&tally, &d, &format!("after undoing {change:?}"));
        let redone = d.reapply(&change);
        tally.apply(&d, &redone);
        assert_tally(&tally, &d, &format!("after redoing {change:?}"));
    }
    assert!(d.is_empty());
}

/// "Recount only the touched lines." Seen from outside: a line the change did
/// not touch keeps the count it had, even when the document handed to `apply`
/// no longer agrees with it there. (A change that does not fit the table is
/// answered with a full recount, which the implementation documents; this
/// one fits.)
#[test]
fn tally_apply_recounts_only_the_lines_the_change_touched() {
    let mut d = Doc::from_text("one two\nthree\nfour five six");
    let mut tally = Tally::of(&d);
    // Behind the tally's back, line 2 goes from three words to one.
    d.replace(Span::new(p(2, 0), p(2, 13)), "four");
    // Then a change to line 0 alone, which the tally is told about.
    let change = d.replace(Span::at(p(0, 0)), "zero ");
    assert_eq!(change.lines(), (0, 1, 1));
    tally.apply(&d, &change);
    assert_eq!(tally.line_words(0), 3);
    assert_eq!(tally.line_words(2), 3, "line 2 was recounted though the change did not touch it");
    assert_eq!(tally.words(), 7);
}

#[test]
fn tally_stays_equal_to_a_full_count_through_random_hostile_edits() {
    let mut rng = Rng::new(0xBB67_AE85_84CA_A73B);
    for _ in 0..rounds(60) {
        let mut d = Doc::from_text(&random_text(&mut rng, 10));
        let mut tally = Tally::of(&d);
        let mut made: Vec<Change> = Vec::new();
        for turn in 0..220 {
            let change = if turn % 5 == 4 && !made.is_empty() {
                // Take the last change back, and sometimes make it again.
                let last = made.pop().expect("not empty");
                let undone = d.revert(&last);
                if rng.below(2) == 0 {
                    tally.apply(&d, &undone);
                    assert_tally(&tally, &d, "after a revert");
                    d.reapply(&last)
                } else {
                    undone
                }
            } else {
                made.clear();
                let (a, b) = (random_pos(&mut rng, &d), random_pos(&mut rng, &d));
                let span = if rng.below(3) == 0 { Span::new(a, b) } else { Span::at(a) };
                // Keep the document from growing without limit.
                let span = if d.len_bytes() > 400 { Span::new(p(0, 0), a) } else { span };
                let change = d.replace(span, rng.pick(HOSTILE));
                made.push(change.clone());
                change
            };
            tally.apply(&d, &change);
            assert_tally(&tally, &d, "after an edit");
        }
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

#[test]
fn a_needle_never_matches_across_a_line_break() {
    let d = Doc::from_text("ab\ncd\n\nef");
    for case_sensitive in [true, false] {
        for needle in ["bc", "abcd", "b\nc", "b\r\nc", "b\rc", "\n", "\r", "d\n\ne", "\nef"] {
            assert_eq!(search::find_all(&d, needle, case_sensitive), vec![], "{needle:?}");
            assert_eq!(search::find_next(&d, needle, p(0, 0), case_sensitive), None, "{needle:?}");
            assert_eq!(search::find_prev(&d, needle, d.end(), case_sensitive), None, "{needle:?}");
        }
    }
}

#[test]
fn an_empty_needle_matches_nothing() {
    for text in ["", "ab\ncd", "\n\n"] {
        let d = Doc::from_text(text);
        for case_sensitive in [true, false] {
            assert_eq!(search::find_all(&d, "", case_sensitive), vec![]);
            assert_eq!(search::find_next(&d, "", p(0, 0), case_sensitive), None);
            assert_eq!(search::find_prev(&d, "", d.end(), case_sensitive), None);
        }
    }
}

#[test]
fn find_all_gives_every_match_in_document_order_without_overlap() {
    let d = Doc::from_text("aaaa b\n\nab aaa\naXa");
    assert_eq!(search::find_all(&d, "aa", true), vec![sp(0, 0, 2), sp(0, 2, 4), sp(2, 3, 5)]);
    assert_eq!(search::find_all(&d, "a", true).len(), 10);
    assert_eq!(search::find_all(&d, "AXA", false), vec![sp(3, 0, 3)]);
    assert_eq!(search::find_all(&d, "AXA", true), vec![]);
    assert_eq!(search::find_all(&d, " ", true), vec![sp(0, 4, 5), sp(2, 2, 3)]);
}

#[test]
fn find_next_gives_the_first_match_starting_at_or_after_from() {
    let d = Doc::from_text("one fish\ntwo fish\n\nred fish");
    assert_eq!(search::find_next(&d, "fish", p(0, 0), true), Some(sp(0, 4, 8)));
    assert_eq!(search::find_next(&d, "fish", p(0, 4), true), Some(sp(0, 4, 8)));
    assert_eq!(search::find_next(&d, "fish", p(0, 5), true), Some(sp(1, 4, 8)));
    assert_eq!(search::find_next(&d, "fish", p(2, 0), true), Some(sp(3, 4, 8)));
    assert_eq!(search::find_next(&d, "FISH", p(2, 0), false), Some(sp(3, 4, 8)));
    assert_eq!(search::find_next(&d, "FISH", p(2, 0), true), None);
}

#[test]
fn find_next_wraps_round_to_the_top() {
    let d = Doc::from_text("one fish\ntwo fish\n\nred");
    assert_eq!(search::find_next(&d, "fish", p(1, 5), true), Some(sp(0, 4, 8)));
    assert_eq!(search::find_next(&d, "fish", d.end(), true), Some(sp(0, 4, 8)));
    assert_eq!(search::find_next(&d, "fish", p(99, 99), true), Some(sp(0, 4, 8)));
    assert_eq!(search::find_next(&d, "one", p(0, 1), true), Some(sp(0, 0, 3)));
    assert_eq!(search::find_next(&d, "bird", p(0, 0), true), None);
}

#[test]
fn find_prev_gives_the_last_match_starting_before_from() {
    let d = Doc::from_text("one fish\ntwo fish\n\nred fish");
    assert_eq!(search::find_prev(&d, "fish", d.end(), true), Some(sp(3, 4, 8)));
    assert_eq!(search::find_prev(&d, "fish", p(3, 4), true), Some(sp(1, 4, 8)));
    assert_eq!(search::find_prev(&d, "fish", p(3, 5), true), Some(sp(3, 4, 8)));
    assert_eq!(search::find_prev(&d, "fish", p(2, 0), true), Some(sp(1, 4, 8)));
    assert_eq!(search::find_prev(&d, "fish", p(1, 4), true), Some(sp(0, 4, 8)));
}

#[test]
fn find_prev_wraps_round_to_the_bottom() {
    let d = Doc::from_text("one fish\ntwo fish\n\nred");
    assert_eq!(search::find_prev(&d, "fish", p(0, 4), true), Some(sp(1, 4, 8)));
    assert_eq!(search::find_prev(&d, "fish", p(0, 0), true), Some(sp(1, 4, 8)));
    assert_eq!(search::find_prev(&d, "red", p(3, 0), true), Some(sp(3, 0, 3)));
    assert_eq!(search::find_prev(&d, "bird", p(3, 0), false), None);
}

#[test]
fn case_insensitive_comparison_is_by_char_to_lowercase() {
    let d = Doc::from_text("Hello HELLO hello h\u{00C9}llo \u{03A3}\u{03C3}\u{03C2} \u{01C5}");
    assert_eq!(search::find_all(&d, "hello", true), vec![sp(0, 12, 17)]);
    assert_eq!(search::find_all(&d, "hElLo", false), vec![sp(0, 0, 5), sp(0, 6, 11), sp(0, 12, 17)]);
    assert_eq!(search::find_all(&d, "H\u{00E9}LLO", false), vec![sp(0, 18, 24)]);
    // Capital sigma lowercases to the medial form only; the final form is a different character.
    assert_eq!(search::find_all(&d, "\u{03A3}", false), vec![sp(0, 25, 27), sp(0, 27, 29)]);
    assert_eq!(search::find_all(&d, "\u{03C2}", false), vec![sp(0, 29, 31)]);
    // A titlecase digraph and its uppercase form lowercase to the same character.
    assert_eq!(search::find_all(&d, "\u{01C4}", false), vec![sp(0, 32, 34)]);
}

#[test]
fn spans_are_right_on_the_original_text_where_lowercasing_changes_byte_lengths() {
    // U+0130 is two bytes and lowercases to three (i and U+0307).
    let d = Doc::from_text("\u{0130}stanbul \u{0130}\u{0130}x i\u{0307}stanbul");
    let found = search::find_all(&d, "X", false);
    assert_eq!(found, vec![sp(0, 14, 15)]);
    assert_eq!(d.slice(found[0]), "x");
    let found = search::find_all(&d, "i\u{0307}STANBUL", false);
    assert_eq!(found, vec![sp(0, 0, 9), sp(0, 16, 26)]);
    assert_eq!(d.slice(found[0]), "\u{0130}stanbul");
    assert_eq!(d.slice(found[1]), "i\u{0307}stanbul");
    assert_eq!(search::find_all(&d, "\u{0130}", false), vec![sp(0, 0, 2), sp(0, 10, 12), sp(0, 12, 14), sp(0, 16, 19)]);
    assert_eq!(search::find_next(&d, "\u{0130}X", p(0, 11), false), Some(sp(0, 12, 15)));
    assert_eq!(search::find_prev(&d, "\u{0130}", p(0, 16), false), Some(sp(0, 12, 14)));
    // Shrinking the other way: U+1E9E (3 bytes) to U+00DF (2), the Kelvin sign (3) to k (1).
    let d = Doc::from_text("\u{1E9E}a \u{212A}a \u{2126}a");
    assert_eq!(search::find_all(&d, "A", false), vec![sp(0, 3, 4), sp(0, 8, 9), sp(0, 13, 14)]);
    assert_eq!(search::find_all(&d, "\u{00DF}A", false), vec![sp(0, 0, 4)]);
    assert_eq!(search::find_all(&d, "Ka", false), vec![sp(0, 5, 9)]);
    assert_eq!(search::find_all(&d, "\u{03C9}A", false), vec![sp(0, 10, 14)]);
}

/// Needles and text for the search property test: heavy on characters whose
/// lowercase forms change length or collide.
#[rustfmt::skip]
const FOLDING: &[&str] = &[
    "a", "A", "b", "B", "ab", "aB", "aa", "AA", " ", " ", "\n", "\u{0130}", "i\u{0307}", "I\u{0307}", "i", "I",
    "\u{0131}", "\u{0307}", "\u{1E9E}", "\u{00DF}", "ss", "\u{212A}", "k", "K", "\u{00E9}", "\u{00C9}", "e\u{0301}",
    "\u{4E00}", "\u{03A3}", "\u{03C3}", "\u{03C2}", "\u{01C4}", "\u{01C5}", "\u{01C6}", "\u{2126}", "\u{03C9}",
    "\u{03A9}", "\u{FF21}", "\u{FF41}", "\u{1F600}", "\u{200D}", ".", "\t",
];

fn check_search(doc: &Doc, needle: &str, rng: &mut Rng, every_position: bool) {
    for case_sensitive in [true, false] {
        let ctx = || format!("needle {needle:?} case_sensitive {case_sensitive} in {:?}", doc.lines());
        let all = search::find_all(doc, needle, case_sensitive);
        assert_eq!(all, ref_find_all(doc, needle, case_sensitive), "find_all {}", ctx());

        // The contract, without the model.
        for span in &all {
            assert!(span.start < span.end && span.start.line == span.end.line, "{}", ctx());
            assert_eq!((doc.clamp(span.start), doc.clamp(span.end)), (span.start, span.end), "{}", ctx());
            let read = doc.slice(*span);
            if case_sensitive {
                assert_eq!(read, needle, "{}", ctx());
            } else {
                assert_eq!(fold(&read), fold(needle), "{}", ctx());
            }
        }
        for pair in all.windows(2) {
            assert!(pair[0].end <= pair[1].start, "overlap or disorder {}", ctx());
        }
        if needle.is_empty() || needle.contains('\n') || needle.contains('\r') {
            assert!(all.is_empty(), "{}", ctx());
        }

        // Next and previous step through the matches find_all gives. (Whether an occurrence that overlaps
        // one of those also counts is not settled by the contract; the implementation says it does not.)
        let froms = if every_position { positions(doc) } else { (0..12).map(|_| random_pos(rng, doc)).collect() };
        for from in froms {
            let at = doc.clamp(from);
            let next = all.iter().find(|span| span.start >= at).or(all.first()).copied();
            let prev = all.iter().rev().find(|span| span.start < at).or(all.last()).copied();
            assert_eq!(search::find_next(doc, needle, from, case_sensitive), next, "find_next from {from:?} {}", ctx());
            assert_eq!(search::find_prev(doc, needle, from, case_sensitive), prev, "find_prev from {from:?} {}", ctx());
        }
    }
}

#[test]
fn searches_agree_with_a_plain_scan_on_case_folding_text() {
    let mut rng = Rng::new(0x3C6E_F372_FE94_F82B);
    for _ in 0..rounds(2500) {
        let mut text = String::new();
        for _ in 0..rng.below(28) {
            text.push_str(rng.pick(FOLDING));
        }
        let doc = Doc::from_text(&text);
        let mut needle = String::new();
        for _ in 0..rng.below(4) {
            needle.push_str(rng.pick(FOLDING));
        }
        check_search(&doc, &needle, &mut rng, true);
    }
}

#[test]
fn searches_for_pieces_of_the_document_itself_find_them() {
    let mut rng = Rng::new(0xA54F_F53A_5F1D_36F1);
    for _ in 0..rounds(1200) {
        let doc = Doc::from_text(&random_text(&mut rng, 24));
        // A needle cut from the document on character boundaries, sometimes with its case changed.
        let line = rng.below(doc.line_count());
        let text = doc.line(line);
        let cuts: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
        let a = cuts[rng.below(cuts.len())];
        let b = cuts[rng.below(cuts.len())];
        let piece = &text[a.min(b)..a.max(b)];
        let needle = match rng.below(4) {
            0 => piece.to_uppercase(),
            1 => fold(piece),
            2 => format!("{piece}{}", rng.pick(HOSTILE)),
            _ => piece.to_string(),
        };
        check_search(&doc, &needle, &mut rng, false);
        if !piece.is_empty() {
            // The piece as written is certainly there, both ways.
            assert!(!search::find_all(&doc, piece, true).is_empty(), "{piece:?} in {:?}", doc.lines());
            assert!(!search::find_all(&doc, piece, false).is_empty(), "{piece:?} in {:?}", doc.lines());
            assert!(search::find_next(&doc, piece, p(line, a.min(b)), true).is_some());
            assert!(search::find_prev(&doc, piece, p(line, a.min(b)), false).is_some());
        }
    }
}

#[test]
fn searches_hold_up_on_very_long_lines() {
    let line = format!("{}\u{0130}{}needle{}", "a".repeat(30_000), " ".repeat(10_000), "\u{4E00}".repeat(5_000));
    let doc = Doc::from_text(&format!("{line}\n\n{line}"));
    let at = 30_000 + 2 + 10_000;
    assert_eq!(search::find_all(&doc, "NEEDLE", false), vec![sp(0, at, at + 6), sp(2, at, at + 6)]);
    assert_eq!(search::find_all(&doc, "needle", true), vec![sp(0, at, at + 6), sp(2, at, at + 6)]);
    assert_eq!(search::find_next(&doc, "i\u{0307}", p(0, 30_002), false), Some(sp(2, 30_000, 30_002)));
    assert_eq!(search::find_prev(&doc, "i\u{0307}", p(0, 30_002), false), Some(sp(0, 30_000, 30_002)));
    assert_eq!(search::find_all(&doc, &"a".repeat(30_000), false).len(), 2);
    assert_eq!(search::find_all(&doc, &"A".repeat(10_000), false).len(), 6);
    assert_eq!(search::find_all(&doc, &format!("{}b", "a".repeat(200)), false), vec![]);
}

// ---------------------------------------------------------------------------
// history: a desk that edits the way the app does
// ---------------------------------------------------------------------------

/// A document with its history and cursor. Three things are kept in step
/// from the changes alone, never from the document itself: a second copy of
/// the document, a table with one entry per line (through `Change::splice`),
/// and a running word count fed from the second copy.
struct Desk {
    doc: Doc,
    shadow: Doc,
    table: Vec<Option<String>>,
    tally: Tally,
    history: History,
    cursor: Cursor,
    now: u64,
}

impl Desk {
    fn new(text: &str) -> Desk {
        Desk::with_history(text, History::new())
    }

    fn with_history(text: &str, history: History) -> Desk {
        let doc = Doc::from_text(text);
        let cursor = Cursor::at(doc.end());
        let table = doc.lines().iter().cloned().map(Some).collect();
        Desk { shadow: doc.clone(), tally: Tally::of(&doc), table, doc, history, cursor, now: 0 }
    }

    fn text(&self) -> String {
        text_of(&self.doc)
    }

    /// Feeds one change to everything that follows the document by changes.
    fn follow(&mut self, change: &Change) {
        let echoed = self.shadow.replace(Span { start: change.start, end: change.old_end }, &change.inserted);
        assert_eq!(&echoed, change, "a change did not replay as Doc::replace would return it");
        change.splice(&mut self.table, || None);
        self.tally.apply(&self.shadow, change);
    }

    /// Checks that everything following by changes has kept up.
    fn settle(&mut self) {
        assert_eq!(self.shadow.lines(), self.doc.lines(), "the changes did not add up to the document");
        assert_eq!(self.table.len(), self.doc.line_count(), "the per-line table lost step");
        for (i, entry) in self.table.iter_mut().enumerate() {
            match entry {
                Some(kept) => assert_eq!(kept.as_str(), self.doc.line(i), "line {i} changed without a change"),
                None => *entry = Some(self.doc.line(i).to_string()),
            }
        }
        assert_eq!(self.tally.words(), ref_count_words(&text_of(&self.doc)), "the tally lost step");
    }

    fn edit_with(&mut self, span: Span, text: &str, kind: EditKind, after: Option<Cursor>) -> Change {
        let before = self.cursor;
        let change = self.doc.replace(span, text);
        self.follow(&change);
        self.cursor = after.unwrap_or(Cursor::at(change.end));
        self.history.record(change.clone(), kind, before, self.cursor, self.now);
        change
    }

    fn edit(&mut self, span: Span, text: &str, kind: EditKind) -> Change {
        self.edit_with(span, text, kind, None)
    }

    /// Types `text` a character at a time, 40 ms apart.
    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.now = self.now.wrapping_add(40);
            self.edit(Span::at(self.cursor.head), ch.encode_utf8(&mut [0; 4]), EditKind::Insert);
        }
    }

    fn backspace_span(&self) -> Span {
        let head = self.doc.clamp(self.cursor.head);
        let offset = self.doc.offset_of(head);
        let start = if offset == 0 { head } else { self.doc.pos_at(offset - 1) };
        Span::new(start, head)
    }

    fn delete_span(&self) -> Span {
        let head = self.doc.clamp(self.cursor.head);
        let offset = self.doc.offset_of(head);
        let end = (1..=4).map(|n| self.doc.pos_at(offset + n)).find(|&q| q != head).unwrap_or(head);
        Span::new(head, end)
    }

    fn backspace(&mut self) -> Change {
        self.now = self.now.wrapping_add(40);
        self.edit(self.backspace_span(), "", EditKind::Delete)
    }

    fn delete(&mut self) -> Change {
        self.now = self.now.wrapping_add(40);
        self.edit(self.delete_span(), "", EditKind::Delete)
    }

    fn replayed(&mut self, replay: Option<Replay>) -> Option<Replay> {
        let replay = replay?;
        for change in &replay.changes {
            self.follow(change);
        }
        self.settle();
        self.cursor = replay.cursor;
        Some(replay)
    }

    fn undo(&mut self) -> Option<Replay> {
        let replay = self.history.undo(&mut self.doc);
        self.replayed(replay)
    }

    fn redo(&mut self) -> Option<Replay> {
        let replay = self.history.redo(&mut self.doc);
        self.replayed(replay)
    }

    /// Undoes until nothing is left, giving the text after each step.
    fn undo_all(&mut self) -> Vec<String> {
        let mut texts = Vec::new();
        while self.undo().is_some() {
            texts.push(self.text());
        }
        texts
    }
}

// ---------------------------------------------------------------------------
// history: one case for each sentence of the contract
// ---------------------------------------------------------------------------

fn assert_plain_data<T: Clone + Copy + PartialEq + Eq + std::fmt::Debug>() {}
fn assert_clone_debug<T: Clone + std::fmt::Debug>() {}

#[test]
fn the_public_types_have_the_traits_the_contract_lists() {
    assert_plain_data::<Cursor>();
    assert_plain_data::<EditKind>();
    assert_clone_debug::<Replay>();
    assert_clone_debug::<Tally>();
    assert_eq!(Cursor::default(), Cursor { head: p(0, 0), anchor: p(0, 0) });
    let replay = Replay { cursor: Cursor::at(p(1, 2)), changes: vec![Change::default()] };
    assert_eq!(replay.clone().changes.len(), 1);
}

/// Every function has the shape the contract gives it. This only has to compile.
#[test]
fn the_public_functions_have_the_shapes_the_contract_gives() {
    let _: [Motion; 13] = [
        motion::left,
        motion::right,
        motion::word_left,
        motion::word_right,
        motion::word_end,
        motion::sentence_next,
        motion::sentence_prev,
        motion::paragraph_next,
        motion::paragraph_prev,
        motion::line_start,
        motion::line_end,
        motion::doc_start,
        motion::doc_end,
    ];
    let _: [fn(&Doc, Pos) -> Span; 3] = [motion::word_at, motion::sentence_at, motion::paragraph_at];
    let _: [fn(&str) -> usize; 3] = [count::words, count::clusters, count::sentences];
    let _: fn(&Doc) -> Tally = Tally::of;
    let _: fn(&mut Tally, &Doc, &Change) = Tally::apply;
    let _: fn(&Tally) -> usize = Tally::words;
    let _: fn(&Tally, usize) -> usize = Tally::line_words;
    let _: fn(&Doc, &str, bool) -> Vec<Span> = search::find_all;
    type Step = fn(&Doc, &str, Pos, bool) -> Option<Span>;
    let _: [Step; 2] = [search::find_next, search::find_prev];
    let _: fn(Pos) -> Cursor = Cursor::at;
    let _: fn(&Cursor) -> Span = Cursor::span;
    let _: fn(&Cursor) -> bool = Cursor::has_selection;
    let _: fn() -> History = History::new;
    let _: fn(usize) -> History = History::with_limit;
    let _: fn(&mut History, Change, EditKind, Cursor, Cursor, u64) = History::record;
    let _: [fn(&mut History); 4] = [History::seal, History::begin_group, History::end_group, History::clear];
    let _: [fn(&mut History, &mut Doc) -> Option<Replay>; 2] = [History::undo, History::redo];
    let _: [fn(&History) -> bool; 2] = [History::can_undo, History::can_redo];
}

#[test]
fn a_cursor_selects_what_lies_between_anchor_and_head() {
    let at = Cursor::at(p(3, 1));
    assert_eq!((at.head, at.anchor), (p(3, 1), p(3, 1)));
    assert!(!at.has_selection());
    assert!(at.span().is_empty());
    assert_eq!(at.span().start, p(3, 1));
    for (anchor, head) in [(p(0, 2), p(1, 4)), (p(1, 4), p(0, 2)), (p(2, 5), p(2, 1))] {
        let cursor = Cursor { head, anchor };
        assert!(cursor.has_selection());
        assert_eq!(cursor.span(), Span::new(anchor, head));
        assert_eq!((cursor.span().start, cursor.span().end), (anchor.min(head), anchor.max(head)));
    }
}

#[test]
fn a_new_history_has_nothing_to_undo_or_redo() {
    let mut doc = Doc::from_text("text");
    for mut history in [History::new(), History::with_limit(3), History::with_limit(0)] {
        assert!(!history.can_undo() && !history.can_redo());
        assert!(history.undo(&mut doc).is_none());
        assert!(history.redo(&mut doc).is_none());
        history.seal();
        history.end_group();
        history.begin_group();
        history.end_group();
        history.clear();
        assert!(!history.can_undo() && !history.can_redo());
    }
    assert_eq!(text_of(&doc), "text");
}

#[test]
fn history_new_keeps_10_000_steps_and_drops_the_oldest() {
    let mut doc = Doc::new();
    let mut history = History::new();
    for i in 0..10_050u64 {
        let before = Cursor::at(doc.end());
        let change = doc.replace(Span::at(doc.end()), "x");
        history.record(change, EditKind::Other, before, Cursor::at(doc.end()), i);
    }
    let mut undone = 0;
    while history.undo(&mut doc).is_some() {
        undone += 1;
    }
    assert_eq!(undone, 10_000);
    assert_eq!(doc.line(0).len(), 50);
    let mut redone = 0;
    while history.redo(&mut doc).is_some() {
        redone += 1;
    }
    assert_eq!(redone, 10_000);
    assert_eq!(doc.line(0).len(), 10_050);
}

#[test]
fn with_limit_keeps_that_many_steps_and_drops_the_oldest() {
    for limit in [1usize, 2, 3, 7] {
        let mut desk = Desk::with_history("", History::with_limit(limit));
        for i in 0..10 {
            desk.edit(Span::at(desk.cursor.head), &i.to_string(), EditKind::Other);
        }
        let texts = desk.undo_all();
        assert_eq!(texts.len(), limit);
        assert_eq!(texts[limit - 1], "0123456789"[..10 - limit]);
        assert!(!desk.history.can_undo());
        let mut redone = 0;
        while desk.redo().is_some() {
            redone += 1;
        }
        assert_eq!(redone, limit);
        assert_eq!(desk.text(), "0123456789");
    }
}

#[test]
fn a_noop_change_is_ignored() {
    let mut desk = Desk::new("abc");
    let noop = desk.edit(Span::at(p(0, 1)), "", EditKind::Insert);
    assert!(noop.is_noop());
    assert!(!desk.history.can_undo());
    assert!(desk.undo().is_none());
    // In a group too: a group of nothing is no step.
    desk.history.begin_group();
    desk.edit(Span::at(p(0, 1)), "", EditKind::Other);
    desk.edit(Span::new(p(0, 2), p(0, 2)), "", EditKind::Delete);
    desk.history.end_group();
    assert!(!desk.history.can_undo());
    assert_eq!(desk.text(), "abc");
}

#[test]
fn record_clears_redo() {
    let mut desk = Desk::new("");
    desk.type_text("one two");
    assert!(desk.undo().is_some());
    assert!(desk.history.can_redo());
    desk.type_text("x");
    assert!(!desk.history.can_redo());
    assert!(desk.redo().is_none());
    assert_eq!(desk.text(), "one x");
    // Inside a group as well.
    assert!(desk.undo().is_some());
    desk.history.begin_group();
    assert!(desk.history.can_redo());
    desk.type_text("y");
    assert!(!desk.history.can_redo());
    desk.history.end_group();
}

#[test]
fn seal_closes_the_open_step() {
    let mut desk = Desk::new("");
    desk.type_text("ab");
    desk.history.seal();
    desk.type_text("cd");
    assert_eq!(desk.undo_all(), ["ab", ""]);
    // The same keys without the seal are one step.
    let mut desk = Desk::new("");
    desk.type_text("ab");
    desk.type_text("cd");
    assert_eq!(desk.undo_all(), [""]);
}

#[test]
fn everything_recorded_in_a_group_is_one_step() {
    let mut desk = Desk::new("one two three");
    desk.history.begin_group();
    desk.edit(Span::new(p(0, 8), p(0, 13)), "3", EditKind::Other);
    desk.now += 5000;
    desk.edit(Span::new(p(0, 4), p(0, 7)), "2\n", EditKind::Insert);
    desk.edit(Span::new(p(0, 0), p(0, 3)), "", EditKind::Delete);
    desk.now += 5000;
    desk.edit(Span::at(p(1, 0)), "\n\n", EditKind::Insert);
    desk.history.end_group();
    assert_eq!(desk.text(), " 2\n\n\n 3");
    assert_eq!(desk.undo_all(), ["one two three"]);
    assert!(desk.redo().is_some());
    assert_eq!(desk.text(), " 2\n\n\n 3");
    assert!(desk.redo().is_none());
}

#[test]
fn a_group_does_not_merge_with_typing_before_or_after_it() {
    let mut desk = Desk::new("");
    desk.type_text("a");
    desk.history.begin_group();
    desk.type_text("b");
    desk.history.end_group();
    desk.type_text("c");
    assert_eq!(desk.undo_all(), ["ab", "a", ""]);
}

#[test]
fn groups_nest() {
    let mut desk = Desk::new("");
    desk.history.begin_group();
    desk.type_text("a");
    desk.history.begin_group();
    desk.type_text("b\n");
    desk.history.begin_group();
    desk.history.end_group();
    desk.type_text("c");
    desk.history.end_group();
    desk.now += 9000;
    desk.type_text(" d");
    desk.history.end_group();
    assert_eq!(desk.text(), "ab\nc d");
    assert_eq!(desk.undo_all(), [""]);
    assert!(desk.redo().is_some());
    assert_eq!(desk.text(), "ab\nc d");
    // Once the outermost group has ended, recording is back to normal.
    desk.type_text(" e");
    desk.history.seal();
    desk.type_text("f");
    assert_eq!(desk.undo_all(), ["ab\nc d e", "ab\nc d ", "ab\nc d", ""]);
}

#[test]
fn undo_and_redo_report_what_there_is_to_do() {
    let mut desk = Desk::new("");
    assert!(!desk.history.can_undo() && !desk.history.can_redo());
    desk.type_text("ab");
    assert!(desk.history.can_undo() && !desk.history.can_redo());
    assert!(desk.undo().is_some());
    assert!(!desk.history.can_undo() && desk.history.can_redo());
    assert!(desk.undo().is_none());
    assert!(desk.redo().is_some());
    assert!(desk.history.can_undo() && !desk.history.can_redo());
    assert!(desk.redo().is_none());
    assert_eq!(desk.text(), "ab");
}

#[test]
fn clear_forgets_every_step() {
    let mut desk = Desk::new("");
    desk.type_text("one two three");
    assert!(desk.undo().is_some());
    assert!(desk.history.can_undo() && desk.history.can_redo());
    desk.history.clear();
    assert!(!desk.history.can_undo() && !desk.history.can_redo());
    assert!(desk.undo().is_none() && desk.redo().is_none());
    assert_eq!(desk.text(), "one two ");
    // What comes after cannot join a step that is gone.
    desk.type_text("x");
    assert_eq!(desk.undo_all(), ["one two "]);
}

#[test]
fn typing_merges_a_word_and_its_trailing_space_into_one_step() {
    let mut desk = Desk::new("");
    desk.type_text("hello brave  world ");
    assert_eq!(desk.undo_all(), ["hello brave  ", "hello ", ""]);
    for text in ["hello ", "hello brave  ", "hello brave  world "] {
        assert!(desk.redo().is_some());
        assert_eq!(desk.text(), text);
    }
}

#[test]
fn whitespace_after_whitespace_and_letters_after_letters_do_not_begin_a_new_word() {
    let mut desk = Desk::new("");
    desk.type_text("ab");
    desk.type_text("  \t ");
    assert_eq!(desk.undo_all(), [""]);
    // Whitespace typed first, then a word: the word begins a new step.
    let mut desk = Desk::new("");
    desk.type_text("  x y");
    assert_eq!(desk.undo_all(), ["  x ", "  ", ""]);
}

#[test]
fn a_seal_since_the_previous_record_prevents_merging() {
    let mut desk = Desk::new("abcdef");
    desk.backspace();
    desk.history.seal();
    desk.backspace();
    desk.backspace();
    assert_eq!(desk.undo_all(), ["abcde", "abcdef"]);
}

#[test]
fn changes_of_different_kinds_do_not_merge() {
    let mut desk = Desk::new("");
    desk.type_text("abc");
    desk.backspace();
    desk.type_text("d");
    assert_eq!(desk.text(), "abd");
    assert_eq!(desk.undo_all(), ["ab", "abc", ""]);
}

#[test]
fn changes_of_kind_other_never_merge() {
    let mut desk = Desk::new("abc");
    desk.edit(Span::at(p(0, 3)), "d", EditKind::Other);
    desk.edit(Span::at(p(0, 4)), "e", EditKind::Other);
    desk.edit(Span::new(p(0, 4), p(0, 5)), "", EditKind::Other);
    desk.edit(Span::new(p(0, 3), p(0, 4)), "", EditKind::Other);
    assert_eq!(desk.undo_all(), ["abcd", "abcde", "abcd", "abc"]);
    // Nor with an Insert or a Delete next to them.
    let mut desk = Desk::new("abc");
    desk.edit(Span::at(p(0, 3)), "d", EditKind::Insert);
    desk.edit(Span::at(p(0, 4)), "e", EditKind::Other);
    desk.edit(Span::at(p(0, 5)), "f", EditKind::Insert);
    assert_eq!(desk.undo_all(), ["abcde", "abcd", "abc"]);
}

#[test]
fn changes_within_1000_ms_of_the_previous_record_merge() {
    let mut desk = Desk::new("");
    for (wait, ch) in [(0, "a"), (999, "b"), (1000, "c"), (1, "d"), (0, "e"), (1000, "f")] {
        desk.now += wait;
        desk.edit(Span::at(desk.cursor.head), ch, EditKind::Insert);
    }
    // Three seconds from first key to last, but never more than a second between two.
    assert_eq!(desk.now, 3000);
    assert_eq!(desk.undo_all(), [""]);
}

#[test]
fn changes_more_than_1000_ms_after_the_previous_record_do_not_merge() {
    let mut desk = Desk::new("");
    for (wait, ch) in [(0, "a"), (1001, "b"), (40, "c"), (60_000, "d")] {
        desk.now += wait;
        desk.edit(Span::at(desk.cursor.head), ch, EditKind::Insert);
    }
    assert_eq!(desk.undo_all(), ["abc", "a", ""]);
    let mut desk = Desk::new("abcdef");
    desk.backspace();
    desk.now += 1001;
    desk.backspace();
    assert_eq!(desk.undo_all(), ["abcde", "abcdef"]);
}

#[test]
fn a_change_with_a_line_break_merges_with_nothing() {
    let mut desk = Desk::new("");
    desk.type_text("ab\ncd");
    assert_eq!(desk.undo_all(), ["ab\n", "ab", ""]);
    // Deleting: the backspace that joins two lines stands between two runs.
    let mut desk = Desk::new("ab\ncd");
    desk.cursor = Cursor::at(p(1, 1));
    for _ in 0..3 {
        desk.backspace();
    }
    assert_eq!(desk.text(), "ad");
    assert_eq!(desk.undo_all(), ["abd", "ab\nd", "ab\ncd"]);
    // And forward: delete, delete over the break, delete.
    let mut desk = Desk::new("ab\ncd");
    desk.cursor = Cursor::at(p(0, 1));
    for _ in 0..3 {
        desk.delete();
    }
    assert_eq!(desk.text(), "ad");
    assert_eq!(desk.undo_all(), ["acd", "a\ncd", "ab\ncd"]);
    // A break in what a change removed counts as much as one in what it inserted: the first key typed over
    // a selection that runs across a line break stands alone, and the rest of the word is a second step.
    let mut desk = Desk::new("ab\ncd");
    desk.cursor = Cursor { anchor: p(0, 1), head: p(1, 1) };
    desk.now += 40;
    desk.edit(Span::new(p(0, 1), p(1, 1)), "h", EditKind::Insert);
    desk.type_text("ello");
    assert_eq!(desk.text(), "ahellod");
    assert_eq!(desk.undo_all(), ["ahd", "ab\ncd"]);
}

#[test]
fn an_insert_merges_only_when_it_starts_where_the_previous_change_ended() {
    // Typing somewhere else.
    let mut desk = Desk::new("");
    desk.type_text("ab");
    desk.cursor = Cursor::at(p(0, 0));
    desk.type_text("cd");
    assert_eq!(desk.text(), "cdab");
    assert_eq!(desk.undo_all(), ["ab", ""]);
    // Inserting at the START of the previous insertion, again and again: never adjacent.
    let mut desk = Desk::new("");
    for ch in ["a", "b", "c"] {
        desk.now += 40;
        desk.cursor = Cursor::at(p(0, 0));
        desk.edit(Span::at(p(0, 0)), ch, EditKind::Insert);
    }
    assert_eq!(desk.text(), "cba");
    assert_eq!(desk.undo_all(), ["ba", "a", ""]);
    // Typing over a selection and carrying on from its end is adjacent.
    let mut desk = Desk::new("hello world");
    desk.now += 40;
    desk.edit(Span::new(p(0, 0), p(0, 5)), "X", EditKind::Insert);
    desk.type_text("YZ");
    assert_eq!(desk.text(), "XYZ world");
    assert_eq!(desk.undo_all(), ["hello world"]);
}

#[test]
fn a_delete_merges_as_backspace_or_as_forward_delete() {
    // Backspace: its old_end equals the previous change's start.
    let mut desk = Desk::new("abcd\u{00E9}f");
    desk.cursor = Cursor::at(p(0, 6));
    for _ in 0..3 {
        desk.backspace();
    }
    assert_eq!(desk.text(), "abf");
    assert_eq!(desk.undo_all(), ["abcd\u{00E9}f"]);
    // Forward delete: its start equals the previous change's start.
    let mut desk = Desk::new("ab\u{00E9}def");
    desk.cursor = Cursor::at(p(0, 1));
    for _ in 0..3 {
        desk.delete();
    }
    assert_eq!(desk.text(), "aef");
    assert_eq!(desk.undo_all(), ["ab\u{00E9}def"]);
    // The two mixed at one place.
    let mut desk = Desk::new("abcdefg");
    desk.cursor = Cursor::at(p(0, 4));
    desk.backspace();
    desk.delete();
    desk.backspace();
    desk.delete();
    assert_eq!(desk.text(), "abg");
    assert_eq!(desk.undo_all(), ["abcdefg"]);
    assert!(desk.redo().is_some());
    assert_eq!(desk.text(), "abg");
}

#[test]
fn a_delete_that_is_not_adjacent_does_not_merge() {
    let mut desk = Desk::new("abcdefgh");
    desk.cursor = Cursor::at(p(0, 8));
    desk.backspace();
    desk.cursor = Cursor::at(p(0, 3));
    desk.backspace();
    desk.cursor = Cursor::at(p(0, 0));
    desk.delete();
    assert_eq!(desk.text(), "bdefg");
    assert_eq!(desk.undo_all(), ["abdefg", "abcdefg", "abcdefgh"]);
}

#[test]
fn undo_reverts_in_reverse_order_and_gives_the_first_before_cursor() {
    let mut desk = Desk::new("hello world");
    let selection = Cursor { anchor: p(0, 5), head: p(0, 0) };
    desk.cursor = selection;
    desk.now = 40;
    desk.edit(selection.span(), "X", EditKind::Insert);
    desk.type_text("YZ");
    let replay = desk.undo().expect("a step to undo");
    assert_eq!(desk.text(), "hello world");
    assert_eq!(replay.cursor, selection);

    // A group whose changes only come apart in reverse order.
    let mut desk = Desk::new("ab");
    let first = Cursor { anchor: p(0, 2), head: p(0, 1) };
    desk.cursor = first;
    desk.history.begin_group();
    let c1 = desk.edit(Span::at(p(0, 1)), "1\n2", EditKind::Other);
    let c2 = desk.edit(Span::new(p(0, 0), p(1, 1)), "", EditKind::Other);
    let c3 = desk.edit(Span::at(p(0, 0)), "\n\nx", EditKind::Other);
    desk.history.end_group();
    assert_eq!(desk.text(), "\n\nxb");
    let replay = desk.undo().expect("a step to undo");
    assert_eq!(desk.text(), "ab");
    assert_eq!(replay.cursor, first);
    // The changes as Doc::replace returned them while replaying, in the order applied.
    let mut check = Doc::from_text("\n\nxb");
    let want: Vec<Change> = [&c3, &c2, &c1].into_iter().map(|c| check.revert(c)).collect();
    assert_eq!(replay.changes, want);
}

#[test]
fn redo_reapplies_in_order_and_gives_the_last_after_cursor() {
    let mut desk = Desk::new("ab");
    desk.history.begin_group();
    let c1 = desk.edit_with(Span::at(p(0, 1)), "1\n2", EditKind::Other, Some(Cursor::at(p(0, 0))));
    let c2 = desk.edit_with(Span::new(p(0, 0), p(1, 1)), "", EditKind::Other, Some(Cursor::at(p(0, 1))));
    let last = Cursor { anchor: p(0, 0), head: p(2, 1) };
    let c3 = desk.edit_with(Span::at(p(0, 0)), "\n\nx", EditKind::Other, Some(last));
    desk.history.end_group();
    assert!(desk.undo().is_some());
    assert_eq!(desk.text(), "ab");
    let replay = desk.redo().expect("a step to redo");
    assert_eq!(desk.text(), "\n\nxb");
    assert_eq!(replay.cursor, last);
    assert_eq!(replay.changes, vec![c1, c2, c3]);

    // A merged run: the cursor after its last key.
    let mut desk = Desk::new("say ");
    desk.type_text("mitsqanaqan\u{0313}");
    let undone = desk.undo().expect("a step to undo");
    assert_eq!(undone.cursor, Cursor::at(p(0, 4)));
    let redone = desk.redo().expect("a step to redo");
    assert_eq!(redone.cursor, Cursor::at(p(0, 17)));
    assert_eq!(desk.text(), "say mitsqanaqan\u{0313}");
}

// ---------------------------------------------------------------------------
// history: a model of the merging rules, and random sessions against it
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct ModelStep {
    text_before: String,
    text_after: String,
    before: Cursor,
    after: Cursor,
}

#[derive(Clone, Debug)]
struct ModelLast {
    kind: EditKind,
    at: u64,
    change: Change,
}

/// The contract's merging rules, kept as whole texts so that nothing depends
/// on how changes are stored.
struct Model {
    undo: Vec<ModelStep>,
    redo: Vec<ModelStep>,
    open: bool,
    depth: usize,
    last: Option<ModelLast>,
    limit: usize,
}

fn has_break(change: &Change) -> bool {
    change.removed.contains('\n') || change.inserted.contains('\n')
}

/// "A recorded change joins the open step only when ALL of these hold."
fn contract_merges(prev: &ModelLast, change: &Change, kind: EditKind, now: u64) -> bool {
    if kind != prev.kind || kind == EditKind::Other {
        return false;
    }
    if now.abs_diff(prev.at) > 1000 {
        return false;
    }
    if has_break(&prev.change) || has_break(change) {
        return false;
    }
    match kind {
        EditKind::Insert => {
            if change.start != prev.change.end {
                return false;
            }
            let prev_ended_in_space = prev.change.inserted.chars().last().is_some_and(char::is_whitespace);
            let begins_a_word = change.inserted.chars().next().is_some_and(|ch| !ch.is_whitespace());
            !(prev_ended_in_space && begins_a_word)
        }
        EditKind::Delete => change.old_end == prev.change.start || change.start == prev.change.start,
        EditKind::Other => false,
    }
}

impl Model {
    fn new(limit: usize) -> Model {
        Model { undo: Vec::new(), redo: Vec::new(), open: false, depth: 0, last: None, limit }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        change: &Change,
        kind: EditKind,
        before: Cursor,
        after: Cursor,
        now: u64,
        text_before: &str,
        text_after: &str,
    ) {
        if change.is_noop() {
            return;
        }
        self.redo.clear();
        let continues = self.last.as_ref().is_some_and(|prev| contract_merges(prev, change, kind, now));
        let joins = self.open && !self.undo.is_empty() && (self.depth > 0 || continues);
        if joins {
            let step = self.undo.last_mut().expect("not empty");
            step.text_after = text_after.to_string();
            step.after = after;
        } else {
            self.undo.push(ModelStep {
                text_before: text_before.to_string(),
                text_after: text_after.to_string(),
                before,
                after,
            });
            while self.undo.len() > self.limit {
                self.undo.remove(0);
            }
        }
        self.open = true;
        self.last = Some(ModelLast { kind, at: now, change: change.clone() });
    }

    fn seal(&mut self) {
        self.open = false;
    }

    fn begin_group(&mut self) {
        if self.depth == 0 {
            self.open = false;
        }
        self.depth += 1;
    }

    fn end_group(&mut self) {
        self.depth -= 1;
        if self.depth == 0 {
            self.open = false;
        }
    }

    fn undo(&mut self) -> Option<ModelStep> {
        let step = self.undo.pop()?;
        self.open = false;
        self.redo.push(step.clone());
        Some(step)
    }

    fn redo(&mut self) -> Option<ModelStep> {
        let step = self.redo.pop()?;
        self.open = false;
        self.undo.push(step.clone());
        Some(step)
    }
}

struct Session {
    desk: Desk,
    model: Model,
    log: Vec<String>,
    seed: u64,
}

impl Session {
    fn new(seed: u64, start: &str, limit: usize) -> Session {
        Session {
            desk: Desk::with_history(start, History::with_limit(limit)),
            model: Model::new(limit),
            log: vec![format!("start {start:?} limit {limit}")],
            seed,
        }
    }

    fn fail(&self, what: &str) -> ! {
        let from = self.log.len().saturating_sub(30);
        panic!("{what}\nseed {:#x}\nlast operations:\n{}", self.seed, self.log[from..].join("\n"));
    }

    fn agree(&self) {
        if self.desk.history.can_undo() == self.model.undo.is_empty() {
            self.fail("can_undo disagrees with the model");
        }
        if self.desk.history.can_redo() == self.model.redo.is_empty() {
            self.fail("can_redo disagrees with the model");
        }
    }

    fn edit(&mut self, span: Span, text: &str, kind: EditKind, after: Option<Cursor>) {
        let text_before = self.desk.text();
        let before = self.desk.cursor;
        let change = self.desk.edit_with(span, text, kind, after);
        let now = self.desk.now;
        self.log.push(format!("t={now} {kind:?} {change:?}"));
        self.model.record(&change, kind, before, self.desk.cursor, now, &text_before, &self.desk.text());
        self.agree();
    }

    fn seal(&mut self) {
        self.log.push("seal".into());
        self.desk.history.seal();
        self.model.seal();
    }

    fn begin_group(&mut self) {
        self.log.push("begin_group".into());
        self.desk.history.begin_group();
        self.model.begin_group();
    }

    fn end_group(&mut self) {
        self.log.push("end_group".into());
        self.desk.history.end_group();
        self.model.end_group();
    }

    fn undo(&mut self) -> bool {
        self.log.push("undo".into());
        let got = self.desk.undo();
        let want = self.model.undo();
        self.compare(got, want.map(|step| (step.text_before, step.before)), "undo")
    }

    fn redo(&mut self) -> bool {
        self.log.push("redo".into());
        let got = self.desk.redo();
        let want = self.model.redo();
        self.compare(got, want.map(|step| (step.text_after, step.after)), "redo")
    }

    fn compare(&mut self, got: Option<Replay>, want: Option<(String, Cursor)>, what: &str) -> bool {
        match (got, want) {
            (None, None) => false,
            (Some(replay), Some((text, cursor))) => {
                if self.desk.text() != text {
                    self.fail(&format!("{what} gave {:?}, the contract's steps give {text:?}", self.desk.text()));
                }
                if replay.cursor != cursor {
                    self.fail(&format!("{what} gave cursor {:?}, the contract's steps give {cursor:?}", replay.cursor));
                }
                self.agree();
                true
            }
            (got, want) => {
                self.fail(&format!("{what}: got a step {}, the model {}", got.is_some(), want.is_some()));
            }
        }
    }
}

const WAITS: &[u64] = &[0, 0, 1, 40, 40, 40, 300, 999, 1000, 1001, 2500];
const KINDS: [EditKind; 3] = [EditKind::Insert, EditKind::Delete, EditKind::Other];

/// A random session of editing against the model. Things the contract does
/// not settle are kept out: no seal, undo or redo inside a group, and no
/// end_group without a begin_group.
fn model_session(seed: u64, limit: usize, operations: usize) {
    let mut rng = Rng::new(seed);
    let start = random_text(&mut rng, 8);
    let mut s = Session::new(seed, &start, limit);
    for _ in 0..operations {
        s.desk.now += WAITS[rng.below(WAITS.len())];
        if s.desk.doc.len_bytes() > 300 {
            // Keep the document small: cut it down as one more ordinary edit.
            let cut = random_pos(&mut rng, &s.desk.doc);
            s.edit(Span::new(p(0, 0), cut), "", KINDS[rng.below(3)], None);
            continue;
        }
        let grouped = s.model.depth > 0;
        match rng.below(30) {
            0 | 1 if !grouped => s.seal(),
            2 if s.model.depth < 3 => s.begin_group(),
            3 | 4 if grouped => s.end_group(),
            5 => {
                // Move the cursor without telling the history, sometimes leaving a selection.
                let head = s.desk.doc.clamp(random_pos(&mut rng, &s.desk.doc));
                let anchor = if rng.below(3) == 0 { s.desk.doc.clamp(random_pos(&mut rng, &s.desk.doc)) } else { head };
                s.desk.cursor = Cursor { head, anchor };
                s.log.push(format!("cursor {:?}", s.desk.cursor));
            }
            6 | 7 if !grouped => {
                s.undo();
            }
            8 if !grouped => {
                s.redo();
            }
            9..=12 => {
                s.desk.now += 40;
                let span = s.desk.backspace_span();
                s.edit(span, "", EditKind::Delete, None);
            }
            13..=15 => {
                s.desk.now += 40;
                let span = s.desk.delete_span();
                s.edit(span, "", EditKind::Delete, None);
            }
            16 => {
                let span = Span::new(random_pos(&mut rng, &s.desk.doc), random_pos(&mut rng, &s.desk.doc));
                s.edit(span, rng.pick(HOSTILE), KINDS[rng.below(3)], None);
            }
            17 => {
                let span = Span::new(random_pos(&mut rng, &s.desk.doc), random_pos(&mut rng, &s.desk.doc));
                s.edit(span, "", EditKind::Delete, None);
            }
            18 => {
                // A paste that leaves what it pasted selected.
                let at = s.desk.cursor.head;
                let change_start = s.desk.doc.clamp(at);
                let text = rng.pick(HOSTILE);
                s.edit(Span::at(at), text, KINDS[rng.below(3)], None);
                s.desk.cursor = Cursor { anchor: change_start, head: s.desk.cursor.head };
            }
            19 => {
                // Typing over the selection.
                let span = s.desk.cursor.span();
                s.edit(span, rng.pick(HOSTILE), EditKind::Insert, None);
            }
            20 => s.edit(Span::at(s.desk.cursor.head), "", KINDS[rng.below(3)], None),
            _ => {
                let piece = rng.pick(HOSTILE);
                for ch in piece.chars() {
                    s.desk.now += [0, 40, 40, 120][rng.below(4)];
                    let at = s.desk.cursor.head;
                    s.edit(Span::at(at), ch.encode_utf8(&mut [0; 4]), EditKind::Insert, None);
                }
            }
        }
    }
    while s.model.depth > 0 {
        s.end_group();
    }
    while s.redo() {}
    let last = s.desk.text();
    while s.undo() {}
    if s.desk.history.can_undo() {
        s.fail("steps left after the model ran out");
    }
    while s.redo() {}
    if s.desk.text() != last {
        s.fail("redoing everything did not restore the final text");
    }
    s.desk.settle();
}

#[test]
fn random_sessions_fall_into_the_steps_the_contract_describes() {
    let mut seeds = Rng::new(0x9E37_79B9_7F4A_7C15);
    for _ in 0..rounds(300) {
        model_session(seeds.next(), 10_000, 220);
    }
}

#[test]
fn random_sessions_with_small_limits_drop_the_oldest_steps() {
    let mut seeds = Rng::new(0xC2B2_AE3D_27D4_EB4F);
    for round in 0..rounds(200) {
        model_session(seeds.next(), [1, 2, 3, 5, 8][round % 5], 160);
    }
}

/// Anything goes: seals, undo and redo inside groups, unmatched end_group,
/// clear, a clock that jumps both ways, kinds that do not describe the
/// change. However the steps fall, undoing everything gives back the text
/// the history started from, redoing everything gives the final text, and
/// every replayed change is exactly what Doc::replace returns.
#[test]
fn chaotic_sessions_undo_to_the_start_and_redo_to_the_end() {
    let mut rng = Rng::new(0x1656_67B1_9E37_79F9);
    for _ in 0..rounds(250) {
        let start = random_text(&mut rng, 10);
        let mut desk = Desk::new(&start);
        let mut origin = desk.text();
        for _ in 0..200 {
            desk.now = match rng.below(12) {
                0 => rng.next(),
                1 => desk.now.saturating_sub(rng.below(3000) as u64),
                2 => u64::MAX - rng.below(3) as u64,
                3 => 0,
                _ => desk.now.saturating_add(WAITS[rng.below(WAITS.len())]),
            };
            if desk.doc.len_bytes() > 300 {
                let cut = random_pos(&mut rng, &desk.doc);
                desk.edit(Span::new(p(0, 0), cut), "", KINDS[rng.below(3)]);
                continue;
            }
            match rng.below(28) {
                0 => desk.history.seal(),
                1 | 2 => desk.history.begin_group(),
                3..=5 => desk.history.end_group(),
                6 => {
                    desk.cursor =
                        Cursor { head: random_pos(&mut rng, &desk.doc), anchor: random_pos(&mut rng, &desk.doc) }
                }
                7 | 8 => {
                    desk.undo();
                }
                9 => {
                    desk.redo();
                }
                10 if rng.below(8) == 0 => {
                    desk.history.clear();
                    origin = desk.text();
                }
                11..=13 => {
                    desk.backspace();
                }
                14 | 15 => {
                    desk.delete();
                }
                16 | 17 => {
                    let span = Span::new(random_pos(&mut rng, &desk.doc), random_pos(&mut rng, &desk.doc));
                    desk.edit(span, rng.pick(HOSTILE), KINDS[rng.below(3)]);
                }
                18 => {
                    let span = desk.cursor.span();
                    desk.edit(span, rng.pick(HOSTILE), KINDS[rng.below(3)]);
                }
                19 => {
                    // The same change, mislabelled, right where the last one was.
                    let at = desk.cursor.head;
                    desk.edit(Span::at(at), rng.pick(HOSTILE), EditKind::Delete);
                    let span = desk.backspace_span();
                    desk.edit(span, "", EditKind::Insert);
                }
                _ => desk.type_text(rng.pick(HOSTILE)),
            }
        }
        while desk.redo().is_some() {}
        let last = desk.text();
        let mut texts = vec![last.clone()];
        texts.extend(desk.undo_all());
        assert_eq!(desk.text(), origin, "undoing everything did not restore the original text");
        assert!(!desk.history.can_undo());
        for text in texts.iter().rev().skip(1) {
            assert!(desk.redo().is_some());
            assert_eq!(&desk.text(), text, "a redo did not land on the text its undo left from");
        }
        assert!(desk.redo().is_none());
        assert_eq!(desk.text(), last, "redoing everything did not restore the final text");
        desk.settle();
    }
}

/// Changes that never came from this document must not bring anything down.
#[test]
fn made_up_changes_and_cursors_cannot_panic() {
    let mut rng = Rng::new(0x27D4_EB2F_1656_67C5);
    let wild = |rng: &mut Rng| -> usize {
        match rng.below(6) {
            0 => usize::MAX,
            1 => usize::MAX - rng.below(3),
            2 => 0,
            _ => rng.below(9),
        }
    };
    for _ in 0..rounds(400) {
        let mut doc = Doc::from_text(&random_text(&mut rng, 8));
        let mut history = History::with_limit(rng.below(6));
        let mut tally = Tally::of(&doc);
        for _ in 0..40 {
            let change = Change {
                start: p(wild(&mut rng), wild(&mut rng)),
                old_end: p(wild(&mut rng), wild(&mut rng)),
                end: p(wild(&mut rng), wild(&mut rng)),
                removed: rng.pick(HOSTILE).to_string(),
                inserted: rng.pick(HOSTILE).to_string(),
            };
            let cursor = Cursor { head: p(wild(&mut rng), wild(&mut rng)), anchor: p(wild(&mut rng), wild(&mut rng)) };
            let _ = (cursor.span(), cursor.has_selection());
            tally.apply(&doc, &change);
            let _ = (tally.words(), tally.line_words(wild(&mut rng)));
            history.record(change, KINDS[rng.below(3)], cursor, cursor, rng.next());
            match rng.below(8) {
                0 => history.seal(),
                1 => history.begin_group(),
                2 => history.end_group(),
                3 => {
                    history.undo(&mut doc);
                }
                4 => {
                    history.redo(&mut doc);
                }
                _ => {}
            }
        }
        while history.undo(&mut doc).is_some() {}
        while history.redo(&mut doc).is_some() {}
        // After a made-up change the tally may be anything, but one that does not fit must leave it right.
        let stray = Change { start: p(usize::MAX, 0), ..Change::default() };
        tally.apply(&doc, &stray);
        assert_eq!(tally.words(), count::words(&text_of(&doc)));
    }
}

// ---------------------------------------------------------------------------
// Arbitrary code points
// ---------------------------------------------------------------------------

/// The curated pieces above cover the classes of character the rules name.
/// This goes wider: text drawn from the whole of Unicode, through every
/// motion from every position, every count, and searches for pieces of it.
#[test]
fn motions_counts_and_searches_agree_with_the_models_on_arbitrary_code_points() {
    let mut rng = Rng::new(0x510E_527F_ADE6_82D1);
    for _ in 0..rounds(3000) {
        let text = random_unicode_text(&mut rng, 24);
        assert_eq!(count::words(&text), ref_count_words(&text), "words of {text:?}");
        assert_eq!(count::clusters(&text), ref_count_clusters(&text), "clusters of {text:?}");
        assert_eq!(count::sentences(&text), ref_count_sentences(&text), "sentences of {text:?}");
        let doc = Doc::from_text(&text);
        check_motions(&doc);
        assert_tally(&Tally::of(&doc), &doc, "a fresh tally");

        let line = rng.below(doc.line_count());
        let cuts: Vec<usize> = doc.line(line).char_indices().map(|(i, _)| i).chain([doc.line(line).len()]).collect();
        let (a, b) = (cuts[rng.below(cuts.len())], cuts[rng.below(cuts.len())]);
        let piece = &doc.line(line)[a.min(b)..a.max(b)];
        let needle = match rng.below(5) {
            0 => piece.to_uppercase(),
            1 => fold(piece),
            2 => format!("{piece}{}", random_char(&mut rng)),
            3 => (0..1 + rng.below(3)).map(|_| random_char(&mut rng)).collect(),
            _ => piece.to_string(),
        };
        check_search(&doc, &needle, &mut rng, false);
    }
}

/// Typing, deleting and pasting arbitrary characters: the word count, the
/// per-line table and the second copy of the document all follow from the
/// changes alone, and everything comes back on undo and goes again on redo.
#[test]
fn sessions_of_arbitrary_code_points_undo_and_redo_exactly() {
    let mut rng = Rng::new(0x9B05_688C_1F83_D9AB);
    for _ in 0..rounds(150) {
        let start = random_unicode_text(&mut rng, 12);
        let mut desk = Desk::new(&start);
        let origin = desk.text();
        for _ in 0..150 {
            desk.now += WAITS[rng.below(WAITS.len())];
            if desk.doc.len_bytes() > 300 {
                let cut = random_pos(&mut rng, &desk.doc);
                desk.edit(Span::new(p(0, 0), cut), "", EditKind::Delete);
                continue;
            }
            match rng.below(16) {
                0 => desk.history.seal(),
                1 => desk.cursor = Cursor::at(desk.doc.clamp(random_pos(&mut rng, &desk.doc))),
                2 | 3 => {
                    desk.undo();
                }
                4 => {
                    desk.redo();
                }
                5..=7 => {
                    desk.backspace();
                }
                8 | 9 => {
                    desk.delete();
                }
                10 => {
                    let span = Span::new(random_pos(&mut rng, &desk.doc), random_pos(&mut rng, &desk.doc));
                    let pasted = random_unicode_text(&mut rng, 6);
                    desk.edit(span, &pasted, KINDS[rng.below(3)]);
                }
                _ => {
                    let typed = random_unicode_text(&mut rng, 4);
                    desk.type_text(&typed);
                }
            }
            desk.settle();
        }
        while desk.redo().is_some() {}
        let last = desk.text();
        let steps = desk.undo_all().len();
        assert_eq!(desk.text(), origin, "undoing everything did not restore the original text");
        for _ in 0..steps {
            assert!(desk.redo().is_some());
        }
        assert!(desk.redo().is_none());
        assert_eq!(desk.text(), last, "redoing everything did not restore the final text");
    }
}

// ---------------------------------------------------------------------------
// Where two parts of the contract meet
// ---------------------------------------------------------------------------

/// `History::undo` and `redo` apply every change of a step before handing
/// back the list, so for a step of several changes (a group) the caller only
/// has the document as the last of them left it, while `Tally::apply` asks
/// for the document as it was straight after the one change it is given. The
/// contract does not say what the obvious loop does then. It holds, by way
/// of the full recount `apply` falls back on when its table and the document
/// disagree about the number of lines; this test is here so that taking the
/// fallback out is noticed.
#[test]
fn a_whole_replay_can_be_fed_to_the_tally_with_the_document_as_it_ends_up() {
    let mut rng = Rng::new(0x94D0_49BB_1331_11EB);
    for _ in 0..rounds(400) {
        let mut doc = Doc::from_text(&random_text(&mut rng, 14));
        let mut history = History::new();
        let mut tally = Tally::of(&doc);
        let nowhere = Cursor::default();
        for _ in 0..12 {
            history.begin_group();
            for _ in 0..1 + rng.below(5) {
                let (a, b) = (random_pos(&mut rng, &doc), random_pos(&mut rng, &doc));
                let span = if rng.below(2) == 0 { Span::new(a, b) } else { Span::at(a) };
                let change = doc.replace(span, rng.pick(HOSTILE));
                tally.apply(&doc, &change);
                history.record(change, KINDS[rng.below(3)], nowhere, nowhere, 0);
            }
            history.end_group();
            assert_tally(&tally, &doc, "after a group");
            for _ in 0..rng.below(4) {
                let replay = if rng.below(3) == 0 { history.redo(&mut doc) } else { history.undo(&mut doc) };
                for change in replay.iter().flat_map(|replay| &replay.changes) {
                    tally.apply(&doc, change);
                }
                assert_tally(&tally, &doc, "after replaying a step change by change against the final document");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cost per key
// ---------------------------------------------------------------------------

/// A run of keys on one line of `a`s, every change made by `Doc::replace`:
/// first a single change of `len` characters standing in for the `len` keys
/// that came before (reaching that point a key at a time costs the very time
/// these tests are about), then `keys` single keys carrying on from it. Gives
/// the changes and the text before and after the whole run.
fn long_run(kind: &str, len: usize, keys: usize) -> (Vec<Change>, String, String) {
    let total = len + keys;
    let mut doc = if kind == "typing" { Doc::new() } else { Doc::from_text(&"a".repeat(total)) };
    let before = text_of(&doc);
    let mut changes = Vec::with_capacity(keys + 1);
    if len > 0 {
        changes.push(match kind {
            "typing" => doc.replace(Span::at(p(0, 0)), &"a".repeat(len)),
            "backspace" => doc.replace(Span::new(p(0, keys), p(0, total)), ""),
            _ => doc.replace(Span::new(p(0, 0), p(0, len)), ""),
        });
    }
    for _ in 0..keys {
        let end = doc.end();
        changes.push(match kind {
            "typing" => doc.replace(Span::at(end), "a"),
            "backspace" => doc.replace(Span::new(p(0, end.col - 1), end), ""),
            _ => doc.replace(Span::new(p(0, 0), p(0, 1)), ""),
        });
    }
    (changes, before, text_of(&doc))
}

/// Nanoseconds to record one key that joins a run already `len` characters
/// long: the least of three batches of a hundred keys, after fifty to warm
/// up. Also checks that the whole run is one step and comes back exactly.
fn cost_of_a_key(kind: &str, edit: EditKind, len: usize) -> f64 {
    const WARM: usize = 50;
    const BATCH: usize = 100;
    let (changes, before, after) = long_run(kind, len, WARM + 3 * BATCH);
    let mut history = History::new();
    let nowhere = Cursor::default();
    let mut clock = 0;
    let mut changes = changes.into_iter();
    let opening = if len > 0 { 1 } else { 0 };
    for change in changes.by_ref().take(opening + WARM) {
        clock += 1;
        history.record(change, edit, nowhere, nowhere, clock);
    }
    let mut least = f64::MAX;
    for _ in 0..3 {
        let batch: Vec<Change> = changes.by_ref().take(BATCH).collect();
        let started = std::time::Instant::now();
        for change in batch {
            clock += 1;
            history.record(change, edit, nowhere, nowhere, clock);
        }
        least = least.min(started.elapsed().as_nanos() as f64 / BATCH as f64);
    }
    let mut doc = Doc::from_text(&after);
    assert!(history.undo(&mut doc).is_some());
    assert!(text_of(&doc) == before, "{kind}: undoing the run did not give back the text before it");
    assert!(!history.can_undo(), "{kind}: the run was more than one step");
    assert!(history.redo(&mut doc).is_some());
    assert!(text_of(&doc) == after, "{kind}: redoing the run did not give back the text after it");
    least
}

/// How many times as much a key costs after a run of 3,200,000 characters as
/// after one of 100,000. About 1 when the cost of a key does not depend on
/// the run; about 32 when it is in proportion to it.
fn growth_over_a_run_32_times_as_long(kind: &str, edit: EditKind) -> f64 {
    let fresh = cost_of_a_key(kind, edit, 0);
    let short = cost_of_a_key(kind, edit, 100_000);
    let long = cost_of_a_key(kind, edit, 3_200_000);
    println!("{kind}: {fresh:.0} ns a key at the start of a run, {short:.0} ns 100,000 in, {long:.0} ns 3,200,000 in");
    long / short
}

/// Recording a key that joins a run used to look through the whole of the
/// run so far for a line break: `fold` called `has_break` on the change it
/// had been folding every earlier key into. So a key cost time in proportion
/// to the run before it and a run of n keys cost n squared.
///
/// It took a long run to notice: measured here in a debug build, about 5
/// microseconds a key 100,000 characters in and 150 microseconds 3,200,000
/// in, against half a microsecond at the start. A run of typing is ended by
/// the next word, so in practice this was holding Delete through a very long
/// paragraph, or carrying on from a long one-line paste or cut recorded as
/// Insert or Delete. The text already folded is known to hold no break.
#[test]
fn the_cost_of_recording_a_key_does_not_grow_with_the_length_of_the_run() {
    let typing = growth_over_a_run_32_times_as_long("typing", EditKind::Insert);
    let deleting = growth_over_a_run_32_times_as_long("forward delete", EditKind::Delete);
    assert!(
        typing < 4.0 && deleting < 4.0,
        "with the run before it 32 times as long, a typed key costs {typing:.0} times as much and a forward delete {deleting:.0} times"
    );
}

/// Holding Backspace: the same search for a line break, and on top of it
/// each key used to move the whole of the removed text along to put one
/// character in front of it (`insert_str(0, ..)`).
#[test]
fn the_cost_of_recording_a_backspace_does_not_grow_with_the_length_of_the_run() {
    let ratio = growth_over_a_run_32_times_as_long("backspace", EditKind::Delete);
    assert!(ratio < 4.0, "a backspace costs {ratio:.0} times as much when the run before it is 32 times as long");
}

/// Searching without case used to try the whole needle afresh at every
/// character that could begin a match, so text that kept almost matching
/// cost text length times needle length: here a line of one character, and
/// needles that are a shorter run of it and then something else. The
/// case-sensitive search over the same text does not care how long the
/// needle is.
///
/// Ordinary prose never came near this. It took a long needle and text with
/// long runs of what the needle begins with (a rule of dashes, a run of
/// spaces), which matters only if every key typed into a search box runs
/// `find_all` over a large document.
#[test]
fn the_cost_of_a_search_without_case_does_not_grow_with_text_length_times_needle_length() {
    let doc = Doc::from_text(&"a".repeat(8_000));
    let short = format!("{}b", "a".repeat(4));
    let long = format!("{}b", "a".repeat(512));
    let time = |needle: &str, case_sensitive: bool| -> f64 {
        let mut least = f64::MAX;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            assert!(search::find_all(&doc, needle, case_sensitive).is_empty());
            least = least.min(started.elapsed().as_nanos() as f64);
        }
        least
    };
    let (exact_short, exact_long) = (time(&short, true), time(&long, true));
    let (folded_short, folded_long) = (time(&short, false), time(&long, false));
    println!("case-sensitive: {exact_short:.0} ns and {exact_long:.0} ns; without case: {folded_short:.0} ns and {folded_long:.0} ns");
    let ratio = folded_long / folded_short;
    assert!(ratio < 8.0, "a needle 100 times as long made the search without case {ratio:.0} times as slow");
}
