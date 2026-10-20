//! The grid and its regions on the glass, at the size the design tables
//! describe and at sizes they do not.

mod common;

use common::{head, read, rig};
use ok_gfx::Rgb;
use ok_host::Event;
use ok_ui::canvas::CursorBox;
use ok_ui::harness::Rig;
use ok_ui::look::{Look, Strength, Surface};

const PARAGRAPH: &str = "The orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm.";

/// A text long enough to pass the platen on any screen: a heading and
/// thirty paragraphs.
fn long_text() -> String {
    let mut text = String::from("# The orchard\n\n");
    for _ in 0..30 {
        text.push_str(PARAGRAPH);
        text.push_str("\n\n");
    }
    text.push_str("The last row");
    text
}

#[test]
fn at_160_by_49_every_region_is_where_the_design_tables_put_it() {
    let mut r = rig(&long_text());
    let look = Look::manila();
    let screen = r.screen().clone();
    assert_eq!((screen.cols(), screen.rows()), (160, 49));
    // Columns 41 to 118 are the sheet for the full height; the rest is desk.
    for row in 0..49 {
        for col in 0..160 {
            let surface = if (41..=118).contains(&col) { Surface::Sheet } else { Surface::Desk };
            assert_eq!(screen.at(col, row).map(|cell| cell.surface), Some(surface), "column {col} row {row}");
        }
    }
    // One pixel of ground above and below the grid, which on the sheet is
    // paper and round it desk.
    for y in [0, 1079] {
        assert_eq!(r.pixel(41 * 12 - 1, y), Some(look.desk));
        assert_eq!(r.pixel(41 * 12, y), Some(look.paper));
        assert_eq!(r.pixel(119 * 12 - 1, y), Some(look.paper));
        assert_eq!(r.pixel(119 * 12, y), Some(look.desk));
    }
    // Rows 0 to 44 are text and row 28 is the platen, where the last row of
    // this text is being written.
    assert_eq!(read(&mut r, 47, 112, 28), "The last row");
    assert_eq!(r.screen().cursor, Some(CursorBox { col: 47 + 12, row: 28, wide: 1, tall: 1, hollow: false }));
    assert_eq!(read(&mut r, 41, 118, 29), "");
    assert!(!read(&mut r, 47, 112, 0).is_empty(), "text runs to the top row of the sheet");
    // Nothing is written outside the measure but hanging marks and bars.
    for row in 0..45 {
        assert_eq!(read(&mut r, 113, 118, row), "", "row {row}");
        assert_eq!(read(&mut r, 45, 46, row), "", "row {row}");
    }
    // Row 45 empty, row 46 the say row, row 47 the machine line, row 48 empty.
    assert_eq!(read(&mut r, 41, 118, 45), "");
    assert_eq!(read(&mut r, 41, 118, 46), "");
    assert_eq!(read(&mut r, 47, 49, 47), "ok");
    assert!(read(&mut r, 50, 112, 47).ends_with(" words"));
    assert_eq!(screen.at(112, 47).map(|cell| (cell.ch, cell.strength)), Some(('s', Strength::HALF)));
    assert_eq!(read(&mut r, 0, 159, 48), "");
    // A title that is the only heading is not labelled: the paragraph he
    // is in is, by its opening words, level with the notch beside the map.
    // Three words can be read whole on the page and get no label at all.
    assert_eq!((0..45).filter(|&row| !read(&mut r, 134, 157, row).is_empty()).count(), 0);
    r.press("ctrl+up");
    r.press("ctrl+up");
    let geometry = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    let notch = geometry.y(r.app.page().caret().row).map(|y| r.app.grid().cell_at(0, y).1).unwrap();
    assert_eq!(read(&mut r, 134, 157, notch), "The orchard was eleven");
    assert_eq!((0..45).filter(|&row| !read(&mut r, 134, 157, row).is_empty()).count(), 1);
    assert_eq!(read(&mut r, 119, 133, 1), "", "the map is pixels, not cells");
}

