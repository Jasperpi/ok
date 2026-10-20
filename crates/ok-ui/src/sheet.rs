//! The sheet: the text itself, as DESIGN.md's "The sheet" has it.
//!
//! Markup is never hidden and never moves. Every cell of text is a
//! character that is in the file; marks are drawn at half strength and the
//! words they govern take the attribute. Inverse video means the cursor, a
//! selection, a hit, the link the cursor is in, and TK.
//!
//! The one thing on the paper that is not in the file is the rule of a
//! word limit: a row of equals signs and the limit, set above the row the
//! first word past the limit is on. Rows here are rows of the paper, which
//! count that one (see [`crate::page::Page::paper_row`]).

use ok_gfx::clusters;
use ok_text::links::links_in;
use ok_text::markup::{self, LineKind, Style};
use ok_text::wrap::Row;
use ok_text::{is_combining, str_width, Pos, Span};

use crate::canvas::{Canvas, Ink};
use crate::look::{Strength, Surface};
use crate::machine;
use crate::page::tks;
use crate::scene::Scene;

/// How one line is struck: an ink for every stretch of it, and the
/// stretches that are in inverse video on top of that.
struct Struck {
    /// Each stretch as the byte it ends before and its ink, in order.
    spans: Vec<(usize, Ink)>,
    inverse: Vec<(usize, usize)>,
    /// The ink of the line's own words where nothing else applies.
    words: Ink,
    /// The ink of markup characters, and of the mark hanging in the gutter.
    marks: Ink,
    /// The byte from which the line is past the word limit, if any of it
    /// is: from there on nothing is stronger than half.
    past: Option<usize>,
}

impl Struck {
    fn of(s: &Scene, line: usize, own: (usize, usize), selection: Option<Span>) -> Struck {
        let text = s.page.doc().line(line);
        let kind = s.page.kind(line);
        let fenced = matches!(kind, LineKind::Code | LineKind::Fence);
        let mine = own.0 <= line && line <= own.1;
        let strength = s.flow.text(line, mine, Strength::INK, s.now_ms);
        let words = Ink { strength, double: kind == LineKind::Heading(2), under: false, inverse: false, big: s.page.layout().shape(line).scale == 2 };
        let marks = Ink { strength: strength.min(Strength::HALF), double: false, ..words };
        let wikis = if !fenced && text.contains("[[") { links_in(text) } else { Vec::new() };
        let past = match s.page.over() {
            Some(over) if line > over.line => Some(0),
            Some(over) if line == over.line => Some(over.col),
            _ => None,
        };

        let spans = markup::pieces(text, kind)
            .into_iter()
            .map(|piece| {
                let ink = match piece.style {
                    Style::Plain | Style::Code | Style::Url => words,
                    Style::Syntax => marks,
                    Style::Strong => Ink { double: true, ..words },
                    Style::Emph => Ink { under: true, ..words },
                    Style::StrongEmph => Ink { double: true, under: true, ..words },
                    Style::Link => {
                        // A link to a text not yet started has its name at
                        // half strength, like its brackets.
                        let wiki = wikis.iter().find(|wiki| wiki.start <= piece.start && piece.end <= wiki.end);
                        match wiki {
                            Some(wiki) if !s.session.exists(wiki.target) => Ink { strength: marks.strength, ..words },
                            _ => words,
                        }
                    }
                };
                (piece.end, ink)
            })
            .collect();

        let mut inverse = Vec::new();
        let head = s.page.cursor.head;
        match selection {
            Some(selected) => {
                if selected.start.line <= line && line <= selected.end.line {
                    let from = if selected.start.line == line { selected.start.col } else { 0 };
                    let to = if selected.end.line == line { selected.end.col } else { text.len() };
                    inverse.push((from, to));
                }
            }
            // The link the cursor is in is inverse from bracket to bracket.
            None if head.line == line => inverse.extend(wikis.iter().find(|wiki| wiki.start <= head.col && head.col < wiki.end).map(|wiki| (wiki.start, wiki.end))),
            None => {}
        }
        if !fenced && text.contains("TK") {
            inverse.extend(tks(text));
        }
        inverse.extend(s.line.hits().iter().filter(|hit| hit.start.line == line && hit.end.line == line).map(|hit| (hit.start.col, hit.end.col)));
        Struck { spans, inverse, words, marks, past }
    }

