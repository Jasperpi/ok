//! Adversarial tests for `ok_text::wrap`, written against the contract and
//! using only the public API.
//!
//! How this file is laid out:
//!
//!   1. a deterministic xorshift generator and hostile text
//!   2. reference models (clusters, wrap, the cell grid, the row index)
//!   3. property tests over thousands of random inputs
//!   4. one targeted case per sentence of the contract
//!   5. FAILING: reduced cases where the implementation departs from the
//!      contract (search for "FAILING"); none fails as wrap.rs stands
//!   6. hazards outside the contract, `#[ignore]`d while they stand so they
//!      do not colour the run (`cargo test --offline -- --ignored` shows
//!      them); none stands now
//!
//! Where the contract is silent or contradicts itself, the models follow the
//! decisions the owner of wrap.rs documented, so that the property tests
//! exercise everything else. Those places are marked "OWNER'S READING".
//!
//! Everything random is driven by a fixed-seed xorshift, so a run is
//! repeatable. `OK_ADVERSARIAL_SCALE=25 cargo test --offline --test
//! adversarial` runs the random tests 25 times over (about a minute).
//!
//! SECOND ROUND. wrap.rs changed after the first version of this file: a
//! space or tab is now whitespace whatever marks sit on it. The models here
//! follow that, the two tests that used to fail for it now pass and have
//! moved to section 4, and the hunt went on from there.
//!
//! THIRD ROUND, by the owner of wrap.rs in answer to the second. Up and Down
//! now land on the nearest place a cursor can rest, which only a tab can
//! tell from the cluster under the wanted cell: the test about that passes
//! and has lost its `failing_` name, the model's `up` and `down` follow the
//! contract's sentence instead of going through `pos_at`, and the property
//! test beside it no longer steps round rows with tabs. `apply` counts a
//! change's lines without overflowing, so the one hazard runs un-ignored.
//! The four unit tests in src/ whose reference model still took a marked
//! space for a letter are in step again, and plain `cargo test --offline`
//! reaches this file.
//!
//! FOURTH ROUND, with the page's shapes. `Layout` no longer takes a hang for
//! each line but a `Shape`: bytes hung in the gutter, a scale of 1 or 2, the
//! hang, and an inset. Every test that was here still passes a hang and
//! nothing else, through `hanging`, and checks what it always checked.
//! Section 7 is new: documents whose lines are of mixed shape and height,
//! against a model of the grid built row by row.

use ok_text::wrap::{cell_of, col_at, wrap, Layout, Row, Shape};
use ok_text::{is_combining, str_width, Change, Doc, Pos, Span};
use std::cell::RefCell;
use std::time::Instant;

// ───────────────────────────── 1. generator ─────────────────────────────

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
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
            (self.next() >> 16) as usize % n
        }
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick(&mut self, xs: &'static [&'static str]) -> &'static str {
        xs[self.below(xs.len())]
    }
}

/// How many times over to run the random tests. `OK_ADVERSARIAL_SCALE=20
/// cargo test --offline --test adversarial` for a longer campaign.
fn scale() -> usize {
    std::env::var("OK_ADVERSARIAL_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1)
}

const LETTERS: &[&str] =&["a", "b", "c", "d", "e", "i", "o", "x", "y", "z", "Q", "7"];
const WORDS: &[&str] =
    &["a", "I", "to", "the", "and", "word", "prose", "editor", "paragraph", "antidisestablishmentarianism"];
const SPACES: &[&str] = &[" ", " ", " ", " ", "  ", "   ", "\t", " \t", "\t\t", "        "];
const BREAKERS: &[&str] = &["-", "/", "\u{2013}", "\u{2014}", "--", "\u{2010}", "-/", "//"];
const NON_BREAKERS: &[&str] =
    &[".", ",", ";", ":", "_", "\\", "'", "\u{2011}", "\u{2212}", "\u{00A0}", "\u{2009}", "(", ")"];
const MARKS: &[&str] = &[
    "\u{0301}",
    "\u{0313}",
    "\u{0308}\u{0304}",
    "\u{0335}\u{0321}\u{0353}\u{0310}\u{0351}\u{0300}",
    "\u{20DD}",
    "\u{FE0F}",
    "\u{200D}",
    "\u{200B}",
    "\u{200E}",
    "\u{200F}",
    "\u{202E}",
    "\u{202C}",
    "\u{2060}",
    "\u{FEFF}",
    "\u{E0100}",
    "\u{05B4}",
    "\u{064E}",
    "\u{0E31}",
];
const WIDE: &[&str] = &[
    "\u{4E00}",
    "\u{4E8C}\u{4E09}",
    "\u{FF21}",
    "\u{AC00}",
    "\u{3042}",
    "\u{3000}",
    "\u{1F600}",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{1F44D}\u{1F3FD}",
    "\u{20000}",
];
const RTL: &[&str] = &[
    "\u{05E9}\u{05DC}\u{05D5}\u{05DD}",
    "\u{05E2}\u{05B4}\u{05D1}\u{05B0}\u{05E8}\u{05B4}\u{05D9}\u{05EA}",
    "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}",
    "\u{0628}\u{0650}\u{0633}\u{0652}\u{0645}\u{0650}",
    "\u{200F}",
    "\u{202B}",
    "\u{202C}",
];
const ODD: &[&str] = &[
    "\u{0130}",
    "i\u{0307}",
    "\u{00DF}",
    "\u{00E9}",
    "e\u{0301}",
    "n\u{0313}",
    "\u{10348}",
    "\u{1F1FA}\u{1F1F8}",
    "\u{2764}\u{FE0F}",
    " \u{0301}",
    "\t\u{0313}",
    " \u{200F}",
];
/// Spaces and tabs with something drawn over them, or with a direction mark
/// or joiner after them: the clusters wrap.rs changed its mind about.
const MARKED_SPACES: &[&str] = &[
    " \u{0301}",
    " \u{200F}",
    " \u{200E}",
    " \u{200B}",
    " \u{FE0F}",
    " \u{202C}",
    " \u{20DD}",
    " \u{0335}\u{0321}\u{0353}",
    "\t\u{0313}",
    "\t\u{200D}",
    "  \u{0301} ",
    " \u{200F} \u{200F}",
    "\u{200F} ",
];

fn atom(rng: &mut Rng, lists: &[&'static [&'static str]]) -> &'static str {
    let list = lists[rng.below(lists.len())];
    list[rng.below(list.len())]
}

/// One line of hostile text: no line breaks, anything else goes.
fn hostile_line(rng: &mut Rng) -> String {
    let mut s = String::new();
    let n = rng.below(28);
    match rng.below(13) {
        10 | 11 => {
            // Words held apart by marked whitespace, sometimes starting or
            // ending with it.
            for _ in 0..n {
                s.push_str(atom(
                    rng,
                    &[MARKED_SPACES, MARKED_SPACES, MARKED_SPACES, WORDS, LETTERS, LETTERS, BREAKERS, SPACES, WIDE, MARKS],
                ));
            }
        }
        12 => {
            // Mixed-direction prose: every space is followed by a direction mark.
            for i in 0..n {
                if i > 0 {
                    s.push_str([" \u{200F}", " \u{200E}", " ", "\u{200F} "][rng.below(4)]);
                }
                s.push_str(if rng.one_in(2) { rng.pick(RTL) } else { rng.pick(WORDS) });
                if rng.one_in(7) {
                    s.push_str(rng.pick(BREAKERS));
                }
            }
        }
        0 | 1 => {
            for i in 0..n {
                if i > 0 {
                    s.push_str(if rng.one_in(8) { "  " } else { " " });
                }
                s.push_str(rng.pick(WORDS));
                if rng.one_in(6) {
                    s.push_str(rng.pick(BREAKERS));
                    s.push_str(rng.pick(WORDS));
                }
                if rng.one_in(9) {
                    s.push_str(rng.pick(NON_BREAKERS));
                }
            }
        }
        2 | 3 | 4 => {
            for _ in 0..n {
                s.push_str(atom(
                    rng,
                    &[LETTERS, WORDS, SPACES, SPACES, BREAKERS, NON_BREAKERS, MARKS, WIDE, RTL, ODD],
                ));
            }
        }
        5 => {
            for _ in 0..n {
                s.push_str(atom(rng, &[SPACES, SPACES, SPACES, LETTERS, BREAKERS, MARKS]));
            }
        }
        6 => {
            let len = rng.below(220);
            for _ in 0..len {
                s.push_str(atom(rng, &[LETTERS, LETTERS, LETTERS, MARKS, NON_BREAKERS]));
            }
        }
        7 => {
            for _ in 0..n {
                s.push_str(atom(rng, &[MARKS, MARKS, LETTERS, SPACES, ODD]));
            }
        }
        8 => {
            for _ in 0..n {
                s.push_str(atom(rng, &[WIDE, WIDE, RTL, SPACES, BREAKERS, MARKS]));
            }
        }
        _ => {
            let k = rng.below(12);
            for _ in 0..k {
                s.push_str(rng.pick(SPACES));
            }
        }
    }
    s
}

fn hostile_measure(rng: &mut Rng) -> usize {
    match rng.below(20) {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4..=13 => 1 + rng.below(24),
        14 | 15 => 40 + rng.below(80),
        16 => 1000,
        17 => usize::MAX,
        18 => usize::MAX - 1,
        _ => 4 + rng.below(6),
    }
}

fn hostile_hang(rng: &mut Rng, measure: usize) -> usize {
    match rng.below(12) {
        0..=3 => 0,
        4..=7 => rng.below(9),
        8 => measure.max(1) - 1,
        9 => measure,
        10 => measure.saturating_add(1),
        _ => usize::MAX,
    }
}

fn hostile_doc(rng: &mut Rng, max_lines: usize) -> Doc {
    let mut text = String::new();
    let lines = 1 + rng.below(max_lines);
    for i in 0..lines {
        if i > 0 {
            text.push_str(["\n", "\n", "\r\n", "\r"][rng.below(4)]);
        }
        match rng.below(8) {
            0 => {}
            1 => {
                text.push_str("- ");
                text.push_str(&hostile_line(rng));
            }
            2 => {
                text.push_str("    ");
                text.push_str(&hostile_line(rng));
            }
            _ => text.push_str(&hostile_line(rng)),
        }
    }
    Doc::from_text(&text)
}

/// A stand-in for markup::hang_of: list items and quotes hang under their
/// text, indented lines under their indent.
fn hang_list(line: &str) -> usize {
    let body = line.trim_start_matches(' ');
    let lead = line.len() - body.len();
    if body.starts_with("- ") || body.starts_with("> ") {
        lead + 2
    } else {
        lead
    }
}

/// A hang that goes to every extreme.
fn hang_wild(line: &str) -> usize {
    match line.len() % 6 {
        0 => 0,
        1 => usize::MAX,
        2 => line.len(),
        3 => 3,
        4 => 1,
        _ => str_width(line) / 2,
    }
}

fn hang_zero(_: &str) -> usize {
    0
}

/// The shape of a line that does nothing but hang: all a line could do
/// before shapes, and what every test older than section 7 asks for.
fn hanging<'a>(hang: &'a dyn Fn(&str) -> usize) -> impl Fn(usize, &str) -> Shape + 'a {
    move |_, line| Shape::hanging(hang(line))
}

fn r(start: usize, end: usize, indent: usize, cells: usize) -> Row {
    Row { start, end, indent, cells }
}

fn p(line: usize, col: usize) -> Pos {
    Pos { line, col }
}

fn texts<'a>(line: &'a str, rows: &[Row]) -> Vec<&'a str> {
    rows.iter().map(|row| &line[row.start..row.end]).collect()
}

// ───────────────────────────── 2. models ─────────────────────────────

/// A cluster: a character and the combining marks drawn over it.
///
/// OWNER'S READING: marks at the very start of a line, which have no base,
/// join the first base that follows (otherwise two cursor positions would
/// share a cell and the locate/pos_at round trip could not hold).
#[derive(Clone, Copy, Debug)]
struct Cl {
    start: usize,
    end: usize,
    width: usize,
    /// A space or tab, with or without marks drawn over it. OWNER'S READING
    /// (second version of wrap.rs): the base decides, so a space carrying a
    /// direction mark, and equally one carrying a visible accent, is
    /// whitespace: it gives a break opportunity, hangs, and is left out of
    /// `cells`. The contract does not say what a marked space is.
    ws: bool,
    /// OWNER'S READING: U+2010 is counted with the hyphen-minus.
    hyphen: bool,
}

fn clusters(line: &str) -> Vec<Cl> {
    let mut starts: Vec<usize> = Vec::new();
    let mut seen_base = false;
    for (i, ch) in line.char_indices() {
        let mark = is_combining(ch);
        if i == 0 || (!mark && seen_base) {
            starts.push(i);
        }
        seen_base |= !mark;
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(line.len());
        let text = &line[start..end];
        let base = text.chars().find(|&c| !is_combining(c));
        out.push(Cl {
            start,
            end,
            width: str_width(text),
            ws: matches!(base, Some(' ' | '\t')),
            hyphen: matches!(base, Some('-' | '/' | '\u{2013}' | '\u{2014}' | '\u{2010}')),
        });
    }
    out
}

/// The wrapping rules as a walk over unbreakable segments, which is not how
/// wrap.rs does it (one pass over clusters with a remembered opportunity).
///
/// A segment runs from one break opportunity to the next: some text, then
/// the whitespace that hangs after it. A segment goes on the current row if
/// its text fits; otherwise the row ends and the segment starts the next
/// one, where it is cut between clusters if it is too long even alone.
fn model_wrap(line: &str, measure: usize, hang: usize) -> Vec<Row> {
    let measure = measure.max(1);
    let hang = if hang >= measure { 0 } else { hang };
    let cs = clusters(line);
    let n = cs.len();
    if n == 0 {
        return vec![r(0, 0, 0, 0)];
    }
    // OWNER'S READING: leading whitespace is text in every respect: it takes
    // cells, never hangs, and no row may end just because it does.
    let lead = cs.iter().take_while(|c| c.ws).count();
    let hangs = |k: usize| k >= lead && cs[k].ws;
    let at = |k: usize| if k < n { cs[k].start } else { line.len() };

    let mut bounds = vec![0usize];
    for k in 1..n {
        if !hangs(k) && (hangs(k - 1) || cs[k - 1].hyphen) {
            bounds.push(k);
        }
    }
    bounds.push(n);

    let mut rows: Vec<Row> = Vec::new();
    let mut first = 0usize; // first cluster of the row being filled
    let mut indent = 0usize;
    let mut avail = measure;
    let mut x = 0usize; // cells used so far, hanging whitespace included
    let mut text = 0usize; // cells up to the last cluster that does not hang
    let mut placed = 0usize;

    for seg in bounds.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let mut t = b;
        while t > a && hangs(t - 1) {
            t -= 1;
        }
        let need: usize = cs[a..t].iter().map(|c| c.width).sum();
        if placed > 0 && x + need > avail {
            rows.push(r(at(first), at(a), indent, text));
            first = a;
            indent = hang;
            avail = measure - hang;
            x = 0;
            placed = 0;
        }
        // A segment that starts a row is cut between clusters wherever the
        // row is full; every cluster on the row is then text of its own.
        for k in a..t {
            if placed > 0 && x + cs[k].width > avail {
                rows.push(r(at(first), at(k), indent, x));
                first = k;
                indent = hang;
                avail = measure - hang;
                x = 0;
                placed = 0;
            }
            x += cs[k].width;
            placed += 1;
        }
        text = x;
        for c in &cs[t..b] {
            x += c.width;
            placed += 1;
        }
    }

    // OWNER'S READING: on the last row, whitespace ending the line counts
    // towards `cells` only while it fits (the contract says both "cells is
    // what the text occupies" and "indent + cells <= measure").
    let mut t = n;
    while t > first && hangs(t - 1) {
        t -= 1;
    }
    let mut cells = text;
    let mut reach = text;
    for c in &cs[t..n] {
        reach += c.width;
        if reach <= avail {
            cells = reach;
        }
    }
    rows.push(r(at(first), line.len(), indent, cells));
    rows
}

