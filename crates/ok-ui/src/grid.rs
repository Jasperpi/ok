//! The grid and its regions, for any client size.
//!
//! Everything on screen sits on cells. This file owns the arithmetic that
//! says how many cells there are, where the sheet lies among them and what
//! each column and row is for; every painter asks a [`Grid`] where things go
//! and none of them does sums of its own.

use ok_gfx::Rect;

/// A run of columns, both ends included, as DESIGN.md writes them ("47 to
/// 112"). Empty when `last` is before `first`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cols {
    pub first: i32,
    pub last: i32,
}

impl Cols {
    pub fn width(self) -> i32 {
        (self.last - self.first + 1).max(0)
    }

    pub fn contains(self, col: i32) -> bool {
        self.first <= col && col <= self.last
    }
}

/// The columns of the map and the two columns that flank it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapCols {
    /// Where the bar level with the cursor's row is drawn.
    pub notch: i32,
    pub marks: Cols,
    /// Where a tick is drawn beside a row holding a TK or a hit.
    pub flags: i32,
}

/// The measure when no setting says otherwise.
pub const MEASURE: usize = 66;

/// Columns of the sheet before the text: its edge, three for hanging marks,
/// one for change bars, one empty.
const BEFORE_TEXT: i32 = 6;
/// Columns of the sheet after the text: one where the cursor rests after a
/// full row, and five of margin.
const AFTER_TEXT: i32 = 6;

/// Further than any pixel or cell that could matter, and small enough that
/// two of them added together still fit in an `i32`.
const FAR: i64 = 1 << 28;

/// The one screen laid out for one client size at one scale.
///
/// Columns and rows count from 0 at the top left. Pixels left over when the
/// client size is not a whole number of cells are split evenly round the
/// grid and are ground like everything else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grid {
    /// The client area in pixels.
    pub width: i32,
    pub height: i32,
    /// 1, 2 or 3: how many screen pixels a pixel of the face takes.
    pub scale: i32,
    /// One cell in screen pixels.
    pub cell_w: i32,
    pub cell_h: i32,
    pub cols: i32,
    pub rows: i32,
    /// Pixels of ground left of column 0 and above row 0.
    pub left: i32,
    pub top: i32,
    /// Cells of text to a row.
    pub measure: i32,
    /// The sheet, which runs the full height of the window.
    pub sheet: Cols,
    /// Hanging marks (`#`, `>`), right-aligned to its last column.
    pub gutter: Cols,
    /// The column of change bars.
    pub bar: i32,
    /// The text. One column past it is where the cursor rests after a full
    /// row.
    pub text: Cols,
    /// The contents, when the left side has room for them.
    pub contents: Option<Cols>,
    /// The map, when the right side has room for it.
    pub map: Option<MapCols>,
    /// Labels and, at their foot, links, when the right side has room.
    pub labels: Option<Cols>,
    /// Rows of text, counted from row 0. The row after them is always empty.
    pub text_rows: i32,
    /// The row the line being written is held at.
    pub platen: i32,
    /// The one place the machine speaks in sentences.
    pub say: Option<i32>,
    /// The machine line: `ok`, the line's typing, the facts.
    pub machine: Option<i32>,
}

