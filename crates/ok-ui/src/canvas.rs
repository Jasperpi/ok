//! The canvas: the one place where cells become pixels.
//!
//! A painter says which surface, which cell, what text, how strongly and
//! with which of the machine's few attributes (double strike, underscore,
//! inverse video, double size). The canvas turns that into pixels through
//! the [`FaceSet`], and keeps a second picture beside the first: the
//! [`Screen`], which records the character and attributes put in every cell
//! so that what is on the glass can be read back without comparing pixels.
//!
//! The face is whatever the `FaceSet` holds. For an outline face the canvas
//! asks the host to rasterize whatever a text needs before drawing it, so
//! switching faces is a matter of changing the `FaceSet` and the grid's cell
//! and touches nothing here.

use ok_gfx::{clusters, FaceSet, Frame, Pen, Rect, Rgb};
use ok_host::Host;
use ok_text::{cell_width, str_width};

use crate::grid::{Cols, Grid};
use crate::look::{Look, Strength, Surface};

/// How a run of text is struck.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ink {
    pub strength: Strength,
    /// Double strike: the glyph drawn again one pixel to the right.
    pub double: bool,
    /// An underscore struck in the same cell as the letter.
    pub under: bool,
    /// Inverse video: the cell filled with ink and the glyph left as ground.
    pub inverse: bool,
    /// Double size: each glyph two cells wide and two rows tall.
    pub big: bool,
}

impl Ink {
    pub const INK: Ink = Ink::at(Strength::INK);
    pub const HALF: Ink = Ink::at(Strength::HALF);
    pub const FAINT: Ink = Ink::at(Strength::FAINT);

    /// Plain text at one strength.
    pub const fn at(strength: Strength) -> Ink {
        Ink { strength, double: false, under: false, inverse: false, big: false }
    }

    pub const fn inverse(mut self, on: bool) -> Ink {
        self.inverse = on;
        self
    }

    pub const fn big(mut self, on: bool) -> Ink {
        self.big = on;
        self
    }

    /// Grid cells one cell of this text takes each way.
    pub const fn size(&self) -> i32 {
        if self.big {
            2
        } else {
            1
        }
    }
}

/// What was put in one cell of the grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The character drawn, a space where nothing was. Of a double-size
    /// glyph only the top left cell holds the character; the other three
    /// hold a space and are marked `big`.
    pub ch: char,
    pub surface: Surface,
    /// `GROUND` where nothing was drawn.
    pub strength: Strength,
    pub double: bool,
    pub under: bool,
    pub inverse: bool,
    pub big: bool,
}

/// Where the cursor was drawn, in cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CursorBox {
    pub col: i32,
    pub row: i32,
    pub wide: i32,
    pub tall: i32,
    /// A hollow box marks the place on the page while the cursor is away.
    pub hollow: bool,
}

/// The screen as text: what the last frame put in every cell.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Screen {
    cols: i32,
    rows: i32,
    cells: Vec<Cell>,
    /// Rows that had a change bar drawn beside them.
    bars: Vec<bool>,
    /// The solid cursor, wherever it is: on the page, or on the line.
    pub cursor: Option<CursorBox>,
    /// The hollow box that keeps the cursor's place on the page while the
    /// cursor is on the line or the keyboard is elsewhere.
    pub kept: Option<CursorBox>,
}

impl Screen {
    /// Empties the screen for a new frame on `grid`.
    fn reset(&mut self, grid: &Grid) {
        self.cols = grid.cols.max(0);
        self.rows = grid.rows.max(0);
        self.cells.clear();
        for _ in 0..self.rows {
            for col in 0..self.cols {
                let surface = if grid.sheet.contains(col) { Surface::Sheet } else { Surface::Desk };
                self.cells.push(Cell { ch: ' ', surface, strength: Strength::GROUND, double: false, under: false, inverse: false, big: false });
            }
        }
        self.bars.clear();
        self.bars.resize(self.rows as usize, false);
        self.cursor = None;
        self.kept = None;
    }

    fn slot(&mut self, col: i32, row: i32) -> Option<&mut Cell> {
        if col < 0 || row < 0 || col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells.get_mut((row * self.cols + col) as usize)
    }

    pub fn cols(&self) -> i32 {
        self.cols
    }