/// The same rules a third way, as a search: a row takes the whole rest of
/// the line if that fits, else it ends at the furthest opportunity whose text
/// fits, else it takes as many clusters as fit. Quadratic, so only for short
/// lines; it is here so that the segment model above is not the only thing
/// wrap.rs is compared with.
fn search_wrap(line: &str, measure: usize, hang: usize) -> Vec<Row> {
    let measure = measure.max(1);
    let hang = if hang >= measure { 0 } else { hang };
    let cs = clusters(line);
    let n = cs.len();
    if n == 0 {
        return vec![r(0, 0, 0, 0)];
    }
    let lead = cs.iter().take_while(|c| c.ws).count();
    let hangs = |k: usize| k >= lead && cs[k].ws;
    let at = |k: usize| if k < n { cs[k].start } else { line.len() };
    let width = |a: usize, b: usize| -> usize { cs[a..b].iter().map(|c| c.width).sum() };

    let mut rows: Vec<Row> = Vec::new();
    let mut i = 0usize;
    while i < n {
        let (indent, avail) = if rows.is_empty() { (0, measure) } else { (hang, measure - hang) };
        // Where the text of a row from i to k stops: before whatever hangs.
        let text_end = |k: usize| {
            let mut t = k;
            while t > i && hangs(t - 1) {
                t -= 1;
            }
            t
        };
        let may_end_at = |k: usize| k == n || (!hangs(k) && (hangs(k - 1) || cs[k - 1].hyphen));
        let fits = |k: usize| {
            let t = text_end(k);
            t - i == 1 || width(i, t) <= avail
        };
        let k = match (i + 1..=n).rev().find(|&k| may_end_at(k) && fits(k)) {
            Some(k) => k,
            None => {
                let mut k = i + 1;
                while k < n && width(i, k + 1) <= avail {
                    k += 1;
                }
                k
            }
        };
        let t = text_end(k);
        let mut cells = width(i, t);
        if k == n {
            let mut reach = cells;
            for c in &cs[t..n] {
                reach += c.width;
                if reach <= avail {
                    cells = reach;
                }
            }
        }
        rows.push(r(at(i), at(k), indent, cells));
        i = k;
    }
    rows
}

/// Everything the contract says about the rows of one line, checked without
/// reference to the models above.
fn check_rows(line: &str, measure: usize, hang: usize, rows: &[Row]) {
    let ctx = || format!("line {line:?} measure {measure} hang {hang} rows {rows:?}");
    let m = measure.max(1);
    let h = if hang >= m { 0 } else { hang };
    assert!(!rows.is_empty(), "no rows: {}", ctx());
    let cs = clusters(line);
    let n = cs.len();
    if n == 0 {
        assert_eq!(rows, [r(0, 0, 0, 0)], "{}", ctx());
        return;
    }
    let lead = cs.iter().take_while(|c| c.ws).count();
    let hangs = |k: usize| k < n && k >= lead && cs[k].ws;
    let opportunity = |k: usize| k > 0 && k < n && !hangs(k) && (hangs(k - 1) || cs[k - 1].hyphen);
    let index_of = |byte: usize| -> usize {
        if byte == line.len() {
            n
        } else {
            cs.binary_search_by_key(&byte, |c| c.start)
                .unwrap_or_else(|_| panic!("byte {byte} is not a cluster boundary: {}", ctx()))
        }
    };
    let width = |a: usize, b: usize| -> usize { cs[a..b].iter().map(|c| c.width).sum() };

    assert_eq!(rows[0].start, 0, "{}", ctx());
    assert_eq!(rows[rows.len() - 1].end, line.len(), "{}", ctx());
    for (i, row) in rows.iter().enumerate() {
        let last = i + 1 == rows.len();
        assert_eq!(row.indent, if i == 0 { 0 } else { h }, "indent of row {i}: {}", ctx());
        if i > 0 {
            assert_eq!(row.start, rows[i - 1].end, "gap before row {i}: {}", ctx());
        }
        assert!(row.start < row.end, "row {i} is empty: {}", ctx());
        let (a, b) = (index_of(row.start), index_of(row.end));
        let avail = m - row.indent;
        let mut t = b;
        while t > a && hangs(t - 1) {
            t -= 1;
        }
        assert!(t > a, "row {i} holds nothing but hanging whitespace: {}", ctx());
        let (text_w, all_w) = (width(a, t), width(a, b));
        assert_eq!(all_w, str_width(&line[row.start..row.end]), "{}", ctx());

        assert!(
            row.cells <= avail || (t - a == 1 && cs[a].width > avail),
            "row {i} is over the measure without being a single over-wide cluster: {}",
            ctx()
        );
        if last {
            let mut want = text_w;
            let mut reach = text_w;
            for c in &cs[t..b] {
                reach += c.width;
                if reach <= avail {
                    want = reach;
                }
            }
            assert_eq!(row.cells, want, "cells of the last row: {}", ctx());
        } else {
            assert_eq!(row.cells, text_w, "cells of row {i}: {}", ctx());
            assert!(!hangs(b), "row {} starts with hanging whitespace: {}", i + 1, ctx());
            if opportunity(b) {
                // Greedy: what follows, up to the next place a row could
                // end, did not fit on this row.
                let mut e = b + 1;
                while e < n && !hangs(e) && !opportunity(e) {
                    e += 1;
                }
                assert!(all_w + width(b, e) > avail, "row {i} ended before it was full: {}", ctx());
            } else {
                assert!(all_w + cs[b].width > avail, "row {i} was cut before it was full: {}", ctx());
                assert!(
                    !(a + 1..b).any(|k| opportunity(k)),
                    "row {i} was cut inside a word although it could have ended at an opportunity: {}",
                    ctx()
                );
            }
        }
    }
}

/// cell_of and col_at against a cell-by-cell walk of each row.
fn check_cells(line: &str, rows: &[Row]) {
    let cs = clusters(line);
    for (i, row) in rows.iter().enumerate() {
        let ctx = || format!("line {line:?} row {i} {row:?}");
        let inside: Vec<&Cl> = cs.iter().filter(|c| row.start <= c.start && c.start < row.end).collect();
        let mut off = 0usize;
        for c in &inside {
            // Every byte of the cluster, character boundary or not, is drawn
            // where the cluster starts.
            for byte in c.start..c.end {
                assert_eq!(cell_of(line, row, byte), row.indent.saturating_add(off), "cell_of {byte}: {}", ctx());
            }
            for d in 0..c.width {
                if let Some(cell) = row.indent.checked_add(off + d) {
                    assert_eq!(col_at(line, row, cell, true), c.start, "col_at {cell}: {}", ctx());
                    assert_eq!(col_at(line, row, cell, false), c.start, "col_at {cell}: {}", ctx());
                }
            }
            off += c.width;
        }
        let past = row.indent.saturating_add(off);
        assert_eq!(cell_of(line, row, row.end), past, "cell_of at the row's end: {}", ctx());
        assert_eq!(cell_of(line, row, row.end.saturating_add(1)), past, "{}", ctx());
        assert_eq!(cell_of(line, row, usize::MAX), past, "{}", ctx());
        assert_eq!(cell_of(line, row, 0), row.indent, "cell_of byte 0: {}", ctx());
        if row.start > 0 {
            assert_eq!(cell_of(line, row, row.start - 1), row.indent, "cell_of left of the row: {}", ctx());
        }
        if row.indent > 0 {
            for flag in [true, false] {
                assert_eq!(col_at(line, row, 0, flag), row.start, "col_at in the indent: {}", ctx());
                assert_eq!(col_at(line, row, row.indent - 1, flag), row.start, "{}", ctx());
            }
        }
        // A line of nothing but marks has no cells at all, so "past the
        // text" and "the cluster drawn over the cell" are the same cell
        // there; the contract does not say which wins.
        let no_cells = !inside.is_empty() && off == 0;
        if let (Some(first_past), false) = (row.indent.checked_add(off), no_cells) {
            let last_start = inside.last().map_or(row.start, |c| c.start);
            for cell in [first_past, first_past.saturating_add(1), first_past.saturating_add(999), usize::MAX] {
                assert_eq!(col_at(line, row, cell, true), row.end, "col_at past the text, last row: {}", ctx());
                assert_eq!(col_at(line, row, cell, false), last_start, "col_at past the text: {}", ctx());
            }
        }
    }
}

/// The whole-document queries, worked out from the model rows.
struct Model {
    rows: Vec<Vec<Row>>,
    cs: Vec<Vec<Cl>>,
    first: Vec<usize>,
    total: usize,
}

impl Model {
    fn new(doc: &Doc, measure: usize, hang: &dyn Fn(&str) -> usize) -> Model {
        let rows: Vec<Vec<Row>> = doc.lines().iter().map(|l| model_wrap(l, measure, hang(l))).collect();
        let cs: Vec<Vec<Cl>> = doc.lines().iter().map(|l| clusters(l)).collect();
        let mut first = Vec::with_capacity(rows.len());
        let mut total = 0;
        for line in &rows {
            first.push(total);
            total += line.len();
        }
        Model { rows, cs, first, total }
    }

    fn line_of_row(&self, row: usize) -> (usize, usize) {
        let row = row.min(self.total - 1);
        let line = self.first.partition_point(|&f| f <= row) - 1;
        (line, row - self.first[line])
    }

    /// Back to the start of the cluster `col` falls in; the line's end is a
    /// place of its own.
    fn snap(&self, line: usize, col: usize, len: usize) -> usize {
        if col >= len {
            return len;
        }
        let cs = &self.cs[line];
        let k = cs.partition_point(|c| c.start <= col) - 1;
        cs[k].start
    }

    fn row_of(&self, doc: &Doc, at: Pos) -> (Pos, usize) {
        let at = doc.clamp(at);
        let text = doc.line(at.line);
        let col = self.snap(at.line, at.col, text.len());
        let rows = &self.rows[at.line];
        let ri = rows.iter().position(|row| col < row.end).unwrap_or(rows.len() - 1);
        (Pos { line: at.line, col }, ri)
    }

    fn locate(&self, doc: &Doc, at: Pos) -> (usize, usize) {
        let (at, ri) = self.row_of(doc, at);
        let row = self.rows[at.line][ri];
        let text = doc.line(at.line);
        (self.first[at.line] + ri, row.indent.saturating_add(str_width(&text[row.start..at.col])))
    }

    fn pos_at(&self, doc: &Doc, row: usize, cell: usize) -> Pos {
        let (line, ri) = self.line_of_row(row);
        let rows = &self.rows[line];
        let row = rows[ri];
        let last = ri + 1 == rows.len();
        let Some(target) = cell.checked_sub(row.indent) else {
            return Pos { line, col: row.start };
        };
        let mut x = 0usize;
        let mut last_start = row.start;
        for c in self.cs[line].iter().filter(|c| row.start <= c.start && c.start < row.end) {
            // A line of nothing but marks answers for the cell it is drawn
            // at, or its start could not round-trip.
            if target < x + c.width || (c.width == 0 && target == x) {
                return Pos { line, col: c.start };
            }
            x += c.width;
            last_start = c.start;
        }
        Pos { line, col: if last { doc.line(line).len() } else { last_start } }
    }

    /// Where a cursor moved onto a global row lands when it wants `cell`:
    /// of all the places it can be drawn on that row, the nearest.
    ///
    /// THIRD ROUND. Until now `up` and `down` here went straight through
    /// `pos_at`, the owner's first reading, which sends a cursor three cells
    /// into a tab to the tab's far-off left edge. CONTRACT, of `up`: "one row
    /// up, landing as near as possible to the wanted cell". wrap.rs now does
    /// that, and this model with it.
    ///
    /// OWNER'S READING: of two places equally near, the one `pos_at` names.
    /// That is the left one, except on a line of nothing but marks, whose
    /// start and end are both drawn at cell 0 and whose end answers for
    /// every cell past 0.
    fn land(&self, doc: &Doc, global: usize, cell: usize) -> Pos {
        let under = self.pos_at(doc, global, cell);
        let stops = self.stops(doc, global);
        let far = |at: Pos| stops.iter().find(|stop| stop.0 == at).map(|stop| stop.1.abs_diff(cell));
        let nearest = stops.iter().min_by_key(|stop| stop.1.abs_diff(cell)).map_or(under, |stop| stop.0);
        if far(under) == far(nearest) {
            under
        } else {
            nearest
        }
    }

    fn up(&self, doc: &Doc, at: Pos, cell: usize) -> Pos {
        match self.locate(doc, at).0 {
            0 => Pos { line: 0, col: 0 },
            row => self.land(doc, row - 1, cell),
        }
    }

    fn down(&self, doc: &Doc, at: Pos, cell: usize) -> Pos {
        let row = self.locate(doc, at).0;
        if row + 1 >= self.total {
            doc.end()
        } else {
            self.land(doc, row + 1, cell)
        }
    }

    fn row_start(&self, doc: &Doc, at: Pos) -> Pos {
        let (at, ri) = self.row_of(doc, at);
        Pos { line: at.line, col: self.rows[at.line][ri].start }
    }

    /// OWNER'S READING: a row broken after a hyphen counts as broken inside
    /// a word, because "just after the last non-whitespace character" would
    /// be the row's end, which is drawn on the next row.
    fn row_end(&self, doc: &Doc, at: Pos) -> Pos {
        let (at, ri) = self.row_of(doc, at);
        let rows = &self.rows[at.line];
        let text = doc.line(at.line);
        if ri + 1 == rows.len() {
            return Pos { line: at.line, col: text.len() };
        }
        let row = rows[ri];
        let cs = &self.cs[at.line];
        let lead = cs.iter().take_while(|c| c.ws).count();
        let inside: Vec<(usize, &Cl)> =
            cs.iter().enumerate().filter(|(_, c)| row.start <= c.start && c.start < row.end).collect();
        let (last_k, last) = inside[inside.len() - 1];
        let col = if last.ws && last_k >= lead {
            inside.iter().rev().find(|(k, c)| !(c.ws && *k >= lead)).map(|(_, c)| c.end).unwrap()
        } else {
            last.start
        };
        Pos { line: at.line, col }
    }

    /// Every place a cursor can be drawn on a global row, with its cell.
    ///
    /// The cells are added up cluster by cluster. Measuring each one from
    /// the row's start gives the same numbers, but `land` asks this for
    /// every move and some rows here are a thousand clusters long.
    fn stops(&self, doc: &Doc, global: usize) -> Vec<(Pos, usize)> {
        let (line, ri) = self.line_of_row(global);
        let rows = &self.rows[line];
        let row = rows[ri];
        let cs = &self.cs[line];
        let (a, b) = (cs.partition_point(|c| c.start < row.start), cs.partition_point(|c| c.start < row.end));
        let mut cell = row.indent;
        let mut out: Vec<(Pos, usize)> = Vec::with_capacity(b - a + 1);
        for c in &cs[a..b] {
            out.push((Pos { line, col: c.start }, cell));
            cell = cell.saturating_add(c.width);
        }
        if ri + 1 == rows.len() {
            out.push((Pos { line, col: doc.line(line).len() }, cell));
        }
        out
    }
}

/// Two layouts compared through the public accessors only.
fn assert_same_layout(a: &Layout, b: &Layout, doc: &Doc, what: &str) {
    assert_eq!(a.measure(), b.measure(), "{what}: measure");
    assert_eq!(a.total_rows(), b.total_rows(), "{what}: total_rows");
    for line in 0..doc.line_count() + 2 {
        assert_eq!(a.rows(line), b.rows(line), "{what}: rows of line {line} ({:?})", doc.line(line));
        assert_eq!(a.first_row(line), b.first_row(line), "{what}: first_row of line {line}");
    }
    for row in 0..a.total_rows() + 2 {
        assert_eq!(a.line_of_row(row), b.line_of_row(row), "{what}: line_of_row {row}");
    }
}

