//! Soft wrapping. A line of the document is a whole paragraph, so before it
//! can be drawn it is cut into rows that fit the measure. [`wrap`] cuts one
//! line; [`Layout`] keeps the rows of every line and knows which row of the
//! document each line starts on.
//!
//! Text is measured in clusters: a character together with the combining
//! marks drawn over it. A cluster is never split between rows, and a cursor
//! never sits inside one.
//!
//! Not every line is set the same way. A heading may be double size, the
//! mark that opens a heading or a quote hangs in the gutter beside the text,
//! a quote is set in from both edges and a list item's later rows hang under
//! its first letter. The page says which with a [`Shape`] for each line, and
//! the layout does the arithmetic: its rows and cells are rows and cells of
//! the grid, whatever the size of the line they fall on.

use crate::doc::{Change, Doc, Pos};
use crate::width::{cell_width, is_combining, str_width};

/// One row of a wrapped line.
///
/// `start..end` are the bytes of the line shown on this row. The rows of a
/// line follow one another without gaps: the first starts at 0 and the last
/// ends at the line's length. `indent` is the number of blank cells before
/// the text: 0 on the first row, the hanging indent on the rest. `cells` is
/// how many cells the text fills after the indent, leaving out whitespace
/// that hangs off the end of the row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
    pub cells: usize,
}

/// How a cluster behaves when a row has to end near it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A space or tab, with or without marks on it. A row may end after a
    /// run of these.
    Space,
    /// A hyphen, dash or slash. A row may end right after it.
    Hyphen,
    Word,
}

struct Cluster {
    end: usize,
    width: usize,
    kind: Kind,
}

/// Reads the cluster that starts at byte `at`, or nothing at the end of the
/// line. Marks at the very start of a line have no character to sit on, so
/// they join the first one that follows.
fn cluster_at(line: &str, at: usize) -> Option<Cluster> {
    let mut chars = line.get(at..)?.chars();
    let mut base = chars.next()?;
    let mut end = at + base.len_utf8();
    let mut based = !is_combining(base);
    for ch in chars {
        if !is_combining(ch) {
            if based {
                break;
            }
            based = true;
            base = ch;
        }
        end += ch.len_utf8();
    }
    // The base decides. A space is a space whatever sits on it: prose in a
    // right-to-left script puts a direction mark after one all the time, and
    // that must not glue the words on either side together.
    let kind = match base {
        ' ' | '\t' => Kind::Space,
        '-' | '/' | '\u{2010}' | '\u{2013}' | '\u{2014}' => Kind::Hyphen,
        _ => Kind::Word,
    };
    Some(Cluster { end, width: cell_width(base), kind })
}

fn floor_boundary(line: &str, at: usize) -> usize {
    let mut at = at.min(line.len());
    while !line.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Moves `col` back to the start of the cluster it falls in. The end of the
/// line counts as a start.
fn cluster_start(line: &str, col: usize) -> usize {
    let mut at = floor_boundary(line, col);
    let here = line.get(at..).and_then(|rest| rest.chars().next());
    let Some(here) = here else {
        return at;
    };
    let before = line.get(..at).unwrap_or("");
    if is_combining(here) {
        match before.char_indices().rev().find(|&(_, ch)| !is_combining(ch)) {
            Some((base, _)) => at = base,
            None => return 0,
        }
    }
    // Marks with nothing before them belong to this cluster too.
    let before = line.get(..at).unwrap_or("");
    if before.chars().rev().all(is_combining) {
        0
    } else {
        at
    }
}

/// The row's byte range, pulled inside `line` so that a row left over from
/// an older text cannot reach past the end or into a character.
fn span_in(line: &str, row: &Row) -> (usize, usize) {
    let end = floor_boundary(line, row.end);
    (floor_boundary(line, row.start.min(end)), end)
}

/// The most rows set aside before a line has been measured. Its length in
/// bytes says little about its rows (a megabyte of hanging spaces is one
/// row), so the guess is kept small and the rows grow from there.
const RESERVE: usize = 16;

/// Cuts one line into rows of at most `measure` cells, filling each row as
/// far as it will go.
///
/// A row may end after a run of spaces or tabs, or right after a hyphen, a
/// dash or a slash that has something other than a space after it. The
/// whitespace a row ends at stays on that row and may hang past the measure;
/// the next row starts at the first character after it. A space or tab with
/// marks on it is whitespace all the same. A word too long for
/// a row is cut between clusters, and every row holds at least one cluster
/// even when that cluster alone is wider than the row. Whitespace at the very
/// start of the line is text like any other: it takes room, it is never left
/// hanging, and no row ends just because it does.
///
/// Rows after the first are indented by `hang` cells and have that much less
/// room; a `hang` that would leave no room is ignored. A `measure` of 0
/// counts as 1. An empty line is a single row of zeros.
///
/// On the last row, whitespace at the end of the line counts towards `cells`
/// only as far as it fits, so `indent + cells` stays within the measure.
pub fn wrap(line: &str, measure: usize, hang: usize) -> Vec<Row> {
    let measure = measure.max(1);
    let hang = if hang >= measure { 0 } else { hang };
    let mut rows = Vec::with_capacity((line.len() / measure + 1).min(RESERVE));

    let mut row = Row::default();
    let mut avail = measure;
    // Cells taken on this row so far. `text` stops before whitespace at the
    // end; `shown` also counts as much of that whitespace as still fits.
    let (mut used, mut text, mut shown) = (0, 0, 0);
    // The latest place this row may end, and the cells it has if it ends there.
    let mut opportunity: Option<(usize, usize)> = None;
    let mut prev = Kind::Word;
    let mut leading = true;
    let mut at = 0;

    while let Some(c) = cluster_at(line, at) {
        if c.kind == Kind::Space && !leading {
            used += c.width;
            if used <= avail {
                shown = used;
            }
            prev = Kind::Space;
            at = c.end;
            continue;
        }
        leading &= c.kind == Kind::Space;
        if prev != Kind::Word {
            opportunity = Some((at, text));
        }
        if at > row.start && used + c.width > avail {
            // Nothing between an opportunity and here can end a row, but the
            // next row may be narrower than this one, so what is carried over
            // is laid out again from the opportunity. That happens at most
            // once for any cluster.
            let (end, cells) = opportunity.unwrap_or((at, text));
            rows.push(Row { end, cells, ..row });
            row = Row { start: end, end, indent: hang, cells: 0 };
            avail = measure - hang;
            (used, text, shown) = (0, 0, 0);
            opportunity = None;
            prev = Kind::Word;
            at = end;
            continue;
        }
        used += c.width;
        text = used;
        shown = used;
        prev = if c.kind == Kind::Hyphen { Kind::Hyphen } else { Kind::Word };
        at = c.end;
    }
    rows.push(Row { end: line.len(), cells: shown, ..row });
    // A layout keeps these for as long as the line lives.
    rows.shrink_to_fit();
    rows
}

/// The cell, indent included, at which the character starting at byte `col`
/// is drawn on `row`. For `col == row.end` this is the cell just past the
/// row's last character, which may lie beyond the measure.
///
/// `col` is first brought into `row.start..=row.end` and back to the start
/// of its cluster.
pub fn cell_of(line: &str, row: &Row, col: usize) -> usize {
    let (start, end) = span_in(line, row);
    let col = cluster_start(line, col.clamp(start, end)).max(start);
    row.indent.saturating_add(line.get(start..col).map_or(0, str_width))
}

/// The byte offset of the cluster drawn over `cell` on `row`. Cells inside
/// the indent give the start of the row.
///
/// Past the text, the last row of a line gives `row.end`. Any other row
/// gives the start of its last cluster, because `row.end` is where the next
/// row begins and a cursor put there would be drawn one row down.
pub fn col_at(line: &str, row: &Row, cell: usize, last_row: bool) -> usize {
    col_for(line, row, cell, last_row, false)
}

/// The walk behind [`col_at`]. With `nearest` it answers the question Up
/// and Down ask instead: of the places a cursor can rest on `row`, which is
/// drawn nearest to `cell`?
///
/// That is the far edge of the cluster under the cell, not its start, when
/// the far edge is strictly the nearer, which takes a cluster at least three
/// cells wide: a tab. The end of a row is a place to rest only on a line's
/// last row.
fn col_for(line: &str, row: &Row, cell: usize, last_row: bool, nearest: bool) -> usize {
    let (start, end) = span_in(line, row);
    let Some(target) = cell.checked_sub(row.indent) else {
        return start;
    };
    let (mut at, mut last, mut x) = (start, start, 0);
    while at < end {
        let Some(c) = cluster_at(line, at) else {
            break;
        };
        // A line holding nothing but marks is one cluster with no cells of
        // its own; it still answers for the cell it is drawn at.
        if target < x + c.width || (c.width == 0 && target == x) {
            let rests = c.end < end || (c.end == end && last_row);
            let nearer = x + c.width - target < target - x;
            return if nearest && rests && nearer { c.end } else { at };
        }
        x += c.width;
        last = at;
        at = c.end;
    }
    if last_row {
        end
    } else {
        last
    }
}

/// Where End puts the cursor on a row that has another row after it: just
/// past the last character before the whitespace the row was broken at, or,
/// when the row was broken inside a word, on its last cluster.
fn end_of_text(line: &str, row: &Row) -> usize {
    let (start, end) = span_in(line, row);
    let (mut at, mut last) = (start, start);
    let mut after_text = None;
    let mut hanging = false;
    while at < end {
        let Some(c) = cluster_at(line, at) else {
            break;
        };
        hanging = c.kind == Kind::Space;
        if !hanging {
            after_text = Some(c.end);
        }
        last = at;
        at = c.end;
    }
    match after_text {
        Some(col) if hanging => col,
        _ => last,
    }
}

/// How one line of the document is set on the page.
///
/// `hang` and `inset` are counted in cells of the line's own size, as the
/// `indent` and `cells` of its [`Row`]s are: on a double-size line one of
/// them is two cells of the grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shape {
    /// Bytes at the start of the line that hang in the gutter beside the
    /// text: the `#` of a heading and the space after it, the `>` of a
    /// quote. They are part of no row, so the line's first row starts after
    /// them. Brought back onto a character boundary inside the line.
    pub hung: usize,
    /// How many cells of the grid one cell of this line takes each way: 1,
    /// or 2 for a double-size line, which wraps at half the measure and whose
    /// rows are each two rows of the grid tall. 0 counts as 1 and anything
    /// above 2 as 2.
    pub scale: usize,
    /// Blank cells before the text on every row after the first. One that
    /// would leave no room is ignored, as [`wrap`] has it.
    pub hang: usize,
    /// Cells kept clear at each edge of the measure, on every row. One that
    /// would leave no room is ignored.
    pub inset: usize,
}

