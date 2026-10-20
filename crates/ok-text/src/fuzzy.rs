//! The matcher behind the prompt. One needle is tried against many
//! candidates (commands, names of texts, headings) and the best come first.

/// How a needle lines up with one candidate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match {
    /// Higher is better. Only worth comparing between candidates of the
    /// same needle.
    pub score: i32,
    /// The byte offset in the candidate of each matched character,
    /// ascending.
    pub hits: Vec<usize>,
}

/// Every hit earns this, which keeps ordinary scores above zero.
const HIT: i64 = 64;
/// More for a hit on the candidate's first character.
const START: i64 = 44;
/// More for a hit just after a separator. Larger than the worst lead
/// penalty, so the start of a word deep in a candidate still beats the
/// middle of a word near its front.
const SEPARATOR: i64 = 36;
/// More for a hit straight after the previous one. With `GAP` it outweighs
/// `START` and the worst lead penalty together, so letters that sit together
/// at the start of a word, however deep, beat the same letters picked off
/// the starts of separate words, even from the candidate's first character.
const RUN: i64 = 48;
/// More for a hit in the same case as the needle: enough to break a tie,
/// not enough to outweigh where the hit is.
const CASE: i64 = 2;
/// The cost of skipping one character between two hits, of each further
/// character skipped, and the most any one gap costs. The cap keeps a long
/// path in front of a good file name from sinking it.
const GAP: i64 = 12;
const GAP_MORE: i64 = 4;
const GAP_MOST: i64 = 28;
/// The cost of each character before the first hit, and how many of them
/// are counted.
const LEAD: i64 = 1;
const LEAD_REACH: usize = 12;
/// A candidate loses one point for each character the needle does not
/// account for, up to this many.
const LENGTH_MOST: usize = 32;

/// The most one hit can earn, and the most the penalties that are charged
/// once can take away. `Needle::step` is built from these.
const HIT_MOST: i64 = HIT + START + RUN + CASE;
const ONCE_MOST: i64 = LEAD * LEAD_REACH as i64 + LENGTH_MOST as i64;

/// Marks a cell of the table that no alignment reaches.
const NONE: i64 = i64::MIN / 2;

/// The largest table held whole, in cells. A larger one is worked through
/// in halves instead (see `Needle::solve`), so that a long needle against a
/// very long line costs time but never more than a few rows of memory.
const TABLE_MOST: usize = 1 << 21;

/// The room each tier of `Needle::tier` has in an `i32` score: a fifth of
/// the range, there being five tiers.
const TIER_ROOM: i64 = i32::MAX as i64 / 5;

/// Matches `needle` against `candidate` as a subsequence, ignoring case and
/// any whitespace in the needle. `None` when the needle's characters do not
/// all appear in order.
///
/// Of all the ways the needle can line up, the best scoring one is found,
/// not the leftmost, whatever the lengths, in time proportional to needle
/// length times candidate length. A hit scores more at the very start of
/// the candidate, just after a separator (space `/` `\` `-` `_` `.`),
/// straight after the previous hit, and in the needle's own case. Gaps
/// between hits cost, as do characters before the first hit and, lightly,
/// the candidate's length; each of these costs stops growing at a limit.
///
/// A few things are settled before any of that, each outranking all that
/// come after it. A candidate equal to the needle ignoring case outscores
/// every candidate that is not. Next comes one equal to the needle with its
/// whitespace left out. Then every candidate that starts with the needle,
/// as typed or with its whitespace left out. Then one that would be the
/// needle or start with it but for the amount of whitespace. Only among
/// the rest, and within each of those groups, do hits and gaps decide.
///
/// Case is ignored by comparing lowercase forms. The capital I with a dot
/// (U+0130) has a lowercase of two characters, an `i` and a combining dot,
/// and stands for both: it is one hit however many of them the needle used.
///
/// An empty or all-whitespace needle matches anything with score 0 and no
/// hits.
pub fn fuzzy(needle: &str, candidate: &str) -> Option<Match> {
    Needle::new(needle).find(candidate, &mut Scratch::default(), TABLE_MOST)
}

/// Matches every candidate and returns the ones that match as (index into
/// the input, its match), best first, with ties in input order. An empty or
/// all-whitespace needle returns every candidate in input order.
pub fn rank<'a, I: IntoIterator<Item = &'a str>>(needle: &str, candidates: I) -> Vec<(usize, Match)> {
    let needle = Needle::new(needle);
    let mut scratch = Scratch::default();
    let mut found: Vec<(usize, Match)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| Some((index, needle.find(candidate, &mut scratch, TABLE_MOST)?)))
        .collect();
    found.sort_by(|a, b| b.1.score.cmp(&a.1.score));
    found
}

/// The form a character is compared in when case is ignored: its lowercase.
/// That is one character for all but the capital I with a dot, which gives
/// an `i` and a combining dot. The two lowercase sigmas of Greek are one
/// letter here, as they are to a reader.
fn fold(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase().map(|lower| if lower == '\u{03C2}' { '\u{03C3}' } else { lower })
}

/// Hands `each` the characters of `fold(c)` one at a time. Every character
/// of every candidate comes through here, so ASCII, which is nearly all of
/// them, is kept clear of the case tables.
fn fold_into(c: char, mut each: impl FnMut(char)) {
    if c.is_ascii() {
        each(c.to_ascii_lowercase());
    } else {
        fold(c).for_each(each);
    }
}

fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '/' | '\\' | '-' | '_' | '.')
}

/// The folded characters of `text` with each run of whitespace made one
/// space and none at either end.
fn squashed(text: &str) -> impl Iterator<Item = char> + '_ {
    text.split_whitespace()
        .enumerate()
        .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars().flat_map(fold)))
}