    pub fn rows(&self) -> i32 {
        self.rows
    }

    /// The cell at a column and row, or `None` outside the grid.
    pub fn at(&self, col: i32, row: i32) -> Option<Cell> {
        if col < 0 || row < 0 || col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells.get((row * self.cols + col) as usize).copied()
    }

    /// One row as a string, one character for each cell. Empty outside the
    /// grid.
    pub fn row(&self, row: i32) -> String {
        (0..self.cols).filter_map(|col| self.at(col, row)).map(|cell| cell.ch).collect()
    }

    /// A stretch of one row with the blank cells at its end left off.
    pub fn span(&self, cols: Cols, row: i32) -> String {
        let text: String = (cols.first..=cols.last).filter_map(|col| self.at(col, row)).map(|cell| cell.ch).collect();
        text.trim_end().to_string()
    }

    /// A stretch of a double-size row read as the words on it: every second
    /// cell, with the blank cells at the end left off.
    pub fn big(&self, cols: Cols, row: i32) -> String {
        let text: String = (cols.first..=cols.last).step_by(2).filter_map(|col| self.at(col, row)).map(|cell| cell.ch).collect();
        text.trim_end().to_string()
    }

    /// The whole screen, one string for each row of the grid.
    pub fn lines(&self) -> Vec<String> {
        (0..self.rows).map(|row| self.row(row)).collect()
    }

    /// The first place `needle` is written on one row, as (column, row),
    /// looking down the screen from the top.
    pub fn find(&self, needle: &str) -> Option<(i32, i32)> {
        let want: Vec<char> = needle.chars().collect();
        if want.is_empty() {
            return None;
        }
        (0..self.rows).find_map(|row| {
            let line: Vec<char> = self.row(row).chars().collect();
            line.windows(want.len()).position(|window| window == want.as_slice()).map(|col| (col as i32, row))
        })
    }

    /// True when a change bar was drawn beside this row.
    pub fn bar(&self, row: i32) -> bool {
        row >= 0 && self.bars.get(row as usize).copied().unwrap_or(false)
    }
}

/// A frame being drawn.
pub struct Canvas<'a> {
    frame: &'a mut Frame,
    faces: &'a mut FaceSet,
    host: &'a mut dyn Host,
    /// The family and pixel height of an outline face, so the host can be
    /// asked for glyphs. `None` with a bitmap face, which needs nothing.
    outline: Option<(String, u32)>,
    grid: &'a Grid,
    look: &'a Look,
    screen: &'a mut Screen,
    /// The columns and rows drawing is confined to, rows as first and one
    /// past the last.
    within: Option<(Cols, i32, i32)>,
    /// Pixels everything is moved down by: the paper's displacement while
    /// it rolls. The screen records cells where they come to rest.
    shift: i32,
    /// The cursor is an underscore under its character, not a block.
    underline: bool,
    /// No hollow box is drawn: what is on the page is only being shown,
    /// and there is no place in it to keep.
    placeless: bool,
}