    fn inverted(&self, at: usize) -> bool {
        self.inverse.iter().any(|&(from, to)| from <= at && at < to)
    }

    /// The ink of the character starting at byte `at`. Inverse video is one
    /// solid block: a mark inside it is as strong as the words round it,
    /// and so is a word past the limit.
    fn ink(&self, at: usize) -> Ink {
        let found = self.spans.partition_point(|&(end, _)| end <= at);
        let ink = self.spans.get(found).map_or(self.words, |&(_, ink)| ink);
        if self.inverted(at) {
            Ink { strength: self.words.strength, inverse: true, ..ink }
        } else if self.past.is_some_and(|from| at >= from) {
            Ink { strength: ink.strength.min(Strength::HALF), ..ink }
        } else {
            ink
        }
    }

    /// The ink of the mark in the gutter, which is single size whatever the
    /// line is.
    fn hung(&self) -> Ink {
        let strength = if self.inverted(0) { self.words.strength } else { self.marks.strength };
        Ink { big: false, strength, ..self.marks }.inverse(self.inverted(0))
    }
}

/// Draws the text of one row from column `col` on and returns the column
/// after its last cell. Runs of one ink go down together. Whitespace may
/// hang off the end of a row without limit, so the walk stops at the edge
/// of the sheet: a megabyte of spaces costs no more than a row of words.
fn row_text(c: &mut Canvas, struck: &Struck, text: &str, row: &Row, col: i32, at_row: i32) -> i32 {
    let Some(shown) = text.get(row.start..row.end) else { return col };
    let edge = c.grid().sheet.last;
    let size = struck.words.size();
    let mut col = col;
    let mut reach = col;
    let mut run: Option<(usize, Ink)> = None;
    let mut at = row.start;
    for piece in clusters(shown) {
        if reach > edge {
            break;
        }
        let ink = struck.ink(at);
        match run {
            Some((_, running)) if running == ink => {}
            Some((from, running)) => {
                col += c.text(Surface::Sheet, col, at_row, text.get(from..at).unwrap_or(""), running);
                run = Some((at, ink));
            }
            None => run = Some((at, ink)),
        }
        reach = reach.saturating_add(str_width(piece).min(8) as i32 * size);
        at += piece.len();
    }
    if let Some((from, running)) = run {
        col += c.text(Surface::Sheet, col, at_row, text.get(from..at).unwrap_or(""), running);
    }
    col
}

/// The rule of a word limit on one row of the sheet: equals signs across
/// the measure, faint as the leader dots of the contents are, and the
/// limit at its right end at half strength, as a count is. Nothing here is
/// in the file, so nothing here is in ink.
fn limit_rule(c: &mut Canvas, limit: usize, at_row: i32) {
    let text = c.grid().text;
    let label = machine::words(limit);
    let room = text.width() - str_width(&label) as i32 - 1;
    if room >= 3 {
        c.text(Surface::Sheet, text.first, at_row, &"=".repeat(room as usize), Ink::FAINT);
    }
    c.text_ending(Surface::Sheet, text.last, at_row, &label, Ink::HALF);
}

/// What a pass over the rows draws.
#[derive(Clone, Copy)]
struct Pass {
    /// Rows of the paper to draw, first and one past the last.
    from: i64,
    to: i64,
    /// Rows of the paper to leave out: the typing row while the paper
    /// rolls, which is drawn afterwards where it rests.
    except: Option<(i64, i64)>,
    /// Draw a double-size row only when both its grid rows are wanted. At
    /// rest half a heading at the edge of the sheet is left off rather than
    /// shown as half-letters; while the paper rolls everything is cut
    /// wherever the edge falls.
    whole: bool,
}