fn starts_with(mut long: impl Iterator<Item = char>, mut short: impl Iterator<Item = char>) -> bool {
    short.all(|c| long.next() == Some(c))
}

/// What a gap of `skipped` characters between two hits costs.
fn gap_cost(skipped: usize) -> i64 {
    let more = skipped.saturating_sub(1).min(GAP_MOST as usize) as i64;
    (GAP + GAP_MORE * more).min(GAP_MOST)
}

/// What it costs for the first hit to land on character `col`.
fn lead_cost(col: usize) -> i64 {
    LEAD * col.min(LEAD_REACH) as i64
}

/// A needle made ready to be tried against any number of candidates.
struct Needle<'a> {
    text: &'a str,
    /// Its characters without whitespace. `low` is what gets compared: the
    /// characters of each one's lowercase form, so nearly always one each.
    /// `raw` holds, for each of those, the character as typed.
    raw: Vec<char>,
    low: Vec<char>,
    /// How far apart the tiers of `Needle::tier` sit. No alignment can
    /// score above `HIT_MOST` a character or lose more than `GAP_MOST` a
    /// character and `ONCE_MOST` besides, so a step wider than that whole
    /// range puts every candidate of one tier above every candidate of the
    /// tier below.
    step: i64,
}

/// A candidate laid out the way a needle is: `low` holds the characters of
/// each character's lowercase form, and every column of the table is one
/// entry of it.
#[derive(Default)]
struct Candidate {
    /// For each entry of `low`, the byte offset of the character it came
    /// from, and that character as written.
    offsets: Vec<usize>,
    raw: Vec<char>,
    low: Vec<char>,
}

impl Candidate {
    fn load(&mut self, text: &str) {
        self.offsets.clear();
        self.raw.clear();
        self.low.clear();
        for (offset, c) in text.char_indices() {
            fold_into(c, |folded| {
                self.offsets.push(offset);
                self.raw.push(c);
                self.low.push(folded);
            });
        }
    }
}

/// Working memory, kept between candidates so that ranking a long list does
/// not allocate for each one.
#[derive(Default)]
struct Scratch {
    candidate: Candidate,
    table: Table,
}

/// The table the best alignment is read from.
///
/// Row `i` is needle character `i`. It only has columns `first[i]` to
/// `last[i]`: the leftmost and the rightmost place that character can sit
/// in any alignment at all.
#[derive(Default)]
struct Table {
    first: Vec<usize>,
    last: Vec<usize>,
    /// Scores of the row above and of the row being filled.
    above: Vec<i64>,
    row: Vec<i64>,
    /// When the table is held whole: for every cell of every row but the
    /// top one, the column the hit before it sits in, and where each of
    /// those rows starts in here.
    back: Vec<usize>,
    row_start: Vec<usize>,
    /// When it is not: for every cell of the row above and of the row being
    /// filled, the column its best alignment took through the middle row.
    via_above: Vec<usize>,
    via: Vec<usize>,
}