impl Grid {
    /// Lays the screen out for a client area of `width` by `height` pixels.
    /// `face` is the face's cell in its own pixels, `scale` the setting (1
    /// to 3), `measure` the setting in cells.
    ///
    /// The measure is the setting, or the columns less 14 if that is
    /// smaller. The sheet is the measure and 12 more, and what is left is
    /// split between the sides, the odd column going to the right. The
    /// contents are drawn when the left side has 24 columns, the map when
    /// the right has room for it and 4 more, labels when it has room for the
    /// map and 16 more (15 and 27 at a measure of 66). No size is too small:
    /// below five rows the empty rows go first and then the say row, and a
    /// sheet wider than the window is simply cut off by it.
    pub fn new(width: usize, height: usize, face: (u32, u32), scale: u32, measure: usize) -> Grid {
        let scale = scale.clamp(1, 3) as i32;
        let cell_w = (face.0.clamp(1, 256) as i32) * scale;
        let cell_h = (face.1.clamp(1, 256) as i32) * scale;
        let width = width.min(1 << 20) as i32;
        let height = height.min(1 << 20) as i32;
        let (cols, rows) = (width / cell_w, height / cell_h);
        let left = (width - cols * cell_w) / 2;
        let top = (height - rows * cell_h) / 2;

        let measure = (measure.clamp(1, 1000) as i32).min(cols - 14).max(1);
        let sheet_w = BEFORE_TEXT + measure + AFTER_TEXT;
        let spare = (cols - sheet_w).max(0);
        let (left_side, right_side) = (spare / 2, spare - spare / 2);
        let sheet = Cols { first: left_side, last: left_side + sheet_w - 1 };
        let text = Cols { first: sheet.first + BEFORE_TEXT, last: sheet.first + BEFORE_TEXT + measure - 1 };

        // The map is two pixels a letter at scale 1, so its width in cells
        // follows from the measure and the face.
        let map_cells = (measure * 2 * scale + cell_w - 1) / cell_w;
        let beside = sheet.last + 1;
        let map = (right_side >= map_cells + 4).then(|| MapCols {
            notch: beside + 1,
            marks: Cols { first: beside + 2, last: beside + 1 + map_cells },
            flags: beside + 2 + map_cells,
        });
        let labels = (right_side >= map_cells + 16).then(|| Cols { first: beside + 4 + map_cells, last: cols - 3 });

        let (text_rows, say, machine) = match rows {
            5.. => (rows - 4, Some(rows - 3), Some(rows - 2)),
            4 => (2, Some(2), Some(3)),
            3 => (1, Some(1), Some(2)),
            2 => (1, None, Some(1)),
            1 => (1, None, None),
            _ => (0, None, None),
        };
        let platen = ((62 * text_rows + 50) / 100).min(text_rows - 1).max(0);

        Grid {
            width,
            height,
            scale,
            cell_w,
            cell_h,
            cols,
            rows,
            left,
            top,
            measure,
            sheet,
            gutter: Cols { first: sheet.first + 1, last: sheet.first + 3 },
            bar: sheet.first + 4,
            text,
            contents: (left_side >= 24).then(|| Cols { first: 2, last: left_side - 5 }),
            map,
            labels,
            text_rows,
            platen,
            say,
            machine,
        }
    }

    /// True when the window has no room for even one cell of text: before
    /// it has a size, and while it is minimized.
    pub fn is_blank(&self) -> bool {
        self.cols <= 0 || self.text_rows <= 0
    }

    /// The left edge of a column in pixels. A column far outside any screen
    /// is brought to a far but finite place, so that a rectangle built from
    /// the answer can always be added up.
    pub fn x(&self, col: i32) -> i32 {
        (self.left as i64 + col as i64 * self.cell_w as i64).clamp(-FAR, FAR) as i32
    }

    /// The top edge of a row in pixels, kept finite like [`Grid::x`].
    pub fn y(&self, row: i32) -> i32 {
        (self.top as i64 + row as i64 * self.cell_h as i64).clamp(-FAR, FAR) as i32
    }

    /// The pixels of a block of cells.
    pub fn cells(&self, col: i32, row: i32, wide: i32, tall: i32) -> Rect {
        let w = (wide as i64 * self.cell_w as i64).clamp(0, FAR) as i32;
        let h = (tall as i64 * self.cell_h as i64).clamp(0, FAR) as i32;
        Rect::new(self.x(col), self.y(row), w, h)
    }

    /// The column and row a pixel falls in. Pixels in the ground round the
    /// grid give columns and rows outside it.
    pub fn cell_at(&self, x: i32, y: i32) -> (i32, i32) {
        let col = (x as i64 - self.left as i64).div_euclid(self.cell_w as i64);
        let row = (y as i64 - self.top as i64).div_euclid(self.cell_h as i64);
        (col.clamp(-FAR, FAR) as i32, row.clamp(-FAR, FAR) as i32)
    }

    /// The sheet in pixels: its columns, and the whole height of the window,
    /// the leftover pixels above and below the grid included.
    pub fn sheet_rect(&self) -> Rect {
        Rect::new(self.x(self.sheet.first), 0, self.sheet.width() * self.cell_w, self.height)
    }

    /// The pixels of the rows of text on the sheet, where the paper moves.
    pub fn text_rect(&self) -> Rect {
        Rect::new(self.x(self.sheet.first), self.y(0), self.sheet.width() * self.cell_w, self.text_rows * self.cell_h)
    }

    /// The column where `ok` is printed.
    pub fn ok_col(&self) -> i32 {
        self.text.first
    }

    /// The column the line's typing starts at: after `ok` and a space.
    pub fn typing_col(&self) -> i32 {
        self.text.first + 3
    }

    /// The rows the contents may fill: from row 1 to the last row of text.
    pub fn contents_rows(&self) -> (i32, i32) {
        (1, self.text_rows - 1)
    }

    /// The row the links block is bottom-aligned to: the empty row under the
    /// text.
    pub fn links_foot(&self) -> i32 {
        self.text_rows.min(self.rows - 1)
    }

    /// The side of the square a letter makes in the map at full size, in
    /// pixels: two at scale 1. A row of the map is twice that.
    pub fn map_unit(&self) -> i32 {
        2 * self.scale
    }