/// Everything the contract promises about a layout and its document.
fn check_layout(doc: &Doc, layout: &Layout, measure: usize, hang: &dyn Fn(&str) -> usize, rng: &mut Rng) {
    let model = Model::new(doc, measure, hang);
    let total = model.total;
    let lines = doc.line_count();
    assert!(total >= 1);
    assert_eq!(layout.total_rows(), total, "total_rows");
    for past in [lines, lines + 1, lines + 1000, usize::MAX] {
        assert!(layout.rows(past).is_empty(), "rows past the end");
        assert_eq!(layout.first_row(past), total, "first_row past the end");
    }
    for line in 0..lines {
        let text = doc.line(line);
        let rows = layout.rows(line);
        assert_eq!(rows, &model.rows[line][..], "rows of line {line}: {text:?} at {measure}");
        assert_eq!(rows, &wrap(text, measure, hang(text))[..], "the layout's rows are wrap's rows: {text:?}");
        check_rows(text, measure, hang(text), rows);
        assert_eq!(layout.first_row(line), model.first[line], "first_row {line}");
        for ri in 0..rows.len() {
            assert_eq!(layout.line_of_row(model.first[line] + ri), (line, ri), "line_of_row");
        }
    }
    let last = model.line_of_row(total - 1);
    for past in [total, total + 1, total + 1000, usize::MAX] {
        assert_eq!(layout.line_of_row(past), last, "line_of_row past the end");
    }

    for line in 0..lines {
        let text = doc.line(line);
        let cs = &model.cs[line];
        let stride = 1 + cs.len() / 120;
        let skip = rng.below(stride);
        let mut stops: Vec<Pos> = cs.iter().skip(skip).step_by(stride).map(|c| p(line, c.start)).collect();
        if let Some(c) = cs.first() {
            stops.push(p(line, c.start));
        }
        if let Some(c) = cs.last() {
            stops.push(p(line, c.start));
        }
        for &at in &stops {
            let (row, cell) = layout.locate(doc, at);
            assert_eq!((row, cell), model.locate(doc, at), "locate {at:?} in {text:?} at {measure}");
            assert_eq!(layout.pos_at(doc, row, cell), at, "round trip of {at:?} in {text:?} at {measure}");
            check_moves(doc, layout, &model, at, rng);
        }
        let end = p(line, text.len());
        let (row, cell) = layout.locate(doc, end);
        assert_eq!((row, cell), model.locate(doc, end), "locate the end of {text:?} at {measure}");
        assert_eq!(row, model.first[line] + model.rows[line].len() - 1, "a line's end is on its last row");
        assert_eq!(layout.locate(doc, layout.pos_at(doc, row, cell)), (row, cell), "{text:?} at {measure}");
        check_moves(doc, layout, &model, end, rng);

        // Every byte, the inside of characters and clusters included, and
        // a little past the end.
        if text.len() < 400 {
            for col in 0..text.len() + 3 {
                let at = p(line, col);
                assert_eq!(layout.locate(doc, at), model.locate(doc, at), "locate {at:?} in {text:?} at {measure}");
            }
        }
    }

    for row in (0..total).step_by(1 + total / 150) {
        for cell in [0, 1, 2, rng.below(measure.clamp(1, 200) + 8), usize::MAX] {
            assert_eq!(layout.pos_at(doc, row, cell), model.pos_at(doc, row, cell), "pos_at({row}, {cell}) at {measure}");
        }
    }
}

fn check_moves(doc: &Doc, layout: &Layout, model: &Model, at: Pos, rng: &mut Rng) {
    let total = model.total;
    let (row, cell) = layout.locate(doc, at);
    let ctx = || format!("{at:?} (row {row}, cell {cell}) in {:?}", doc.line(at.line));
    for want in [cell, 0, rng.below(40), usize::MAX] {
        let below = layout.down(doc, at, want);
        assert_eq!(below, model.down(doc, at, want), "down to cell {want} from {}", ctx());
        if row + 1 < total {
            assert_eq!(layout.locate(doc, below).0, row + 1, "down lands one row down from {}", ctx());
            let back = layout.up(doc, below, want);
            assert_eq!(layout.locate(doc, back).0, row, "down then up returns to the row of {}", ctx());
        } else {
            assert_eq!(below, doc.end(), "down from the last row is the document end");
        }
        let above = layout.up(doc, at, want);
        assert_eq!(above, model.up(doc, at, want), "up to cell {want} from {}", ctx());
        if row > 0 {
            assert_eq!(layout.locate(doc, above).0, row - 1, "up lands one row up from {}", ctx());
            let back = layout.down(doc, above, want);
            assert_eq!(layout.locate(doc, back).0, row, "up then down returns to the row of {}", ctx());
        } else {
            assert_eq!(above, p(0, 0), "up from the first row is the document start");
        }
    }
    let home = layout.row_start(doc, at);
    assert_eq!(home, model.row_start(doc, at), "row_start of {}", ctx());
    assert_eq!(layout.locate(doc, home).0, row, "Home stays on the row of {}", ctx());
    let end = layout.row_end(doc, at);
    assert_eq!(end, model.row_end(doc, at), "row_end of {}", ctx());
    assert_eq!(layout.locate(doc, end).0, row, "End stays on the row of {}", ctx());
    assert!(home <= end, "{}", ctx());
}

// ───────────────────────────── 3. properties ─────────────────────────────

#[test]
fn wrap_matches_the_segment_model_on_hostile_lines() {
    let mut rng = Rng::new(1);
    for _ in 0..20_000 * scale() {
        let line = hostile_line(&mut rng);
        let measure = hostile_measure(&mut rng);
        let hang = hostile_hang(&mut rng, measure);
        assert_eq!(
            wrap(&line, measure, hang),
            model_wrap(&line, measure, hang),
            "line {line:?} measure {measure} hang {hang}"
        );
    }
}

#[test]
fn wrap_keeps_every_invariant_on_hostile_lines() {
    let mut rng = Rng::new(2);
    for _ in 0..20_000 * scale() {
        let line = hostile_line(&mut rng);
        let measure = hostile_measure(&mut rng);
        let hang = hostile_hang(&mut rng, measure);
        let rows = wrap(&line, measure, hang);
        check_rows(&line, measure, hang, &rows);
    }
}

/// Every string up to six characters over a small alphabet that has one of
/// everything the rules care about.
#[test]
fn wrap_is_right_for_every_short_line_over_a_small_alphabet() {
    let alphabet = ['a', ' ', '-', '\t', '\u{4E00}', '\u{0301}'];
    let mut line = String::new();
    for len in 0..=6usize {
        let count = alphabet.len().pow(len as u32);
        for mut code in 0..count {
            line.clear();
            for _ in 0..len {
                line.push(alphabet[code % alphabet.len()]);
                code /= alphabet.len();
            }
            for (measure, hang) in [(1, 0), (2, 0), (2, 1), (3, 0), (3, 2), (4, 1), (5, 0), (5, 3), (7, 2)] {
                let rows = wrap(&line, measure, hang);
                assert_eq!(rows, model_wrap(&line, measure, hang), "line {line:?} measure {measure} hang {hang}");
                assert_eq!(rows, search_wrap(&line, measure, hang), "line {line:?} measure {measure} hang {hang}");
                check_rows(&line, measure, hang, &rows);
            }
        }
    }
}

/// The same, over an alphabet built around the clusters the second version
/// of wrap.rs treats differently: a space under a visible accent, a space
/// before a direction mark, a marked tab, and a mark with no base before it.
#[test]
fn wrap_is_right_for_every_short_line_of_marked_whitespace() {
    let alphabet = ["a", " ", " \u{0301}", " \u{200F}", "\t\u{0313}", "-", "\u{0301}", "\u{4E00}"];
    let mut line = String::new();
    for len in 0..=5usize {
        let count = alphabet.len().pow(len as u32);
        for mut code in 0..count {
            line.clear();
            for _ in 0..len {
                line.push_str(alphabet[code % alphabet.len()]);
                code /= alphabet.len();
            }
            for (measure, hang) in [(1, 0), (2, 0), (2, 1), (3, 0), (3, 2), (4, 1), (5, 0), (6, 3)] {
                let rows = wrap(&line, measure, hang);
                assert_eq!(rows, model_wrap(&line, measure, hang), "line {line:?} measure {measure} hang {hang}");
                assert_eq!(rows, search_wrap(&line, measure, hang), "line {line:?} measure {measure} hang {hang}");
                check_rows(&line, measure, hang, &rows);
                check_cells(&line, &rows);
            }
        }
    }
}

#[test]
fn wrap_matches_the_search_model_on_short_hostile_lines() {
    let mut rng = Rng::new(5);
    let mut done = 0;
    while done < 12_000 * scale() {
        let line = hostile_line(&mut rng);
        if line.len() > 300 {
            continue;
        }
        done += 1;
        let measure = hostile_measure(&mut rng);
        let hang = hostile_hang(&mut rng, measure);
        assert_eq!(
            wrap(&line, measure, hang),
            search_wrap(&line, measure, hang),
            "line {line:?} measure {measure} hang {hang}"
        );
    }
}

#[test]
fn cell_of_and_col_at_agree_with_a_cell_by_cell_walk() {
    let mut rng = Rng::new(3);
    for _ in 0..6_000 * scale() {
        let line = hostile_line(&mut rng);
        let measure = hostile_measure(&mut rng);
        let hang = hostile_hang(&mut rng, measure);
        let rows = wrap(&line, measure, hang);
        check_cells(&line, &rows);
    }
}

#[test]
fn cell_of_and_col_at_for_every_short_line() {
    let alphabet = ['a', ' ', '-', '\t', '\u{4E00}', '\u{0301}', '\u{00E9}'];
    let mut line = String::new();
    for len in 0..=4usize {
        let count = alphabet.len().pow(len as u32);
        for mut code in 0..count {
            line.clear();
            for _ in 0..len {
                line.push(alphabet[code % alphabet.len()]);
                code /= alphabet.len();
            }
            for (measure, hang) in [(1, 0), (2, 1), (3, 0), (4, 2), (9, 4)] {
                check_cells(&line, &wrap(&line, measure, hang));
            }
        }
    }
}

/// Stale or made-up rows are outside the contract, but the two free
/// functions are public and take any `Row`, so at least they must not panic
/// and must hand back a byte offset the line really has.
#[test]
fn cell_of_and_col_at_survive_rows_that_belong_to_no_line() {
    let mut rng = Rng::new(4);
    let wild = |rng: &mut Rng| match rng.below(5) {
        0 => usize::MAX - rng.below(3),
        1 => 0,
        _ => rng.below(60),
    };
    for _ in 0..20_000 * scale() {
        let line = hostile_line(&mut rng);
        let row = r(wild(&mut rng), wild(&mut rng), wild(&mut rng), wild(&mut rng));
        let (col, cell) = (wild(&mut rng), wild(&mut rng));
        cell_of(&line, &row, col);
        for flag in [true, false] {
            let got = col_at(&line, &row, cell, flag);
            assert!(line.is_char_boundary(got), "col_at gave {got} in {line:?} for {row:?}");
        }
    }
}

fn run_edits_once(seed: u64, measure: usize, hang: &dyn Fn(&str) -> usize, steps: usize) {
    const PIECES: &[&str] = &[
        "",
        "a",
        " ",
        "  ",
        "\t",
        "-",
        "word",
        "two words",
        "\n",
        "\n\n",
        "\r\n",
        "\r",
        "x\ny",
        "\n- item ",
        "\n    indented",
        "\n> quote ",
        "\u{0301}",
        "e\u{0301}",
        "\u{4E00}",
        "\u{1F468}\u{200D}\u{1F469}",
        "\u{05E9}\u{05DC}\u{05D5}\u{05DD} ",
        "\u{0130}",
        "well-known",
        "either/or ",
        "the quick brown fox jumps over the lazy dog ",
        "antidisestablishmentarianism",
        " \u{200F}",
        "\u{200B}",
    ];
    let mut rng = Rng::new(seed);
    let mut doc = Doc::new();
    let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let recording = |line: &str| {
        asked.borrow_mut().push(line.to_string());
        hang(line)
    };
    let mut layout = Layout::new(&doc, measure, &hanging(&recording));
    let mut undo: Vec<Change> = Vec::new();

    let wild_pos = |rng: &mut Rng, doc: &Doc| -> Pos {
        let line = if rng.one_in(40) { usize::MAX } else { rng.below(doc.line_count() + 1) };
        let col = if rng.one_in(40) { usize::MAX } else { rng.below(doc.line(line).len() + 3) };
        p(line, col)
    };

    for step in 0..steps {
        let change = if !undo.is_empty() && rng.one_in(9) {
            let last = undo.pop().unwrap();
            doc.revert(&last)
        } else {
            let a = wild_pos(&mut rng, &doc);
            let b = match rng.below(8) {
                0..=2 => a,
                3..=5 => p(a.line, a.col.saturating_add(rng.below(7))),
                6 => p(a.line.saturating_add(rng.below(4)), rng.below(12)),
                _ => wild_pos(&mut rng, &doc),
            };
            // Keep the document from growing without bound, with a large
            // deletion that is itself an edit to keep in step with.
            let (a, b) = if doc.len_bytes() > 5000 { (p(rng.below(3), 0), p(doc.line_count() / 2, 0)) } else { (a, b) };
            let mut text = String::new();
            for _ in 0..rng.below(3) {
                if rng.one_in(6) {
                    text.push_str(&hostile_line(&mut rng));
                } else {
                    text.push_str(rng.pick(PIECES));
                }
            }
            let change = doc.replace(Span::new(a, b), &text);
            undo.push(change.clone());
            change
        };

        asked.borrow_mut().clear();
        layout.apply(&doc, &change, &hanging(&recording));
        let (first, _, new) = change.lines();
        {
            let asked = asked.borrow();
            let touched: Vec<&str> = (first..first + new).map(|i| doc.line(i)).collect();
            assert!(
                asked.len() <= new,
                "step {step}: apply wrapped {} lines for a change that touched {new}",
                asked.len()
            );
            for text in asked.iter() {
                assert!(
                    touched.contains(&text.as_str()),
                    "step {step}: apply asked for the hang of {text:?}, which the change did not touch"
                );
            }
        }
        let fresh = Layout::new(&doc, measure, &hanging(hang));
        assert_same_layout(&layout, &fresh, &doc, &format!("seed {seed} step {step} measure {measure}"));
        assert!(layout == fresh, "seed {seed} step {step} measure {measure}: apply differs from a fresh layout");
        if step % 97 == 96 {
            check_layout(&doc, &layout, measure, hang, &mut rng);
        }
    }
    check_layout(&doc, &layout, measure, hang, &mut rng);

    let change = doc.replace(Span::new(p(0, 0), doc.end()), "");
    layout.apply(&doc, &change, &hanging(hang));
    assert_same_layout(&layout, &Layout::new(&Doc::new(), measure, &hanging(hang)), &doc, "emptied");
}

fn run_edits(seed: u64, measure: usize, hang: &dyn Fn(&str) -> usize, steps: usize) {
    for round in 0..scale() as u64 {
        run_edits_once(seed + 1000 * round, measure, hang, steps);
    }
}

#[test]
fn a_layout_after_apply_equals_one_built_fresh_narrow() {
    run_edits(11, 1, &hang_list, 500);
    run_edits(12, 2, &hang_wild, 500);
    run_edits(13, 3, &hang_list, 500);
}

#[test]
fn a_layout_after_apply_equals_one_built_fresh_medium() {
    run_edits(21, 7, &hang_list, 700);
    run_edits(22, 12, &hang_wild, 700);
    run_edits(23, 24, &hang_zero, 700);
}

#[test]
fn a_layout_after_apply_equals_one_built_fresh_wide_and_extreme() {
    run_edits(31, 72, &hang_list, 700);
    run_edits(32, 0, &hang_wild, 300);
    run_edits(33, usize::MAX, &hang_wild, 400);
    run_edits(34, 113, &hang_zero, 500);
}

/// Every document of up to four characters over an alphabet that has a
/// line break, a list marker and one of each kind of cluster.
#[test]
fn every_tiny_document_keeps_every_layout_promise() {
    let alphabet = ['a', ' ', '-', '\n', '\t', '\u{4E00}', '\u{0301}'];
    let mut rng = Rng::new(39);
    let mut text = String::new();
    for len in 0..=4usize {
        let count = alphabet.len().pow(len as u32);
        for mut code in 0..count {
            text.clear();
            for _ in 0..len {
                text.push(alphabet[code % alphabet.len()]);
                code /= alphabet.len();
            }
            let doc = Doc::from_text(&text);
            for measure in [1, 2, 3, 5] {
                let layout = Layout::new(&doc, measure, &hanging(&hang_list));
                check_layout(&doc, &layout, measure, &hang_list, &mut rng);
            }
        }
    }
}

#[test]
fn hostile_documents_keep_every_layout_promise() {
    let mut rng = Rng::new(40);
    for round in 0..260 * scale() {
        let doc = hostile_doc(&mut rng, 14);
        let measure = match round % 5 {
            0 => hostile_measure(&mut rng),
            _ => 1 + rng.below(30),
        };
        let hang: &dyn Fn(&str) -> usize = match rng.below(3) {
            0 => &hang_zero,
            1 => &hang_list,
            _ => &hang_wild,
        };
        let layout = Layout::new(&doc, measure, &hanging(hang));
        if measure > 0 {
            assert_eq!(layout.measure(), measure);
        }
        check_layout(&doc, &layout, measure, hang, &mut rng);
    }
}