impl<'a> Needle<'a> {
    fn new(text: &'a str) -> Needle<'a> {
        let mut raw = Vec::new();
        let mut low = Vec::new();
        for c in text.chars().filter(|c| !c.is_whitespace()) {
            for folded in fold(c) {
                raw.push(c);
                low.push(folded);
            }
        }
        let step = (HIT_MOST + GAP_MOST).saturating_mul(low.len() as i64).saturating_add(ONCE_MOST + 1);
        Needle { text, raw, low, step }
    }

    /// Matches one candidate. `most` is the largest table to hold whole.
    fn find(&self, text: &str, scratch: &mut Scratch, most: usize) -> Option<Match> {
        if self.low.is_empty() {
            return Some(Match { score: 0, hits: Vec::new() });
        }
        if !self.is_within(text) {
            return None;
        }
        let Scratch { candidate, table } = scratch;
        candidate.load(text);
        let cols = self.align(candidate, table, most)?;
        let score = self.score(text, candidate, &cols);
        let mut hits: Vec<usize> = cols.iter().filter_map(|&col| candidate.offsets.get(col).copied()).collect();
        // Two columns of one character are one hit.
        hits.dedup();
        Some(Match { score, hits })
    }

    /// Whether the needle is a subsequence of `text`. Most candidates fail
    /// here, before anything is laid out for them.
    fn is_within(&self, text: &str) -> bool {
        let mut found = 0;
        for c in text.chars() {
            fold_into(c, |folded| found += usize::from(self.low.get(found) == Some(&folded)));
            if found == self.low.len() {
                return true;
            }
        }
        found == self.low.len()
    }

    /// What hitting candidate character `col` with needle character `i` is
    /// worth, before anything about the hits around it.
    fn hit(&self, candidate: &Candidate, i: usize, col: usize) -> i64 {
        let place = match col.checked_sub(1).and_then(|before| candidate.raw.get(before)) {
            None => START,
            Some(&before) if is_separator(before) => SEPARATOR,
            Some(_) => 0,
        };
        let case = if self.raw.get(i) == candidate.raw.get(col) { CASE } else { 0 };
        HIT + place + case
    }

    /// The score of one alignment, where `cols[i]` is the candidate
    /// character that needle character `i` hits. This is the definition of
    /// the score; `Needle::align` only looks for the alignment that makes it
    /// largest.
    fn tally(&self, candidate: &Candidate, cols: &[usize]) -> i64 {
        let mut total = 0;
        let mut before = None;
        for (i, &col) in cols.iter().enumerate() {
            total += self.hit(candidate, i, col);
            total += match before {
                None => -lead_cost(col),
                Some(before) if before + 1 == col => RUN,
                Some(before) => -gap_cost(col.saturating_sub(before + 1)),
            };
            before = Some(col);
        }
        total
    }

    /// The whole score of a candidate under one alignment: its tier first,
    /// and within the tier its tally less a point for each character the
    /// needle does not account for.
    fn score(&self, text: &str, candidate: &Candidate, cols: &[usize]) -> i32 {
        let unmatched = candidate.raw.len().saturating_sub(cols.len()).min(LENGTH_MOST) as i64;
        let within = (self.tally(candidate, cols) - unmatched).clamp(0, self.step - 1);
        let tier = self.tier(text, candidate);
        let whole = if self.step <= TIER_ROOM {
            tier * self.step + within
        } else {
            // A needle millions of characters long. Its scores are scaled
            // down to fit, which can tie two candidates of one tier but
            // never lets a tier reach the one above.
            tier * TIER_ROOM + (within as i128 * TIER_ROOM as i128 / self.step as i128) as i64
        };
        whole.clamp(0, i32::MAX as i64) as i32
    }

    /// How nearly the candidate is the needle itself, ignoring case: 4 when
    /// it is the needle as typed; 3 when it is the needle with its
    /// whitespace left out, which is all that was matched; 2 when it starts
    /// with the needle in either of those forms; 1 when it would be the
    /// needle or start with it but for the amount of whitespace; and 0
    /// otherwise.
    fn tier(&self, text: &str, candidate: &Candidate) -> i64 {
        let typed = || self.text.chars().flat_map(fold);
        let whole = || text.chars().flat_map(fold);
        if typed().eq(whole()) {
            4
        } else if candidate.low == self.low {
            3
        } else if starts_with(whole(), typed()) || candidate.low.starts_with(&self.low) {
            2
        } else if starts_with(squashed(text), squashed(self.text)) {
            1
        } else {
            0
        }
    }

    /// The columns of the best alignment: `cols[i]` is where needle
    /// character `i` sits. `None` only if the needle is not a subsequence
    /// after all. `most` is the largest table to hold whole, in cells.
    fn align(&self, candidate: &Candidate, table: &mut Table, most: usize) -> Option<Vec<usize>> {
        let rows = self.low.len();
        let mut cols = vec![0; rows];
        for bounds in [&mut table.first, &mut table.last] {
            bounds.clear();
            bounds.resize(rows, 0);
        }
        self.solve(candidate, table, (0, rows.checked_sub(1)?), (None, None), most, &mut cols)?;
        Some(cols)
    }

    /// Finds the best alignment of needle characters `rows.0..=rows.1` and
    /// writes their columns to `cols`. An end of the stretch that is pinned
    /// must sit on the given column.
    ///
    /// A stretch whose table is small enough is filled and read back by
    /// `Needle::trace`. A larger one is never held whole. One pass down it
    /// finds where its best alignment crosses the middle row, and that
    /// column pins the two halves, each of which is then solved on its own.
    /// The halves have half the rows each and share out the columns between
    /// them, so all the passes together cost about twice the first.
    fn solve(
        &self,
        candidate: &Candidate,
        table: &mut Table,
        rows: (usize, usize),
        pins: (Option<usize>, Option<usize>),
        most: usize,
        cols: &mut [usize],
    ) -> Option<()> {
        let (top, bottom) = rows;
        let cells = self.windows(&candidate.low, table, rows, pins)?;
        if bottom - top < 2 || cells <= most {
            return self.trace(candidate, table, rows, pins.0, cols);
        }
        let middle = top + (bottom - top) / 2;
        let cross = self.crossing(candidate, table, rows, pins.0, middle)?;
        self.solve(candidate, table, (top, middle), (pins.0, Some(cross)), most, cols)?;
        self.solve(candidate, table, (middle, bottom), (Some(cross), pins.1), most, cols)
    }

    /// Sets `first` and `last` for the rows of a stretch: the leftmost and
    /// the rightmost column each character can take in any alignment that
    /// keeps the pinned ends. Returns how many cells that leaves.
    fn windows(
        &self,
        hay: &[char],
        table: &mut Table,
        (top, bottom): (usize, usize),
        (start, end): (Option<usize>, Option<usize>),
    ) -> Option<usize> {
        let Table { first, last, .. } = table;
        let mut col = start.unwrap_or(0);
        for i in top..=bottom {
            let want = self.low.get(i)?;
            col += hay.get(col..)?.iter().position(|c| c == want)?;
            *first.get_mut(i)? = col;
            col += 1;
        }
        let mut stop = end.map_or(hay.len(), |col| col + 1);
        for i in (top..=bottom).rev() {
            let want = self.low.get(i)?;
            stop = hay.get(..stop)?.iter().rposition(|c| c == want)?;
            *last.get_mut(i)? = stop;
        }
        if let Some(col) = start {
            if first[top] != col {
                return None;
            }
            last[top] = col;
        }
        if let Some(col) = end {
            if last[bottom] != col {
                return None;
            }
            first[bottom] = col;
        }
        let mut cells = 0usize;
        for i in top..=bottom {
            cells = cells.saturating_add(last[i].checked_sub(first[i])? + 1);
        }
        Some(cells)
    }

    /// Fills `above` with the scores of the top row of a stretch. A pinned
    /// top is one cell worth nothing, since whatever led to it is the same
    /// for every alignment through it. Otherwise the stretch starts the
    /// needle, and each column is worth its hit less its lead.
    fn open(&self, candidate: &Candidate, table: &mut Table, top: usize, start: Option<usize>) {
        table.above.clear();
        if start.is_some() {
            table.above.push(0);
            return;
        }
        for col in table.first[top]..=table.last[top] {
            let fits = candidate.low.get(col) == self.low.get(top);
            table.above.push(if fits { self.hit(candidate, top, col) - lead_cost(col) } else { NONE });
        }
    }

    /// Fills `row` for needle character `i` from `above`, the row of the
    /// character before it, and notes in `from` the column that character
    /// sits in on the best way to each cell. `bounds` is the first and last
    /// column of the row above, then of this row.
    ///
    /// Cell (i, col) is the best tally of needle characters up to `i` with
    /// character `i` on `col`. It is reached from the row above in one of
    /// three ways: straight on from the hit just before it, across a gap
    /// that is still paying by the character, or across a gap so long it
    /// costs the flat most. The best of each is carried along the row as it
    /// is filled, so a cell costs constant time.
    fn step(&self, candidate: &Candidate, i: usize, bounds: [usize; 4], above: &[i64], row: &mut Vec<i64>, from: &mut [usize]) {
        let (needle, hay) = (&self.low, &candidate.low);
        let [above_lo, above_hi, lo, hi] = bounds;
        row.clear();
        row.resize(hi - lo + 1, NONE);
        // The best way to arrive across a gap, from a hit at least two
        // columns back: `near` while the gap pays by the character, `far`
        // for the flat most.
        let (mut near, mut near_col) = (NONE, 0);
        let (mut far, mut far_col) = (NONE, 0);
        for col in above_lo + 1..=hi {
            if near > NONE {
                near -= GAP_MORE;
            }
            if col >= above_lo + 2 && col - 2 <= above_hi {
                let score = above[col - 2 - above_lo];
                if score > NONE {
                    if score - GAP > near {
                        (near, near_col) = (score - GAP, col - 2);
                    }
                    if score > far {
                        (far, far_col) = (score, col - 2);
                    }
                }
            }
            if col < lo || hay[col] != needle[i] {
                continue;
            }
            let (mut best, mut best_col) = (NONE, 0);
            if col - 1 <= above_hi && above[col - 1 - above_lo] > NONE {
                (best, best_col) = (above[col - 1 - above_lo] + RUN, col - 1);
            }
            if near > best {
                (best, best_col) = (near, near_col);
            }
            if far > NONE && far - GAP_MOST > best {
                (best, best_col) = (far - GAP_MOST, far_col);
            }
            if best > NONE {
                row[col - lo] = best + self.hit(candidate, i, col);
                from[col - lo] = best_col;
            }
        }
    }

    /// Solves a stretch by filling its whole table, keeping for every cell
    /// below the top row the column it was reached from, and reading the
    /// best alignment back from the bottom row.
    fn trace(&self, candidate: &Candidate, table: &mut Table, (top, bottom): (usize, usize), start: Option<usize>, cols: &mut [usize]) -> Option<()> {
        self.open(candidate, table, top, start);
        let Table { first, last, above, row, back, row_start, .. } = table;
        row_start.clear();
        let mut cells = 0;
        for i in top + 1..=bottom {
            row_start.push(cells);
            cells += last[i] - first[i] + 1;
        }
        back.clear();
        back.resize(cells, 0);
        for i in top + 1..=bottom {
            let at = row_start[i - top - 1];
            let from = back.get_mut(at..at + last[i] - first[i] + 1)?;
            self.step(candidate, i, [first[i - 1], last[i - 1], first[i], last[i]], above, row, from);
            std::mem::swap(above, row);
        }
        let mut col = best_of(above, first[bottom])?;
        *cols.get_mut(bottom)? = col;
        for i in (top + 1..=bottom).rev() {
            col = *back.get(row_start[i - top - 1] + col.checked_sub(first[i])?)?;
            *cols.get_mut(i - 1)? = col;
        }
        Some(())
    }

    /// Where the best alignment of a stretch puts needle character
    /// `middle`, found without keeping the table. Each row carries, beside
    /// its scores, the column that the best way to each of its cells took
    /// through the middle row, so the answer can be read off the bottom row.
    fn crossing(&self, candidate: &Candidate, table: &mut Table, (top, bottom): (usize, usize), start: Option<usize>, middle: usize) -> Option<usize> {
        self.open(candidate, table, top, start);
        let Table { first, last, above, row, via_above, via, .. } = table;
        for i in top + 1..=bottom {
            via.clear();
            via.resize(last[i] - first[i] + 1, 0);
            self.step(candidate, i, [first[i - 1], last[i - 1], first[i], last[i]], above, row, via);
            // Just below the middle row, the column a cell was reached from
            // is the crossing itself. Further down it is looked up there.
            if i > middle + 1 {
                for (cell, &score) in via.iter_mut().zip(row.iter()) {
                    if score > NONE {
                        *cell = *via_above.get(cell.checked_sub(first[i - 1])?)?;
                    }
                }
            }
            std::mem::swap(above, row);
            std::mem::swap(via_above, via);
        }
        let col = best_of(above, first[bottom])?;
        via_above.get(col - first[bottom]).copied()
    }
}

/// The column of the best score in a row whose first cell is column `lo`,
/// the leftmost of equals. `None` when no alignment reaches the row.
fn best_of(scores: &[i64], lo: usize) -> Option<usize> {
    let mut best = NONE;
    let mut found = None;
    for (offset, &score) in scores.iter().enumerate() {
        if score > best {
            (best, found) = (score, Some(lo + offset));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u32);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as usize % n.max(1)
        }

        fn text(&mut self, alphabet: &[char], most: usize) -> String {
            (0..self.below(most + 1)).map(|_| alphabet[self.below(alphabet.len())]).collect()
        }
    }