impl Default for Shape {
    /// A plain paragraph: nothing hung, single size, no hang, no inset.
    fn default() -> Shape {
        Shape { hung: 0, scale: 1, hang: 0, inset: 0 }
    }
}

impl Shape {
    /// A single-size line whose rows after the first hang by `hang` cells.
    pub fn hanging(hang: usize) -> Shape {
        Shape { hang, ..Shape::default() }
    }
}

/// Cuts one line into rows as its shape asks, and gives the shape back as it
/// was really used: `hung` on a character boundary, `scale` 1 or 2, an
/// impossible `inset` dropped.
///
/// The rows are [`wrap`]'s rows of the text after the hung bytes, at the
/// measure the scale and the inset leave, moved along by the hung bytes and
/// in by the inset. So [`cell_of`] and [`col_at`] read them like any others,
/// in cells of the line's own size counted from the edge of the measure.
fn set(line: &str, measure: usize, shape: Shape) -> (Shape, Vec<Row>) {
    let scale = shape.scale.clamp(1, 2);
    let hung = floor_boundary(line, shape.hung);
    let room = (measure / scale).max(1);
    let inset = if shape.inset < room.div_ceil(2) { shape.inset } else { 0 };
    let mut rows = wrap(line.get(hung..).unwrap_or(""), room - 2 * inset, shape.hang);
    if hung > 0 || inset > 0 {
        for row in &mut rows {
            row.start += hung;
            row.end += hung;
            row.indent += inset;
        }
    }
    (Shape { hung, scale, hang: shape.hang, inset }, rows)
}

/// The rows of a whole document: the wrapped rows of every line, how each
/// line is set, and for every line the number of grid rows above it.
///
/// Rows and cells handed in and out are those of the grid. A double-size
/// line's rows are two grid rows tall and its cells two grid cells wide;
/// either grid row of such a row, and either grid cell of such a cell, names
/// it. A position inside a line's hung bytes is drawn by the page in the
/// gutter; here it answers as the start of the line's first row.
///
/// A layout belongs to one document at one measure. Keep it in step by
/// calling [`Layout::apply`] with each [`Change`] the document hands back,
/// and [`Layout::rewrap`] when the measure changes or lines change shape
/// without changing text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    measure: usize,
    lines: Vec<Vec<Row>>,
    /// One per line, as [`set`] gave it back.
    shapes: Vec<Shape>,
    /// `starts[i]` is the grid row of line `i`'s first row. One entry more
    /// than there are lines; the last is the total.
    starts: Vec<usize>,
}

impl Layout {
    /// Wraps every line of `doc`. `shape` is asked how each line is set,
    /// with the line's number and its text.
    pub fn new(doc: &Doc, measure: usize, shape: &dyn Fn(usize, &str) -> Shape) -> Layout {
        let mut layout = Layout { measure, lines: Vec::new(), shapes: Vec::new(), starts: Vec::new() };
        layout.rewrap(doc, measure, shape);
        layout
    }

    /// The measure the rows were wrapped to, exactly as it was given.
    pub fn measure(&self) -> usize {
        self.measure
    }

    /// Wraps the whole document again, for a new measure or new shapes.
    pub fn rewrap(&mut self, doc: &Doc, measure: usize, shape: &dyn Fn(usize, &str) -> Shape) {
        self.measure = measure;
        self.lines.clear();
        self.shapes.clear();
        for (i, line) in doc.lines().iter().enumerate() {
            let (shape, rows) = set(line, measure, shape(i, line));
            self.lines.push(rows);
            self.shapes.push(shape);
        }
        self.starts.clear();
        self.reindex(0, self.lines.len());
    }

    /// Brings the layout in step with one edit. `doc` is the document after
    /// the change. Only the lines the change touched are wrapped again; the
    /// row numbers of the lines below are then shifted, and not even that
    /// when the edit left the number of rows as it was.
    ///
    /// A layout that turns out not to match the document, or a change that
    /// no edit to it could have made, gets the layout rebuilt whole rather
    /// than left wrong.
    pub fn apply(&mut self, doc: &Doc, change: &Change, shape: &dyn Fn(usize, &str) -> Shape) {
        // The same lines as `Change::lines` gives, held as where they stop
        // rather than how many there are. A change built by hand can carry
        // line numbers that the count would overflow on.
        let first = change.start.line;
        let stops = (change.old_end.line.checked_add(1), change.end.line.checked_add(1));
        let (Some(old_stop), Some(new_stop)) = stops else {
            return self.rewrap(doc, self.measure, shape);
        };
        let known = self.lines.len().min(self.shapes.len());
        if first >= old_stop || first >= new_stop || old_stop > known || new_stop > doc.line_count() {
            return self.rewrap(doc, self.measure, shape);
        }
        let (shapes, fresh): (Vec<Shape>, Vec<Vec<Row>>) =
            (first..new_stop).map(|i| set(doc.line(i), self.measure, shape(i, doc.line(i)))).unzip();
        let before: usize = (first..old_stop).map(|line| self.height(line)).sum();
        let after: usize = fresh.iter().zip(&shapes).map(|(rows, shape)| rows.len() * shape.scale).sum();
        self.lines.splice(first..old_stop, fresh);
        self.shapes.splice(first..old_stop, shapes);
        if self.lines.len() != doc.line_count() {
            return self.rewrap(doc, self.measure, shape);
        }
        let to = if old_stop == new_stop && before == after { new_stop } else { self.lines.len() };
        self.reindex(first, to);
    }

    /// Recounts the first row of each line after `from`, up to line `to`.
    fn reindex(&mut self, from: usize, to: usize) {
        self.starts.resize(self.lines.len() + 1, 0);
        let mut row = self.starts.get(from).copied().unwrap_or(0);
        for line in from..to.min(self.lines.len()) {
            row += self.height(line);
            self.starts[line + 1] = row;
        }
    }

    /// How many grid rows one row of `line` is tall: 1 or 2.
    fn unit(&self, line: usize) -> usize {
        self.shapes.get(line).map_or(1, |shape| shape.scale.clamp(1, 2))
    }

    /// Grid rows taken by all the rows of `line`.
    fn height(&self, line: usize) -> usize {
        self.lines.get(line).map_or(0, Vec::len) * self.unit(line)
    }

    /// Grid rows in the whole document. Always at least 1.
    pub fn total_rows(&self) -> usize {
        self.starts.last().copied().unwrap_or(0).max(1)
    }

    /// The rows of one line, or an empty slice past the end.
    pub fn rows(&self, line: usize) -> &[Row] {
        self.lines.get(line).map(Vec::as_slice).unwrap_or(&[])
    }

    /// How one line is set, as it was really used: `hung` on a character
    /// boundary, `scale` 1 or 2. A plain paragraph past the end.
    pub fn shape(&self, line: usize) -> Shape {
        self.shapes.get(line).copied().unwrap_or_default()
    }

    /// The grid row of the line's first row, or [`Layout::total_rows`] past
    /// the end.
    pub fn first_row(&self, line: usize) -> usize {
        match self.starts.get(line) {
            Some(&row) if line < self.lines.len() => row,
            _ => self.total_rows(),
        }
    }

    /// The line a grid row belongs to, and which of that line's rows it is:
    /// both grid rows of a double-size row give the same answer. Rows past
    /// the end give the last row.
    pub fn line_of_row(&self, row: usize) -> (usize, usize) {
        let row = row.min(self.total_rows() - 1);
        let last_line = self.lines.len().saturating_sub(1);
        let line = self.starts.partition_point(|&start| start <= row).saturating_sub(1).min(last_line);
        (line, row.saturating_sub(self.first_row(line)) / self.unit(line))
    }

    /// Clamps `p` and finds which of its line's rows it is drawn on. A
    /// position where one row ends and the next begins belongs to the next.
    fn row_of(&self, doc: &Doc, p: Pos) -> (Pos, usize) {
        let p = doc.clamp(p);
        let rows = self.rows(p.line);
        let row = rows.partition_point(|row| row.end <= p.col).min(rows.len().saturating_sub(1));
        (p, row)
    }

    /// The grid row and the grid cell where a cursor at `p` is drawn: the
    /// top row and the left cell of it, on a double-size line. A position
    /// exactly at a soft break is drawn at the start of the row that follows
    /// it, and one inside the hung bytes at the start of the line's first
    /// row.
    pub fn locate(&self, doc: &Doc, p: Pos) -> (usize, usize) {
        let (p, row) = self.row_of(doc, p);
        let unit = self.unit(p.line);
        match self.rows(p.line).get(row) {
            Some(found) => {
                let cell = cell_of(doc.line(p.line), found, p.col);
                (self.first_row(p.line) + row * unit, cell.saturating_mul(unit))
            }
            None => (self.total_rows() - 1, 0),
        }
    }