/// "landing as near as possible to the wanted cell", checked for every row.
///
/// THIRD ROUND. This used to pass over every row with a tab on it, the one
/// cluster wide enough for the answer to matter, because wrap.rs sent the
/// cursor to the tab's left edge from anywhere inside it. It no longer does
/// (see `contract_up_and_down_land_on_the_nearer_edge_of_a_tab`), so the
/// rows with tabs are checked like the rest.
#[test]
fn up_and_down_land_as_near_as_possible() {
    let mut rng = Rng::new(41);
    for _ in 0..200 * scale() {
        let doc = hostile_doc(&mut rng, 10);
        let measure = 1 + rng.below(24);
        let layout = Layout::new(&doc, measure, &hanging(&hang_list));
        let model = Model::new(&doc, measure, &hang_list);
        for row in 0..model.total {
            let from = model.stops(&doc, row)[0].0;
            for (target, to_down) in [(row.wrapping_sub(1), false), (row + 1, true)] {
                if target >= model.total {
                    continue;
                }
                let stops = model.stops(&doc, target);
                let (line, _) = model.line_of_row(target);
                // Each stop is drawn where the text before it on the row ends.
                let (first, indent) = stops[0];
                for &(at, cell) in &stops {
                    assert_eq!(cell, indent + str_width(&doc.line(line)[first.col..at.col]), "cell of the stop {at:?}");
                }
                for want in 0..measure + 6 {
                    let got = if to_down { layout.down(&doc, from, want) } else { layout.up(&doc, from, want) };
                    let best = stops.iter().map(|&(_, cell)| cell.abs_diff(want)).min().unwrap();
                    let (_, got_cell) = *stops
                        .iter()
                        .find(|&&(at, _)| at == got)
                        .unwrap_or_else(|| panic!("{got:?} is not a cursor stop of row {target}"));
                    assert_eq!(
                        got_cell.abs_diff(want),
                        best,
                        "wanted cell {want} on row {target} ({:?}), got {got:?} at cell {got_cell}; stops {stops:?}",
                        doc.line(line)
                    );
                }
            }
        }
    }
}

/// Positions out of range and inside multi-byte characters behave as their
/// clamped selves, and every answer is a real position.
#[test]
fn wild_positions_behave_as_their_clamped_selves() {
    let mut rng = Rng::new(42);
    let wild = |rng: &mut Rng, near: usize| match rng.below(6) {
        0 => usize::MAX,
        1 => usize::MAX - 1,
        2 => near + rng.below(4),
        _ => rng.below(near + 2),
    };
    for _ in 0..400 * scale() {
        let doc = hostile_doc(&mut rng, 8);
        let measure = hostile_measure(&mut rng);
        let layout = Layout::new(&doc, measure, &hanging(&hang_wild));
        for _ in 0..60 {
            let line = wild(&mut rng, doc.line_count());
            let at = p(line, wild(&mut rng, doc.line(line).len()));
            let tame = doc.clamp(at);
            let cell = wild(&mut rng, 30);
            let row = wild(&mut rng, layout.total_rows());
            assert_eq!(layout.locate(&doc, at), layout.locate(&doc, tame), "locate {at:?}");
            assert!(layout.locate(&doc, at).0 < layout.total_rows());
            let answers = [
                (layout.up(&doc, at, cell), layout.up(&doc, tame, cell)),
                (layout.down(&doc, at, cell), layout.down(&doc, tame, cell)),
                (layout.row_start(&doc, at), layout.row_start(&doc, tame)),
                (layout.row_end(&doc, at), layout.row_end(&doc, tame)),
                (layout.pos_at(&doc, row, cell), layout.pos_at(&doc, row.min(layout.total_rows() - 1), cell)),
            ];
            for (got, want) in answers {
                assert_eq!(got, want, "asked at {at:?}, row {row}, cell {cell}");
                assert_eq!(doc.clamp(got), got, "{got:?} is not a position of the document");
            }
        }
    }
}

/// Outside the contract, which says nothing about a layout asked about a
/// document it was not made for. But the interface will hold a layout and a
/// document side by side for the life of the program, and a slip there (one
/// missed `apply`) should cost a wrongly placed cursor and not the session:
/// so no panic, and every answer a position the document really has.
#[test]
fn a_layout_asked_about_another_document_answers_with_real_positions() {
    let mut rng = Rng::new(43);
    let wild = |rng: &mut Rng, near: usize| match rng.below(6) {
        0 => usize::MAX,
        1 => usize::MAX - 1,
        2 => near + rng.below(4),
        _ => rng.below(near + 2),
    };
    for _ in 0..300 * scale() {
        let made_for = hostile_doc(&mut rng, 8);
        let mut asked = hostile_doc(&mut rng, 8);
        let measure = hostile_measure(&mut rng);
        let mut layout = Layout::new(&made_for, measure, &hanging(&hang_wild));
        for round in 0..3 {
            for _ in 0..30 {
                let line = wild(&mut rng, asked.line_count());
                let at = p(line, wild(&mut rng, asked.line(line).len()));
                let cell = wild(&mut rng, 30);
                let row = wild(&mut rng, layout.total_rows());
                let (found, _) = layout.locate(&asked, at);
                assert!(found < layout.total_rows());
                layout.line_of_row(row);
                layout.first_row(row);
                let answers = [
                    layout.pos_at(&asked, row, cell),
                    layout.up(&asked, at, cell),
                    layout.down(&asked, at, cell),
                    layout.row_start(&asked, at),
                    layout.row_end(&asked, at),
                ];
                for got in answers {
                    assert_eq!(asked.clamp(got), got, "{got:?} is not a position of the document asked about");
                }
                // The rows of the wrong line, handed to the free functions.
                let text = asked.line(rng.below(asked.line_count()));
                for stale in layout.rows(rng.below(made_for.line_count())) {
                    cell_of(text, stale, cell);
                    for flag in [true, false] {
                        assert!(text.is_char_boundary(col_at(text, stale, cell, flag)));
                    }
                }
            }
            // An edit to the other document, applied to this layout.
            let a = p(wild(&mut rng, asked.line_count()), wild(&mut rng, 12));
            let b = p(wild(&mut rng, asked.line_count()), wild(&mut rng, 12));
            let text = hostile_line(&mut rng);
            let change = asked.replace(Span::new(a, b), if round == 1 { "\n\n" } else { &text });
            layout.apply(&asked, &change, &hanging(&hang_wild));
        }
    }
}

/// U+0130 lower-cases to three bytes from two; nothing here may assume a
/// character keeps its length, and wrapping must not care about case at all.
#[test]
fn dotted_capital_i_is_one_ordinary_cell() {
    let line = "\u{0130}stanbul \u{0130}\u{0130}\u{0130} i\u{0307}i\u{0307}";
    for measure in 1..12 {
        for hang in 0..3 {
            let rows = wrap(line, measure, hang);
            assert_eq!(rows, model_wrap(line, measure, hang));
            check_rows(line, measure, hang, &rows);
            check_cells(line, &rows);
        }
    }
    assert_eq!(wrap("\u{0130}\u{0130}\u{0130}", 2, 0), [r(0, 4, 0, 2), r(4, 6, 0, 1)]);
}

// ── speed ──

fn seconds(mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

/// Times `small` and `big` and asks that `big` take no more than `factor`
/// times as long (plus a little, for runs too short to time). Other work on
/// the machine can make one measurement lie, so a bad one is taken again
/// before it is believed.
fn assert_scales(what: &str, factor: f64, mut small: impl FnMut(), mut big: impl FnMut()) {
    let mut seen = Vec::new();
    for _ in 0..4 {
        let (a, b) = (seconds(&mut small), seconds(&mut big));
        if b < a * factor + 0.08 {
            return;
        }
        seen.push((a, b));
    }
    panic!("{what}: the larger input is more than {factor} times slower, four times running: {seen:?}");
}

#[test]
fn wrap_is_linear_in_the_length_of_the_line() {
    let patterns: &[(&str, &str, &str)] = &[
        ("one long word", "", "a"),
        ("short words", "", "ab "),
        ("leading spaces only", "", " "),
        ("a word then spaces", "x", " "),
        ("a word then tabs", "x", "\t"),
        ("hyphens", "", "-"),
        ("hyphenated", "", "a-"),
        ("slashes and spaces", "", "a/ "),
        ("marks only", "", "\u{0301}"),
        ("one base under many marks", "a", "\u{0301}"),
        ("marked letters", "", "e\u{0301}\u{0308}"),
        ("wide", "", "\u{4E00}"),
        ("joined emoji", "", "\u{1F468}\u{200D}"),
        ("marked spaces", "", "a \u{0301}"),
        ("words longer than a hanging row", "", "abcdefgh "),
        ("words held apart by a space and a direction mark", "", "ab \u{200F}"),
        ("leading spaces under marks", "", " \u{0301}"),
        ("a hyphen then a marked tab", "", "a-\t\u{0313}"),
        ("a mark with no base, then spaces", "\u{0301}", " "),
    ];
    for &(what, head, unit) in patterns {
        for (measure, hang) in [(1, 0), (10, 9), (72, 4), (usize::MAX, 0)] {
            let make = |n: usize| format!("{head}{}", unit.repeat(n));
            let (small, big) = (make(15_000), make(120_000));
            // Eight times the text: a linear pass takes eight times as long,
            // a quadratic one sixty-four times.
            assert_scales(
                &format!("wrap of {what} at measure {measure}, hang {hang}"),
                30.0,
                || drop(wrap(&small, measure, hang)),
                || drop(wrap(&big, measure, hang)),
            );
        }
    }
}

/// The per-keystroke queries on a very long paragraph cost what a row costs,
/// not what the paragraph costs.
#[test]
fn queries_do_not_grow_with_the_length_of_the_paragraph() {
    let build = |words: usize| {
        let doc = Doc::from_text(&"lorem ipsum dolor-sit e\u{0301}t \u{4E00}\u{4E8C} ".repeat(words));
        let layout = Layout::new(&doc, 60, &hanging(&hang_zero));
        (doc, layout)
    };
    let queries = |doc: &Doc, layout: &Layout| {
        let len = doc.line(0).len();
        let mut rng = Rng::new(7);
        for _ in 0..4000 {
            let at = p(0, rng.below(len + 1));
            let (row, cell) = layout.locate(doc, at);
            layout.pos_at(doc, row, cell);
            layout.up(doc, at, cell);
            layout.down(doc, at, cell);
            layout.row_start(doc, at);
            layout.row_end(doc, at);
        }
    };
    let (small, big) = (build(2_000), build(32_000));
    assert_scales(
        "4000 cursor queries on a paragraph sixteen times as long",
        4.0,
        || queries(&small.0, &small.1),
        || queries(&big.0, &big.1),
    );
}

#[test]
fn the_row_index_is_logarithmic_or_better() {
    let build = |lines: usize| {
        let doc = Doc::from_text(&"one two three four five six\n\nseven\n".repeat(lines / 3));
        Layout::new(&doc, 9, &hanging(&hang_zero))
    };
    let lookups = |layout: &Layout| {
        let total = layout.total_rows();
        let mut rng = Rng::new(8);
        let mut sum = 0usize;
        for _ in 0..60_000 {
            let (line, _) = layout.line_of_row(rng.below(total));
            sum = sum.wrapping_add(layout.first_row(line));
        }
        assert!(sum > 0);
    };
    let (small, big) = (build(3_000), build(96_000));
    assert_scales("60000 index lookups with 32 times the lines", 4.0, || lookups(&small), || lookups(&big));
}

/// Typing inside one line of a large document: only that line is wrapped
/// again, whatever happens to the number of rows.
#[test]
fn apply_wraps_nothing_but_the_touched_lines_in_a_large_document() {
    let text: String = (0..3000).map(|i| format!("line number {i} with a few words in it\n")).collect();
    let mut doc = Doc::from_text(&text);
    let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let recording = |line: &str| {
        asked.borrow_mut().push(line.to_string());
        0
    };
    let mut layout = Layout::new(&doc, 20, &hanging(&recording));
    assert_eq!(asked.borrow().len(), doc.line_count());

    let edits: Vec<(Span, &str)> = vec![
        (Span::at(p(1500, 4)), "x"),                                  // rows unchanged
        (Span::at(p(1500, 4)), " and many more words than before "), // rows change
        (Span::at(p(0, 0)), "first "),
        (Span::at(p(2999, 5)), "\n"),                       // a line more
        (Span::new(p(10, 3), p(13, 2)), ""),                // three lines fewer
        (Span::new(p(40, 0), p(41, 0)), "a\nb\nc\nd\ne"),   // several lines for one
        (Span::at(p(usize::MAX, usize::MAX)), "the end"),
    ];
    for (span, text) in edits {
        asked.borrow_mut().clear();
        let change = doc.replace(span, text);
        layout.apply(&doc, &change, &hanging(&recording));
        let (first, _, new) = change.lines();
        let touched: Vec<&str> = (first..first + new).map(|i| doc.line(i)).collect();
        let asked = asked.borrow();
        assert!(asked.len() <= new, "apply wrapped {} lines for a change that touched {new}", asked.len());
        for text in asked.iter() {
            assert!(touched.contains(&text.as_str()), "apply wrapped the untouched line {text:?}");
        }
        assert_same_layout(&layout, &Layout::new(&doc, 20, &hanging(&hang_zero)), &doc, "large document");
    }
}

// ───────────────────── 4. the contract, sentence by sentence ─────────────────────

#[test]
fn api_has_the_signatures_the_contract_lists() {
    fn row_derives<T: Clone + Copy + PartialEq + Eq + std::fmt::Debug + Default>() {}
    row_derives::<Row>();
    let _: fn(&str, usize, usize) -> Vec<Row> = wrap;
    let _: fn(&str, &Row, usize) -> usize = cell_of;
    let _: fn(&str, &Row, usize, bool) -> usize = col_at;
    let _: fn(&Doc, usize, &dyn Fn(usize, &str) -> Shape) -> Layout = Layout::new;
    let _: fn(&Layout) -> usize = Layout::measure;
    let _: fn(&mut Layout, &Doc, usize, &dyn Fn(usize, &str) -> Shape) = Layout::rewrap;
    let _: fn(&mut Layout, &Doc, &Change, &dyn Fn(usize, &str) -> Shape) = Layout::apply;
    let _: fn(&Layout, usize) -> Shape = Layout::shape;
    let _: fn(usize) -> Shape = Shape::hanging;
    assert_eq!(Shape::default(), Shape { hung: 0, scale: 1, hang: 0, inset: 0 });
    assert_eq!(Shape::hanging(3), Shape { hung: 0, scale: 1, hang: 3, inset: 0 });
    let _: fn(&Layout) -> usize = Layout::total_rows;
    let _: fn(&Layout, usize) -> &[Row] = Layout::rows;
    let _: fn(&Layout, usize) -> usize = Layout::first_row;
    let _: fn(&Layout, usize) -> (usize, usize) = Layout::line_of_row;
    let _: fn(&Layout, &Doc, Pos) -> (usize, usize) = Layout::locate;
    let _: fn(&Layout, &Doc, usize, usize) -> Pos = Layout::pos_at;
    let _: fn(&Layout, &Doc, Pos, usize) -> Pos = Layout::up;
    let _: fn(&Layout, &Doc, Pos, usize) -> Pos = Layout::down;
    let _: fn(&Layout, &Doc, Pos) -> Pos = Layout::row_start;
    let _: fn(&Layout, &Doc, Pos) -> Pos = Layout::row_end;
    let row = Row { start: 1, end: 2, indent: 3, cells: 4 };
    assert_eq!((row.start, row.end, row.indent, row.cells), (1, 2, 3, 4));
    assert_eq!(Row::default(), r(0, 0, 0, 0));
}

#[test]
fn contract_an_empty_line_is_one_row_of_zeros() {
    for measure in [0, 1, 7, usize::MAX] {
        for hang in [0, 1, 6, 7, 8, usize::MAX] {
            assert_eq!(wrap("", measure, hang), [r(0, 0, 0, 0)], "measure {measure} hang {hang}");
        }
    }
}

#[test]
fn contract_wrap_never_returns_an_empty_vec() {
    for line in ["", " ", "\t", "\u{0301}", "a", "\u{4E00}", "   ", "\u{200D}\u{200D}"] {
        for measure in 0..5 {
            for hang in 0..5 {
                assert!(!wrap(line, measure, hang).is_empty(), "{line:?} {measure} {hang}");
            }
        }
    }
}

#[test]
fn contract_a_measure_of_zero_is_a_measure_of_one() {
    for line in ["", "abc", "a b  c", "\u{4E00}\u{4E8C}", "e\u{0301}e\u{0301}", "\tx", "a-b/c", "  lead"] {
        for hang in [0, 1, 5, usize::MAX] {
            assert_eq!(wrap(line, 0, hang), wrap(line, 1, hang), "{line:?} hang {hang}");
        }
    }
    assert_eq!(texts("abc", &wrap("abc", 0, 0)), ["a", "b", "c"]);
}

#[test]
fn contract_wrap_is_greedy() {
    // Each row takes every word that fits before the next one starts.
    let line = "aa bb cc dd ee";
    assert_eq!(texts(line, &wrap(line, 5, 0)), ["aa bb ", "cc dd ", "ee"]);
    assert_eq!(texts(line, &wrap(line, 8, 0)), ["aa bb cc ", "dd ee"]);
    assert_eq!(texts(line, &wrap(line, 14, 0)), ["aa bb cc dd ee"]);
    assert_eq!(texts(line, &wrap(line, 13, 0)), ["aa bb cc dd ", "ee"]);
    // A word that fits exactly is kept, and its space hangs.
    let line = "abcd efgh";
    assert_eq!(wrap(line, 4, 0), [r(0, 5, 0, 4), r(5, 9, 0, 4)]);
}

#[test]
fn contract_rows_are_contiguous_and_cover_the_line_exactly() {
    let line = "The rows of a line are contiguous\u{2014}and cover it/exactly, e\u{0301}ven \u{4E00}\u{4E8C}\u{4E09} so.   ";
    for measure in 0..40 {
        for hang in 0..6 {
            let rows = wrap(line, measure, hang);
            assert_eq!(rows[0].start, 0);
            assert_eq!(rows[rows.len() - 1].end, line.len());
            for pair in rows.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "measure {measure} hang {hang}");
            }
            assert_eq!(texts(line, &rows).concat(), line);
        }
    }
}