    fn score(needle: &str, candidate: &str) -> i32 {
        fuzzy(needle, candidate).map(|m| m.score).unwrap_or_else(|| panic!("{needle:?} should match {candidate:?}"))
    }

    fn hits(needle: &str, candidate: &str) -> Vec<usize> {
        fuzzy(needle, candidate).map(|m| m.hits).unwrap_or_default()
    }

    /// The folded form of a character whose lowercase is one character.
    fn low(c: char) -> char {
        fold(c).next().unwrap_or(c)
    }

    fn folded(text: &str) -> Vec<char> {
        text.chars().flat_map(fold).collect()
    }

    /// The best score over every possible alignment, found the slow way.
    fn brute(needle: &Needle, text: &str, candidate: &Candidate) -> Option<i32> {
        fn walk(needle: &Needle, text: &str, candidate: &Candidate, cols: &mut Vec<usize>, best: &mut Option<i32>) {
            if cols.len() == needle.low.len() {
                let score = needle.score(text, candidate, cols);
                *best = Some(best.map_or(score, |b| b.max(score)));
                return;
            }
            let from = cols.last().map_or(0, |&col| col + 1);
            for col in from..candidate.low.len() {
                if candidate.low[col] == needle.low[cols.len()] {
                    cols.push(col);
                    walk(needle, text, candidate, cols, best);
                    cols.pop();
                }
            }
        }
        let mut best = None;
        walk(needle, text, candidate, &mut Vec::new(), &mut best);
        best
    }