impl<'a> Canvas<'a> {
    /// Starts a frame: the desk everywhere, the sheet over it, and an empty
    /// [`Screen`].
    pub fn new(
        frame: &'a mut Frame,
        faces: &'a mut FaceSet,
        host: &'a mut dyn Host,
        grid: &'a Grid,
        look: &'a Look,
        screen: &'a mut Screen,
    ) -> Canvas<'a> {
        let outline = faces.active().as_outline().map(|face| (face.name().to_string(), face.height()));
        frame.reset_clip();
        frame.clear(look.desk);
        frame.fill(grid.sheet_rect(), look.paper);
        screen.reset(grid);
        Canvas { frame, faces, host, outline, grid, look, screen, within: None, shift: 0, underline: false, placeless: false }
    }

    /// Leaves the hollow box out from now on, or draws it again. The box
    /// keeps the cursor's place in his text; a text that is only being
    /// shown on the page has no such place.
    pub fn hide_place(&mut self, on: bool) {
        self.placeless = on;
    }

    /// Has the cursor drawn as an underscore under its character from now
    /// on, or as a block again: the `cursor` setting.
    pub fn underline_cursor(&mut self, on: bool) {
        self.underline = on;
    }

    /// The underscore of an underline cursor: a stroke in the mark colour
    /// along the foot of the cells the cursor is on.
    fn cursor_stroke(&mut self, surface: Surface, col: i32, row: i32, wide: i32, tall: i32) {
        let place = self.block(col, row, wide, tall);
        let thick = 2 * self.grid.scale * tall.max(1);
        self.frame.fill(Rect::new(place.x, place.bottom() - thick, place.w, thick), self.look.mark_on(surface));
    }

    pub fn grid(&self) -> &'a Grid {
        self.grid
    }

    pub fn look(&self) -> &'a Look {
        self.look
    }

    /// Confines drawing to a block of cells until [`Canvas::release`].
    pub fn confine(&mut self, cols: Cols, first_row: i32, rows: i32) {
        self.frame.set_clip(self.grid.cells(cols.first, first_row, cols.width(), rows));
        self.within = Some((cols, first_row, first_row.saturating_add(rows.max(0))));
    }

    pub fn release(&mut self) {
        self.frame.reset_clip();
        self.within = None;
    }

    /// Moves everything drawn from now on down by `pixels`.
    pub fn set_shift(&mut self, pixels: i32) {
        self.shift = pixels;
    }

    fn records(&self, col: i32, row: i32) -> bool {
        match self.within {
            Some((cols, first, end)) => cols.contains(col) && first <= row && row < end,
            None => true,
        }
    }

    /// Has the host rasterize what `text` needs from an outline face.
    fn fill_faces(&mut self, text: &str) {
        if let Some((family, height)) = &self.outline {
            let host = &mut *self.host;
            self.faces.fill(text, |ch| host.rasterize(family, *height, ch));
        }
    }

    fn block(&self, col: i32, row: i32, wide: i32, tall: i32) -> Rect {
        let cells = self.grid.cells(col, row, wide, tall);
        Rect::new(cells.x, cells.y.saturating_add(self.shift), cells.w, cells.h)
    }

    /// The colour of text at a strength on a surface.
    pub fn colour(&self, surface: Surface, strength: Strength) -> Rgb {
        self.look.shade(surface, strength)
    }

    /// Draws `text` from a cell on and returns how many grid columns it
    /// took. Widths come from the text crate's table, never from the face: a
    /// combining mark rides on its letter, a wide character takes two cells,
    /// a tab four blank ones, and a control character is a box of its hex
    /// digits. Nothing here can fail or draw outside the frame.
    pub fn text(&mut self, surface: Surface, col: i32, row: i32, text: &str, ink: Ink) -> i32 {
        self.fill_faces(text);
        let fore = self.look.shade(surface, ink.strength);
        let ground = self.look.ground(surface);
        let mut at = col;
        for piece in clusters(text) {
            let cells = str_width(piece).min(8) as i32;
            self.cluster((surface, at, row), piece, cells, ink, (fore, ground));
            at = at.saturating_add(cells * ink.size());
        }
        at.saturating_sub(col)
    }

    /// [`Canvas::text`] set so that its last cell is in column `last`.
    pub fn text_ending(&mut self, surface: Surface, last: i32, row: i32, text: &str, ink: Ink) {
        let wide = (str_width(text) as i32).saturating_mul(ink.size());
        self.text(surface, last + 1 - wide, row, text, ink);
    }

    /// Draws one cluster in `cells` cells of its own size at a surface,
    /// column and row, in the two colours given: the ink and the ground.
    fn cluster(&mut self, (surface, col, row): (Surface, i32, i32), piece: &str, cells: i32, ink: Ink, (fore, ground): (Rgb, Rgb)) {
        let size = ink.size();
        let wide = cells.max(1) * size;
        let place = self.block(col, row, wide, size);
        if ink.inverse && cells > 0 {
            self.frame.fill(place, fore);
        }
        let colour = if ink.inverse { ground } else { fore };
        let pen = Pen::new(colour).scale((self.grid.scale * size) as u32).bold(ink.double);
        let first = piece.chars().next().unwrap_or(' ');
        if !matches!(first, ' ' | '\t') && !ink.strength.is_ground() {
            self.faces.draw_cluster_in(self.frame, piece, place.x, place.y, cells.clamp(1, 2) as u32, pen);
        }
        if ink.under && cells > 0 && !ink.strength.is_ground() {
            self.faces.underline(self.frame, place.x, place.y, cells as u32, pen);
        }
        if cells == 0 {
            return;
        }
        // What the cell is said to hold is the character that fills it: a
        // mark with no cell of its own that leads the piece is passed over.
        let filling = piece.chars().find(|&ch| cell_width(ch) > 0).unwrap_or(first);
        let shown = if filling == '\t' { ' ' } else { filling };
        for down in 0..size {
            for along in 0..wide {
                if !self.records(col + along, row + down) {
                    continue;
                }
                if let Some(cell) = self.screen.slot(col + along, row + down) {
                    let head = down == 0 && along == 0;
                    *cell = Cell {
                        ch: if head { shown } else { ' ' },
                        surface,
                        strength: ink.strength,
                        double: ink.double,
                        under: ink.under,
                        inverse: ink.inverse,
                        big: ink.big,
                    };
                }
            }
        }
    }

    /// Puts a block of cells back to bare ground.
    pub fn erase(&mut self, surface: Surface, col: i32, row: i32, wide: i32, tall: i32) {
        let place = self.block(col, row, wide, tall);
        self.frame.fill(place, self.look.ground(surface));
        for down in 0..tall.clamp(0, 4096) {
            for along in 0..wide.clamp(0, 4096) {
                if !self.records(col + along, row + down) {
                    continue;
                }
                if let Some(cell) = self.screen.slot(col + along, row + down) {
                    *cell = Cell { ch: ' ', surface, strength: Strength::GROUND, double: false, under: false, inverse: false, big: false };
                }
            }
        }
    }

    /// The solid cursor: a block in the mark colour with the character it
    /// sits on left as ground. `piece` is that character with its marks, or
    /// empty at the end of a line; `cells` how many cells of its own size it
    /// takes.
    pub fn cursor(&mut self, surface: Surface, col: i32, row: i32, piece: &str, cells: i32, ink: Ink) {
        self.fill_faces(piece);
        let size = ink.size();
        let cells = cells.clamp(1, 2);
        let mark = self.look.mark_on(surface);
        let ground = self.look.ground(surface);
        if self.underline {
            // The character as it is printed, and the stroke under it.
            let fore = self.look.shade(surface, ink.strength);
            self.cluster((surface, col, row), if piece.is_empty() { " " } else { piece }, cells, ink.inverse(false), (fore, ground));
            self.cursor_stroke(surface, col, row, cells * size, size);
            if self.records(col, row) {
                self.screen.cursor = Some(CursorBox { col, row, wide: cells * size, tall: size, hollow: false });
            }
            return;
        }
        let inverse = Ink { inverse: true, strength: Strength::INK, ..ink };
        self.cluster((surface, col, row), if piece.is_empty() { " " } else { piece }, cells, inverse, (mark, ground));
        if self.records(col, row) {
            self.screen.cursor = Some(CursorBox { col, row, wide: cells * size, tall: size, hollow: false });
        }
    }

    /// The solid cursor where the text round it is already in inverse video
    /// (a link the cursor is in, a TK, a hit). The cell stays a block like
    /// its neighbours, so the bar they make is unbroken, and is told from
    /// them by a line of ground ruled round it just inside its edge: the
    /// hollow box that keeps the cursor's place, in negative.
    pub fn cursor_within(&mut self, surface: Surface, col: i32, row: i32, piece: &str, cells: i32, ink: Ink) {
        self.cursor(surface, col, row, piece, cells, ink);
        let size = ink.size();
        let place = self.block(col, row, cells.clamp(1, 2) * size, size);
        let ground = self.look.ground(surface);
        let line = self.grid.scale;
        let inner = Rect::new(place.x + line, place.y + line, place.w - 2 * line, place.h - 2 * line);
        if inner.w > 2 * line && inner.h > 2 * line {
            self.frame.fill(Rect::new(inner.x, inner.y, inner.w, line), ground);
            self.frame.fill(Rect::new(inner.x, inner.bottom() - line, inner.w, line), ground);
            self.frame.fill(Rect::new(inner.x, inner.y, line, inner.h), ground);
            self.frame.fill(Rect::new(inner.right() - line, inner.y, line, inner.h), ground);
        }
    }

    /// The hollow cursor: the outline of a block, drawn over whatever is in
    /// the cells, to mark the place while the cursor is elsewhere.
    pub fn hollow(&mut self, surface: Surface, col: i32, row: i32, wide: i32, tall: i32) {
        if self.placeless {
            return;
        }
        let place = self.block(col, row, wide, tall);
        let mark = self.look.mark_on(surface);
        let line = self.grid.scale;
        self.frame.fill(Rect::new(place.x, place.y, place.w, line), mark);
        self.frame.fill(Rect::new(place.x, place.bottom() - line, place.w, line), mark);
        self.frame.fill(Rect::new(place.x, place.y, line, place.h), mark);
        self.frame.fill(Rect::new(place.right() - line, place.y, line, place.h), mark);
        if self.records(col, row) {
            self.screen.kept = Some(CursorBox { col, row, wide, tall, hollow: true });
        }
    }

    /// A change bar: a stroke two pixels wide at scale 1, in the mark
    /// colour, down the middle of the bar column beside `tall` rows.
    pub fn change_bar(&mut self, row: i32, tall: i32) {
        let cells = self.block(self.grid.bar, row, 1, tall);
        let wide = 2 * self.grid.scale;
        let stroke = Rect::new(cells.x + (cells.w - wide) / 2, cells.y, wide, cells.h);
        self.frame.fill(stroke, self.look.mark_on(Surface::Sheet));
        for down in 0..tall.clamp(0, 4) {
            if self.records(self.grid.bar, row + down) && row + down >= 0 {
                if let Some(bar) = self.screen.bars.get_mut((row + down) as usize) {
                    *bar = true;
                }
            }
        }
    }

    /// Fills pixels directly, for the map, which is not made of cells. The
    /// paper's displacement does not apply.
    pub fn pixels(&mut self, place: Rect, colour: Rgb) {
        self.frame.fill(place, colour);
    }

    /// Cuts drawing down to a rectangle of pixels until [`Canvas::release`].
    pub fn confine_pixels(&mut self, place: Rect) {
        self.frame.set_clip(place);
    }
}

