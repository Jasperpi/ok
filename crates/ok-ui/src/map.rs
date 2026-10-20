//! The right side: the map of the whole text, the labels beside it, and
//! the links block at its foot.
//!
//! The map is the text in miniature with the same line breaks as the page:
//! a small square for every letter, spaces left open, so words read as
//! dashes and paragraphs as blocks. It never shrinks to fit. A longer text
//! makes a longer map, which is the point.

use ok_gfx::Rect;
use ok_text::markup::{self, LineKind};
use ok_text::{cell_width, str_width, Pos};

use crate::canvas::{fit_name, set_rows, Canvas, Ink};
use crate::contents::{self, Struck};
use crate::flow::Part;
use crate::grid::Grid;
use crate::line::Pane;
use crate::look::{Strength, Surface};
use crate::page::{tks, Page};
use crate::scene::Scene;

/// Where the map's rows fall on the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    /// The pixels the marks may take at full size.
    pub area: Rect,
    /// The left edge of the marks. At full size that is the area's; at
    /// half size the marks keep to the right of the area, beside the flags
    /// and the labels, and the notch comes across with them.
    pub left: i32,
    /// The side of a letter's square in pixels: two at scale 1, one once
    /// the text has outgrown the map at that size.
    pub letter: i32,
    /// Pixels from one row of the text to the next: twice the letter.
    pub pitch: i32,
    /// The first row of the text the map shows, and how many it shows.
    pub first: usize,
    pub shown: usize,
}

impl Geometry {
    /// The top of a row of the text in pixels, if the map shows that row.
    pub fn y(&self, row: usize) -> Option<i32> {
        (self.first <= row && row < self.first + self.shown).then(|| self.area.y + (row - self.first) as i32 * self.pitch)
    }

    /// The row of the text drawn at a pixel row, if there is one.
    pub fn row_at(&self, y: i32) -> Option<usize> {
        let down = (y - self.area.y).div_euclid(self.pitch.max(1));
        (0 <= down && (down as usize) < self.shown).then(|| self.first + down as usize)
    }
}

/// Lays the map out for a text. At full size a letter is two pixels square
/// and a row four; a text with more rows than fit that way is drawn once at
/// half that; and one too long even so scrolls, keeping the cursor's row
/// near the middle. `None` when the screen has no room for a map.
pub fn geometry(grid: &Grid, page: &Page) -> Option<Geometry> {
    let area = grid.map_area()?;
    let total = page.layout().total_rows();
    let full = grid.map_unit();
    let letter = if total as i64 * (2 * full) as i64 <= area.h as i64 { full } else { (full / 2).max(1) };
    let pitch = 2 * letter;
    let fits = (area.h / pitch).max(0) as usize;
    let cursor = page.layout().locate(page.doc(), page.cursor.head).0;
    let first = if total <= fits { 0 } else { cursor.saturating_sub(fits / 2).min(total - fits) };
    let left = area.x + area.w - grid.measure * letter;
    Some(Geometry { area, left, letter, pitch, first, shown: fits.min(total - first) })
}

/// The pixels of margin round the marks on their strip of paper, at
/// scale 1.
const MARGIN: i32 = 4;