    #[test]
    fn matches_a_subsequence_whatever_its_case() {
        assert_eq!(hits("abc", "abc"), [0, 1, 2]);
        assert_eq!(hits("ABC", "xaxbxc"), [1, 3, 5]);
        assert_eq!(hits("a c", "abc"), [0, 2]);
        assert_eq!(hits("\u{00E9}t", "\u{00C9}t\u{00E9}: \u{00E9}t"), [0, 2]);
        assert_eq!(hits("\u{4E00}b", "a\u{4E00}\u{4E8C}b"), [1, 7]);
        assert_eq!(fuzzy("abc", "acb"), None);
        assert_eq!(fuzzy("abc", "ab"), None);
        assert_eq!(fuzzy("a", ""), None);
        assert_eq!(fuzzy("aa", "a"), None);
    }

    #[test]
    fn an_empty_needle_matches_everything_with_nothing() {
        let nothing = Some(Match { score: 0, hits: Vec::new() });
        assert_eq!(fuzzy("", "anything"), nothing);
        assert_eq!(fuzzy(" \t ", "anything"), nothing);
        assert_eq!(fuzzy("", ""), nothing);
    }

    #[test]
    fn finds_the_best_alignment_not_the_leftmost() {
        // Leftmost would take the "a" of "an" and jump to "pp".
        assert_eq!(hits("app", "an apple"), [3, 4, 5]);
        // The word start wins over the earlier letters inside a word.
        assert_eq!(hits("cat", "concatenate/cat"), [12, 13, 14]);
        assert_eq!(hits("nt", "notes/next/text"), [0, 11]);
        assert_eq!(hits("ca", "essays/common app"), [7, 14]);
        // A run is kept together even when its first letter appears earlier,
        // and even when the scattered letters sit on better places.
        assert_eq!(hits("ab", "a xab"), [3, 4]);
        assert_eq!(hits("ab", "a-b ab"), [4, 5]);
        assert_eq!(hits("abc", "a_b_c and far along abc"), [20, 21, 22]);
        // Initials are found across long words.
        assert_eq!(hits("tfm", "Toggle focus mode"), [0, 7, 13]);
        // Where two places are otherwise as good, the needle's case decides.
        assert_eq!(hits("B", "twelve chars b B"), [15]);
        assert_eq!(hits("b", "twelve chars b B"), [13]);
    }

    #[test]
    fn scores_reward_what_the_contract_names() {
        // The very start, then a separator, then anywhere.
        assert!(score("d", "draft") > score("d", "a draft"));
        assert!(score("d", "a draft") > score("d", "adraft"));
        for separator in [' ', '/', '\\', '-', '_', '.'] {
            assert!(score("d", &format!("ab{separator}d")) > score("d", "abxd"), "{separator:?}");
        }
        // Runs of consecutive hits.
        assert!(score("ft", "x draft") > score("ft", "x draf t"));
        assert!(score("ft", "x draft") > score("ft", "x fat"));
        // Exact case.
        assert!(score("Draft", "x Draft") > score("Draft", "x draft"));
        assert!(score("draft", "x draft") > score("draft", "x DRAFT"));
        // Gaps cost more the longer they are.
        assert!(score("dt", "xdat") > score("dt", "xdaat"));
        assert!(score("dt", "xdaat") > score("dt", "xdaaat"));
        // Hits deep into the candidate.
        assert!(score("d", "abd") > score("d", "abcd"));
        // The candidate's length, lightly.
        assert!(score("dra", "x draft") > score("dra", "x drafts"));
        assert!(score("draft", "x drafts and a great deal more besides") > score("draft", "x dr a f t"));
    }

    #[test]
    fn an_equal_candidate_beats_everything_and_a_prefix_beats_scatter() {
        assert!(score("draft", "DRAFT") > score("draft", "draft two"));
        assert!(score("draft", "DRAFT") > score("draft", "drafts"));
        assert!(score("common app", "Common App") > score("common app", "commonapp"));
        assert!(score("common app", "Common App") > score("common app", "common  app"));
        assert!(score("draft", "DRAFT of the long essay about nothing at all") > score("draft", "d-r-a-f-t"));
        assert!(score("draft", "DRAFT of the long essay about nothing at all") > score("draft", "a draft"));
        assert!(score("common app", "Common application essay") > score("common app", "a common app"));
    }