    /// Where the marks of the map may go, in pixels: as wide as the measure
    /// at one unit a letter, and as tall as the rows from row 1 to the say
    /// row (which at 49 rows is room for the 253 rows DESIGN.md counts).
    /// They start four pixels under the top of row 1, so that the strip of
    /// paper they lie on, with its four pixels of margin, has its top edge
    /// on the grid, level with the first row of the contents and of the
    /// labels. `None` when there is no map.
    pub fn map_area(&self) -> Option<Rect> {
        let map = self.map?;
        let area = Rect::new(self.x(map.marks.first), self.y(1) + 4 * self.scale, self.measure * self.map_unit(), (self.rows - 3) * self.cell_h);
        (!area.is_empty()).then_some(area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GALLANT: (u32, u32) = (12, 22);

    fn at(width: usize, height: usize) -> Grid {
        Grid::new(width, height, GALLANT, 1, MEASURE)
    }

    fn cols(first: i32, last: i32) -> Cols {
        Cols { first, last }
    }

    /// The two tables of DESIGN.md, column by column and row by row.
    #[test]
    fn a_full_screen_is_160_by_49_laid_out_as_the_design_tables_say() {
        let g = at(1920, 1080);
        assert_eq!((g.cols, g.rows), (160, 49));
        assert_eq!((g.left, g.top), (0, 1), "one pixel of ground above and below");
        assert_eq!((g.cell_w, g.cell_h), (12, 22));
        assert_eq!(g.contents, Some(cols(2, 36)));
        assert_eq!(g.sheet, cols(41, 118));
        assert_eq!(g.gutter, cols(42, 44));
        assert_eq!(g.bar, 45);
        assert_eq!(g.text, cols(47, 112));
        assert_eq!(g.measure, 66);
        assert_eq!(g.map, Some(MapCols { notch: 120, marks: cols(121, 131), flags: 132 }));
        assert_eq!(g.labels, Some(cols(134, 157)));
        assert_eq!(g.text_rows, 45);
        assert_eq!(g.platen, 28);
        assert_eq!((g.say, g.machine), (Some(46), Some(47)));
        assert_eq!((g.ok_col(), g.typing_col()), (47, 50));
        assert_eq!(g.links_foot(), 45);
        assert_eq!(g.sheet_rect(), Rect::new(492, 0, 936, 1080));
        assert_eq!(g.text_rect(), Rect::new(492, 1, 936, 990));
        // 46 rows of 22 pixels is 253 rows of the map at 4 pixels each.
        assert_eq!(g.map_area(), Some(Rect::new(1452, 27, 132, 1012)));
        assert_eq!(g.map_area().unwrap().h / (2 * g.map_unit()), 253);
    }

    #[test]
    fn eighty_by_twenty_four_is_the_sheet_and_a_column_of_desk_each_side() {
        let g = at(960, 528);
        assert_eq!((g.cols, g.rows), (80, 24));
        assert_eq!(g.measure, 66);
        assert_eq!(g.sheet, cols(1, 78));
        assert_eq!(g.text, cols(7, 72));
        assert_eq!((g.contents, g.map, g.labels), (None, None, None));
        assert_eq!(g.text_rows, 20);
        assert_eq!(g.platen, 12);
        assert_eq!((g.say, g.machine), (Some(21), Some(22)));
        assert_eq!(g.map_area(), None);
    }

    #[test]
    fn the_sides_appear_as_the_window_makes_room_for_them() {
        // 14 columns a side: nothing but the sheet.
        let g = at(1280, 720);
        assert_eq!((g.cols, g.rows, g.left, g.top), (106, 32, 4, 8));
        assert_eq!(g.sheet, cols(14, 91));
        assert_eq!((g.contents, g.map, g.labels), (None, None, None));
        // 17 on the left and 18 on the right: the map alone.
        let g = at(1366, 768);
        assert_eq!(g.cols, 113);
        assert_eq!(g.sheet, cols(17, 94));
        assert_eq!(g.map, Some(MapCols { notch: 96, marks: cols(97, 107), flags: 108 }));
        assert_eq!((g.contents, g.labels), (None, None));
        // 27 and 28: contents, map and eleven columns of labels.
        let g = at(1600, 900);
        assert_eq!(g.cols, 133);
        assert_eq!(g.contents, Some(cols(2, 22)));
        assert_eq!(g.sheet, cols(27, 104));
        assert_eq!(g.labels, Some(cols(120, 130)));
        // Exactly at each threshold, and one short of it.
        for (columns, contents, map, labels) in [(106, false, false, false), (107, false, true, false), (125, false, true, false), (126, true, true, false), (130, true, true, false), (131, true, true, true)] {
            let g = at(columns * 12, 1080);
            assert_eq!((g.contents.is_some(), g.map.is_some(), g.labels.is_some()), (contents, map, labels), "{columns} columns");
        }
    }

    #[test]
    fn a_narrow_window_takes_the_measure_down_with_it() {
        let g = at(70 * 12, 30 * 22);
        assert_eq!(g.measure, 56);
        assert_eq!(g.sheet, cols(1, 68));
        assert_eq!(g.text, cols(7, 62));
        // A measure set wider than the default is honoured while it fits.
        let g = Grid::new(1920, 1080, GALLANT, 1, 100);
        assert_eq!(g.measure, 100);
        assert_eq!(g.sheet, cols(24, 135));
        assert_eq!(g.map.map(|m| m.marks), Some(cols(138, 154)));
        assert_eq!(g.labels, None);
    }

    #[test]
    fn scale_two_and_three_make_bigger_cells_on_the_same_rules() {
        let g = Grid::new(1920, 1080, GALLANT, 2, MEASURE);
        assert_eq!((g.cell_w, g.cell_h, g.cols, g.rows), (24, 44, 80, 24));
        assert_eq!((g.left, g.top), (0, 12));
        assert_eq!(g.sheet, cols(1, 78));
        assert_eq!((g.text_rows, g.platen), (20, 12));
        let g = Grid::new(1920, 1080, GALLANT, 3, MEASURE);
        assert_eq!((g.cell_w, g.cell_h, g.cols, g.rows), (36, 66, 53, 16));
        assert_eq!(g.measure, 39);
        assert_eq!(g.sheet, cols(1, 51));
        assert_eq!((g.text_rows, g.platen), (12, 7));
        // On a screen with room, the map grows with the scale.
        let g = Grid::new(3840, 2160, GALLANT, 2, MEASURE);
        assert_eq!((g.cols, g.rows), (160, 49));
        assert_eq!(g.map_unit(), 4);
        assert_eq!(g.map_area(), Some(Rect::new(2904, 54, 264, 2024)));
        // A scale outside 1 to 3 is brought inside.
        assert_eq!(Grid::new(1920, 1080, GALLANT, 0, MEASURE).scale, 1);
        assert_eq!(Grid::new(1920, 1080, GALLANT, 9, MEASURE).scale, 3);
    }

    #[test]
    fn the_last_rows_to_go_are_the_text_and_the_machine_line() {
        let rows = |n: usize| {
            let g = at(1920, n * 22);
            (g.text_rows, g.say, g.machine, g.platen)
        };
        assert_eq!(rows(6), (2, Some(3), Some(4), 1));
        assert_eq!(rows(5), (1, Some(2), Some(3), 0));
        assert_eq!(rows(4), (2, Some(2), Some(3), 1));
        assert_eq!(rows(3), (1, Some(1), Some(2), 0));
        assert_eq!(rows(2), (1, None, Some(1), 0));
        assert_eq!(rows(1), (1, None, None, 0));
        assert_eq!(rows(0), (0, None, None, 0));
    }

    #[test]
    fn no_size_is_too_small_or_too_large_to_lay_out() {
        for (width, height) in [(0, 0), (1, 1), (11, 21), (12, 22), (0, 1080), (1920, 0), (100, 100), (179, 1), (usize::MAX, usize::MAX)] {
            for scale in 0..5 {
                let g = Grid::new(width, height, GALLANT, scale, MEASURE);
                assert!(g.measure >= 1 && g.cols >= 0 && g.rows >= 0);
                assert!(g.text.width() == g.measure && g.sheet.first >= 0);
                assert!(g.platen >= 0 && (g.text_rows == 0 || g.platen < g.text_rows));
                assert!(g.left >= 0 && g.top >= 0 && g.left < g.cell_w && g.top < g.cell_h);
                g.cell_at(i32::MAX, i32::MIN);
                g.cells(i32::MAX, i32::MAX, i32::MAX, i32::MAX);
                g.map_area();
            }
        }
        // A face with a cell of nothing is given a cell of one pixel.
        assert_eq!(Grid::new(100, 100, (0, 0), 1, 0).measure, 1);
    }

    #[test]
    fn a_pixel_finds_its_cell() {
        let g = at(1280, 720);
        assert_eq!(g.cell_at(4, 8), (0, 0));
        assert_eq!(g.cell_at(15, 29), (0, 0));
        assert_eq!(g.cell_at(16, 30), (1, 1));
        assert_eq!(g.cell_at(0, 0), (-1, -1));
        assert_eq!((g.x(14), g.y(31)), (172, 690));
    }
}