#[test]
fn at_80_by_24_there_is_the_sheet_and_nothing_else() {
    let mut r = Rig::with_text(&long_text(), 960, 528);
    let look = Look::manila();
    assert_eq!((r.screen().cols(), r.screen().rows()), (80, 24));
    assert_eq!(r.app.grid().platen, 12);
    assert_eq!(read(&mut r, 7, 72, 12), "The last row");
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((19, 12)));
    assert_eq!(read(&mut r, 7, 9, 22), "ok");
    assert!(read(&mut r, 10, 72, 22).ends_with(" words"));
    assert_eq!(read(&mut r, 0, 79, 20), "");
    assert_eq!(read(&mut r, 0, 79, 23), "");
    for row in 0..24 {
        assert_eq!(read(&mut r, 0, 0, row), "");
        assert_eq!(read(&mut r, 79, 79, row), "");
    }
    assert_eq!(r.pixel(11, 100), Some(look.desk));
    assert_eq!(r.pixel(12, 100), Some(look.paper));
    assert_eq!(r.pixel(79 * 12 - 1, 100), Some(look.paper));
    assert_eq!(r.pixel(79 * 12, 100), Some(look.desk));
    // The same 66 cells to a row as on the full screen.
    let full = rig(&long_text());
    assert_eq!(r.app.page().layout().total_rows(), full.app.page().layout().total_rows());
}

#[test]
fn at_1280_by_720_the_leftover_pixels_are_ground() {
    let mut r = Rig::with_text(&long_text(), 1280, 720);
    let look = Look::manila();
    let grid = r.app.grid().clone();
    assert_eq!((grid.cols, grid.rows, grid.left, grid.top), (106, 32, 4, 8));
    assert_eq!((grid.contents, grid.map, grid.labels), (None, None, None));
    assert_eq!(grid.platen, 17);
    assert_eq!(read(&mut r, 20, 85, 17), "The last row");
    // Four pixels of desk at the left and right, eight of ground above and
    // below; the sheet runs through those.
    for (x, y) in [(0, 0), (3, 360), (1279, 719), (1276, 360), (100, 0), (100, 719)] {
        assert_eq!(r.pixel(x, y), Some(look.desk), "({x}, {y})");
    }
    let sheet_x = grid.x(grid.sheet.first);
    for y in [0, 7, 712, 719] {
        assert_eq!(r.pixel(sheet_x, y), Some(look.paper), "row {y}");
        assert_eq!(r.pixel(sheet_x - 1, y), Some(look.desk), "row {y}");
    }
    assert_eq!(read(&mut r, 20, 22, 30), "ok");
}

#[test]
fn awkward_sizes_and_sizes_below_the_minimum_all_draw() {
    let sizes = [
        (1919, 1079), (1921, 1081), (1366, 768), (1600, 900), (1024, 768), (800, 600), (720, 264), (719, 263), (640, 480), (333, 777), (181, 45), (180, 44), (179, 43),
        (168, 22), (100, 100), (30, 2000), (2000, 30), (13, 23), (12, 22), (11, 21), (1, 1), (0, 0), (0, 500), (500, 0), (5000, 3000),
    ];
    for (width, height) in sizes {
        let mut r = Rig::with_text(&long_text(), width, height);
        r.draw();
        r.type_text("x TK *y* **z** [[w]]\n# H\n> q\n- i\n");
        for chord in ["up", "ctrl+up", "pageup", "home", "shift+down", "ctrl+end", "alt+up", "ctrl+a", "right", "backspace", "ctrl+z", "pagedown"] {
            r.press(chord);
        }
        r.wheel(-3);
        r.click(width as i32 / 2, height as i32 / 2);
        r.wait(100);
        let grid = r.app.grid().clone();
        let screen = r.screen().clone();
        assert_eq!((screen.cols(), screen.rows()), (grid.cols, grid.rows), "{width} by {height}");
        assert_eq!((grid.cols, grid.rows), (width as i32 / 12, height as i32 / 22));
        // The cursor, when it is drawn, is on the sheet's rows of text.
        if let Some(at) = screen.cursor {
            assert!(grid.sheet.contains(at.col) && 0 <= at.row && at.row < grid.text_rows, "{width} by {height}: {at:?}");
        }
        // The machine line says ok wherever there is a machine line wide
        // enough to hold it.
        if let Some(machine) = grid.machine.filter(|_| grid.cols > grid.text.first + 1) {
            assert_eq!(screen.span(ok_ui::grid::Cols { first: grid.text.first, last: grid.text.first + 1 }, machine), "ok", "{width} by {height}");
        }
        // Nothing is ever drawn on the row kept empty under the text.
        if grid.rows >= 5 {
            assert_eq!(screen.span(grid.sheet, grid.text_rows), "", "{width} by {height}");
        }
        assert_eq!(r.wants().cell, (12, 22));
    }
}