    #[test]
    fn rank_orders_best_first_and_keeps_input_order_on_ties() {
        let names = ["a draft", "draft", "nothing", "drafts", "d r a f t", "draft"];
        let ranked = rank("draft", names);
        assert_eq!(ranked.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [1, 5, 3, 0, 4]);
        assert_eq!(ranked[0].1, fuzzy("draft", "draft").unwrap());

        let all = rank("  ", names);
        assert_eq!(all.len(), names.len());
        for (at, (index, found)) in all.iter().enumerate() {
            assert_eq!((*index, found), (at, &Match { score: 0, hits: Vec::new() }));
        }
        assert_eq!(rank("x", Vec::<&str>::new()), []);
        let owned = ["one".to_string(), "two".to_string()];
        assert_eq!(rank("o", owned.iter().map(String::as_str)).len(), 2);
    }

    #[test]
    fn random_matches_are_the_best_there_is() {
        let alphabet = ['a', 'b', 'A', 'B', ' ', '-', '/', '\u{00E9}', '\u{00C9}', '\u{4E00}'];
        let mut rng = Rng(0x2545_F491);
        let mut scratch = Scratch::default();
        let mut matched = 0;
        for _ in 0..20_000 {
            let needle_text = rng.text(&alphabet, 4);
            let text = rng.text(&alphabet, 11);
            let needle = Needle::new(&needle_text);
            let mut candidate = Candidate::default();
            candidate.load(&text);
            let found = fuzzy(&needle_text, &text);
            if needle.low.is_empty() {
                assert_eq!(found, Some(Match { score: 0, hits: Vec::new() }));
                continue;
            }
            let best = brute(&needle, &text, &candidate);
            assert_eq!(found.as_ref().map(|m| m.score), best, "{needle_text:?} in {text:?}");
            // However little of the table may be held at once, the answer
            // is as good.
            for most in [0, 5, 12] {
                let halved = needle.find(&text, &mut scratch, most);
                assert_eq!(halved.map(|m| m.score), best, "{needle_text:?} in {text:?} holding {most}");
            }
            let Some(found) = found else { continue };
            matched += 1;

            // The hits spell the needle, in order, on character boundaries.
            assert_eq!(found.hits.len(), needle.low.len());
            assert!(found.hits.windows(2).all(|pair| pair[0] < pair[1]), "{needle_text:?} in {text:?}");
            let spelled: Vec<char> = found.hits.iter().map(|&at| low(text[at..].chars().next().unwrap())).collect();
            assert_eq!(spelled, needle.low, "{needle_text:?} in {text:?}");

            // And the score is the score of exactly those hits.
            let cols: Vec<usize> = found.hits.iter().map(|at| candidate.offsets.binary_search(at).unwrap()).collect();
            assert_eq!(needle.score(&text, &candidate, &cols), found.score, "{needle_text:?} in {text:?}");
        }
        assert!(matched > 2000, "only {matched} matched");
    }

    #[test]
    fn random_equal_candidates_outscore_all_others() {
        let alphabet = ['a', 'b', 'c', 'A', 'B', ' ', '-', '.', '\u{00E9}', '\u{00C9}'];
        let flip = |c: char, up: bool| if up { c.to_uppercase().next().unwrap_or(c) } else { low(c) };
        let mut rng = Rng(0x0BAD_5EED);
        for _ in 0..4000 {
            let needle = rng.text(&alphabet, 7);
            if needle.trim().is_empty() {
                continue;
            }
            let equal: String = needle.chars().map(|c| flip(c, rng.below(2) == 0)).collect();
            let equal_score = score(&needle, &equal);
            for _ in 0..12 {
                // Near misses are the dangerous ones: the needle with a
                // little added, taken away or changed.
                let mut other: Vec<char> = needle.chars().collect();
                for _ in 0..1 + rng.below(3) {
                    let at = rng.below(other.len() + 1);
                    let c = alphabet[rng.below(alphabet.len())];
                    match rng.below(3) {
                        0 => other.insert(at, c),
                        1 if at < other.len() => drop(other.remove(at)),
                        _ if at < other.len() => other[at] = c,
                        _ => other.push(c),
                    }
                }
                let other: String = other.into_iter().collect();
                if folded(&other) == folded(&needle) {
                    continue;
                }
                if let Some(found) = fuzzy(&needle, &other) {
                    assert!(equal_score > found.score, "{needle:?}: {equal:?} against {other:?}");
                }
            }
        }
    }

    #[test]
    fn random_prefix_candidates_outscore_scattered_ones() {
        let letters = ['a', 'b', 'c', 'd', 'A', 'B', '\u{00E9}'];
        let filler = ['x', 'y', ' ', '-', '/', '.', '_', 'X'];
        let mut rng = Rng(0x1357_9BDF);
        for _ in 0..4000 {
            let needle = rng.text(&letters, 5) + "ab";
            let tail = rng.text(&filler, 40);
            let prefixed: String = needle.chars().map(|c| if rng.below(2) == 0 { low(c) } else { c }).chain(tail.chars()).collect();

            // The same letters with something between at least two of them,
            // dressed up to score as well as scatter can: in the needle's
            // own case, with letters sitting just after separators.
            let mut scattered = String::new();
            let split = 1 + rng.below(needle.chars().count() - 1);
            for (i, c) in needle.chars().enumerate() {
                if i == split || (i > 0 && rng.below(3) == 0) {
                    scattered.push(filler[2 + rng.below(5)]);
                }
                scattered.push(c);
            }
            assert!(score(&needle, &prefixed) > score(&needle, &scattered), "{needle:?}: {prefixed:?} against {scattered:?}");
        }
    }