#[test]
fn contract_indent_is_zero_on_the_first_row_and_the_hang_on_the_rest() {
    let line = "one two three four five six seven";
    let rows = wrap(line, 12, 4);
    assert!(rows.len() >= 3);
    assert_eq!(rows[0].indent, 0);
    assert!(rows[1..].iter().all(|row| row.indent == 4));
    // Continuation rows may use measure - hang cells for text.
    assert!(rows[1..].iter().all(|row| row.cells <= 8));
    assert_eq!(wrap("aaaa bbbb cc dd", 8, 4), [r(0, 5, 0, 4), r(5, 10, 4, 4), r(10, 13, 4, 2), r(13, 15, 4, 2)]);
    assert_eq!(wrap("aaaa bbbb cc dd", 9, 4), [r(0, 10, 0, 9), r(10, 15, 4, 5)]);
    assert_eq!(wrap("aaaa bbbb cc dd", 9, 5), [r(0, 10, 0, 9), r(10, 13, 5, 2), r(13, 15, 5, 2)]);
    // One cell left for text.
    assert_eq!(wrap("ab cd", 3, 2), [r(0, 3, 0, 2), r(3, 4, 2, 1), r(4, 5, 2, 1)]);
}

#[test]
fn contract_a_hang_as_wide_as_the_measure_or_wider_is_no_hang() {
    let line = "aaa bbb ccc";
    let plain = wrap(line, 3, 0);
    assert_eq!(plain, [r(0, 4, 0, 3), r(4, 8, 0, 3), r(8, 11, 0, 3)]);
    for hang in [3, 4, 100, usize::MAX] {
        assert_eq!(wrap(line, 3, hang), plain, "hang {hang}");
    }
    // Against a measure of 0, which is 1, any hang at all is too wide.
    assert_eq!(wrap("ab", 0, 1), [r(0, 1, 0, 1), r(1, 2, 0, 1)]);
    assert_eq!(wrap(line, usize::MAX, usize::MAX), [r(0, 11, 0, 11)]);
}

#[test]
fn contract_cells_leave_out_the_whitespace_a_row_was_broken_at() {
    // "not counting whitespace left hanging at the end of a row that was
    // broken at that whitespace"
    assert_eq!(wrap("aa bb", 4, 0), [r(0, 3, 0, 2), r(3, 5, 0, 2)]);
    assert_eq!(wrap("aaaa      bb", 4, 0), [r(0, 10, 0, 4), r(10, 12, 0, 2)]);
    // Whitespace inside a row is text like any other.
    assert_eq!(wrap("a  b cc", 5, 0), [r(0, 5, 0, 4), r(5, 7, 0, 2)]);
    // A row broken inside a word or after a hyphen has nothing hanging.
    assert_eq!(wrap("abcdef", 4, 0), [r(0, 4, 0, 4), r(4, 6, 0, 2)]);
    assert_eq!(wrap("ab-cdef", 4, 0), [r(0, 3, 0, 3), r(3, 7, 0, 4)]);
}

#[test]
fn contract_whitespace_stays_on_its_row_and_may_hang_past_the_measure() {
    let line = "abcd     efgh";
    let rows = wrap(line, 4, 0);
    assert_eq!(texts(line, &rows), ["abcd     ", "efgh"]);
    assert_eq!(rows[0].cells, 4);
    // "the next row starts at the first non-whitespace character"
    let line = "ab \t \t cd";
    let rows = wrap(line, 3, 0);
    assert_eq!(texts(line, &rows), ["ab \t \t ", "cd"]);
    assert_eq!(rows[0].cells, 2);
}

#[test]
fn contract_a_tab_is_four_cells() {
    assert_eq!(wrap("a\tb", 6, 0), [r(0, 3, 0, 6)]);
    assert_eq!(wrap("a\tb", 5, 0), [r(0, 2, 0, 1), r(2, 3, 0, 1)]);
    let row = wrap("\tb", 9, 0)[0];
    assert_eq!(cell_of("\tb", &row, 1), 4);
    assert_eq!(wrap("\t", 9, 0), [r(0, 1, 0, 4)]);
}

#[test]
fn contract_a_row_may_end_after_a_hyphen_a_dash_or_a_slash() {
    for mark in ["-", "\u{2013}", "\u{2014}", "/"] {
        let line = format!("abc{mark}defg");
        let rows = wrap(&line, 5, 0);
        assert_eq!(texts(&line, &rows), [format!("abc{mark}"), "defg".to_string()], "{mark:?}");
        assert_eq!(rows[0].cells, 4, "{mark:?}");
    }
}

#[test]
fn contract_a_hyphen_before_a_space_or_the_end_is_no_opportunity_of_its_own() {
    // "that has a non-space character after it": here the space is the
    // opportunity, and it hangs.
    let line = "abc- defg";
    let rows = wrap(line, 5, 0);
    assert_eq!(texts(line, &rows), ["abc- ", "defg"]);
    assert_eq!(rows[0].cells, 4);
    // Nothing after the hyphen at all: the word is cut as a word.
    let line = "abcd-";
    assert_eq!(texts(line, &wrap(line, 4, 0)), ["abcd", "-"]);
    // The break goes to the last opportunity that fits.
    let line = "a-b-c-d-e";
    assert_eq!(texts(line, &wrap(line, 5, 0)), ["a-b-", "c-d-e"]);
    let line = "aa/bb cc/dd";
    assert_eq!(texts(line, &wrap(line, 7, 0)), ["aa/bb ", "cc/dd"]);
}

#[test]
fn contract_nothing_else_is_a_break_opportunity() {
    for mark in [".", ",", ":", ";", "_", "\\", "'", "(", "\u{00A0}", "\u{2212}"] {
        let line = format!("abc{mark}defg");
        assert_eq!(
            texts(&line, &wrap(&line, 5, 0)),
            [format!("abc{mark}d"), "efg".to_string()],
            "{mark:?} is not in the contract's list"
        );
    }
}

#[test]
fn contract_a_long_word_is_cut_at_the_last_cluster_that_fits() {
    let line = "abcdefghij";
    assert_eq!(wrap(line, 4, 0), [r(0, 4, 0, 4), r(4, 8, 0, 4), r(8, 10, 0, 2)]);
    // With an opportunity before it, the word first moves to its own row.
    let line = "ab cdefghij k";
    assert_eq!(texts(line, &wrap(line, 4, 0)), ["ab ", "cdef", "ghij ", "k"]);
    // Wide characters: three cells hold one of them.
    let line = "\u{4E00}\u{4E8C}\u{4E09}";
    assert_eq!(wrap(line, 3, 0), [r(0, 3, 0, 2), r(3, 6, 0, 2), r(6, 9, 0, 2)]);
    assert_eq!(wrap(line, 4, 0), [r(0, 6, 0, 4), r(6, 9, 0, 2)]);
}

#[test]
fn contract_every_row_holds_a_cluster_even_one_wider_than_the_measure() {
    let line = "\u{4E00}\u{4E8C}";
    assert_eq!(wrap(line, 1, 0), [r(0, 3, 0, 2), r(3, 6, 0, 2)]);
    assert_eq!(wrap(line, 0, 0), [r(0, 3, 0, 2), r(3, 6, 0, 2)]);
    // One cell left beside the hang.
    assert_eq!(wrap(line, 3, 2), [r(0, 3, 0, 2), r(3, 6, 2, 2)]);
    // A wide character is never split to fill a row's last cell.
    assert_eq!(wrap("a\u{4E00}", 2, 0), [r(0, 1, 0, 1), r(1, 4, 0, 2)]);
}

#[test]
fn contract_a_combining_mark_is_never_separated_from_its_base() {
    let line = "e\u{0301}\u{0302}\u{0303}\u{0304}e\u{0301}\u{0302}\u{0303}\u{0304}";
    assert_eq!(wrap(line, 1, 0), [r(0, 9, 0, 1), r(9, 18, 0, 1)]);
    // Joiners and variation selectors are zero cells too.
    let line = "\u{2764}\u{FE0F}\u{2764}\u{FE0F}";
    assert_eq!(wrap(line, 1, 0), [r(0, 6, 0, 1), r(6, 12, 0, 1)]);
    // A mark on a hanging space hangs with it; a mark on the last
    // character before a break stays before the break.
    let line = "ab\u{0301} cd";
    assert_eq!(wrap(line, 2, 0), [r(0, 5, 0, 2), r(5, 7, 0, 2)]);
    let line = "ab\u{0301}cd";
    assert_eq!(wrap(line, 2, 0), [r(0, 4, 0, 2), r(4, 6, 0, 2)]);
    for measure in 1..6 {
        for row in wrap("zo\u{0308}\u{0304}e\u{0301} ya\u{0313}\u{0301}-b\u{20DD}", measure, 1) {
            let text = "zo\u{0308}\u{0304}e\u{0301} ya\u{0313}\u{0301}-b\u{20DD}";
            assert!(text.is_char_boundary(row.start));
            assert!(!text[row.start..].starts_with(is_combining), "a row starts with a mark at {measure}");
        }
    }
}

#[test]
fn contract_leading_whitespace_is_ordinary_text_on_the_first_row() {
    // It takes cells and is counted.
    assert_eq!(wrap("  ab", 4, 0), [r(0, 4, 0, 4)]);
    assert_eq!(wrap("  ab cd", 4, 0), [r(0, 5, 0, 4), r(5, 7, 0, 2)]);
    assert_eq!(wrap("\tab cd", 6, 0), [r(0, 4, 0, 6), r(4, 6, 0, 2)]);
    // It is not left hanging: a line that is all indent and one word does
    // not get a first row with nothing counted on it.
    let rows = wrap("    ab", 6, 0);
    assert_eq!(rows, [r(0, 6, 0, 6)]);
    assert!(wrap("    abc def", 8, 0)[0].cells >= 7);
}

#[test]
fn contract_cell_of() {
    let line = "- one two three";
    let rows = wrap(line, 9, 2);
    assert_eq!(rows, [r(0, 10, 0, 9), r(10, 15, 2, 5)]);
    // "The cell (indent included) at which the character starting at byte
    // col is drawn"
    assert_eq!(cell_of(line, &rows[0], 0), 0);
    assert_eq!(cell_of(line, &rows[0], 6), 6);
    assert_eq!(cell_of(line, &rows[1], 10), 2);
    assert_eq!(cell_of(line, &rows[1], 13), 5);
    // "for col == row.end, the cell just past the row's last character (not
    // clamped to the measure)"
    assert_eq!(cell_of(line, &rows[0], 10), 10);
    assert_eq!(cell_of(line, &rows[1], 15), 7);
    let line = "abcd      ef";
    let rows = wrap(line, 4, 0);
    assert_eq!(cell_of(line, &rows[0], rows[0].end), 10);
    // "col is first clamped into start..=end"
    let line = "- one two three";
    let rows = wrap(line, 9, 2);
    assert_eq!(cell_of(line, &rows[1], 0), 2);
    assert_eq!(cell_of(line, &rows[1], 9), 2);
    assert_eq!(cell_of(line, &rows[0], 11), 10);
    assert_eq!(cell_of(line, &rows[0], usize::MAX), 10);
    // "and back to a cluster start"
    let line = "a\u{4E00}n\u{0313}\u{0301}b";
    let rows = wrap(line, 20, 0);
    assert_eq!(cell_of(line, &rows[0], 1), 1);
    assert_eq!(cell_of(line, &rows[0], 2), 1); // inside the wide character
    assert_eq!(cell_of(line, &rows[0], 3), 1);
    assert_eq!(cell_of(line, &rows[0], 4), 3);
    assert_eq!(cell_of(line, &rows[0], 5), 3); // on the first mark
    assert_eq!(cell_of(line, &rows[0], 6), 3); // inside the first mark
    assert_eq!(cell_of(line, &rows[0], 7), 3); // on the second mark
    assert_eq!(cell_of(line, &rows[0], 9), 4);
    assert_eq!(cell_of(line, &rows[0], 10), 5);
}

#[test]
fn contract_col_at() {
    let line = "- one two three";
    let rows = wrap(line, 9, 2);
    // "The byte col of the cluster drawn over the given cell"
    assert_eq!(col_at(line, &rows[0], 0, false), 0);
    assert_eq!(col_at(line, &rows[0], 5, false), 5);
    assert_eq!(col_at(line, &rows[1], 2, true), 10);
    assert_eq!(col_at(line, &rows[1], 6, true), 14);
    // "(cells left of the indent give row.start)"
    assert_eq!(col_at(line, &rows[1], 0, true), 10);
    assert_eq!(col_at(line, &rows[1], 1, false), 10);
    // "Past the text: row.end when last_row is true"
    assert_eq!(col_at(line, &rows[1], 7, true), 15);
    assert_eq!(col_at(line, &rows[1], usize::MAX, true), 15);
    // "otherwise the start of the row's last cluster (never row.end ...)"
    assert_eq!(col_at(line, &rows[0], 10, false), 9);
    assert_eq!(col_at(line, &rows[0], usize::MAX, false), 9);
    assert_eq!(col_at(line, &rows[1], 7, false), 14);
    // Wide characters and tabs answer for each of their cells; a marked
    // letter is one cluster.
    let line = "a\u{4E00}\tn\u{0313}b";
    let rows = wrap(line, 40, 0);
    assert_eq!(
        (0..10).map(|cell| col_at(line, &rows[0], cell, true)).collect::<Vec<_>>(),
        [0, 1, 1, 4, 4, 4, 4, 5, 8, 9]
    );
    assert_eq!(col_at(line, &rows[0], 9, false), 8);
    // The last cluster of a row that is followed by another may be wide.
    let line = "\u{4E00}\u{4E8C}\u{4E09}";
    let rows = wrap(line, 4, 0);
    assert_eq!(col_at(line, &rows[0], 9, false), 3);
    assert_eq!(col_at("", &Row::default(), 0, true), 0);
    assert_eq!(col_at("", &Row::default(), 9, true), 0);
}

fn sample() -> (Doc, Layout) {
    //   row 0  alpha beta·      (line 0)
    //   row 1  gamma
    //   row 2                   (line 1)
    //   row 3  - one two·       (line 2)
    //   row 4    three
    let doc = Doc::from_text("alpha beta gamma\n\n- one two three");
    let layout = Layout::new(&doc, 10, &hanging(&hang_list));
    (doc, layout)
}

#[test]
fn contract_layout_rows_and_index() {
    let (doc, layout) = sample();
    assert_eq!(layout.measure(), 10);
    assert_eq!(layout.rows(0), [r(0, 11, 0, 10), r(11, 16, 0, 5)]);
    assert_eq!(layout.rows(1), [r(0, 0, 0, 0)]);
    assert_eq!(layout.rows(2), [r(0, 10, 0, 9), r(10, 15, 2, 5)]);
    // "an empty slice past the end"
    assert!(layout.rows(3).is_empty());
    assert!(layout.rows(usize::MAX).is_empty());
    assert_eq!(layout.total_rows(), 5);
    // "global row of the line's first row; total_rows() past the end"
    assert_eq!([0, 1, 2].map(|line| layout.first_row(line)), [0, 2, 3]);
    assert_eq!([3, 4, usize::MAX].map(|line| layout.first_row(line)), [5, 5, 5]);
    // "(line, row within the line); rows past the end give the last row"
    assert_eq!([0, 1, 2, 3, 4].map(|row| layout.line_of_row(row)), [(0, 0), (0, 1), (1, 0), (2, 0), (2, 1)]);
    assert_eq!([5, 6, usize::MAX].map(|row| layout.line_of_row(row)), [(2, 1); 3]);
    assert_eq!(doc.line_count(), 3);
}