/// Draws the rows one pass asks for. `own` is the first and last line of
/// the paragraph the cursor is in.
fn rows(c: &mut Canvas, s: &Scene, pass: Pass, own: (usize, usize), selection: Option<Span>) {
    let grid = c.grid();
    let (doc, layout) = (s.page.doc(), s.page.layout());
    let top = s.view.top();
    if pass.to <= 0 || pass.from >= pass.to {
        return;
    }
    let rule = s.page.rule();
    let paper = |text_row: usize| text_row as i64 + i64::from(rule.is_some_and(|(_, at)| text_row >= at));
    let skipped = |at: i64| pass.except.is_some_and(|(from, to)| from <= at && at < to);
    if let Some((limit, at)) = rule.map(|(limit, at)| (limit, at as i64)) {
        if pass.from <= at && at < pass.to && !skipped(at) {
            limit_rule(c, limit, (at - top) as i32);
        }
    }
    // A line is changed today when it is stamped so. A blank line shows it
    // only between two lines that do: the blank row Enter writes under an
    // older paragraph is not something written.
    let stamped = |line: usize| s.today != 0 && s.page.stamp(line) == s.today;
    // The row of the text on the first row of the paper wanted, or the one
    // before it: starting a row early costs nothing.
    let first = (pass.from.max(1) - 1) as usize;
    let (mut line, _) = layout.line_of_row(first.min(layout.total_rows().saturating_sub(1)));
    while line < doc.line_count() {
        let line_top = layout.first_row(line);
        if paper(line_top) >= pass.to {
            break;
        }
        let text = doc.line(line);
        let shape = layout.shape(line);
        let unit = shape.scale as i64;
        let wanted = |at: i64| {
            let on_sheet = if pass.whole { at >= pass.from && at + unit <= pass.to } else { at + unit > pass.from && at < pass.to };
            on_sheet && !skipped(at)
        };
        let line_rows = layout.rows(line);
        let row_at = |within: usize| paper(line_top + within * shape.scale);
        if (0..line_rows.len()).any(|within| wanted(row_at(within))) {
            let struck = Struck::of(s, line, own, selection);
            let between = line > 0 && line + 1 < doc.line_count() && stamped(line - 1) && stamped(line + 1);
            let changed = stamped(line) && (s.page.kind(line) != LineKind::Blank || between);
            let line_end = Pos::new(line, text.len());
            let break_selected = selection.is_some_and(|selected| selected.start <= line_end && line_end < selected.end);
            for (within, row) in line_rows.iter().enumerate() {
                let at = row_at(within);
                if !wanted(at) {
                    continue;
                }
                let at_row = (at - top) as i32;
                let col = grid.text.first + (row.indent as i64 * unit) as i32;
                let end = row_text(c, &struck, text, row, col, at_row);
                // A selected line break shows as one inverse cell after the
                // line, so a selection across blank lines can be seen.
                if break_selected && within + 1 == line_rows.len() {
                    c.text(Surface::Sheet, end.min(grid.text.last + 1), at_row, " ", struck.words.inverse(true));
                }
                if changed {
                    c.change_bar(at_row, unit as i32);
                }
                if within == 0 && shape.hung > 0 {
                    c.text_ending(Surface::Sheet, grid.gutter.last, at_row + unit as i32 - 1, s.page.hung_mark(line), struck.hung());
                }
            }
        }
        line += 1;
    }
}