    #[test]
    fn random_ranks_are_sorted_and_complete() {
        let alphabet = ['a', 'b', 'c', ' ', '/', 'A'];
        let mut rng = Rng(0x7777_1111);
        for _ in 0..500 {
            let needle = rng.text(&alphabet, 3);
            let pool: Vec<String> = (0..rng.below(30)).map(|_| rng.text(&alphabet, 9)).collect();
            let ranked = rank(&needle, pool.iter().map(String::as_str));
            for pair in ranked.windows(2) {
                let ((a, x), (b, y)) = (&pair[0], &pair[1]);
                assert!(x.score > y.score || (x.score == y.score && a < b), "{needle:?} {pool:?}");
            }
            for (index, text) in pool.iter().enumerate() {
                let found = fuzzy(&needle, text);
                let listed = ranked.iter().find(|(i, _)| *i == index).map(|(_, m)| m.clone());
                assert_eq!(found, listed, "{needle:?} {text:?}");
            }
        }
    }

    #[test]
    fn random_unicode_never_breaks_a_character() {
        const COMMON: &[char] = &['a', 'A', ' ', '/', '\\', '-', '_', '.', '\u{00E9}', '\u{00C9}', '\u{0130}', '\u{03A3}', '\u{03C2}', '\u{4E00}', '\t'];
        let mut rng = Rng(0xFEED_F00D);
        let mut soup = |most: usize| -> String {
            (0..rng.below(most + 1))
                .map(|_| match rng.below(6) {
                    0..=2 => COMMON[rng.below(COMMON.len())],
                    3 => char::from_u32(rng.below(0x800) as u32).unwrap_or('a'),
                    4 => char::from_u32(0x800 + rng.below(0xF800) as u32).unwrap_or('\u{4E00}'),
                    _ => char::from_u32(0x1_0000 + rng.below(0x10_0000) as u32).unwrap_or('\u{1F600}'),
                })
                .collect()
        };
        let mut matched = 0;
        for _ in 0..20_000 {
            let (needle, text) = (soup(4), soup(30));
            let wanted: Vec<char> = needle.chars().filter(|c| !c.is_whitespace()).flat_map(fold).collect();
            let Some(found) = fuzzy(&needle, &text) else { continue };
            matched += usize::from(!wanted.is_empty());
            assert!(found.hits.windows(2).all(|pair| pair[0] < pair[1]), "{needle:?} in {text:?}");
            // The hits spell the needle: exactly, unless a capital dotted I
            // was hit for only one of the two characters it stands for.
            let spelled: Vec<char> = found.hits.iter().filter_map(|&at| text.get(at..)?.chars().next()).flat_map(fold).collect();
            let mut rest = spelled.iter();
            assert!(wanted.iter().all(|want| rest.any(|c| c == want)), "{needle:?} in {text:?}");
            if !text.contains('\u{0130}') {
                assert_eq!(spelled, wanted, "{needle:?} in {text:?}");
            }
            // The empty candidate only matches the empty needle.
            let kept: &[usize] = if wanted.is_empty() { &[0, 1, 2] } else { &[0, 2] };
            let expected: Vec<(usize, Match)> = kept.iter().map(|&index| (index, found.clone())).collect();
            assert_eq!(rank(&needle, [text.as_str(), "", text.as_str()]), expected, "{needle:?} in {text:?}");
        }
        assert!(matched > 1000, "{matched}");
    }

    /// A table too large to hold is worked through in halves, and comes to
    /// the same score as the whole table does.
    #[test]
    fn random_halved_tables_score_as_whole_ones_do() {
        let alphabet = ['a', 'b', 'c', 'A', 'x', 'x', ' ', '-', 'i', '\u{0130}'];
        let mut rng = Rng(0x0A11_6E55);
        let mut scratch = Scratch::default();
        let mut halved = 0;
        for _ in 0..3000 {
            let needle_text = rng.text(&alphabet, 24);
            let text = rng.text(&alphabet, 160);
            let needle = Needle::new(&needle_text);
            let Some(whole) = needle.find(&text, &mut scratch, usize::MAX) else { continue };
            for most in [0, 40, 600] {
                let found = needle.find(&text, &mut scratch, most);
                assert_eq!(found.as_ref().map(|m| m.score), Some(whole.score), "{needle_text:?} in {text:?} holding {most}");
                assert!(found.is_some_and(|m| m.hits.windows(2).all(|pair| pair[0] < pair[1])), "{needle_text:?} in {text:?}");
            }
            halved += usize::from(needle.low.len() > 2);
        }
        assert!(halved > 1000, "{halved}");
    }