#[test]
fn contract_total_rows_is_at_least_one() {
    for measure in [0, 1, 80, usize::MAX] {
        let doc = Doc::new();
        let layout = Layout::new(&doc, measure, &hanging(&hang_wild));
        assert_eq!(layout.total_rows(), 1);
        assert_eq!(layout.rows(0), [r(0, 0, 0, 0)]);
        assert_eq!(layout.first_row(0), 0);
        assert_eq!(layout.first_row(1), 1);
        assert_eq!(layout.line_of_row(0), (0, 0));
        assert_eq!(layout.line_of_row(9), (0, 0));
        assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
        assert_eq!(layout.locate(&doc, p(5, 5)), (0, 0));
        assert_eq!(layout.pos_at(&doc, 3, 3), p(0, 0));
        assert_eq!(layout.up(&doc, p(0, 0), 4), p(0, 0));
        assert_eq!(layout.down(&doc, p(0, 0), 4), p(0, 0));
        assert_eq!(layout.row_start(&doc, p(0, 0)), p(0, 0));
        assert_eq!(layout.row_end(&doc, p(0, 0)), p(0, 0));
    }
}

#[test]
fn contract_the_hang_is_asked_of_each_lines_text() {
    let doc = Doc::from_text("- one two three\nplain words here\n    - deep list item");
    let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let recording = |line: &str| {
        asked.borrow_mut().push(line.to_string());
        hang_list(line)
    };
    let layout = Layout::new(&doc, 12, &hanging(&recording));
    let mut asked = asked.borrow().clone();
    asked.sort();
    let mut lines = doc.lines().to_vec();
    lines.sort();
    assert_eq!(asked, lines);
    assert_eq!(layout.rows(0)[1].indent, 2);
    assert_eq!(layout.rows(1)[1].indent, 0);
    assert_eq!(layout.rows(2)[1].indent, 6);
}

#[test]
fn contract_locate_and_the_soft_break() {
    let (doc, layout) = sample();
    assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
    assert_eq!(layout.locate(&doc, p(0, 9)), (0, 9));
    // On the space the row was broken at: still this row, cell 10.
    assert_eq!(layout.locate(&doc, p(0, 10)), (0, 10));
    // "A position exactly at a soft break belongs to the START of the
    // following row."
    assert_eq!(layout.locate(&doc, p(0, 11)), (1, 0));
    assert_eq!(layout.locate(&doc, p(0, 16)), (1, 5));
    assert_eq!(layout.locate(&doc, p(1, 0)), (2, 0));
    assert_eq!(layout.locate(&doc, p(2, 9)), (3, 9));
    // The following row is indented: its start is drawn after the indent.
    assert_eq!(layout.locate(&doc, p(2, 10)), (4, 2));
    assert_eq!(layout.locate(&doc, p(2, 15)), (4, 7));
    // A break inside a word and one after a hyphen behave the same way.
    let doc = Doc::from_text("abcdefgh\nwell-known");
    let layout = Layout::new(&doc, 5, &hanging(&hang_zero));
    assert_eq!(layout.locate(&doc, p(0, 4)), (0, 4));
    assert_eq!(layout.locate(&doc, p(0, 5)), (1, 0));
    assert_eq!(layout.locate(&doc, p(1, 4)), (2, 4));
    assert_eq!(layout.locate(&doc, p(1, 5)), (3, 0));
}

#[test]
fn contract_pos_at_is_the_inverse_of_locate() {
    let (doc, layout) = sample();
    assert_eq!(layout.pos_at(&doc, 0, 0), p(0, 0));
    assert_eq!(layout.pos_at(&doc, 0, 10), p(0, 10));
    // Past the text of a row that has another after it: its last cluster.
    assert_eq!(layout.pos_at(&doc, 0, 11), p(0, 10));
    assert_eq!(layout.pos_at(&doc, 0, usize::MAX), p(0, 10));
    // Past the text of a line's last row: the line's end.
    assert_eq!(layout.pos_at(&doc, 1, 5), p(0, 16));
    assert_eq!(layout.pos_at(&doc, 1, 99), p(0, 16));
    assert_eq!(layout.pos_at(&doc, 2, 7), p(1, 0));
    // Inside the indent: the row's start.
    assert_eq!(layout.pos_at(&doc, 4, 0), p(2, 10));
    assert_eq!(layout.pos_at(&doc, 4, 1), p(2, 10));
    assert_eq!(layout.pos_at(&doc, 4, 2), p(2, 10));
    assert_eq!(layout.pos_at(&doc, 4, 3), p(2, 11));
    // Rows past the end give the last row.
    assert_eq!(layout.pos_at(&doc, 5, 3), p(2, 11));
    assert_eq!(layout.pos_at(&doc, usize::MAX, usize::MAX), p(2, 15));
}

#[test]
fn contract_up_and_down() {
    let (doc, layout) = sample();
    // "one row up, landing as near as possible to the wanted cell"
    assert_eq!(layout.up(&doc, p(0, 13), 2), p(0, 2));
    assert_eq!(layout.up(&doc, p(1, 0), 3), p(0, 14));
    assert_eq!(layout.up(&doc, p(1, 0), 40), p(0, 16));
    assert_eq!(layout.up(&doc, p(2, 4), 8), p(1, 0));
    assert_eq!(layout.up(&doc, p(2, 12), 0), p(2, 0));
    // A wanted cell past a row that has another after it cannot land on the
    // row's end, which is drawn one row down.
    assert_eq!(layout.up(&doc, p(0, 13), 40), p(0, 10));
    // "from the first row, the document start"
    assert_eq!(layout.up(&doc, p(0, 7), 7), p(0, 0));
    assert_eq!(layout.up(&doc, p(0, 0), 99), p(0, 0));
    // "one row down"
    assert_eq!(layout.down(&doc, p(0, 3), 3), p(0, 14));
    assert_eq!(layout.down(&doc, p(0, 3), 40), p(0, 16));
    assert_eq!(layout.down(&doc, p(0, 14), 3), p(1, 0));
    assert_eq!(layout.down(&doc, p(1, 0), 3), p(2, 3));
    // Into an indented row, left of its indent: the row's start.
    assert_eq!(layout.down(&doc, p(2, 1), 1), p(2, 10));
    assert_eq!(layout.down(&doc, p(2, 3), 3), p(2, 11));
    // "from the last row, the document end"
    assert_eq!(layout.down(&doc, p(2, 12), 0), p(2, 15));
    assert_eq!(layout.down(&doc, p(2, 15), 0), p(2, 15));
    // A cursor at a soft break is on the following row, so up from there
    // reaches the row before the break.
    assert_eq!(layout.up(&doc, p(0, 11), 0), p(0, 0));
    assert_eq!(layout.down(&doc, p(0, 11), 0), p(1, 0));
}

#[test]
fn contract_home_and_end_on_the_wrapped_row() {
    let (doc, layout) = sample();
    assert_eq!(layout.row_start(&doc, p(0, 5)), p(0, 0));
    assert_eq!(layout.row_start(&doc, p(0, 10)), p(0, 0));
    assert_eq!(layout.row_start(&doc, p(0, 11)), p(0, 11));
    assert_eq!(layout.row_start(&doc, p(0, 16)), p(0, 11));
    assert_eq!(layout.row_start(&doc, p(1, 0)), p(1, 0));
    assert_eq!(layout.row_start(&doc, p(2, 14)), p(2, 10));
    // "the line end on the last row"
    assert_eq!(layout.row_end(&doc, p(0, 12)), p(0, 16));
    assert_eq!(layout.row_end(&doc, p(1, 0)), p(1, 0));
    assert_eq!(layout.row_end(&doc, p(2, 10)), p(2, 15));
    // "otherwise just after the last non-whitespace character of the row"
    assert_eq!(layout.row_end(&doc, p(0, 0)), p(0, 10));
    assert_eq!(layout.row_end(&doc, p(0, 10)), p(0, 10));
    assert_eq!(layout.row_end(&doc, p(2, 3)), p(2, 9));
    let doc = Doc::from_text("ab      cd");
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    assert_eq!(layout.row_end(&doc, p(0, 6)), p(0, 2));
    // "or the start of the row's last cluster if the row was broken mid-word"
    let doc = Doc::from_text("abcdefgh\n\u{4E00}\u{4E8C}\u{4E09}\nabce\u{0301}\u{0302}fgh");
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    assert_eq!(layout.row_end(&doc, p(0, 1)), p(0, 3));
    assert_eq!(layout.row_end(&doc, p(0, 4)), p(0, 8));
    assert_eq!(layout.row_end(&doc, p(1, 0)), p(1, 3));
    assert_eq!(layout.row_end(&doc, p(2, 0)), p(2, 3));
}

#[test]
fn contract_rewrap_wraps_everything_for_a_new_measure() {
    let (doc, mut layout) = sample();
    for measure in [40, 3, 1, 10, usize::MAX] {
        layout.rewrap(&doc, measure, &hanging(&hang_list));
        assert_eq!(layout.measure(), measure);
        assert_same_layout(&layout, &Layout::new(&doc, measure, &hanging(&hang_list)), &doc, "rewrap");
    }
    // Also for a different document and a different hang.
    let other = Doc::from_text("x\ny y y y y y\n\nz");
    layout.rewrap(&other, 4, &hanging(&hang_zero));
    assert_same_layout(&layout, &Layout::new(&other, 4, &hanging(&hang_zero)), &other, "rewrap of another document");
    assert_eq!(layout.total_rows(), 6);
}

#[test]
fn contract_apply_takes_the_document_after_the_change() {
    let mut doc = Doc::from_text("one two three four\nfive six seven\n- eight nine ten\nlast");
    let mut layout = Layout::new(&doc, 8, &hanging(&hang_list));
    // Typing, splitting, joining, replacing across lines, a no-op, undo and redo.
    let spans: Vec<(Span, &str)> = vec![
        (Span::at(p(1, 4)), " and a half"),
        (Span::at(p(0, 7)), "\n"),
        (Span::new(p(0, 7), p(1, 0)), ""),
        (Span::new(p(0, 3), p(2, 7)), "\r\n\r\n"),
        (Span::at(p(2, 0)), ""),
        (Span::at(p(3, 0)), "- the very "),
        (Span::new(p(0, 0), p(9, 9)), "all of it replaced by one line"),
    ];
    for (span, text) in spans {
        let change = doc.replace(span, text);
        layout.apply(&doc, &change, &hanging(&hang_list));
        assert_same_layout(&layout, &Layout::new(&doc, 8, &hanging(&hang_list)), &doc, "apply");
        let back = doc.revert(&change);
        layout.apply(&doc, &back, &hanging(&hang_list));
        assert_same_layout(&layout, &Layout::new(&doc, 8, &hanging(&hang_list)), &doc, "apply of a revert");
        let again = doc.reapply(&change);
        layout.apply(&doc, &again, &hanging(&hang_list));
        assert_same_layout(&layout, &Layout::new(&doc, 8, &hanging(&hang_list)), &doc, "apply of a reapply");
    }
}

#[test]
fn contract_apply_asks_the_hang_of_the_line_as_it_reads_after_the_change() {
    let mut doc = Doc::from_text("item one two three four");
    let mut layout = Layout::new(&doc, 8, &hanging(&hang_list));
    assert!(layout.rows(0)[1..].iter().all(|row| row.indent == 0));
    let change = doc.replace(Span::at(p(0, 0)), "- ");
    layout.apply(&doc, &change, &hanging(&hang_list));
    assert!(layout.rows(0)[1..].iter().all(|row| row.indent == 2));
    let change = doc.replace(Span::new(p(0, 0), p(0, 2)), "");
    layout.apply(&doc, &change, &hanging(&hang_list));
    assert!(layout.rows(0)[1..].iter().all(|row| row.indent == 0));
}


// ── second round: the sentences the first round's failures were about ──

/// Failed against the first version of wrap.rs, which took a space with
/// anything zero-width after it for a letter. Passes now that `cluster_at`
/// lets the base character decide.
///
/// CONTRACT: "Break opportunities: after a run of spaces or tabs" and "a
/// combining mark is 0 cells and is never separated from its base".
#[test]
fn contract_a_space_followed_by_a_direction_mark_is_still_a_break_opportunity() {
    let line = "ab \u{200F}cde";
    assert_eq!(texts("ab cde", &wrap("ab cde", 4, 0)), ["ab ", "cde"]);
    assert_eq!(texts(line, &wrap(line, 4, 0)), ["ab \u{200F}", "cde"]);
    // Every kind of zero-width character width.rs knows, after a space and
    // after a tab.
    let invisible = [
        "\u{200E}", "\u{200F}", "\u{200B}", "\u{200D}", "\u{2060}", "\u{FEFF}", "\u{202A}", "\u{202C}", "\u{FE0F}",
        "\u{E0100}",
    ];
    for mark in invisible {
        for blank in [" ", "\t"] {
            let line = format!("ab{blank}{mark}cde");
            let rows = wrap(&line, 4, 0);
            assert_eq!(
                texts(&line, &rows),
                [format!("ab{blank}{mark}"), "cde".to_string()],
                "{mark:?} after {blank:?}"
            );
            assert_eq!(rows[0].cells, 2, "{mark:?} after {blank:?}");
        }
    }
}

/// Failed against the first version of wrap.rs; passes now.
///
/// CONTRACT: "the whitespace stays on the row it follows and may hang past
/// the measure; the next row starts at the first non-whitespace character".
#[test]
fn contract_a_space_followed_by_a_direction_mark_still_hangs() {
    let line = "abcd \u{200F}ef";
    let rows = wrap(line, 4, 0);
    assert_eq!(texts("abcd ef", &wrap("abcd ef", 4, 0)), ["abcd ", "ef"]);
    assert_eq!(texts(line, &rows), ["abcd \u{200F}", "ef"]);
    assert_eq!(rows[0].cells, 4);
    // The layout sees the same row: the space is a cursor stop past the
    // measure, the mark is part of it, and End stops before it.
    let doc = Doc::from_text(line);
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    assert_eq!(layout.locate(&doc, p(0, 4)), (0, 4));
    assert_eq!(layout.locate(&doc, p(0, 5)), (0, 4));
    assert_eq!(layout.locate(&doc, p(0, 8)), (1, 0));
    assert_eq!(layout.pos_at(&doc, 0, 4), p(0, 4));
    assert_eq!(layout.pos_at(&doc, 0, 99), p(0, 4));
    assert_eq!(layout.row_end(&doc, p(0, 1)), p(0, 4));
}

/// The case that matters in practice: prose in a right-to-left script,
/// where every space is followed by U+200F. No word is cut.
#[test]
fn contract_mixed_direction_prose_breaks_between_its_words() {
    let (shalom, olam, tov) =
        ("\u{05E9}\u{05DC}\u{05D5}\u{05DD}", "\u{05E2}\u{05D5}\u{05DC}\u{05DD}", "\u{05D8}\u{05D5}\u{05D1}");
    let line = format!("{shalom} \u{200F}{olam} \u{200F}{tov}");
    let rows = wrap(&line, 5, 0);
    assert_eq!(texts(&line, &rows), [format!("{shalom} \u{200F}"), format!("{olam} \u{200F}"), tov.to_string()]);
    assert_eq!(rows.iter().map(|row| row.cells).collect::<Vec<_>>(), [4, 4, 3]);
    // Room for two words: both go on the first row and its space hangs.
    let rows = wrap(&line, 9, 0);
    assert_eq!(texts(&line, &rows), [format!("{shalom} \u{200F}{olam} \u{200F}"), tov.to_string()]);
    assert_eq!(rows[0].cells, 9);
}