#[test]
fn the_measure_follows_a_narrow_window_and_the_text_is_wrapped_again() {
    let mut r = Rig::with_text(PARAGRAPH, 70 * 12, 30 * 22);
    // Rolled to the top of the sheet, where its rows are easy to name.
    r.wheel(-1000);
    assert_eq!(r.app.grid().measure, 56);
    assert_eq!(read(&mut r, 7, 62, 1), "The orchard was eleven acres when my grandfather bought");
    assert_eq!(read(&mut r, 63, 69, 1), "");
    r.size(1920, 1080);
    assert_eq!(read(&mut r, 47, 112, 1), "The orchard was eleven acres when my grandfather bought it and");
    // A measure set by hand holds while the window has room for it.
    r.app.set_measure(40);
    assert_eq!(r.app.grid().measure, 40);
    assert_eq!(read(&mut r, 60, 99, 1), "The orchard was eleven acres when my");
    r.app.set_measure(500);
    assert_eq!(r.app.grid().measure, 100);
    r.app.set_measure(0);
    assert_eq!(r.app.grid().measure, 40);
}

#[test]
fn a_window_resized_to_nothing_and_back_shows_what_it_showed() {
    let mut r = rig(&long_text());
    r.press("up");
    r.press("up");
    let before = (r.screen().lines(), r.screen().cursor, head(&r), r.text());
    for (width, height) in [(0, 0), (1, 1), (0, 1080), (1920, 0)] {
        r.size(width, height);
        r.draw();
        r.type_text("x");
        r.press("backspace");
        r.size(1920, 1080);
        let after = (r.screen().lines(), r.screen().cursor, head(&r), r.text());
        assert_eq!(after, before, "by way of {width} by {height}");
    }
    // The picture is drawn at whatever size the frame turns out to be, even
    // if no resize was sent first.
    let mut r = Rig::new(Default::default());
    r.draw();
    assert_eq!(r.screen().cols(), 0);
}

#[test]
fn scale_two_and_three_draw_the_same_screen_in_bigger_cells() {
    let look = Look::manila();
    let mut r = rig(&long_text());
    r.app.set_scale(2);
    assert_eq!((r.screen().cols(), r.screen().rows()), (80, 24));
    assert_eq!(r.app.grid().platen, 12);
    assert_eq!(read(&mut r, 7, 72, 12), "The last row", "the typing row is at the new platen");
    assert_eq!(r.screen().cursor, Some(CursorBox { col: 19, row: 12, wide: 1, tall: 1, hollow: false }));
    assert_eq!(read(&mut r, 7, 9, 22), "ok");
    // Twelve pixels of ground above the grid; the cursor's block is one
    // cell of 24 by 44.
    let block = r.app.grid().cells(19, 12, 1, 1);
    assert_eq!((block.x, block.y, block.w, block.h), (19 * 24, 12 + 12 * 44, 24, 44));
    for (dx, dy) in [(0, 0), (23, 0), (0, 43), (23, 43), (12, 20)] {
        assert_eq!(r.pixel(block.x + dx, block.y + dy), Some(look.ink));
    }
    assert_eq!(r.pixel(block.x + 24, block.y), Some(look.paper));
    assert_eq!(r.pixel(block.x, block.y + 44), Some(look.paper));
    // Every pixel of a glyph is a two by two block.
    let frame = r.draw();
    let cell = (7 * 24, 12 + 12 * 44);
    for dy in (0..44).step_by(2) {
        for dx in (0..24).step_by(2) {
            let block: Vec<Option<Rgb>> = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|(ox, oy)| frame.get(cell.0 + dx + ox, cell.1 + dy + oy)).collect();
            assert!(block.iter().all(|pixel| *pixel == block[0]), "at {dx}, {dy}");
        }
    }

    r.app.set_scale(3);
    let grid = r.app.grid().clone();
    assert_eq!((grid.cols, grid.rows, grid.measure, grid.platen), (53, 16, 39, 7));
    assert_eq!(read(&mut r, 7, 45, 7), "The last row");
    assert_eq!(r.wants().cell, (36, 66));
    assert_eq!(r.wants().caption_height, 66);
    r.app.set_scale(1);
    assert_eq!(read(&mut r, 47, 112, 28), "The last row");
}