    /// Past the size of table that is held whole, the best alignment is
    /// still the one found, and nothing overflows or stalls on the way.
    #[test]
    fn a_table_too_large_to_hold_is_still_solved() {
        // The needle's letters scattered at the front, and far along the
        // needle itself at the start of a word.
        let needle = "ab".repeat(20);
        let scattered = "axbx".repeat(20);
        let candidate = format!("{scattered}{} {needle}", "x".repeat(70_000));
        let run = scattered.len() + 70_000 + 1;
        assert!(needle.len() * candidate.len() > TABLE_MOST);
        assert_eq!(hits(&needle, &candidate), (run..run + needle.len()).collect::<Vec<_>>());

        // Many rows: halves of halves of halves.
        let needle_text = "ab".repeat(400);
        let candidate = "a-b".repeat(1500);
        let needle = Needle::new(&needle_text);
        let mut scratch = Scratch::default();
        let found = needle.find(&candidate, &mut scratch, TABLE_MOST).unwrap();
        assert_eq!(found.hits.len(), 800);
        assert!(found.hits.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(Some(found.score), needle.find(&candidate, &mut scratch, usize::MAX).map(|m| m.score));

        let same = "x".repeat(200_000);
        assert_eq!(fuzzy(&same, &same).unwrap().hits.len(), 200_000);
        assert_eq!(fuzzy(&same, &same[1..]), None);
    }

    #[test]
    fn the_capital_dotted_i_is_an_i_and_a_dot() {
        // "I" is "i" ignoring case; the dotted capital is not.
        assert!(score("i", "I") > score("i", "\u{0130}"));
        assert!(score("istanbul", "ISTANBUL") > score("istanbul", "\u{0130}STANBUL"));
        // It still starts with an i, and is one hit however much of it the
        // needle used.
        assert_eq!(hits("istanbul", "\u{0130}stanbul"), [0, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(hits("i\u{0307}", "\u{0130}"), [0]);
        assert_eq!(hits("\u{0130}s", "i\u{0307}s"), [0, 1, 3]);
        // Its equal is an i with a dot, not a bare i.
        assert!(score("\u{0130}", "i\u{0307}") > score("\u{0130}", "i\u{0307}x"));
        assert!(score("\u{0130}", "\u{0130}") > score("\u{0130}", "i\u{0307}"));
        assert_eq!(fuzzy("\u{0130}", "i"), None);
        assert_eq!(rank("i", ["\u{0130}", "I"]).iter().map(|(index, _)| *index).collect::<Vec<_>>(), [1, 0]);
    }

    /// The capital dotted I takes two columns of the table, which every
    /// part of the scoring has to count the same way.
    #[test]
    fn random_dotted_matches_are_the_best_there_is() {
        let alphabet = ['i', 'I', '\u{0130}', '\u{0307}', '\u{0131}', 'a', ' ', '-'];
        let mut rng = Rng(0x1570_D07D);
        let mut scratch = Scratch::default();
        let (mut matched, mut merged) = (0, 0);
        for _ in 0..20_000 {
            let needle_text = rng.text(&alphabet, 4);
            let text = rng.text(&alphabet, 9);
            let needle = Needle::new(&needle_text);
            if needle.low.is_empty() {
                continue;
            }
            let mut candidate = Candidate::default();
            candidate.load(&text);
            let best = brute(&needle, &text, &candidate);
            let found = fuzzy(&needle_text, &text);
            assert_eq!(found.as_ref().map(|m| m.score), best, "{needle_text:?} in {text:?}");
            assert_eq!(needle.find(&text, &mut scratch, 0).map(|m| m.score), best, "{needle_text:?} in {text:?}");
            let Some(found) = found else { continue };
            matched += 1;
            merged += usize::from(found.hits.len() < needle.low.len());
            assert!(found.hits.len() <= needle.low.len() && !found.hits.is_empty(), "{needle_text:?} in {text:?}");
            assert!(found.hits.windows(2).all(|pair| pair[0] < pair[1]), "{needle_text:?} in {text:?}");
            assert!(found.hits.iter().all(|&at| text.is_char_boundary(at) && at < text.len()), "{needle_text:?} in {text:?}");
        }
        assert!(matched > 2000 && merged > 100, "{matched} {merged}");
    }

    #[test]
    fn a_candidate_that_starts_with_the_needle_beats_one_that_only_nearly_does() {
        // Starting with the needle as typed, or with its letters.
        for prefix in ["a bc", "a b and a tail that goes on for a good while yet", "abc"] {
            for other in ["a      b", " a b", "a  b c", "a-b", "x a b"] {
                assert!(score("a b", prefix) > score("a b", other), "{prefix:?} against {other:?}");
            }
        }
        // The same but for the amount of whitespace is still worth more
        // than scatter.
        for other in ["a-b", "x a b", "a x b"] {
            assert!(score("a b", "a      b") > score("a b", other), "{other:?}");
            assert!(score("a b", "  a   b c") > score("a b", other), "{other:?}");
        }
        // Whitespace in the needle is not matched, so a candidate that is
        // the rest of it is the next best thing to the needle as typed,
        // whatever its case and however well another one starts.
        assert!(score("DRAFT ", "draft") > score("DRAFT ", "DRAFT two"));
        assert!(score("DRAFT ", "DRAFT two") > score("DRAFT ", "a DRAFT"));
        assert!(score("common app", "Common App") > score("common app", "commonapp"));
        assert!(score("common app", "CommonApp") > score("common app", "common app essay"));
        assert!(score("common app", "common app essay") > score("common app", "common  app"));
        assert!(score("common app", "common  app") > score("common app", "common-app"));
    }

    /// A needle of millions of characters has scores too large for an `i32`.
    /// They are scaled down, and every tier still beats the one below it.
    #[test]
    fn tiers_stay_apart_when_scores_are_scaled_down() {
        let tiers = ["DR AFT", "draft", "drafts", "dr  aft", "d-r-a-f-t"];
        let mut scratch = Scratch::default();
        for step in [None, Some(TIER_ROOM + 1), Some(i64::MAX / 8), Some(i64::MAX)] {
            let mut needle = Needle::new("dr aft");
            needle.step = step.unwrap_or(needle.step);
            let scores: Vec<i32> = tiers.iter().map(|text| needle.find(text, &mut scratch, TABLE_MOST).map_or(-1, |m| m.score)).collect();
            assert!(scores.windows(2).all(|pair| pair[0] > pair[1]) && scores[4] >= 0, "{step:?} {scores:?}");
        }
    }
}