/// OWNER'S READING, pinned here so that a change of mind shows up.
///
/// The second version of wrap.rs went further than the first round asked: a
/// space or tab is whitespace under ANY mark, a visible one included. U+0301
/// over a space draws an acute accent in the blank cell, and that cell now
/// hangs past the measure, is left out of `cells`, and End stops before it.
/// The contract does not say what a marked space is, so this is not a
/// failure; it is listed with the deviations.
#[test]
fn owners_reading_whitespace_under_a_visible_mark_is_whitespace_too() {
    let line = "abcd \u{0301}ef";
    assert_eq!(wrap(line, 4, 0), [r(0, 7, 0, 4), r(7, 9, 0, 2)]);
    let doc = Doc::from_text(line);
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    assert_eq!(layout.row_end(&doc, p(0, 0)), p(0, 4));
    // On the last row it counts only while it fits, as bare whitespace does.
    assert_eq!(wrap("ab \u{0301}", 2, 0), [r(0, 5, 0, 2)]);
    assert_eq!(wrap("ab \u{0301}", 3, 0), [r(0, 5, 0, 3)]);
    // A tab under a mark is four cells of whitespace.
    assert_eq!(wrap("ab\t\u{0313}cd", 4, 0), [r(0, 5, 0, 2), r(5, 7, 0, 2)]);
    // At the start of the line it is text, as bare leading whitespace is,
    // and so is a space that a mark with no base before it has joined.
    assert_eq!(wrap(" \u{0301}ab cd", 3, 0), [r(0, 6, 0, 3), r(6, 8, 0, 2)]);
    assert_eq!(wrap("\u{0301} ab cd", 3, 0), [r(0, 6, 0, 3), r(6, 8, 0, 2)]);
}

/// CONTRACT, of cell_of: "(not clamped to the measure)"; of wrap: the
/// whitespace "stays on the row it follows and may hang past the measure".
/// So a cursor in or after hanging whitespace is drawn past the measure, and
/// no row is made for it.
#[test]
fn contract_a_cursor_in_hanging_whitespace_is_drawn_past_the_measure() {
    let doc = Doc::from_text("abcd      ef\nabcd      ");
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    // A row broken at its whitespace.
    assert_eq!(layout.rows(0), [r(0, 10, 0, 4), r(10, 12, 0, 2)]);
    assert_eq!(layout.locate(&doc, p(0, 4)), (0, 4));
    assert_eq!(layout.locate(&doc, p(0, 9)), (0, 9));
    assert_eq!(layout.locate(&doc, p(0, 10)), (1, 0));
    assert_eq!(layout.pos_at(&doc, 0, 7), p(0, 7));
    assert_eq!(layout.pos_at(&doc, 0, 99), p(0, 9));
    // A line that ends in whitespace: one row, and its end where it falls.
    assert_eq!(layout.rows(1).len(), 1);
    assert_eq!((layout.rows(1)[0].start, layout.rows(1)[0].end), (0, 10));
    assert_eq!(layout.total_rows(), 3);
    assert_eq!(layout.locate(&doc, p(1, 10)), (2, 10));
    assert_eq!(layout.pos_at(&doc, 2, 10), p(1, 10));
    assert_eq!(layout.pos_at(&doc, 2, 6), p(1, 6));
    assert_eq!(layout.row_end(&doc, p(1, 0)), p(1, 10));
}

#[test]
fn contract_apply_after_rewrap_keeps_the_new_measure() {
    let mut doc = Doc::from_text("one two three four five\nsix seven eight");
    let mut layout = Layout::new(&doc, 30, &hanging(&hang_list));
    assert_eq!(layout.total_rows(), 2);
    layout.rewrap(&doc, 6, &hanging(&hang_list));
    let change = doc.replace(Span::at(p(1, 3)), " and a half");
    layout.apply(&doc, &change, &hanging(&hang_list));
    assert_eq!(layout.measure(), 6);
    assert_same_layout(&layout, &Layout::new(&doc, 6, &hanging(&hang_list)), &doc, "apply after rewrap");
    assert!(layout == Layout::new(&doc, 6, &hanging(&hang_list)));
}

/// wrap.rs's own unit tests failed on these four lines in the second round
/// (`wrap_matches_the_slow_reference`,
/// `every_row_is_whole_contiguous_and_within_the_measure`,
/// `a_layout_kept_in_step_equals_one_built_fresh`,
/// `positions_round_trip_through_rows_and_cells`). Each has a space under a
/// mark in it. wrap itself did what its new rule says with them, as both
/// models here confirm: it was the reference in that test module
/// (`ref_clusters`: `space: text == " " || text == "\t"`) that still held
/// the old rule. It has since been brought in step; the lines stay here.
#[test]
fn the_lines_the_unit_tests_in_src_fail_on_are_wrapped_as_the_new_rule_says() {
    let cases: [(&str, usize, usize); 4] = [
        ("\twordn\u{0313}-\u{00E9} \u{0301}-\u{2013} \u{0301}\u{0301}\u{4E8C}\u{4E09}", 3, 4),
        ("\u{4E8C}\u{4E09}- item \u{2013}n\u{0313}a \u{0301}  \u{00E9}\u{4E8C}\u{4E09}a", 11, 2),
        ("mth\u{4E00} \u{0301}e quick-rown\u{2014} f\u{00E9}", 1, 0),
        (
            "    n\u{0313}well-known\u{00E9}the quick brown fox -\u{4E00}word\u{0301}\u{2014}\u{4E00}\u{00E9}\u{4E8C}\u{4E09} \u{0301}longerword-",
            1,
            0,
        ),
    ];
    for (line, measure, hang) in cases {
        let rows = wrap(line, measure, hang);
        assert_eq!(rows, model_wrap(line, measure, hang), "{line:?} at {measure}, hang {hang}");
        assert_eq!(rows, search_wrap(line, measure, hang), "{line:?} at {measure}, hang {hang}");
        check_rows(line, measure, hang, &rows);
        check_cells(line, &rows);
    }
    // The row the third of those tests objects to: a wide character at a
    // measure of 1, with the marked space hanging after it.
    let line = "mth\u{4E00} \u{0301}e quick-rown\u{2014} f\u{00E9}";
    assert_eq!(wrap(line, 1, 0)[3], r(3, 9, 0, 2));
}

// ───────────────────────────── 5. FAILING ─────────────────────────────
//
// Each test in this section is a reduced case of the implementation departing
// from a sentence of the contract, meant to fail until wrap.rs changes.
//
// Nothing here fails as wrap.rs stands. The two that failed in the first
// round for a space followed by a direction mark moved up into section 4 in
// the second; the one about tabs, below, failed in both and passes in the
// third, and keeps its place so the history stays with it.

/// Failed against the first two versions of wrap.rs, where up and down went
/// through pos_at, which gives the cluster drawn over a cell, that is, always
/// its left edge. Passes now that they take the nearer place to rest.
///
/// CONTRACT, of `up`: "one row up, landing as near as possible to the
/// wanted cell" (and `down` likewise), read with `locate`: "(global row,
/// cell) where a cursor at p is drawn".
///
/// A tab is four cells. On the row `\tx` a cursor can be drawn at cell 0
/// (before the tab), cell 4 (before the x) and cell 5 (the line's end). A
/// cursor wanting cell 3 is one cell from the second and three from the
/// first, and used to be sent to the first. The same at the end of a line:
/// on `a\t` the stops are cells 0, 1 and 5, and a cursor wanting cell 4 was
/// put at cell 1.
///
/// For a two-cell character the two edges are equally near, so only tabs
/// (bare or under a mark) showed this. The property test
/// `up_and_down_land_as_near_as_possible` checks the promise on every row.
///
/// How much it matters: DESIGN.md has ok never type a tab itself, so it
/// takes a pasted or opened text with tabs in it.
#[test]
fn contract_up_and_down_land_on_the_nearer_edge_of_a_tab() {
    let doc = Doc::from_text("\tx\nabcde\n\tx\nabcde\na\t");
    let layout = Layout::new(&doc, 20, &hanging(&hang_zero));
    // Where the cursor can be drawn on the rows with tabs in them.
    assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
    assert_eq!(layout.locate(&doc, p(0, 1)), (0, 4));
    assert_eq!(layout.locate(&doc, p(4, 1)), (4, 1));
    assert_eq!(layout.locate(&doc, p(4, 2)), (4, 5));
    // One cell into the tab the left edge is the nearer one, and is given.
    assert_eq!(layout.up(&doc, p(1, 1), 1), p(0, 0));

    let mut wrong: Vec<String> = Vec::new();
    let mut expect = |what: &str, got: Pos, want: Pos| {
        if got != want {
            wrong.push(format!("{what}: got {got:?}, the nearest stop is {want:?}"));
        }
    };
    expect("up to cell 3 of `\\tx` (stops at cells 0, 4, 5)", layout.up(&doc, p(1, 3), 3), p(0, 1));
    expect("down to cell 3 of `\\tx` (stops at cells 0, 4, 5)", layout.down(&doc, p(1, 3), 3), p(2, 1));
    expect("down to cell 4 of `a\\t` (stops at cells 0, 1, 5)", layout.down(&doc, p(3, 4), 4), p(4, 2));
    assert!(wrong.is_empty(), "up and down did not land as near as possible to the wanted cell:\n{wrong:#?}");

    // Halfway across the tab the two edges are equally near and the left
    // one is given, as it is for a wide character. pos_at is untouched: it
    // names the cluster under the cell wherever in the tab the cell is.
    assert_eq!(layout.up(&doc, p(1, 2), 2), p(0, 0));
    assert_eq!(layout.down(&doc, p(3, 3), 3), p(4, 1));
    assert_eq!(layout.pos_at(&doc, 0, 3), p(0, 0));
    assert_eq!(layout.pos_at(&doc, 4, 4), p(4, 1));
    // The far edge of a tab that a row was broken at is the next row's
    // start, where a cursor would be drawn one row down: no stop there.
    let doc = Doc::from_text("ab\t\tcd\nx");
    let layout = Layout::new(&doc, 4, &hanging(&hang_zero));
    assert_eq!(layout.rows(0), [r(0, 4, 0, 2), r(4, 6, 0, 2)]);
    assert_eq!(layout.up(&doc, p(0, 4), 5), p(0, 3));
    assert_eq!(layout.up(&doc, p(0, 4), 9), p(0, 3));
}

// ───────────────────── 6. hazards outside the contract ─────────────────────

/// Not a contract failure: `Doc::replace` never builds such a `Change`.
/// Kept because the owner's notes say apply "is self-healing ... instead of
/// a panic" for hand-built changes, and this one used to panic in a debug
/// build: `Change::lines` computes `old_end.line - start.line + 1`, which
/// overflowed before apply's own guards got to look at it.
///
/// THIRD ROUND. apply now counts the lines itself with checked arithmetic
/// and rebuilds the layout for a change it cannot count, so this runs with
/// the rest (it was `#[ignore]`d), and for every way of reaching the top of
/// the range.
#[test]
fn hazard_apply_survives_a_hand_built_change_reaching_line_usize_max() {
    let doc = Doc::from_text("a\nb");
    let top = usize::MAX;
    let lines = [(0, top, 0), (0, 0, top), (0, top, top), (1, top, 1), (top, top, top), (0, top - 1, 0), (top, 0, 0)];
    for (start, old_end, end) in lines {
        for measure in [0, 5, top] {
            let mut layout = Layout::new(&doc, measure, &hanging(&hang_zero));
            let change = Change { start: p(start, 0), old_end: p(old_end, 0), end: p(end, 0), ..Change::default() };
            layout.apply(&doc, &change, &hanging(&hang_zero));
            let what = format!("after an impossible change of lines {start}, {old_end}, {end}");
            assert_same_layout(&layout, &Layout::new(&doc, measure, &hanging(&hang_zero)), &doc, &what);
        }
    }
}

// ───────────────────── 7. lines of mixed shape and height ─────────────────────
//
// CONTRACT, as the page asked for it: a line's shape says "how many leading
// bytes hang in the gutter and are not part of the rows, the scale of the
// line (1 or 2, so a double-size line wraps at half the measure and each of
// its rows is two grid rows tall), the hanging indent, and an inset from
// both edges", and the layout's "row arithmetic (total rows, first row,
// locate, pos_at, up, down, row start and end)" is to be "correct for lines
// of mixed height".
//
// The model here builds the rows of each line from `model_wrap` on the text
// after the hung bytes, then lays every row out on the grid one grid row at
// a time. Everything the layout says is checked against that grid.

/// How the page reads a line, near enough: `# ` a double-size heading with
/// its mark hung, `## ` a single-size one, `> ` hung and set in by four,
/// list items and indented lines hanging under their text.
fn shape_page(_: usize, line: &str) -> Shape {
    if line.starts_with("# ") {
        Shape { hung: 2, scale: 2, ..Shape::default() }
    } else if line.starts_with("## ") {
        Shape { hung: 3, ..Shape::default() }
    } else if line.starts_with("> ") {
        Shape { hung: 2, inset: 4, ..Shape::default() }
    } else {
        Shape::hanging(hang_list(line))
    }
}

/// Shapes that go to every extreme, including ones no page would ask for:
/// bytes hung from inside a character or past the end, scales of 0 and 9,
/// insets that leave no room, and all of them at once.
fn shape_wild(at: usize, line: &str) -> Shape {
    let pick = line.len().wrapping_mul(7).wrapping_add(at);
    let hung = match pick % 7 {
        0 | 1 => 0,
        2 => 1,
        3 => 2,
        4 => line.len() / 2,
        5 => line.len(),
        _ => usize::MAX,
    };
    let scale = [1, 2, 1, 2, 0, 9, 2][(pick / 7) % 7];
    let inset = [0, 0, 1, 4, 0, usize::MAX, 2][(pick / 49) % 7];
    Shape { hung, scale, hang: hang_wild(line), inset }
}

/// A shape as the layout is to use it, worked out the long way.
fn model_shape(line: &str, measure: usize, shape: Shape) -> Shape {
    let scale = if shape.scale >= 2 { 2 } else { 1 };
    let mut hung = shape.hung.min(line.len());
    while !line.is_char_boundary(hung) {
        hung -= 1;
    }
    let room = (measure / scale).max(1);
    let fits = shape.inset.checked_mul(2).is_some_and(|both| both < room);
    Shape { hung, scale, hang: shape.hang, inset: if fits { shape.inset } else { 0 } }
}

/// A shaped document laid out on the grid.
struct Shaped {
    /// The rows and clusters of every line, as [`Model`] keeps them: the
    /// rows counted one each, whatever their height, and the clusters those
    /// of the text after the hung bytes.
    model: Model,
    shapes: Vec<Shape>,
    /// For every grid row, which of the model's rows it shows.
    shown: Vec<usize>,
    /// For every one of the model's rows, the grid row of its top.
    top: Vec<usize>,
    /// For every one of the model's rows, its line.
    line: Vec<usize>,
}

impl Shaped {
    fn new(doc: &Doc, measure: usize, shape: &dyn Fn(usize, &str) -> Shape) -> Shaped {
        let mut rows: Vec<Vec<Row>> = Vec::new();
        let mut cs: Vec<Vec<Cl>> = Vec::new();
        let mut shapes = Vec::new();
        let mut first = Vec::new();
        let (mut shown, mut top, mut line_of) = (Vec::new(), Vec::new(), Vec::new());
        for (i, line) in doc.lines().iter().enumerate() {
            let used = model_shape(line, measure, shape(i, line));
            let tail = &line[used.hung..];
            let room = (measure / used.scale).max(1) - 2 * used.inset;
            let moved = |row: Row| Row { start: row.start + used.hung, end: row.end + used.hung, indent: row.indent + used.inset, ..row };
            let line_rows: Vec<Row> = model_wrap(tail, room, used.hang).into_iter().map(moved).collect();
            first.push(top.len());
            for _ in &line_rows {
                for _ in 0..used.scale {
                    shown.push(top.len());
                }
                top.push(shown.len() - used.scale);
                line_of.push(i);
            }
            cs.push(clusters(tail).into_iter().map(|c| Cl { start: c.start + used.hung, end: c.end + used.hung, ..c }).collect());
            rows.push(line_rows);
            shapes.push(used);
        }
        let total = top.len();
        Shaped { model: Model { rows, cs, first, total }, shapes, shown, top, line: line_of }
    }

    fn scale_of_row(&self, model_row: usize) -> usize {
        self.shapes[self.line[model_row]].scale
    }

    /// Where a cursor is drawn, for a position at or after the hung bytes.
    fn locate(&self, doc: &Doc, at: Pos) -> (usize, usize) {
        let (row, cell) = self.model.locate(doc, at);
        (self.top[row], cell.saturating_mul(self.scale_of_row(row)))
    }

    fn pos_at(&self, doc: &Doc, grid_row: usize, grid_cell: usize) -> Pos {
        let row = self.shown[grid_row.min(self.shown.len() - 1)];
        self.model.pos_at(doc, row, grid_cell / self.scale_of_row(row))
    }

    /// OWNER'S READING: on a double-size row "nearest to `cell`" is nearest
    /// to the double cell that `cell` falls in, which is what `up` and
    /// `down` are documented to do.
    fn land(&self, doc: &Doc, row: usize, grid_cell: usize) -> Pos {
        self.model.land(doc, row, grid_cell / self.scale_of_row(row))
    }