    /// The position on grid row `row` that answers for grid cell `cell`: the
    /// cluster drawn over it, or with `nearest` the place a cursor can rest
    /// that is drawn closest to it. Rows past the end give the last row.
    fn on_row(&self, doc: &Doc, row: usize, cell: usize, nearest: bool) -> Pos {
        let (line, row) = self.line_of_row(row);
        let rows = self.rows(line);
        let last_row = row + 1 == rows.len();
        let cell = cell / self.unit(line);
        let col = rows.get(row).map_or(0, |found| col_for(doc.line(line), found, cell, last_row, nearest));
        doc.clamp(Pos { line, col })
    }

    /// The position drawn at grid cell `cell` on grid row `row`: the inverse
    /// of [`Layout::locate`], through [`col_at`]. It never answers with a
    /// position inside a line's hung bytes.
    pub fn pos_at(&self, doc: &Doc, row: usize, cell: usize) -> Pos {
        self.on_row(doc, row, cell, false)
    }

    /// One row up from `p`, to the place on that row drawn nearest to
    /// `cell`; of two equally near, the left one. From the first row, the
    /// start of the document.
    ///
    /// This is [`Layout::pos_at`] except inside a tab, where the nearer
    /// edge may be the far one. On a double-size row, nearest means nearest
    /// to the double cell that `cell` falls in.
    pub fn up(&self, doc: &Doc, p: Pos, cell: usize) -> Pos {
        match self.locate(doc, p).0 {
            0 => Pos::default(),
            row => self.on_row(doc, row - 1, cell, true),
        }
    }

    /// One row down from `p`, to the place on that row drawn nearest to
    /// `cell`, as [`Layout::up`] has it. From the last row, the end of the
    /// document. A double-size row is stepped over whole, like any other.
    pub fn down(&self, doc: &Doc, p: Pos, cell: usize) -> Pos {
        let below = self.locate(doc, p).0 + self.unit(doc.clamp(p).line);
        if below >= self.total_rows() {
            doc.end()
        } else {
            self.on_row(doc, below, cell, true)
        }
    }

    /// Home: the start of the wrapped row `p` is drawn on. On a line's first
    /// row that is the first character after any hung bytes.
    pub fn row_start(&self, doc: &Doc, p: Pos) -> Pos {
        let (p, row) = self.row_of(doc, p);
        let col = self.rows(p.line).get(row).map_or(p.col, |found| found.start);
        doc.clamp(Pos { line: p.line, col })
    }