/// The map with its notch and flags, then the labels and the links beside
/// it.
pub fn paint(c: &mut Canvas, s: &Scene) {
    let grid = c.grid();
    let (Some(_), Some(g)) = (grid.map, geometry(grid, s.page)) else { return };
    let look = c.look();
    let (doc, layout) = (s.page.doc(), s.page.layout());
    // A text with no letter in it has nothing to map: a bare strip of paper
    // with a notch stuck to it would only be a stray bar on the desk.
    if !doc.lines().iter().any(|line| line.chars().any(|ch| !ch.is_whitespace())) {
        labels(c, s, &g);
        return;
    }
    let end = g.first + g.shown;
    let wide = grid.measure * g.letter;
    let margin = MARGIN * grid.scale;
    let foot = g.area.y + g.shown as i32 * g.pitch;

    // On a desk the map is a strip of the sheet's own paper, which grows as
    // the text does: four pixels of margin round the marks, top and bottom
    // alike. Where the map shows only part of a long text, the strip stops
    // flush with the marks at the end that is cut off, so that it reads as
    // paper that goes on and not as the whole text.
    let surface = if look.is_flat() { Surface::Desk } else { Surface::Sheet };
    if !look.is_flat() {
        let (last_line, _) = layout.line_of_row(end.saturating_sub(1));
        let scale = layout.shape(last_line).scale.max(1);
        let last_top = layout.first_row(last_line) + (end - 1 - layout.first_row(last_line)) / scale * scale;
        let marks_end = g.y(last_top).map_or(foot, |y| (y + g.letter * scale as i32).min(foot));
        let top = if g.first > 0 { g.area.y } else { g.area.y - margin };
        let bottom = if end < layout.total_rows() { foot } else { marks_end + margin };
        c.pixels(Rect::new(g.left - margin, top, wide + 2 * margin, bottom - top), look.paper);
    }

    // Marks are never dimmed: rows written today in ink, older rows at
    // half, and whatever is past the word limit faint.
    let (today, older, past) = (c.colour(surface, Strength::INK), c.colour(surface, Strength::HALF), c.colour(surface, Strength::FAINT));
    let over = s.page.over();
    let flag = look.mark_on(Surface::Desk);
    // On a desk the edge of the strip sets the notch and the flags apart
    // from the marks. On one flat ground nothing does, and a bar two pixels
    // high beside a row of dashes two pixels high reads as one more word;
    // there they are as tall as a whole row of the map.
    let tick = if look.is_flat() { g.pitch.max(2 * grid.scale) } else { 2 * grid.scale };
    // A flag stands clear of the strip's edge in the flags column, at
    // either size of map.
    let flag_at = |y: i32| Rect::new(g.area.x + g.area.w + (MARGIN + 2) * grid.scale, y, 4 * grid.scale, tick);
    c.confine_pixels(Rect::new(g.left, g.area.y, wide, g.area.h));
    let mut flags: Vec<Rect> = Vec::new();
    let (mut line, _) = layout.line_of_row(g.first);
    while line < doc.line_count() {
        let top = layout.first_row(line);
        if top >= end {
            break;
        }
        let text = doc.line(line);
        let scale = layout.shape(line).scale;
        let square = g.letter * scale as i32;
        let written = if s.today != 0 && s.page.stamp(line) == s.today { today } else { older };
        // The byte of this line from which it is past the limit.
        let past_from = match over {
            Some(over) if line > over.line => 0,
            Some(over) if line == over.line => over.col,
            _ => usize::MAX,
        };
        let rows = layout.rows(line);
        for (within, row) in rows.iter().enumerate() {
            let Some(y) = g.y(top + within * scale) else { continue };
            let mut cell = row.indent as i32;
            let mut run: Option<(i32, usize)> = None;
            let dash = |c: &mut Canvas, (from, byte): (i32, usize), to: i32| {
                let colour = if byte >= past_from { past } else { written };
                c.pixels(Rect::new(g.left + from * square, y, (to - from) * square, square), colour);
            };
            for (at, ch) in text.get(row.start..row.end).unwrap_or("").char_indices() {
                let cells = cell_width(ch) as i32;
                if cells == 0 {
                    continue;
                }
                if ch.is_whitespace() {
                    if let Some(from) = run.take() {
                        dash(c, from, cell);
                    }
                } else if run.is_none() {
                    run = Some((cell, row.start + at));
                }
                cell += cells;
                if cell * square > wide {
                    break;
                }
            }
            if let Some(from) = run {
                dash(c, from, cell);
            }
        }
        if !matches!(s.page.kind(line), LineKind::Code | LineKind::Fence) && text.contains("TK") {
            for (start, _) in tks(text) {
                let within = rows.partition_point(|row| row.end <= start).min(rows.len().saturating_sub(1));
                flags.extend(g.y(top + within * scale).map(flag_at));
            }
        }
        line += 1;
    }
    c.release();

    for hit in s.line.hits() {
        flags.extend(g.y(layout.locate(doc, hit.start).0).map(flag_at));
    }
    for tick in flags {
        c.pixels(tick, flag);
    }
    // The notch: a bar level with the cursor's row, against the left edge
    // of the strip.
    if let Some(y) = g.y(layout.locate(doc, s.page.cursor.head).0) {
        c.pixels(Rect::new(g.left - margin - 6 * grid.scale, y, 6 * grid.scale, tick), flag);
    }

    labels(c, s, &g);
}