/// The cursor: a solid block that never blinks. While the cursor is on the
/// line, or another window has the keyboard, a hollow box keeps its place.
/// With a selection there is no block; the selection is the cursor.
fn cursor(c: &mut Canvas, s: &Scene, own: (usize, usize)) {
    if s.page.cursor.has_selection() {
        return;
    }
    let grid = c.grid();
    let caret = s.page.caret();
    let at_row = s.page.paper_row(caret.row) - s.view.top();
    let rows = grid.text_rows as i64;
    if at_row + caret.size as i64 <= 0 || at_row >= rows {
        return;
    }
    // At rest a double-size row cut by the edge of the sheet is left off.
    // The cursor on it is then a bare block: the letter or the mark it is
    // on, shown with nothing of its line round it, would be read as text
    // that is there.
    let unit = s.page.layout().shape(caret.line).scale.max(1) as i64;
    let row_top = if caret.size as i64 == unit { at_row } else { at_row + 1 - unit };
    let left_off = s.view.shift() == 0 && (row_top < 0 || row_top + unit > rows);
    let (at_row, col) = (at_row as i32, grid.text.first + caret.cell);
    let doc = s.page.doc();
    let head = doc.clamp(s.page.cursor.head);
    let line = doc.line(head.line);
    // A cursor between a character and a mark on it (an edit can leave it
    // there, a movement never does) is in that character's cell, and the
    // block shows the character whole.
    let in_gutter = caret.cell < 0;
    let hung = s.page.layout().shape(head.line).hung;
    let on_mark = line.get(head.col..).and_then(|rest| rest.chars().next()).is_some_and(is_combining);
    let base = line.get(..head.col).and_then(|before| before.char_indices().rev().find(|&(_, ch)| !is_combining(ch))).map(|(at, _)| at);
    let from = match base {
        Some(base) if on_mark && !in_gutter && base >= hung => base,
        _ => head.col,
    };
    // On the mark in the gutter the block holds that character of the mark
    // and nothing of the row beside it.
    let rest = match (left_off, in_gutter) {
        (true, _) => "",
        (false, true) => line.get(head.col..hung.max(head.col)).unwrap_or(""),
        (false, false) => line.get(from..).unwrap_or(""),
    };
    // The block holds the first thing at the cursor that fills a cell. A
    // character with no cell of its own may stand before it (the zero-width
    // space that opens a line copied from a web page, a direction mark);
    // the letter after it is what is in the cell, and must not vanish.
    let mut lead = 0;
    let mut piece = "";
    for part in clusters(rest) {
        piece = part;
        if str_width(part) > 0 || in_gutter {
            break;
        }
        lead += part.len();
    }
    if str_width(piece) == 0 {
        // Nothing but such characters to the end of the line: the block
        // shows them, as the only sign that they are there.
        (lead, piece) = (0, rest);
    }
    let cells = match piece.chars().next() {
        Some('\t') | None => 1,
        Some(_) => str_width(piece).clamp(1, 2) as i32,
    };
    let struck = Struck::of(s, head.line, own, None);
    let ink = if in_gutter { struck.hung() } else { struck.ink(from + lead) };
    let size = caret.size as i32;
    let (wide, tall) = (cells * size, size);
    if s.line.is_open() || !s.focused {
        c.hollow(Surface::Sheet, col, at_row, wide, tall);
    } else {
        if ink.inverse {
            c.cursor_within(Surface::Sheet, col, at_row, piece, cells, ink);
        } else {
            c.cursor(Surface::Sheet, col, at_row, piece, cells, ink);
        }
        // What stood before the letter with no cell of its own is struck
        // over it, as it is everywhere else on the row.
        if let Some(before) = rest.get(..lead).filter(|before| !before.is_empty()) {
            c.text(Surface::Sheet, col, at_row, before, Ink { strength: Strength::INK, ..ink }.inverse(true));
        }
    }
    if let Some(offer) = s.line.offer() {
        c.text(Surface::Sheet, col + wide, at_row, offer, Ink { big: caret.size == 2, ..Ink::HALF });
    }
}

/// The rows of text on the sheet, with their change bars and hanging
/// marks, and the cursor.
pub fn paint(c: &mut Canvas, s: &Scene) {
    let grid = c.grid();
    if grid.text_rows <= 0 {
        return;
    }
    let (doc, layout) = (s.page.doc(), s.page.layout());
    let top = s.view.top();
    let own = s.page.own_paragraph();
    let selection = s.page.cursor.has_selection().then(|| s.page.cursor.span());
    c.confine(grid.sheet, 0, grid.text_rows);

    let shift = s.view.shift();
    let (from, to) = (top, top + grid.text_rows as i64);
    if shift == 0 {
        rows(c, s, Pass { from, to, except: None, whole: true }, own, selection);
    } else {
        // The paper is rolling. Everything is drawn displaced, with the
        // rows the displacement uncovers, except the row being typed on,
        // which is held still: it is drawn last, where it rests, over
        // whatever is sliding past behind it.
        let uncovered = ((shift.abs() + grid.cell_h - 1) / grid.cell_h) as i64;
        let typing = s.page.paper_row(layout.locate(doc, s.page.cursor.head).0);
        let unit = layout.shape(doc.clamp(s.page.cursor.head).line).scale as i64;
        let held = (typing, typing + unit);
        let wider = Pass { from: if shift > 0 { from - uncovered } else { from }, to: if shift < 0 { to + uncovered } else { to }, except: Some(held), whole: false };
        c.set_shift(shift);
        rows(c, s, wider, own, selection);
        c.set_shift(0);
        c.erase(Surface::Sheet, grid.sheet.first, (typing - top) as i32, grid.sheet.width(), unit as i32);
        rows(c, s, Pass { from: held.0, to: held.1, except: None, whole: false }, own, selection);
    }
    cursor(c, s, own);
    c.release();
}