    fn up(&self, doc: &Doc, at: Pos, grid_cell: usize) -> Pos {
        match self.model.locate(doc, at).0 {
            0 => p(0, 0),
            row => self.land(doc, row - 1, grid_cell),
        }
    }

    fn down(&self, doc: &Doc, at: Pos, grid_cell: usize) -> Pos {
        let row = self.model.locate(doc, at).0;
        if row + 1 >= self.model.total {
            doc.end()
        } else {
            self.land(doc, row + 1, grid_cell)
        }
    }
}

/// Everything the contract promises about a layout whose lines are shaped.
fn check_shaped(doc: &Doc, layout: &Layout, measure: usize, shape: &dyn Fn(usize, &str) -> Shape, rng: &mut Rng) {
    let grid = Shaped::new(doc, measure, shape);
    let total = grid.shown.len();
    let lines = doc.line_count();
    assert!(total >= 1);
    assert_eq!(layout.total_rows(), total, "total_rows at {measure}");
    for past in [lines, lines + 1, usize::MAX] {
        assert!(layout.rows(past).is_empty());
        assert_eq!(layout.first_row(past), total);
        assert_eq!(layout.shape(past), Shape::default());
    }
    for line in 0..lines {
        let text = doc.line(line);
        assert_eq!(layout.shape(line), grid.shapes[line], "shape of {text:?} at {measure}");
        assert_eq!(layout.rows(line), &grid.model.rows[line][..], "rows of {text:?} at {measure}");
        assert_eq!(layout.first_row(line), grid.top[grid.model.first[line]], "first_row of line {line}");
        assert_eq!(layout.rows(line)[0].start, grid.shapes[line].hung, "the first row starts after the hung bytes");
    }
    for grid_row in 0..total {
        let row = grid.shown[grid_row];
        let line = grid.line[row];
        assert_eq!(layout.line_of_row(grid_row), (line, row - grid.model.first[line]), "line_of_row {grid_row}");
    }
    for past in [total, total + 1, usize::MAX] {
        assert_eq!(layout.line_of_row(past), layout.line_of_row(total - 1), "line_of_row past the end");
    }

    for line in 0..lines {
        let text = doc.line(line);
        let used = grid.shapes[line];
        let cs = &grid.model.cs[line];
        let stride = 1 + cs.len() / 60;
        let skip = rng.below(stride);
        let mut stops: Vec<Pos> = cs.iter().skip(skip).step_by(stride).map(|c| p(line, c.start)).collect();
        stops.extend(cs.first().map(|c| p(line, c.start)));
        stops.extend(cs.last().map(|c| p(line, c.start)));
        stops.push(p(line, text.len()));
        for &at in &stops {
            let ctx = || format!("{at:?} in {text:?} shaped {used:?} at {measure}");
            let (row, cell) = layout.locate(doc, at);
            assert_eq!((row, cell), grid.locate(doc, at), "locate {}", ctx());
            let found = layout.pos_at(doc, row, cell);
            assert_eq!(layout.locate(doc, found), (row, cell), "what pos_at finds is drawn in the same place: {}", ctx());
            if at.col < text.len() {
                // Any grid cell of the letter, on either of its grid rows.
                for (dr, dc) in [(0, 0), (used.scale - 1, 0), (0, used.scale - 1), (used.scale - 1, used.scale - 1)] {
                    let (r, c) = (row + dr, cell.saturating_add(dc));
                    assert_eq!(layout.pos_at(doc, r, c), at, "round trip through ({r}, {c}) of {}", ctx());
                }
            }
            for want in [cell, 0, rng.below(60), usize::MAX] {
                let below = layout.down(doc, at, want);
                assert_eq!(below, grid.down(doc, at, want), "down to {want} from {}", ctx());
                if row + used.scale < total {
                    assert_eq!(layout.locate(doc, below).0, row + used.scale, "down steps over the whole row from {}", ctx());
                    assert_eq!(layout.locate(doc, layout.up(doc, below, want)).0, row, "down then up from {}", ctx());
                } else {
                    assert_eq!(below, doc.end());
                }
                let above = layout.up(doc, at, want);
                assert_eq!(above, grid.up(doc, at, want), "up to {want} from {}", ctx());
                if row > 0 {
                    let over = grid.shown[row - 1];
                    assert_eq!(layout.locate(doc, above).0, grid.top[over], "up lands on the row above {}", ctx());
                    assert_eq!(layout.locate(doc, layout.down(doc, above, want)).0, row, "up then down from {}", ctx());
                } else {
                    assert_eq!(above, p(0, 0));
                }
            }
            let home = layout.row_start(doc, at);
            assert_eq!(home, grid.model.row_start(doc, at), "row_start of {}", ctx());
            assert!(home.col >= used.hung, "Home stays out of the hung bytes: {}", ctx());
            assert_eq!(layout.locate(doc, home).0, row, "Home stays on the row of {}", ctx());
            let end = layout.row_end(doc, at);
            assert_eq!(end, grid.model.row_end(doc, at), "row_end of {}", ctx());
            assert_eq!(layout.locate(doc, end).0, row, "End stays on the row of {}", ctx());
        }

        // Inside the hung bytes a position answers as the text beside it.
        let beside = p(line, used.hung);
        for col in 0..used.hung {
            let at = p(line, col);
            assert_eq!(layout.locate(doc, at), layout.locate(doc, beside), "locate in the hung bytes of {text:?}");
            assert_eq!(layout.row_start(doc, at), layout.row_start(doc, beside), "{text:?}");
            assert_eq!(layout.row_end(doc, at), layout.row_end(doc, beside), "{text:?}");
            let want = rng.below(40);
            assert_eq!(layout.down(doc, at, want), layout.down(doc, beside, want), "{text:?}");
            if layout.locate(doc, beside).0 > 0 {
                assert_eq!(layout.up(doc, at, want), layout.up(doc, beside, want), "{text:?}");
            }
        }
    }

    for grid_row in (0..total).step_by(1 + total / 150) {
        for cell in [0, 1, 2, 3, rng.below(measure.clamp(1, 200) + 8), usize::MAX] {
            let found = layout.pos_at(doc, grid_row, cell);
            assert_eq!(found, grid.pos_at(doc, grid_row, cell), "pos_at({grid_row}, {cell}) at {measure}");
            let line = grid.line[grid.shown[grid_row]];
            assert!(found.col >= grid.shapes[line].hung, "pos_at never answers inside the hung bytes");
        }
    }
}

/// A document in which most lines open with something the page shapes.
fn shaped_doc(rng: &mut Rng, max_lines: usize) -> Doc {
    let mut text = String::new();
    for i in 0..1 + rng.below(max_lines) {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(["", "", "# ", "# ", "## ", "> ", "> ", "- ", "#", ">", "#\u{0301}", "# \u{0301}", "> \u{4E00}"][rng.below(13)]);
        if !rng.one_in(6) {
            text.push_str(&hostile_line(rng));
        }
    }
    Doc::from_text(&text)
}

#[test]
fn shaped_the_grid_of_one_small_document_by_hand() {
    //   rows 0-1   Big         (line 0: "# " hung, double size, 6 to the row)
    //   rows 2-3   title
    //   row 4                  (line 1)
    //   row 5          a       (line 2: "> " hung, set in by 4, 5 to the row)
    //   row 6          quote
    //   row 7          of
    //   row 8          five
    //   row 9      - one two   (line 3)
    //   row 10       three
    let doc = Doc::from_text("# Big title\n\n> a quote of five\n- one two three");
    let layout = Layout::new(&doc, 13, &shape_page);
    assert_eq!(layout.rows(0), [r(2, 6, 0, 3), r(6, 11, 0, 5)]);
    assert_eq!(layout.rows(2), [r(2, 4, 4, 1), r(4, 10, 4, 5), r(10, 13, 4, 2), r(13, 17, 4, 4)]);
    assert_eq!(layout.rows(3), [r(0, 10, 0, 9), r(10, 15, 2, 5)]);
    assert_eq!(layout.total_rows(), 11);
    assert_eq!([0, 1, 2, 3, 4].map(|line| layout.first_row(line)), [0, 4, 5, 9, 11]);
    let found: Vec<(usize, usize)> = (0..11).map(|row| layout.line_of_row(row)).collect();
    assert_eq!(found, [(0, 0), (0, 0), (0, 1), (0, 1), (1, 0), (2, 0), (2, 1), (2, 2), (2, 3), (3, 0), (3, 1)]);
    // "each of its rows is two grid rows tall", and each cell two cells wide
    assert_eq!(layout.locate(&doc, p(0, 4)), (0, 4));
    assert_eq!(layout.locate(&doc, p(0, 7)), (2, 2));
    for (row, cell) in [(2, 2), (2, 3), (3, 2), (3, 3)] {
        assert_eq!(layout.pos_at(&doc, row, cell), p(0, 7));
    }
    // "leading bytes hang in the gutter and are not part of the rows"
    assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
    assert_eq!(layout.locate(&doc, p(0, 1)), (0, 0));
    assert_eq!(layout.pos_at(&doc, 0, 0), p(0, 2));
    assert_eq!(layout.locate(&doc, p(2, 0)), (5, 4));
    assert_eq!(layout.pos_at(&doc, 5, 0), p(2, 2));
    assert_eq!(layout.row_start(&doc, p(0, 0)), p(0, 2));
    assert_eq!(layout.row_start(&doc, p(2, 1)), p(2, 2));
    // Down from the heading's first row lands on its second, and from there
    // steps over both of that row's grid rows.
    assert_eq!(layout.down(&doc, p(0, 3), 2), p(0, 7));
    assert_eq!(layout.down(&doc, p(0, 7), 2), p(1, 0));
    assert_eq!(layout.up(&doc, p(1, 0), 3), p(0, 7));
    assert_eq!(layout.up(&doc, p(2, 3), 9), p(1, 0));
    assert_eq!(layout.down(&doc, p(1, 0), 6), p(2, 3));
    assert_eq!(layout.down(&doc, p(2, 14), 0), p(3, 0));
    assert_eq!(layout.down(&doc, p(3, 12), 0), p(3, 15));
}

#[test]
fn shaped_documents_keep_every_layout_promise() {
    let mut rng = Rng::new(0x5A9E_D0C5);
    for _ in 0..90 * scale() {
        let doc = shaped_doc(&mut rng, 9);
        let measure = match rng.below(8) {
            0 => rng.below(4),
            1 => 66,
            2 => usize::MAX,
            _ => 1 + rng.below(40),
        };
        let layout = Layout::new(&doc, measure, &shape_page);
        check_shaped(&doc, &layout, measure, &shape_page, &mut rng);
        let layout = Layout::new(&doc, measure, &shape_wild);
        check_shaped(&doc, &layout, measure, &shape_wild, &mut rng);
    }
}

#[test]
fn shaped_every_tiny_document_keeps_every_layout_promise() {
    let mut rng = Rng::new(0x7171);
    let alphabet = ["#", " ", "a", ">", "\n", "-", "\u{0301}", "\u{4E00}"];
    for len in 0..=4usize {
        for code in 0..alphabet.len().pow(len as u32) {
            let mut text = String::new();
            let mut rest = code;
            for _ in 0..len {
                text.push_str(alphabet[rest % alphabet.len()]);
                rest /= alphabet.len();
            }
            let doc = Doc::from_text(&text);
            for measure in [1, 2, 3, 9] {
                let layout = Layout::new(&doc, measure, &shape_page);
                check_shaped(&doc, &layout, measure, &shape_page, &mut rng);
            }
        }
    }
}

#[test]
fn shaped_a_layout_after_apply_equals_one_built_fresh() {
    const OPENERS: &[&str] = &["# ", "## ", "> ", "- ", "\n# ", "\n## ", "\n> ", "\n- ", "\n", "#", " ", "\n\n"];
    for (seed, measure, wild) in [(71u64, 1usize, false), (72, 2, true), (73, 9, false), (74, 13, true), (75, 33, false), (76, 66, false), (77, 66, true), (78, usize::MAX, true)] {
        for round in 0..scale() as u64 {
            let shape: &dyn Fn(usize, &str) -> Shape = if wild { &shape_wild } else { &shape_page };
            let mut rng = Rng::new(seed + 1000 * round);
            let mut doc = Doc::new();
            let mut layout = Layout::new(&doc, measure, shape);
            let mut undo: Vec<Change> = Vec::new();
            for step in 0..400 {
                let change = if !undo.is_empty() && rng.one_in(8) {
                    let last = undo.pop().unwrap();
                    doc.revert(&last)
                } else {
                    let a = p(rng.below(doc.line_count() + 1), rng.below(12));
                    let b = match rng.below(6) {
                        0..=2 => a,
                        3 | 4 => p(a.line, a.col + rng.below(5)),
                        _ => p(a.line + rng.below(3), rng.below(9)),
                    };
                    let (a, b) = if doc.len_bytes() > 3000 { (p(0, 0), p(doc.line_count() / 2, 0)) } else { (a, b) };
                    let text = if rng.one_in(3) { rng.pick(OPENERS).to_string() } else if rng.one_in(5) { hostile_line(&mut rng) } else { rng.pick(WORDS).to_string() + " " };
                    let change = doc.replace(Span::new(a, b), &text);
                    undo.push(change.clone());
                    change
                };
                layout.apply(&doc, &change, shape);
                // A wild shape depends on the line's number, so lines below
                // an edit that added or removed lines have changed shape
                // with no change to their text. The page rewraps for that.
                if wild && change.old_end.line != change.end.line {
                    layout.rewrap(&doc, measure, shape);
                }
                let fresh = Layout::new(&doc, measure, shape);
                assert!(layout == fresh, "seed {seed} step {step} measure {measure}: apply differs from a fresh layout");
                if step % 80 == 79 {
                    check_shaped(&doc, &layout, measure, shape, &mut rng);
                }
            }
            check_shaped(&doc, &layout, measure, shape, &mut rng);
        }
    }
}

/// Turning a line into a heading changes its height and nothing about the
/// number of its rows; every line below still has to move.
#[test]
fn shaped_a_line_that_changes_size_moves_the_lines_below() {
    let mut doc = Doc::from_text("title\nbody words\nmore\nlast");
    let mut layout = Layout::new(&doc, 30, &shape_page);
    assert_eq!([0, 1, 2, 3, 4].map(|line| layout.first_row(line)), [0, 1, 2, 3, 4]);
    let change = doc.replace(Span::at(p(0, 0)), "# ");
    layout.apply(&doc, &change, &shape_page);
    assert_eq!(layout.rows(0), [r(2, 7, 0, 5)]);
    assert_eq!([0, 1, 2, 3, 4].map(|line| layout.first_row(line)), [0, 2, 3, 4, 5]);
    assert_eq!(layout.line_of_row(1), (0, 0));
    assert_eq!(layout.line_of_row(2), (1, 0));
    let change = doc.replace(Span::new(p(0, 1), p(0, 2)), "");
    layout.apply(&doc, &change, &shape_page);
    assert_eq!([0, 1, 2, 3, 4].map(|line| layout.first_row(line)), [0, 1, 2, 3, 4]);
    assert!(layout == Layout::new(&doc, 30, &shape_page));
}

#[test]
fn shaped_a_layout_asked_about_another_document_answers_with_real_positions() {
    let mut rng = Rng::new(0x0DD5_4A9E);
    for _ in 0..150 * scale() {
        let made_for = shaped_doc(&mut rng, 6);
        let mut asked = shaped_doc(&mut rng, 6);
        let measure = hostile_measure(&mut rng);
        let mut layout = Layout::new(&made_for, measure, &shape_wild);
        for _ in 0..10 {
            let wild = |rng: &mut Rng| if rng.one_in(4) { usize::MAX - rng.below(3) } else { rng.below(40) };
            let at = p(wild(&mut rng), wild(&mut rng));
            let (row, cell) = (wild(&mut rng), wild(&mut rng));
            layout.locate(&asked, at);
            layout.line_of_row(row);
            layout.first_row(row);
            layout.shape(row);
            for found in [
                layout.pos_at(&asked, row, cell),
                layout.up(&asked, at, cell),
                layout.down(&asked, at, cell),
                layout.row_start(&asked, at),
                layout.row_end(&asked, at),
            ] {
                assert_eq!(asked.clamp(found), found);
            }
        }
        let change = asked.replace(Span::new(p(rng.below(4), rng.below(9)), p(rng.below(4), rng.below(9))), "# x\n> y");
        layout.apply(&asked, &change, &shape_wild);
        layout.locate(&asked, asked.end());
    }
}