/// The lines of the links block from the top down: how far in, the words,
/// and their strength at rest. Captions and names are both words to be
/// read, so both are at half strength, and are told apart by the names
/// being set in. A name too long for its row turns over onto a second. In
/// a pane too narrow for the longest word of a name at that setting-in, the
/// names move out toward the edge by as much as it takes, sooner than have
/// a word broken across two rows; if that leaves them flush with their
/// captions, an empty row parts the groups.
fn links_block(s: &Scene, width: usize) -> Vec<(i32, String, Strength)> {
    const SET_IN: usize = 2;
    let mut block: Vec<(i32, String, Strength)> = Vec::new();
    let Some(links) = s.session.links() else { return block };
    // A link to a text that does not exist yet ends in a space and a
    // question mark.
    let see_also: Vec<String> = links.see_also.iter().map(|link| if link.exists { link.name.clone() } else { format!("{} ?", link.name) }).collect();
    let longest = see_also.iter().chain(&links.linked_from).take(12).flat_map(|name| name.split_whitespace()).map(str_width).max().unwrap_or(0);
    let set_in = SET_IN.min(width.saturating_sub(longest));
    let room = width.saturating_sub(set_in);
    let mut group = |caption: &str, names: &[String]| {
        if names.is_empty() {
            return;
        }
        // With nothing set in to tell names from captions, an empty row
        // parts one group from the next.
        if set_in == 0 && !block.is_empty() {
            block.push((0, String::new(), Strength::HALF));
        }
        block.push((0, caption.to_string(), Strength::HALF));
        for name in names.iter().take(6) {
            block.extend(set_rows(name, room, 2).into_iter().map(|row| (set_in as i32, row.to_string(), Strength::HALF)));
        }
        if names.len() > 6 {
            block.push((set_in as i32, format!("and {} more", names.len() - 6), Strength::HALF));
        }
    };
    group("See also", &see_also);
    group("Linked from", &links.linked_from);
    block
}

/// True when the text is to be labelled by paragraph: one with no
/// headings, or none but its title. A title that is the only heading is the
/// name of the whole text, which the contents and the facts already give,
/// and as a label would say nothing about where he is.
fn by_paragraph(s: &Scene, headings: &[usize]) -> bool {
    match headings {
        [] => true,
        [only] => (0..s.page.doc().line_count()).find(|&line| s.page.kind(line) != LineKind::Blank) == Some(*only),
        _ => false,
    }
}