    /// End: on a line's last row, the end of the line. On any other row the
    /// cursor has to stay short of `row.end`, which is drawn on the next
    /// row: it goes just after the last character that is not whitespace,
    /// or onto the row's last cluster when the row was broken inside a word.
    pub fn row_end(&self, doc: &Doc, p: Pos) -> Pos {
        let (p, row) = self.row_of(doc, p);
        let rows = self.rows(p.line);
        let text = doc.line(p.line);
        let col = match rows.get(row) {
            Some(found) if row + 1 < rows.len() => end_of_text(text, found),
            _ => text.len(),
        };
        doc.clamp(Pos { line: p.line, col })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Span;
    use std::cell::Cell;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    fn row(start: usize, end: usize, indent: usize, cells: usize) -> Row {
        Row { start, end, indent, cells }
    }

    fn texts<'a>(line: &'a str, rows: &[Row]) -> Vec<&'a str> {
        rows.iter().map(|r| &line[r.start..r.end]).collect()
    }

    fn rng(seed: u32) -> impl FnMut(usize) -> usize {
        let mut s = seed;
        move |n| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as usize % n.max(1)
        }
    }

    /// A stand-in for the markup's hanging indent: list items hang under
    /// their text.
    fn hang_cells(line: &str) -> usize {
        let body = line.trim_start_matches(' ');
        if body.starts_with("- ") {
            line.len() - body.len() + 2
        } else {
            0
        }
    }

    /// The shape of a line that does nothing but hang, which is all a line
    /// could do before shapes.
    fn hang_of(_: usize, line: &str) -> Shape {
        Shape::hanging(hang_cells(line))
    }

    fn plain(_: usize, _: &str) -> Shape {
        Shape::default()
    }

    /// A stand-in for the page's reading of a line: `# ` opens a double-size
    /// heading and `## ` a single-size one, both hung; `> ` is hung and set
    /// in by two; list items hang under their text.
    fn page_shape(_: usize, line: &str) -> Shape {
        if line.starts_with("# ") {
            Shape { hung: 2, scale: 2, ..Shape::default() }
        } else if line.starts_with("## ") {
            Shape { hung: 3, ..Shape::default() }
        } else if line.starts_with("> ") {
            Shape { hung: 2, inset: 2, ..Shape::default() }
        } else {
            Shape::hanging(hang_cells(line))
        }
    }

    struct Ref {
        start: usize,
        width: usize,
        space: bool,
        hyphen: bool,
    }

    /// Clusters worked out the slow way, to check the real ones against.
    fn ref_clusters(line: &str) -> Vec<Ref> {
        let mut starts = Vec::new();
        let mut based = false;
        for (i, ch) in line.char_indices() {
            if i == 0 || (!is_combining(ch) && based) {
                starts.push(i);
            }
            based |= !is_combining(ch);
        }
        starts
            .iter()
            .enumerate()
            .map(|(k, &start)| {
                let end = starts.get(k + 1).copied().unwrap_or(line.len());
                let text = &line[start..end];
                let base = text.chars().find(|&ch| !is_combining(ch));
                Ref {
                    start,
                    width: str_width(text),
                    space: matches!(base, Some(' ' | '\t')),
                    hyphen: matches!(base, Some('-' | '/' | '\u{2010}' | '\u{2013}' | '\u{2014}')),
                }
            })
            .collect()
    }

    /// The wrapping rules written as a search instead of a single pass: for
    /// each row, take the whole rest if it fits, else the furthest
    /// opportunity that fits, else as many clusters as fit.
    fn ref_wrap(line: &str, measure: usize, hang: usize) -> Vec<Row> {
        let measure = measure.max(1);
        let hang = if hang >= measure { 0 } else { hang };
        let cs = ref_clusters(line);
        let n = cs.len();
        let lead = cs.iter().position(|c| !c.space).unwrap_or(n);
        let hangs = |k: usize| cs[k].space && k >= lead;
        let width = |a: usize, b: usize| cs[a..b].iter().map(|c| c.width).sum::<usize>();
        let offset = |k: usize| cs.get(k).map_or(line.len(), |c| c.start);
        let mut rows = Vec::new();
        let mut i = 0;
        while i < n {
            let (indent, avail) = if rows.is_empty() { (0, measure) } else { (hang, measure - hang) };
            let text_end = |k: usize| {
                let mut e = k;
                while e > i && hangs(e - 1) {
                    e -= 1;
                }
                e
            };
            let fits = |k: usize| text_end(k) - i == 1 || width(i, text_end(k)) <= avail;
            let opportunity = |k: usize| !cs[k].space && (hangs(k - 1) || cs[k - 1].hyphen);
            let k = if fits(n) {
                n
            } else if let Some(k) = (i + 1..n).rev().find(|&k| opportunity(k) && fits(k)) {
                k
            } else {
                let mut k = i + 1;
                while k < n && width(i, k + 1) <= avail {
                    k += 1;
                }
                k
            };
            let e = text_end(k);
            let mut cells = width(i, e);
            if k == n {
                for c in &cs[e..n] {
                    if cells + c.width > avail {
                        break;
                    }
                    cells += c.width;
                }
            }
            rows.push(Row { start: offset(i), end: offset(k), indent, cells });
            i = k;
        }
        if rows.is_empty() {
            rows.push(Row::default());
        }
        rows
    }

    const PIECES: [&str; 24] = [
        "a",
        "be",
        "word",
        "longerword",
        " ",
        "  ",
        "\t",
        "-",
        "/",
        "\u{2013}",
        "\u{2014}",
        "\u{4E00}",
        "\u{4E8C}\u{4E09}",
        "n\u{0313}",
        "\u{0301}",
        "\u{00E9}",
        " \u{0301}",
        "well-known",
        "either/or ",
        "the quick brown fox ",
        "\n",
        "\n\n",
        "\n- item ",
        "\n    ",
    ];

    fn random_text(rnd: &mut impl FnMut(usize) -> usize, pieces: usize) -> String {
        (0..rnd(pieces)).map(|_| PIECES[rnd(PIECES.len())]).collect()
    }

    fn random_line(rnd: &mut impl FnMut(usize) -> usize) -> String {
        random_text(rnd, 14).replace('\n', "")
    }

    #[test]
    fn an_empty_line_is_one_row_of_zeros() {
        assert_eq!(wrap("", 10, 0), vec![Row::default()]);
        assert_eq!(wrap("", 0, 7), vec![Row::default()]);
    }

    #[test]
    fn rows_fill_greedily_and_whitespace_hangs() {
        let line = "the quick brown fox";
        let rows = wrap(line, 10, 0);
        assert_eq!(texts(line, &rows), ["the quick ", "brown fox"]);
        assert_eq!(rows, [row(0, 10, 0, 9), row(10, 19, 0, 9)]);
        // However long the run, it stays on the row it follows.
        let line = "ab      cd";
        assert_eq!(wrap(line, 4, 0), [row(0, 8, 0, 2), row(8, 10, 0, 2)]);
        assert_eq!(wrap("one two", 7, 0), [row(0, 7, 0, 7)]);
        assert_eq!(wrap("one two", 6, 0), [row(0, 4, 0, 3), row(4, 7, 0, 3)]);
    }

    #[test]
    fn a_word_longer_than_the_row_is_cut_between_clusters() {
        let line = "abcdefghij";
        assert_eq!(texts(line, &wrap(line, 4, 0)), ["abcd", "efgh", "ij"]);
        let line = "ab cdefghij k";
        assert_eq!(texts(line, &wrap(line, 4, 0)), ["ab ", "cdef", "ghij ", "k"]);
        // A measure of 0 is a measure of 1.
        assert_eq!(texts("abc", &wrap("abc", 0, 0)), ["a", "b", "c"]);
    }

    #[test]
    fn rows_may_end_after_hyphens_dashes_and_slashes() {
        let line = "well-known";
        assert_eq!(wrap(line, 6, 0), [row(0, 5, 0, 5), row(5, 10, 0, 5)]);
        let line = "either/or";
        assert_eq!(texts(line, &wrap(line, 7, 0)), ["either/", "or"]);
        let line = "wait\u{2014}what\u{2013}now";
        assert_eq!(texts(line, &wrap(line, 5, 0)), ["wait\u{2014}", "what\u{2013}", "now"]);
        // A hyphen with a space after it is not an opportunity of its own:
        // the space is, and it hangs.
        let line = "pre- and post-";
        assert_eq!(texts(line, &wrap(line, 4, 0)), ["pre- ", "and ", "post", "-"]);
        assert_eq!(wrap(line, 4, 0)[0], row(0, 5, 0, 4));
        // The break goes to the last opportunity, not the first.
        let line = "a-b-c-d";
        assert_eq!(texts(line, &wrap(line, 5, 0)), ["a-b-", "c-d"]);
    }

    #[test]
    fn wide_characters_and_marks_stay_whole() {
        let line = "\u{4E00}\u{4E8C}\u{4E09}";
        assert_eq!(wrap(line, 5, 0), [row(0, 6, 0, 4), row(6, 9, 0, 2)]);
        // One cluster to a row even when it does not fit.
        assert_eq!(wrap(line, 1, 0), [row(0, 3, 0, 2), row(3, 6, 0, 2), row(6, 9, 0, 2)]);
        let line = "an\u{0313}an\u{0313}";
        assert_eq!(wrap(line, 2, 0), [row(0, 4, 0, 2), row(4, 8, 0, 2)]);
        assert_eq!(texts(line, &wrap(line, 1, 0)), ["a", "n\u{0313}", "a", "n\u{0313}"]);
        // Marks with no base join the character after them.
        let line = "\u{0301}\u{0301}ab";
        assert_eq!(wrap(line, 1, 0), [row(0, 5, 0, 1), row(5, 6, 0, 1)]);
        assert_eq!(wrap("\u{0301}", 3, 0), [row(0, 2, 0, 0)]);
    }

    #[test]
    fn a_tab_is_four_cells() {
        assert_eq!(wrap("a\tb", 6, 0), [row(0, 3, 0, 6)]);
        assert_eq!(wrap("a\tb", 5, 0), [row(0, 2, 0, 1), row(2, 3, 0, 1)]);
        assert_eq!(wrap("\tab", 2, 0), [row(0, 1, 0, 4), row(1, 3, 0, 2)]);
    }

    #[test]
    fn rows_after_the_first_hang() {
        let line = "- one two three";
        assert_eq!(wrap(line, 9, 2), [row(0, 10, 0, 9), row(10, 15, 2, 5)]);
        // The narrower rows cut a word the first row had room for.
        let line = "a bcdefghij";
        let rows = wrap(line, 10, 6);
        assert_eq!(texts(line, &rows), ["a ", "bcde", "fghi", "j"]);
        assert!(rows[1..].iter().all(|r| r.indent == 6));
        // A hang that leaves no room is no hang.
        assert_eq!(wrap("aaa bbb", 3, 3), [row(0, 4, 0, 3), row(4, 7, 0, 3)]);
        assert_eq!(wrap("aaa bbb", 3, 9), [row(0, 4, 0, 3), row(4, 7, 0, 3)]);
        assert_eq!(wrap("aaa bbb", 3, 2), [row(0, 4, 0, 3), row(4, 5, 2, 1), row(5, 6, 2, 1), row(6, 7, 2, 1)]);
    }

    #[test]
    fn leading_whitespace_is_text() {
        // No row ends between the indent and the word after it, so the two
        // are cut as one long word.
        let line = "    abcd ef";
        assert_eq!(wrap(line, 6, 0), [row(0, 6, 0, 6), row(6, 11, 0, 5)]);
        assert_eq!(wrap(line, 4, 0), [row(0, 4, 0, 4), row(4, 9, 0, 4), row(9, 11, 0, 2)]);
        assert_eq!(wrap("      ", 4, 0), [row(0, 4, 0, 4), row(4, 6, 0, 2)]);
        assert_eq!(wrap("  ab", 4, 0), [row(0, 4, 0, 4)]);
    }

    #[test]
    fn whitespace_ending_the_line_counts_only_while_it_fits() {
        assert_eq!(wrap("ab ", 4, 0), [row(0, 3, 0, 3)]);
        assert_eq!(wrap("ab  ", 4, 0), [row(0, 4, 0, 4)]);
        assert_eq!(wrap("abcd   ", 4, 0), [row(0, 7, 0, 4)]);
        assert_eq!(wrap("abc\t", 4, 0), [row(0, 4, 0, 3)]);
        assert_eq!(wrap("ab cd  ", 3, 0), [row(0, 3, 0, 2), row(3, 7, 0, 3)]);
    }

    #[test]
    fn wrap_matches_the_slow_reference() {
        let mut rnd = rng(0x9E37_79B9);
        for _ in 0..6000 {
            let line = random_line(&mut rnd);
            let measure = rnd(13);
            let hang = rnd(6);
            let rows = wrap(&line, measure, hang);
            assert_eq!(rows, ref_wrap(&line, measure, hang), "{line:?} at {measure}, hang {hang}");
        }
    }

    #[test]
    fn every_row_is_whole_contiguous_and_within_the_measure() {
        let mut rnd = rng(0x1234_5678);
        for _ in 0..6000 {
            let line = random_line(&mut rnd);
            let measure = 1 + rnd(12);
            let hang = rnd(8);
            let rows = wrap(&line, measure, hang);
            let clusters = ref_clusters(&line);
            let kept = if hang >= measure { 0 } else { hang };
            assert_eq!(rows[0].start, 0);
            assert_eq!(rows[rows.len() - 1].end, line.len());
            for (i, r) in rows.iter().enumerate() {
                let context = format!("{line:?} at {measure}, hang {hang}, row {i}");
                assert_eq!(r.indent, if i == 0 { 0 } else { kept }, "{context}");
                if i > 0 {
                    assert_eq!(r.start, rows[i - 1].end, "{context}");
                    assert!(r.start < r.end, "{context}");
                    assert!(clusters.iter().any(|c| c.start == r.start), "{context}");
                }
                let inside: Vec<&Ref> = clusters.iter().filter(|c| r.start <= c.start && c.start < r.end).collect();
                let mut text = inside.len();
                while text > 1 && inside[text - 1].space {
                    text -= 1;
                }
                assert!(r.indent + r.cells <= measure || text == 1, "{context}");
                assert!(r.cells <= str_width(&line[r.start..r.end]), "{context}");
                // Only whitespace is ever left out of the count.
                let counted: usize = inside[..text].iter().map(|c| c.width).sum();
                assert!(inside.is_empty() || r.cells >= counted.min(measure - r.indent), "{context}");
            }
        }
    }

    #[test]
    fn cell_of_counts_from_the_indent() {
        let line = "- one two three";
        let rows = wrap(line, 9, 2);
        assert_eq!(cell_of(line, &rows[0], 0), 0);
        assert_eq!(cell_of(line, &rows[0], 6), 6);
        // The end of a row is the cell after everything on it, hanging or not.
        assert_eq!(cell_of(line, &rows[0], 10), 10);
        assert_eq!(cell_of(line, &rows[0], 99), 10);
        assert_eq!(cell_of(line, &rows[1], 10), 2);
        assert_eq!(cell_of(line, &rows[1], 12), 4);
        assert_eq!(cell_of(line, &rows[1], 15), 7);
        assert_eq!(cell_of(line, &rows[1], 3), 2);
    }

    #[test]
    fn cell_of_respects_wide_characters_and_clusters() {
        let line = "a\u{4E00}b";
        let rows = wrap(line, 10, 0);
        assert_eq!(cell_of(line, &rows[0], 1), 1);
        assert_eq!(cell_of(line, &rows[0], 2), 1); // inside the wide character
        assert_eq!(cell_of(line, &rows[0], 4), 3);
        assert_eq!(cell_of(line, &rows[0], 5), 4);
        let line = "n\u{0313}a";
        let rows = wrap(line, 10, 0);
        assert_eq!(cell_of(line, &rows[0], 1), 0); // on the mark: back to its base
        assert_eq!(cell_of(line, &rows[0], 3), 1);
        assert_eq!(cell_of("a\tb", &row(0, 3, 0, 6), 2), 5);
    }

    #[test]
    fn col_at_finds_the_cluster_under_a_cell() {
        let line = "- one two three";
        let rows = wrap(line, 9, 2);
        assert_eq!(col_at(line, &rows[1], 0, true), 10);
        assert_eq!(col_at(line, &rows[1], 2, true), 10);
        assert_eq!(col_at(line, &rows[1], 3, true), 11);
        assert_eq!(col_at(line, &rows[1], 6, true), 14);
        assert_eq!(col_at(line, &rows[1], 7, true), 15);
        assert_eq!(col_at(line, &rows[1], 7, false), 14);
        // Past the text of a row that is followed by another: its last
        // cluster, here the space it was broken at.
        assert_eq!(col_at(line, &rows[0], 50, false), 9);
        let line = "a\u{4E00}b";
        let rows = wrap(line, 10, 0);
        assert_eq!(col_at(line, &rows[0], 1, true), 1);
        assert_eq!(col_at(line, &rows[0], 2, true), 1);
        assert_eq!(col_at(line, &rows[0], 3, true), 4);
        assert_eq!(col_at(line, &rows[0], 4, true), 5);
        assert_eq!(col_at("", &Row::default(), 3, true), 0);
        assert_eq!(col_at("", &Row::default(), 3, false), 0);
    }

    #[test]
    fn rows_that_do_not_match_the_text_are_harmless() {
        let stale = row(2, 400, 3, 9);
        for line in ["", "a", "caf\u{00E9}", "\u{0301}", "\u{4E00}\u{4E00}"] {
            for at in 0..8 {
                let cell = cell_of(line, &stale, at);
                assert!(cell >= 3);
                assert!(line.is_char_boundary(col_at(line, &stale, at, true)));
                assert!(line.is_char_boundary(col_at(line, &stale, at, false)));
            }
            assert!(line.is_char_boundary(end_of_text(line, &stale)));
        }
        let backwards = row(5, 1, 0, 0);
        assert_eq!(cell_of("abcdefg", &backwards, 3), 0);
        assert_eq!(col_at("abcdefg", &backwards, 3, false), 1);
    }

    fn sample() -> (Doc, Layout) {
        let doc = Doc::from_text("alpha beta gamma\n\n- one two three");
        let layout = Layout::new(&doc, 10, &hang_of);
        (doc, layout)
    }

    #[test]
    fn the_index_maps_lines_to_rows_and_back() {
        let (doc, layout) = sample();
        assert_eq!(layout.measure(), 10);
        assert_eq!(layout.rows(0), [row(0, 11, 0, 10), row(11, 16, 0, 5)]);
        assert_eq!(layout.rows(1), [Row::default()]);
        assert_eq!(layout.rows(2), [row(0, 10, 0, 9), row(10, 15, 2, 5)]);
        assert!(layout.rows(3).is_empty());
        assert_eq!(layout.total_rows(), 5);
        assert_eq!([0, 1, 2, 3, 9].map(|l| layout.first_row(l)), [0, 2, 3, 5, 5]);
        assert_eq!([0, 1, 2, 3, 4].map(|r| layout.line_of_row(r)), [(0, 0), (0, 1), (1, 0), (2, 0), (2, 1)]);
        assert_eq!(layout.line_of_row(99), (2, 1));
        let empty = Layout::new(&Doc::new(), 0, &plain);
        assert_eq!(empty.total_rows(), 1);
        assert_eq!(empty.measure(), 0);
        assert_eq!(empty.line_of_row(5), (0, 0));
        assert_eq!(empty.locate(&Doc::new(), p(4, 4)), (0, 0));
        assert_eq!(doc.line_count(), 3);
    }

    #[test]
    fn a_position_at_a_soft_break_is_drawn_on_the_next_row() {
        let (doc, layout) = sample();
        assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
        assert_eq!(layout.locate(&doc, p(0, 10)), (0, 10));
        assert_eq!(layout.locate(&doc, p(0, 11)), (1, 0));
        assert_eq!(layout.locate(&doc, p(0, 16)), (1, 5));
        assert_eq!(layout.locate(&doc, p(1, 0)), (2, 0));
        assert_eq!(layout.locate(&doc, p(2, 10)), (4, 2));
        assert_eq!(layout.locate(&doc, p(2, 12)), (4, 4));
        // Out of range is the end of the document.
        assert_eq!(layout.locate(&doc, p(9, 9)), (4, 7));

        // After a full row the cursor rests one cell past the measure, and
        // so does the space the row was broken at.
        let doc = Doc::from_text("abcd efgh");
        let layout = Layout::new(&doc, 4, &plain);
        assert_eq!(layout.locate(&doc, p(0, 4)), (0, 4));
        assert_eq!(layout.locate(&doc, p(0, 5)), (1, 0));
        assert_eq!(layout.locate(&doc, p(0, 9)), (1, 4));
    }

    #[test]
    fn pos_at_is_the_inverse_of_locate() {
        let (doc, layout) = sample();
        assert_eq!(layout.pos_at(&doc, 0, 3), p(0, 3));
        assert_eq!(layout.pos_at(&doc, 0, 99), p(0, 10));
        assert_eq!(layout.pos_at(&doc, 1, 99), p(0, 16));
        assert_eq!(layout.pos_at(&doc, 2, 4), p(1, 0));
        assert_eq!(layout.pos_at(&doc, 4, 0), p(2, 10));
        assert_eq!(layout.pos_at(&doc, 4, 3), p(2, 11));
        assert_eq!(layout.pos_at(&doc, 99, 99), p(2, 15));
    }

    #[test]
    fn up_and_down_move_by_wrapped_rows() {
        let (doc, layout) = sample();
        assert_eq!(layout.up(&doc, p(0, 13), 2), p(0, 2));
        assert_eq!(layout.up(&doc, p(0, 3), 3), p(0, 0));
        assert_eq!(layout.down(&doc, p(0, 3), 3), p(0, 14));
        assert_eq!(layout.down(&doc, p(0, 14), 3), p(1, 0));
        assert_eq!(layout.down(&doc, p(1, 0), 3), p(2, 3));
        assert_eq!(layout.down(&doc, p(2, 3), 3), p(2, 11));
        assert_eq!(layout.down(&doc, p(2, 12), 0), p(2, 15));
        assert_eq!(layout.up(&doc, p(2, 12), 0), p(2, 0));
    }

    #[test]
    fn up_and_down_take_the_nearer_edge_of_a_tab() {
        // Rows 0 and 2 can hold a cursor at cells 0, 4 and 5; row 4 at
        // cells 0, 1 and 5.
        let doc = Doc::from_text("\tx\nabcde\n\tx\nabcde\na\t");
        let layout = Layout::new(&doc, 20, &plain);
        let wanted = [0, 1, 2, 3, 4, 5, 9];
        assert_eq!(wanted.map(|cell| layout.up(&doc, p(1, 0), cell)), [0, 0, 0, 1, 1, 2, 2].map(|col| p(0, col)));
        assert_eq!(wanted.map(|cell| layout.down(&doc, p(1, 0), cell)), [0, 0, 0, 1, 1, 2, 2].map(|col| p(2, col)));
        assert_eq!(wanted.map(|cell| layout.down(&doc, p(3, 0), cell)), [0, 1, 1, 1, 2, 2, 2].map(|col| p(4, col)));
        // pos_at still names the character under the cell.
        assert_eq!(layout.pos_at(&doc, 0, 3), p(0, 0));
        assert_eq!(layout.pos_at(&doc, 4, 4), p(4, 1));

        // The far edge of a tab that ends a row is where the next row
        // starts, so the cursor stays on the tab.
        let doc = Doc::from_text("ab\t\tcd\nx");
        let layout = Layout::new(&doc, 4, &plain);
        assert_eq!(layout.rows(0), [row(0, 4, 0, 2), row(4, 6, 0, 2)]);
        assert_eq!([4, 5, 6, 9, 40].map(|cell| layout.up(&doc, p(0, 4), cell)), [2, 3, 3, 3, 3].map(|col| p(0, col)));
        // A wide character has no nearer edge: halfway across is its start.
        let doc = Doc::from_text("\u{4E00}\u{4E8C}\nx");
        let layout = Layout::new(&doc, 9, &plain);
        assert_eq!([0, 1, 2, 3, 4].map(|cell| layout.up(&doc, p(1, 0), cell)), [0, 0, 3, 3, 6].map(|col| p(0, col)));
    }

    #[test]
    fn home_and_end_stay_on_the_wrapped_row() {
        let (doc, layout) = sample();
        assert_eq!(layout.row_start(&doc, p(0, 13)), p(0, 11));
        assert_eq!(layout.row_start(&doc, p(0, 3)), p(0, 0));
        // Broken at a space: End stops after the last word.
        assert_eq!(layout.row_end(&doc, p(0, 3)), p(0, 10));
        assert_eq!(layout.row_end(&doc, p(0, 13)), p(0, 16));
        assert_eq!(layout.row_end(&doc, p(1, 0)), p(1, 0));
        assert_eq!(layout.row_end(&doc, p(2, 0)), p(2, 9));

        // Broken inside a word or after a hyphen: End stops on the last cluster.
        let doc = Doc::from_text("abcdefgh\nwell-known\n      x");
        let layout = Layout::new(&doc, 4, &plain);
        assert_eq!(layout.row_end(&doc, p(0, 1)), p(0, 3));
        assert_eq!(layout.row_start(&doc, p(0, 4)), p(0, 4));
        assert_eq!(layout.row_end(&doc, p(0, 4)), p(0, 8));
        assert_eq!(layout.rows(1)[1], row(4, 5, 0, 1));
        assert_eq!(layout.row_end(&doc, p(1, 4)), p(1, 4));
        assert_eq!(layout.row_end(&doc, p(1, 6)), p(1, 8));
        // A row of nothing but the line's leading whitespace.
        assert_eq!(layout.row_end(&doc, p(2, 1)), p(2, 3));
    }

    #[test]
    fn rewrap_takes_a_new_measure() {
        let (doc, mut layout) = sample();
        layout.rewrap(&doc, 40, &hang_of);
        assert_eq!(layout.measure(), 40);
        assert_eq!(layout.total_rows(), 3);
        assert_eq!(layout, Layout::new(&doc, 40, &hang_of));
        layout.rewrap(&doc, 3, &hang_of);
        assert_eq!(layout, Layout::new(&doc, 3, &hang_of));
    }

    #[test]
    fn apply_wraps_only_the_lines_a_change_touched() {
        let mut doc = Doc::from_text("one two three four\nfive six seven\n- eight nine ten\nlast");
        let calls = Cell::new(0);
        let hang = |at: usize, line: &str| {
            calls.set(calls.get() + 1);
            hang_of(at, line)
        };
        let mut layout = Layout::new(&doc, 8, &hang);
        assert_eq!(calls.get(), 4);

        calls.set(0);
        let change = doc.replace(Span::at(p(1, 4)), " and a half");
        layout.apply(&doc, &change, &hang);
        assert_eq!(calls.get(), 1);
        assert_eq!(layout, Layout::new(&doc, 8, &hang_of));

        calls.set(0);
        let change = doc.replace(Span::new(p(0, 3), p(2, 7)), "\n\n");
        layout.apply(&doc, &change, &hang);
        assert_eq!(calls.get(), 3);
        assert_eq!(layout, Layout::new(&doc, 8, &hang_of));

        // The hang is asked about the line as it reads after the change.
        assert_eq!(layout.rows(3), [row(0, 4, 0, 4)]);
        let change = doc.replace(Span::at(p(3, 0)), "- the very ");
        layout.apply(&doc, &change, &hang);
        assert_eq!(layout.rows(3), [row(0, 6, 0, 5), row(6, 11, 2, 4), row(11, 15, 2, 4)]);
        assert_eq!(layout, Layout::new(&doc, 8, &hang_of));
    }

    #[test]
    fn apply_rebuilds_a_layout_that_fell_out_of_step() {
        let mut doc = Doc::from_text("a\nb\nc\nd");
        let mut layout = Layout::new(&doc, 5, &plain);
        doc.replace(Span::new(p(0, 0), p(2, 0)), "");
        let change = doc.replace(Span::at(p(1, 1)), " more words here");
        layout.apply(&doc, &change, &plain);
        assert_eq!(layout, Layout::new(&doc, 5, &plain));

        let impossible = Change { start: p(3, 0), old_end: p(1, 0), end: p(0, 0), ..Change::default() };
        layout.apply(&doc, &impossible, &plain);
        assert_eq!(layout, Layout::new(&doc, 5, &plain));
        let beyond = Change { start: p(40, 0), old_end: p(44, 0), end: p(41, 0), ..Change::default() };
        layout.apply(&doc, &beyond, &plain);
        assert_eq!(layout, Layout::new(&doc, 5, &plain));
        // Line numbers too large to count lines from.
        let top = usize::MAX;
        for (start, old_end, end) in [(0, top, 0), (0, 0, top), (0, top, top), (top, top, top), (1, top - 1, 1)] {
            let wild = Change { start: p(start, 0), old_end: p(old_end, 0), end: p(end, 0), ..Change::default() };
            layout.apply(&doc, &wild, &plain);
            assert_eq!(layout, Layout::new(&doc, 5, &plain));
        }
    }

    /// Wherever the cursor wants to be drawn, Up and Down give it the place
    /// on the row above or below that is nearest: one for each cluster, and
    /// on a line's last row one more past the end.
    #[test]
    fn up_and_down_land_as_near_as_the_row_allows() {
        let mut rnd = rng(0x0DEC_0DED);
        for _ in 0..300 {
            let doc = Doc::from_text(&random_text(&mut rnd, 40));
            let measure = 1 + rnd(24);
            let layout = Layout::new(&doc, measure, &hang_of);
            let total = layout.total_rows();
            for row in 0..total {
                let (line, within) = layout.line_of_row(row);
                let rows = layout.rows(line);
                let found = rows[within];
                let mut cell = found.indent;
                let mut rests = Vec::new();
                for c in ref_clusters(&doc.line(line)[found.start..found.end]) {
                    rests.push(cell);
                    cell += c.width;
                }
                if within + 1 == rows.len() {
                    rests.push(cell);
                }
                for want in 0..measure + 6 {
                    let best = (row, rests.iter().map(|rest| rest.abs_diff(want)).min());
                    let text = doc.line(line);
                    if row + 1 < total {
                        let from = layout.pos_at(&doc, row + 1, want);
                        let (landed, drawn) = layout.locate(&doc, layout.up(&doc, from, want));
                        let got = (landed, Some(drawn.abs_diff(want)));
                        assert_eq!(got, best, "up to cell {want} of {text:?} at {measure}");
                    }
                    if row > 0 {
                        let from = layout.pos_at(&doc, row - 1, want);
                        let (landed, drawn) = layout.locate(&doc, layout.down(&doc, from, want));
                        let got = (landed, Some(drawn.abs_diff(want)));
                        assert_eq!(got, best, "down to cell {want} of {text:?} at {measure}");
                    }
                }
            }
        }
    }

    /// Everything the layout promises about one document, checked position
    /// by position.
    fn check(doc: &Doc, layout: &Layout) {
        let measure = layout.measure().max(1);
        let total = layout.total_rows();
        let mut counted = 0;
        for line in 0..doc.line_count() {
            let text = doc.line(line);
            let rows = layout.rows(line);
            assert_eq!(layout.first_row(line), counted);
            assert_eq!(rows[0].start, 0);
            assert_eq!(rows[rows.len() - 1].end, text.len());
            for (i, r) in rows.iter().enumerate() {
                assert_eq!(layout.line_of_row(counted + i), (line, i));
                assert!(i == 0 || r.start == rows[i - 1].end);
                let single = ref_clusters(&text[r.start..r.end]).iter().filter(|c| !c.space).count() <= 1;
                assert!(r.indent + r.cells <= measure || single, "{text:?} row {i} at {measure}");
            }
            counted += rows.len();

            for c in ref_clusters(text) {
                let at = p(line, c.start);
                let (row, cell) = layout.locate(doc, at);
                assert_eq!(layout.line_of_row(row).0, line);
                assert_eq!(layout.pos_at(doc, row, cell), at, "{text:?} at {measure}");

                let home = layout.row_start(doc, at);
                let end = layout.row_end(doc, at);
                assert!(home <= at && home <= end);
                assert_eq!(layout.locate(doc, home), (row, rows[row - layout.first_row(line)].indent));
                assert_eq!(layout.locate(doc, end).0, row, "{text:?} at {measure}");

                let below = layout.down(doc, at, cell);
                if row + 1 < total {
                    assert_eq!(layout.locate(doc, below).0, row + 1);
                    assert_eq!(layout.locate(doc, layout.up(doc, below, cell)).0, row);
                } else {
                    assert_eq!(below, doc.end());
                }
                let above = layout.up(doc, at, cell);
                if row > 0 {
                    assert_eq!(layout.locate(doc, above).0, row - 1);
                    assert_eq!(layout.locate(doc, layout.down(doc, above, cell)).0, row);
                } else {
                    assert_eq!(above, p(0, 0));
                }
            }

            // The end of a line is drawn on its last row, and what is found
            // there is drawn in the same place.
            let end = p(line, text.len());
            let (row, cell) = layout.locate(doc, end);
            assert_eq!(row, counted - 1);
            assert_eq!(layout.locate(doc, layout.pos_at(doc, row, cell)), (row, cell));
            assert_eq!(layout.row_end(doc, end), end);
        }
        assert_eq!(counted, total);
        assert_eq!(layout.first_row(doc.line_count()), total);
        assert_eq!(layout.line_of_row(total + 3), layout.line_of_row(total - 1));
    }

    #[test]
    fn a_layout_kept_in_step_equals_one_built_fresh() {
        for (seed, measure) in [(0x2545_F491u32, 1), (0x0BAD_5EED, 2), (0x1357_9BDF, 5), (0x0F1E_2D3C, 9), (0x7654_3210, 24), (0x0000_0042, 72)] {
            let mut rnd = rng(seed);
            let mut doc = Doc::new();
            let mut layout = Layout::new(&doc, measure, &hang_of);
            for step in 0..700 {
                let len = doc.len_bytes();
                let a = rnd(len + 1);
                let b = a + rnd(9) * rnd(2);
                let piece = PIECES[rnd(PIECES.len())];
                let change = doc.replace(Span::new(doc.pos_at(a), doc.pos_at(b)), piece);
                layout.apply(&doc, &change, &hang_of);
                assert_eq!(layout, Layout::new(&doc, measure, &hang_of), "step {step} at {measure}");
                if step % 50 == 49 {
                    check(&doc, &layout);
                }
            }
            check(&doc, &layout);

            // Undoing everything brings the layout back to an empty document's.
            let all = Span::new(p(0, 0), doc.end());
            let change = doc.replace(all, "");
            layout.apply(&doc, &change, &hang_of);
            assert_eq!(layout, Layout::new(&Doc::new(), measure, &hang_of));
        }
    }

    #[test]
    fn positions_round_trip_through_rows_and_cells() {
        let mut rnd = rng(0x00C0_FFEE);
        for _ in 0..300 {
            let doc = Doc::from_text(&random_text(&mut rnd, 40));
            let layout = Layout::new(&doc, rnd(30), &hang_of);
            check(&doc, &layout);
        }
    }

    /// A layout asked about a text it was not made for may answer wrongly,
    /// but it must answer, and with a position that text really has.
    #[test]
    fn a_layout_for_another_text_never_panics() {
        let mut rnd = rng(0x5EED_1E55);
        let wild = |rnd: &mut dyn FnMut(usize) -> usize| match rnd(4) {
            0 => usize::MAX - rnd(3),
            _ => rnd(40),
        };
        for _ in 0..1500 {
            let made_for = Doc::from_text(&random_text(&mut rnd, 30));
            let mut asked = Doc::from_text(&random_text(&mut rnd, 30));
            let mut layout = Layout::new(&made_for, rnd(12), &hang_of);
            for _ in 0..12 {
                let at = p(wild(&mut rnd), wild(&mut rnd));
                let (row, cell) = (wild(&mut rnd), wild(&mut rnd));
                layout.locate(&asked, at);
                layout.line_of_row(row);
                layout.first_row(row);
                let found = [
                    layout.pos_at(&asked, row, cell),
                    layout.up(&asked, at, cell),
                    layout.down(&asked, at, cell),
                    layout.row_start(&asked, at),
                    layout.row_end(&asked, at),
                ];
                for q in found {
                    assert_eq!(asked.clamp(q), q);
                }
                let line = asked.line(rnd(asked.line_count()));
                let stale = Row { start: wild(&mut rnd), end: wild(&mut rnd), indent: wild(&mut rnd), cells: wild(&mut rnd) };
                for r in layout.rows(rnd(4)).iter().chain([&stale]) {
                    cell_of(line, r, cell);
                    assert!(line.is_char_boundary(col_at(line, r, cell, rnd(2) == 0)));
                }
            }
            let change = asked.replace(Span::new(p(rnd(4), rnd(9)), p(rnd(4), rnd(9))), PIECES[rnd(PIECES.len())]);
            layout.apply(&asked, &change, &hang_of);
            layout.locate(&asked, asked.end());
        }
    }

    #[test]
    fn positions_outside_the_document_are_clamped() {
        let (doc, layout) = sample();
        for at in [p(0, 999), p(7, 0), p(2, 14), p(usize::MAX, usize::MAX)] {
            let (row, cell) = layout.locate(&doc, at);
            assert!(row < layout.total_rows());
            assert_eq!(layout.locate(&doc, layout.pos_at(&doc, row, cell)), (row, cell));
            layout.up(&doc, at, usize::MAX);
            layout.down(&doc, at, usize::MAX);
            layout.row_start(&doc, at);
            layout.row_end(&doc, at);
        }
        assert_eq!(layout.pos_at(&doc, usize::MAX, usize::MAX), doc.end());
        // Inside a character: the same place as its start.
        let doc = Doc::from_text("caf\u{00E9} n\u{0313}a");
        let layout = Layout::new(&doc, 3, &plain);
        assert_eq!(layout.locate(&doc, p(0, 4)), layout.locate(&doc, p(0, 3)));
        assert_eq!(layout.locate(&doc, p(0, 7)), layout.locate(&doc, p(0, 6)));
        // A layout for some other document must not panic either.
        let other = Doc::from_text("x");
        layout.locate(&other, p(0, 1));
        layout.pos_at(&other, 2, 2);
        layout.row_end(&other, p(0, 0));
        let longer = Doc::from_text("one\ntwo\nthree four five six");
        layout.locate(&longer, p(2, 9));
        layout.down(&longer, p(2, 9), 4);
        layout.row_start(&longer, p(2, 9));
        layout.row_end(&longer, p(2, 9));
    }

    /// Five lines, one of each shape the page has, at a measure of 12:
    ///
    ///   rows 0-1   Big          (line 0, "# " hung, double size)
    ///   rows 2-3   title
    ///   rows 4-5   here
    ///   row 6                   (line 1)
    ///   row 7      plain text   (line 2)
    ///   row 8      of some
    ///   row 9      words
    ///   row 10       quoted     (line 3, "> " hung, set in by 2)
    ///   row 11       words go
    ///   row 12       here
    ///   row 13     - item one   (line 4)
    ///   row 14       two three
    fn shaped_sample() -> (Doc, Layout) {
        let doc = Doc::from_text("# Big title here\n\nplain text of some words\n> quoted words go here\n- item one two three");
        let layout = Layout::new(&doc, 12, &page_shape);
        (doc, layout)
    }

    #[test]
    fn a_shape_hangs_bytes_halves_the_measure_and_sets_rows_in() {
        let (_, layout) = shaped_sample();
        assert_eq!(layout.rows(0), [row(2, 6, 0, 3), row(6, 12, 0, 5), row(12, 16, 0, 4)]);
        assert_eq!(layout.shape(0), Shape { hung: 2, scale: 2, hang: 0, inset: 0 });
        assert_eq!(layout.rows(3), [row(2, 9, 2, 6), row(9, 18, 2, 8), row(18, 22, 2, 4)]);
        assert_eq!(layout.shape(3), Shape { hung: 2, scale: 1, hang: 0, inset: 2 });
        assert_eq!(layout.rows(4), [row(0, 11, 0, 10), row(11, 20, 2, 9)]);
        assert_eq!(layout.shape(9), Shape::default());
        // A line that is nothing but its mark still has a row, after the mark.
        let doc = Doc::from_text("# \n#");
        let layout = Layout::new(&doc, 12, &|_, _| Shape { hung: 2, scale: 2, ..Shape::default() });
        assert_eq!(layout.rows(0), [row(2, 2, 0, 0)]);
        assert_eq!(layout.rows(1), [row(1, 1, 0, 0)]);
        assert_eq!(layout.shape(1).hung, 1);
        assert_eq!(layout.total_rows(), 4);
    }

    #[test]
    fn shapes_that_cannot_be_are_brought_back_to_ones_that_can() {
        let doc = Doc::from_text("caf\u{00E9} au lait");
        // Inside the two-byte letter: back to its start. Past the end: the end.
        let layout = Layout::new(&doc, 6, &|_, _| Shape { hung: 4, ..Shape::default() });
        assert_eq!(layout.shape(0).hung, 3);
        assert_eq!(layout.rows(0)[0].start, 3);
        let layout = Layout::new(&doc, 6, &|_, _| Shape { hung: usize::MAX, scale: 0, hang: usize::MAX, inset: usize::MAX });
        assert_eq!(layout.shape(0), Shape { hung: 13, scale: 1, hang: usize::MAX, inset: 0 });
        assert_eq!(layout.rows(0), [row(13, 13, 0, 0)]);
        // A scale above 2 is 2; an inset that leaves a cell is kept, one
        // that leaves none is dropped.
        let layout = Layout::new(&doc, 6, &|_, _| Shape { scale: 9, inset: 1, ..Shape::default() });
        assert_eq!(layout.shape(0), Shape { hung: 0, scale: 2, hang: 0, inset: 1 });
        assert!(layout.rows(0).iter().all(|r| r.indent == 1 && r.cells <= 1));
        let layout = Layout::new(&doc, 6, &|_, _| Shape { inset: 3, ..Shape::default() });
        assert_eq!(layout.shape(0).inset, 0);
        let layout = Layout::new(&doc, 0, &|_, _| Shape { scale: 2, inset: 1, ..Shape::default() });
        assert_eq!(layout.shape(0).inset, 0);
        assert_eq!(layout.total_rows(), 2 * layout.rows(0).len());
    }

    #[test]
    fn the_index_counts_grid_rows_on_lines_of_mixed_height() {
        let (doc, layout) = shaped_sample();
        assert_eq!(layout.total_rows(), 15);
        assert_eq!([0, 1, 2, 3, 4, 5].map(|l| layout.first_row(l)), [0, 6, 7, 10, 13, 15]);
        let found: Vec<(usize, usize)> = (0..15).map(|r| layout.line_of_row(r)).collect();
        let lines = [0, 0, 0, 0, 0, 0, 1, 2, 2, 2, 3, 3, 3, 4, 4];
        let within = [0, 0, 1, 1, 2, 2, 0, 0, 1, 2, 0, 1, 2, 0, 1];
        assert_eq!(found, lines.into_iter().zip(within).collect::<Vec<_>>());
        assert_eq!(layout.line_of_row(99), (4, 1));
        assert_eq!(doc.line_count(), 5);
    }

    #[test]
    fn locate_and_pos_at_speak_in_grid_rows_and_cells() {
        let (doc, layout) = shaped_sample();
        // A double-size letter is two cells wide: its left cell is given.
        assert_eq!(layout.locate(&doc, p(0, 2)), (0, 0));
        assert_eq!(layout.locate(&doc, p(0, 5)), (0, 6));
        assert_eq!(layout.locate(&doc, p(0, 6)), (2, 0));
        assert_eq!(layout.locate(&doc, p(0, 16)), (4, 8));
        // Inside the hung mark: the start of the first row.
        assert_eq!(layout.locate(&doc, p(0, 0)), (0, 0));
        assert_eq!(layout.locate(&doc, p(0, 1)), (0, 0));
        assert_eq!(layout.locate(&doc, p(3, 0)), (10, 2));
        assert_eq!(layout.locate(&doc, p(3, 2)), (10, 2));
        assert_eq!(layout.locate(&doc, p(3, 9)), (11, 2));
        assert_eq!(layout.locate(&doc, p(4, 13)), (14, 4));
        // Any of the four grid cells of a double-size letter names it, and
        // nothing names a byte of the mark.
        for (row, cell) in [(0, 6), (0, 7), (1, 6), (1, 7)] {
            assert_eq!(layout.pos_at(&doc, row, cell), p(0, 5));
        }
        assert_eq!(layout.pos_at(&doc, 1, 0), p(0, 2));
        assert_eq!(layout.pos_at(&doc, 3, 99), p(0, 11));
        assert_eq!(layout.pos_at(&doc, 5, 99), p(0, 16));
        assert_eq!(layout.pos_at(&doc, 10, 0), p(3, 2));
        assert_eq!(layout.pos_at(&doc, 10, 3), p(3, 3));
        assert_eq!(layout.pos_at(&doc, 12, 99), p(3, 22));
    }

    #[test]
    fn up_and_down_step_over_a_double_size_row_whole() {
        let (doc, layout) = shaped_sample();
        assert_eq!(layout.down(&doc, p(0, 2), 0), p(0, 6));
        assert_eq!(layout.down(&doc, p(0, 3), 2), p(0, 7));
        assert_eq!(layout.down(&doc, p(0, 13), 4), p(1, 0));
        assert_eq!(layout.up(&doc, p(1, 0), 4), p(0, 14));
        assert_eq!(layout.up(&doc, p(1, 0), 5), p(0, 14));
        assert_eq!(layout.up(&doc, p(0, 14), 4), p(0, 8));
        assert_eq!(layout.up(&doc, p(0, 3), 9), p(0, 0));
        assert_eq!(layout.down(&doc, p(1, 0), 5), p(2, 5));
        assert_eq!(layout.down(&doc, p(2, 20), 0), p(3, 2));
        assert_eq!(layout.up(&doc, p(4, 0), 7), p(3, 22));
        assert_eq!(layout.down(&doc, p(4, 12), 3), p(4, 20));
        // From inside the mark, as from the start of the text beside it.
        assert_eq!(layout.down(&doc, p(0, 0), 2), layout.down(&doc, p(0, 2), 2));
        assert_eq!(layout.down(&doc, p(3, 1), 2), layout.down(&doc, p(3, 2), 2));
    }

    #[test]
    fn home_and_end_keep_out_of_the_hung_mark() {
        let (doc, layout) = shaped_sample();
        assert_eq!(layout.row_start(&doc, p(0, 4)), p(0, 2));
        assert_eq!(layout.row_start(&doc, p(0, 0)), p(0, 2));
        assert_eq!(layout.row_start(&doc, p(0, 9)), p(0, 6));
        assert_eq!(layout.row_end(&doc, p(0, 0)), p(0, 5));
        assert_eq!(layout.row_end(&doc, p(0, 13)), p(0, 16));
        assert_eq!(layout.row_start(&doc, p(3, 12)), p(3, 9));
        assert_eq!(layout.row_end(&doc, p(3, 12)), p(3, 17));
    }

    /// Everything the layout promises about one document whose lines are of
    /// mixed shape, checked position by position on the grid.
    fn check_shaped(doc: &Doc, layout: &Layout) {
        let total = layout.total_rows();
        let mut counted = 0;
        for line in 0..doc.line_count() {
            let text = doc.line(line);
            let rows = layout.rows(line);
            let shape = layout.shape(line);
            let unit = shape.scale;
            assert!(unit == 1 || unit == 2);
            assert!(text.is_char_boundary(shape.hung));
            assert_eq!(layout.first_row(line), counted);
            assert_eq!(rows[0].start, shape.hung);
            assert_eq!(rows[rows.len() - 1].end, text.len());
            for (i, r) in rows.iter().enumerate() {
                for half in 0..unit {
                    assert_eq!(layout.line_of_row(counted + i * unit + half), (line, i));
                }
                assert!(i == 0 || r.start == rows[i - 1].end);
                assert!(r.indent >= shape.inset);
            }
            let top_of = |row: usize| counted + row * unit;

            // Every byte of the mark is drawn, as far as the layout knows,
            // where the text beside it starts.
            for (col, _) in text.char_indices().take_while(|&(col, _)| col < shape.hung) {
                assert_eq!(layout.locate(doc, p(line, col)), layout.locate(doc, p(line, shape.hung)), "{text:?}");
            }
            // The text after the mark is read as a line of its own, so marks
            // at its head join the character after them.
            for c in ref_clusters(&text[shape.hung..]) {
                let at = p(line, shape.hung + c.start);
                let (row, cell) = layout.locate(doc, at);
                let within = (row - counted) / unit;
                assert_eq!(row, top_of(within));
                assert_eq!(cell % unit, 0);
                for (dr, dc) in [(0, 0), (unit - 1, 0), (0, unit - 1), (unit - 1, unit - 1)] {
                    assert_eq!(layout.pos_at(doc, row + dr, cell + dc), at, "{text:?} at {}", layout.measure());
                }

                let home = layout.row_start(doc, at);
                let end = layout.row_end(doc, at);
                assert!(home <= at && home <= end && home.col >= shape.hung);
                assert_eq!(layout.locate(doc, home), (row, rows[within].indent * unit));
                assert_eq!(layout.locate(doc, end).0, row, "{text:?}");

                let below = layout.down(doc, at, cell);
                if row + unit < total {
                    assert_eq!(layout.locate(doc, below).0, row + unit, "{text:?}");
                    assert_eq!(layout.locate(doc, layout.up(doc, below, cell)).0, row);
                } else {
                    assert_eq!(below, doc.end());
                }
                let above = layout.up(doc, at, cell);
                if row > 0 {
                    let landed = layout.locate(doc, above).0;
                    let (over, _) = layout.line_of_row(row - 1);
                    assert_eq!(landed + layout.shape(over).scale, row, "{text:?}");
                    assert_eq!(layout.locate(doc, layout.down(doc, above, cell)).0, row);
                } else {
                    assert_eq!(above, p(0, 0));
                }
            }

            let end = p(line, text.len());
            let (row, cell) = layout.locate(doc, end);
            assert_eq!(row, top_of(rows.len() - 1));
            assert_eq!(layout.locate(doc, layout.pos_at(doc, row, cell)), (row, cell));
            assert_eq!(layout.row_end(doc, end), end);
            counted += rows.len() * unit;
        }
        assert_eq!(counted, total);
        assert_eq!(layout.first_row(doc.line_count()), total);
        assert_eq!(layout.line_of_row(total + 3), layout.line_of_row(total - 1));
    }

    #[test]
    fn a_shaped_layout_kept_in_step_equals_one_built_fresh() {
        const SHAPED: [&str; 8] = ["\n# ", "\n## ", "\n> ", "\n- item ", "# ", "> ", "#", " "];
        for (seed, measure) in [(0x2545_F491u32, 1), (0x0BAD_5EED, 2), (0x1357_9BDF, 7), (0x0F1E_2D3C, 12), (0x7654_3210, 33), (0x0000_0042, 66)] {
            let mut rnd = rng(seed);
            let mut doc = Doc::new();
            let mut layout = Layout::new(&doc, measure, &page_shape);
            for step in 0..700 {
                let len = doc.len_bytes();
                let a = rnd(len + 1);
                let b = a + rnd(9) * rnd(2);
                let piece = if rnd(4) == 0 { SHAPED[rnd(SHAPED.len())] } else { PIECES[rnd(PIECES.len())] };
                let change = doc.replace(Span::new(doc.pos_at(a), doc.pos_at(b)), piece);
                layout.apply(&doc, &change, &page_shape);
                assert_eq!(layout, Layout::new(&doc, measure, &page_shape), "step {step} at {measure}");
                if step % 50 == 49 {
                    check_shaped(&doc, &layout);
                }
            }
            check_shaped(&doc, &layout);
            layout.rewrap(&doc, measure + 5, &page_shape);
            check_shaped(&doc, &layout);

            let all = Span::new(p(0, 0), doc.end());
            let change = doc.replace(all, "");
            layout.apply(&doc, &change, &page_shape);
            assert_eq!(layout, Layout::new(&Doc::new(), measure + 5, &page_shape));
        }
    }

    /// The rows below a line move when a line changes height without
    /// changing its number of rows: `apply` has to see that.
    #[test]
    fn apply_moves_the_lines_below_when_a_line_changes_size() {
        let mut doc = Doc::from_text("title\nbody words\nmore");
        let mut layout = Layout::new(&doc, 20, &page_shape);
        assert_eq!([0, 1, 2, 3].map(|l| layout.first_row(l)), [0, 1, 2, 3]);
        let change = doc.replace(Span::at(p(0, 0)), "# ");
        layout.apply(&doc, &change, &page_shape);
        assert_eq!(layout.rows(0).len(), 1);
        assert_eq!([0, 1, 2, 3].map(|l| layout.first_row(l)), [0, 2, 3, 4]);
        let change = doc.replace(Span::new(p(0, 0), p(0, 1)), "");
        layout.apply(&doc, &change, &page_shape);
        assert_eq!([0, 1, 2, 3].map(|l| layout.first_row(l)), [0, 1, 2, 3]);
        assert_eq!(layout, Layout::new(&doc, 20, &page_shape));
    }

    #[test]
    fn a_shaped_layout_for_another_text_never_panics() {
        let mut rnd = rng(0x5EED_2E55);
        let wild = |rnd: &mut dyn FnMut(usize) -> usize| match rnd(4) {
            0 => usize::MAX - rnd(3),
            _ => rnd(40),
        };
        for _ in 0..800 {
            let made_for = Doc::from_text(&format!("# {}\n> {}", random_text(&mut rnd, 20), random_text(&mut rnd, 20)));
            let mut asked = Doc::from_text(&random_text(&mut rnd, 30));
            let mut layout = Layout::new(&made_for, rnd(12), &page_shape);
            for _ in 0..12 {
                let at = p(wild(&mut rnd), wild(&mut rnd));
                let (row, cell) = (wild(&mut rnd), wild(&mut rnd));
                layout.locate(&asked, at);
                layout.line_of_row(row);
                layout.first_row(row);
                layout.shape(row);
                let found = [
                    layout.pos_at(&asked, row, cell),
                    layout.up(&asked, at, cell),
                    layout.down(&asked, at, cell),
                    layout.row_start(&asked, at),
                    layout.row_end(&asked, at),
                ];
                for q in found {
                    assert_eq!(asked.clamp(q), q);
                }
            }
            let change = asked.replace(Span::new(p(rnd(4), rnd(9)), p(rnd(4), rnd(9))), PIECES[rnd(PIECES.len())]);
            layout.apply(&asked, &change, &page_shape);
            layout.locate(&asked, asked.end());
        }
    }
}