#[test]
fn every_look_draws_its_two_surfaces_and_tells_the_shell_its_ground() {
    let mut r = rig(&long_text());
    for look in Look::builtin() {
        assert!(r.app.set_look(&look.name));
        assert_eq!(r.pixel(5, 500), Some(look.desk), "{}", look.name);
        assert_eq!(r.pixel(41 * 12 + 2, 1070), Some(look.paper), "{}", look.name);
        assert_eq!(r.wants().ground, look.desk);
        // The cursor is the mark colour, which is the ink unless the look
        // names another.
        let block = r.app.grid().cells(47 + 12, 28, 1, 1);
        assert_eq!(r.pixel(block.x + 3, block.y + 3), Some(look.mark.unwrap_or(look.ink)), "{}", look.name);
    }
    // A look named when the program starts is the look it starts in.
    let mut r = Rig::new(ok_ui::Options { look: Some(String::from("amber")), ..Default::default() });
    r.size(1920, 1080);
    assert_eq!(r.pixel(5, 500), Some(Rgb(0x19, 0x13, 0x10)));
    assert_eq!(r.app.look().name, "amber");
}

#[test]
fn another_face_brings_its_own_cell_and_the_screen_is_laid_out_for_it() {
    let mut r = rig(&long_text());
    // Spleen is 12 by 24: the same columns, four rows fewer.
    r.app.set_face(ok_gfx::spleen());
    let grid = r.app.grid().clone();
    assert_eq!((grid.cell_w, grid.cell_h, grid.cols, grid.rows, grid.top), (12, 24, 160, 45, 0));
    assert_eq!(read(&mut r, 47, 112, grid.platen), "The last row");
    assert_eq!(r.wants().cell, (12, 24), "and the shell is told, so the window can be sized to whole cells");
    assert_eq!(read(&mut r, 47, 49, 43), "ok");
    // Terminus is 11 by 22: more columns, and a map that is twelve cells wide.
    r.app.set_face(ok_gfx::terminus());
    let grid = r.app.grid().clone();
    assert_eq!((grid.cell_w, grid.cols, grid.rows), (11, 174, 49));
    assert_eq!(grid.map.map(|map| map.marks.width()), Some(12));
    assert_eq!(read(&mut r, grid.text.first, grid.text.last, 28), "The last row");
    // An outline face has no glyphs until the host is asked for them, and
    // the canvas asks for exactly what it is about to draw. This host has
    // none to give, so every character is borrowed from Gallant.
    r.app.set_face(ok_gfx::OutlineFace::new("Courier New", 20, (12, 22), 17));
    assert!(r.host.rasterized.is_empty());
    assert_eq!(read(&mut r, 47, 112, 28), "The last row");
    for ch in "Thelastrowk".chars() {
        assert!(r.host.rasterized.contains(&ch), "{ch}");
    }
    let asked = r.host.rasterized.len();
    r.draw();
    assert_eq!(r.host.rasterized.len(), asked, "a character is asked for once");
    let look = Look::manila();
    let block = r.app.grid().cells(47, 28, 12, 1);
    let frame = r.draw();
    let inked = (0..block.w).flat_map(|dx| (0..block.h).map(move |dy| (dx, dy))).filter(|&(dx, dy)| frame.get(block.x + dx, block.y + dy) == Some(look.ink)).count();
    assert!(inked > 100, "{inked} pixels of ink in the last row");
}

#[test]
fn what_the_shell_is_told_follows_the_screen() {
    let mut r = rig("# Title");
    let wants = r.wants().clone();
    assert_eq!(wants.title, "ok", "a text with no name yet");
    assert_eq!(wants.min_cells, (60, 12));
    assert_eq!(wants.cell, (12, 22));
    assert!(wants.redraw && !wants.animating && !wants.quit && wants.fullscreen);
    // The caret is where the cursor is, for the emoji panel and input
    // methods: two cells each way on a heading.
    let block = r.app.grid().cells(47 + 10, 1, 2, 2);
    assert_eq!(wants.caret, Some(block));
    r.draw();
    assert!(!r.wants().redraw, "nothing has changed since the frame was drawn");
    r.send(Event::Tick);
    assert!(r.wants().redraw);
}