/// The longest start of `text` that fits in `cells` cells, cut between
/// clusters.
pub fn fit(text: &str, cells: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for piece in clusters(text) {
        let wide = str_width(piece);
        if used + wide > cells {
            break;
        }
        used += wide;
        end += piece.len();
    }
    text.get(..end).unwrap_or(text)
}

/// The longest start of `text` that fits in `cells` cells and ends with a
/// whole word; when not even the first word fits, as much of it as does.
pub fn fit_words(text: &str, cells: usize) -> &str {
    let text = text.trim();
    if str_width(text) <= cells {
        return text;
    }
    let cut = fit(text, cells);
    match cut.rfind(char::is_whitespace) {
        // The cut fell between words already when what follows is a space.
        _ if text.get(cut.len()..).is_some_and(|rest| rest.starts_with(char::is_whitespace)) => cut,
        Some(space) => cut.get(..space).unwrap_or(cut).trim_end(),
        None => cut,
    }
}

/// The words that may not end a text cut short: they promise something the
/// cut has taken away.
const JOINERS: [&str; 22] = ["a", "an", "and", "as", "at", "but", "by", "for", "from", "his", "her", "in", "into", "its", "of", "on", "or", "the", "their", "to", "with", "my"];

/// [`fit_words`] for a name or a label: what is left after a cut does not
/// end on punctuation or on a joining word, so that it reads as cut and not
/// as a shorter name.
pub fn fit_name(text: &str, cells: usize) -> &str {
    let whole = text.trim();
    let mut cut = fit_words(whole, cells);
    if cut.len() == whole.len() {
        return cut;
    }
    loop {
        let trimmed = cut.trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | ':' | '-' | '\u{2013}' | '\u{2014}' | '(' | '[' | '\'' | '"'));
        let last = trimmed.rsplit(char::is_whitespace).next().unwrap_or("");
        let shorter = if JOINERS.iter().any(|joiner| last.eq_ignore_ascii_case(joiner)) { trimmed.get(..trimmed.len() - last.len()).unwrap_or(trimmed) } else { trimmed };
        // Never down to nothing: a first word that is a joiner stays.
        if shorter.trim_end().is_empty() {
            return trimmed;
        }
        if shorter.len() == cut.len() {
            return cut;
        }
        cut = shorter;
    }
}