/// The labels beside the map and the links block under them.
fn labels(c: &mut Canvas, s: &Scene, g: &Geometry) {
    let grid = c.grid();
    let Some(cols) = grid.labels else { return };
    let width = cols.width().max(0) as usize;
    let foot = grid.links_foot();

    // A list the line is driving takes the labels' place.
    if let Some(answer) = s.line.answer().filter(|answer| answer.pane == Pane::Labels) {
        let visible = (foot - 1).max(0) as usize;
        let from = contents::window(answer.rows.len(), visible, answer.bar);
        for (at, entry) in answer.rows.iter().enumerate().skip(from).take(visible) {
            let struck = Struck { words: if entry.lit { Strength::INK } else { Strength::HALF }, dots: Strength::FAINT, bar: answer.bar == Some(at) };
            contents::row(c, Surface::Desk, cols, 1 + (at - from) as i32, entry, struck);
        }
        return;
    }

    let block = links_block(s, width);
    let block_top = foot + 1 - block.len() as i32;
    for (down, (indent, words, rest)) in block.iter().enumerate() {
        let strength = s.flow.part(Part::Links, *rest, s.now_ms);
        c.text(Surface::Desk, cols.first + indent, block_top + down as i32, words, Ink::at(strength));
    }
    // Labels stop a clear row short of the links.
    let last = if block.is_empty() { foot } else { block_top - 2 };

    let (doc, layout) = (s.page.doc(), s.page.layout());
    let cursor = s.page.cursor.head;
    let headings: Vec<usize> = (0..doc.line_count()).filter(|&line| matches!(s.page.kind(line), LineKind::Heading(_))).collect();
    // The cell row a row of the text falls in, as the map shows it.
    let cell_row = |row: usize| g.y(row).map(|y| grid.cell_at(0, y).1);

    // A text with no headings of its own is labelled by the opening words
    // of the paragraph he is in, level with the notch, and by nothing
    // else: a list of every paragraph would be the busiest thing on the
    // screen and say nothing the map beside it does not.
    if by_paragraph(s, &headings) {
        let (first, _) = s.page.own_paragraph();
        let opening = doc.line(first).trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '>' | '-' | '*' | '+' | '#'));
        let row = cell_row(layout.locate(doc, cursor).0).map(|row| row.max(1));
        // A paragraph short enough to be read whole in the label is on the
        // page in front of him, and a label that grew letter by letter as
        // he began one would only echo his typing.
        let labelled = str_width(opening) > width && !headings.contains(&first);
        if let Some(row) = row.filter(|&row| labelled && row <= last) {
            let strength = s.flow.part(Part::Labels, Strength::HALF, s.now_ms);
            c.text(Surface::Desk, cols.first, row, fit_name(opening, width), Ink::at(strength));
        }
        return;
    }

    // Each heading where its line falls; the section the cursor is in is
    // the last heading at or above it. If two want the same row the later
    // moves down, and a title too long for its row turns over onto the
    // next when no other label wants that one.
    let current = headings.iter().rev().find(|&&line| line <= cursor.line).copied();
    let titled: Vec<(usize, &str, i32)> = headings
        .iter()
        .filter_map(|&line| {
            let title = markup::heading(doc.line(line)).map_or("", |(_, title)| title);
            let row = cell_row(layout.first_row(line))?;
            (!title.is_empty()).then_some((line, title, row))
        })
        .collect();
    let mut free = 1;
    for (at, &(line, title, wanted)) in titled.iter().enumerate() {
        let row = wanted.max(free);
        if row > last {
            break;
        }
        let rest = if Some(line) == current { Strength::INK } else { Strength::HALF };
        let ink = Ink::at(s.flow.part(Part::Labels, rest, s.now_ms));
        let next = titled.get(at + 1).map_or(last + 1, |&(_, _, wanted)| wanted);
        let turned = if str_width(title) > width && row + 1 < next && row < last { set_rows(title, width, 2) } else { Vec::new() };
        match turned.as_slice() {
            [upper, _] => {
                c.text(Surface::Desk, cols.first, row, upper, ink);
                let lower = title.get(upper.len()..).unwrap_or("");
                c.text(Surface::Desk, cols.first, row + 1, fit_name(lower, width), ink);
                free = row + 2;
            }
            _ => {
                c.text(Surface::Desk, cols.first, row, fit_name(title, width), ink);
                free = row + 1;
            }
        }
    }
}

/// The position a click on the map asks for: the start of the row drawn at
/// that pixel. `None` when the pixel is not on the map.
pub fn click(grid: &Grid, page: &Page, x: i32, y: i32) -> Option<Pos> {
    let (cols, g) = (grid.map?, geometry(grid, page)?);
    let (col, _) = grid.cell_at(x, y);
    if col < cols.notch || col > cols.flags || grid.cell_at(x, y).1 >= grid.rows {
        return None;
    }
    g.row_at(y).map(|row| page.pos_at_text_row(row, 0))
}