/// Sets `text` on rows of `cells` cells, `rows` at most, breaking between
/// words. A row that could end at the end of a sentence does, when the next
/// sentence would not fit whole after it, so that a row is a sentence or
/// several wherever the words allow. A word wider than a row is cut at the
/// row's edge, since nothing else can be done with it. What does not fit
/// the last row is left off at a whole word.
pub fn set_rows(text: &str, cells: usize, rows: usize) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && out.len() < rows {
        if str_width(rest) <= cells {
            out.push(rest);
            break;
        }
        let by_words = fit_words(rest, cells);
        // The last place in what fits where a sentence ends.
        let sentence = by_words
            .char_indices()
            .filter(|&(at, ch)| matches!(ch, '.' | '?') && by_words.get(at + 1..).is_some_and(|after| after.starts_with(' ')))
            .map(|(at, _)| at + 1)
            .last();
        let row = match sentence {
            Some(end) => by_words.get(..end).unwrap_or(by_words),
            None => by_words,
        };
        if row.is_empty() {
            break;
        }
        out.push(row);
        rest = rest.get(row.len()..).unwrap_or("").trim_start();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_name_does_not_end_on_a_comma_or_a_joining_word() {
        assert_eq!(fit_name("If he changes his account", 24), "If he changes");
        assert_eq!(fit_name("If he changes his account", 25), "If he changes his account");
        assert_eq!(fit_name("A third-level heading, plain", 24), "A third-level heading");
        assert_eq!(fit_name("A second paragraph, and then more of it", 24), "A second paragraph");
        assert_eq!(fit_name("The orchard of the", 13), "The orchard");
        assert_eq!(fit_name("The and of", 6), "The");
        assert_eq!(fit_name("Antidisestablishment", 6), "Antidi");
        assert_eq!(fit_name("", 6), "");
    }

    #[test]
    fn a_sentence_too_long_for_a_row_turns_over_at_a_sentence_or_a_word() {
        let newer = "This text changed on disk. Showing the newer one; yours is in earlier.";
        assert_eq!(set_rows(newer, 66, 3), ["This text changed on disk.", "Showing the newer one; yours is in earlier."]);
        let keeps = "Enter keeps measure at 66. Esc puts it back to 72. The default is 66.";
        assert_eq!(set_rows(keeps, 66, 3), ["Enter keeps measure at 66. Esc puts it back to 72.", "The default is 66."]);
        let last = "Last here Tuesday at 23:41. That sitting added 388 words.";
        assert_eq!(set_rows(last, 66, 3), [last]);
        assert_eq!(set_rows(last, 39, 3), ["Last here Tuesday at 23:41.", "That sitting added 388 words."]);
        // No sentence ends in what fits: the row ends at a word.
        assert_eq!(set_rows("Alt moves and deletes by sentence, and carries the paragraph.", 40, 3), ["Alt moves and deletes by sentence, and", "carries the paragraph."]);
        // A word wider than the row is cut at its edge, and three rows are all there are.
        assert_eq!(set_rows(&"x".repeat(200), 66, 3), vec!["x".repeat(66); 3]);
        assert_eq!(set_rows("one two three", 5, 2), ["one", "two"]);
        assert!(set_rows("", 66, 3).is_empty() && set_rows("words", 66, 0).is_empty());
        assert!(set_rows("words", 0, 3).is_empty());
    }

    #[test]
    fn text_is_cut_between_clusters_and_between_words() {
        assert_eq!(fit("orchard", 4), "orch");
        assert_eq!(fit("n\u{0313}a", 1), "n\u{0313}");
        assert_eq!(fit("\u{4E00}\u{4E8C}", 3), "\u{4E00}");
        assert_eq!(fit("", 3), "");
        assert_eq!(fit_words("Grandfather interview", 30), "Grandfather interview");
        assert_eq!(fit_words("The orchard was eleven acres", 16), "The orchard was");
        assert_eq!(fit_words("The orchard was eleven acres", 15), "The orchard was");
        assert_eq!(fit_words("The orchard was eleven acres", 14), "The orchard");
        assert_eq!(fit_words("Antidisestablishment", 6), "Antidi");
        assert_eq!(fit_words("  spaced  ", 20), "spaced");
    }
}
