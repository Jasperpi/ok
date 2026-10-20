//! What the design critic found on the glass, each finding held in place
//! by what the screen now reads. The findings about the paper's position
//! are in `platen.rs`, those about the say row, the contents' rhythm, the
//! labels and the change bars in `sheet.rs`; here are the rest.

mod common;

use std::path::PathBuf;

use common::{put, read, rig};
use ok_gfx::Rgb;
use ok_ui::canvas::CursorBox;
use ok_ui::harness::Rig;
use ok_ui::look::{Look, Strength};
use ok_ui::session::Opened;
use ok_ui::Options;

fn desk() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk")
}

/// The orchard essay on its desk in a window of this size.
fn essay(width: usize, height: usize) -> Rig {
    let mut r = Rig::new(Options { file: Some(desk().join("College/The orchard.md")), folder: Some(desk()), look: None, store: false });
    r.size(width, height);
    r
}

/// The colour at a pixel.
fn at(r: &mut Rig, x: i32, y: i32) -> Rgb {
    r.pixel(x, y).expect("a pixel on the screen")
}

// ───────────────────────── the cursor on inverse video ─────────────────────────

/// "The cursor is a solid block." On a TK, which is inverse already, it is
/// still one: both letters stay in the bar, and the cursor's is the one
/// with a line of paper ruled round it just inside its edge.
#[test]
fn the_cursor_on_a_tk_is_a_block_in_the_bar_and_not_a_hole_in_it() {
    let look = Look::manila();
    let mut r = rig("It was TK then.");
    for (byte, col) in [(7, 54), (8, 55)] {
        put(&mut r, 0, byte);
        assert_eq!(r.screen().cursor, Some(CursorBox { col, row: 1, wide: 1, tall: 1, hollow: false }));
        assert!(r.screen().at(54, 1).unwrap().inverse && r.screen().at(55, 1).unwrap().inverse);
        for cell in [54, 55] {
            let block = r.app.grid().cells(cell, 1, 1, 1);
            // The outermost pixels are ink all the way round on both: the
            // bar's outline is unbroken.
            for (dx, dy) in [(0, 0), (11, 0), (0, 21), (11, 21), (6, 0), (0, 11)] {
                assert_eq!(at(&mut r, block.x + dx, block.y + dy), look.ink, "cell {cell} at {dx}, {dy}");
            }
            // One pixel in, only the cursor's cell has the line of paper.
            let ruled = [(1, 1), (10, 1), (1, 20), (10, 20), (1, 11), (10, 11)].iter().all(|&(dx, dy)| at(&mut r, block.x + dx, block.y + dy) == look.paper);
            assert_eq!(ruled, cell == col, "cell {cell} with the cursor on {col}");
        }
    }
    // Off the TK the cursor is the plain block, with no line in it.
    put(&mut r, 0, 3);
    let block = r.app.grid().cells(50, 1, 1, 1);
    assert_eq!(at(&mut r, block.x + 1, block.y + 1), look.ink);
}

// ───────────────────────── narrow panes ─────────────────────────

/// At 1600 by 900 the contents are 21 cells wide and the labels 11. No
/// name is cut inside a word: it gives up its leader dots, then turns over.
#[test]
fn in_a_narrow_pane_names_turn_over_and_no_word_is_cut() {
    let mut r = essay(1600, 900);
    let grid = r.app.grid().clone();
    let (contents, labels) = (grid.contents.unwrap(), grid.labels.unwrap());
    assert_eq!((contents.width(), labels.width()), (21, 11));
    let rows: Vec<String> = (1..=24).map(|row| r.screen().span(contents, row)).collect();
    assert_eq!(
        rows,
        [
            "College",
            "  Essay plan .... 210",
            "  Grandfather",
            "    interview ... 630",
            "  Leadership",
            "    essay ....... 348",
            "  Orchard notes 1,204",
            "  The orchard ... 130",
            "  Why engineering 351",
            "",
            "Mock trial",
            "  Closing",
            "    argument .. 1,105",
            "  Cross of the",
            "    detective   1,430",
            "  Opening",
            "    statement ... 890",
            "",
            "Student government",
            "  Budget speech   540",
            "  Homecoming run",
            "    of show ..... 760",
            "",
            "",
        ]
    );
    // The open text is lit on its row, and the values read straight down
    // the right edge whatever the names do.
    assert_eq!(r.screen().at(contents.first + 2, 8).map(|cell| cell.strength), Some(Strength::INK));
    for row in [2, 4, 6, 7, 8, 9, 13, 15, 17, 20, 22] {
        assert_ne!(r.screen().at(contents.last, row).map(|cell| cell.ch), Some(' '), "row {row}");
    }
    // The links: eleven cells hold "Grandfather" only with nothing set in,
    // so the names move out, and an empty row parts the two groups.
    r.type_text(" See [[Grandfather interview]].");
    let foot = grid.links_foot();
    let block: Vec<String> = (foot - 5..=foot).map(|row| r.screen().span(labels, row)).collect();
    assert_eq!(block, ["See also", "Grandfather", "interview", "", "Linked from", "Essay plan"]);
    // With room, names are set in by two under their captions.
    let mut r = essay(1920, 1080);
    r.type_text(" See [[Grandfather interview]].");
    let block: Vec<String> = (42..=45).map(|row| read(&mut r, 134, 157, row)).collect();
    assert_eq!(block, ["See also", "  Grandfather interview", "Linked from", "  Essay plan"]);
}

// ───────────────────────── the map ─────────────────────────

fn long(paragraphs: usize) -> String {
    let mut text = String::from("# Title\n\n");
    for n in 0..paragraphs {
        text.push_str(if n == 40 { "A paragraph with a TK in it and words enough to take up a row or two of the sheet.\n\n" } else { "A paragraph of plain words that is long enough to take up a few rows of the sheet, so that the map has something to show.\n\n" });
    }
    text
}

/// Past 253 rows the map is at half size. The strip keeps to the right of
/// its columns, beside the flags and the labels; the notch comes across to
/// its left edge; and a flag is still four pixels by two in column 132.
#[test]
fn at_half_size_the_strip_keeps_to_the_labels_and_the_flag_to_its_column() {
    let look = Look::manila();
    let mut r = rig(&long(100));
    r.press("ctrl+home");
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    assert_eq!((g.letter, g.pitch, g.first), (1, 2, 0));
    assert_eq!(g.left, g.area.x + 66, "66 letters of one pixel, against the right edge of 132");
    // The strip: four pixels of margin round the marks, right edge where
    // the full-size strip has it.
    let below = g.area.y + 20;
    assert_eq!(at(&mut r, g.left - 4, below), look.paper);
    assert_eq!(at(&mut r, g.left - 5, below), look.desk);
    assert_eq!(at(&mut r, g.area.x + 135, below), look.paper);
    assert_eq!(at(&mut r, g.area.x + 136, below), look.desk);
    assert_eq!(at(&mut r, g.area.x, below), look.desk, "nothing where the full-size strip began");
    // The notch, six by two, against the strip's left edge and level with
    // the cursor's row, which is the title's.
    let notch_y = g.area.y;
    assert_eq!(at(&mut r, g.left - 10, notch_y), look.desk_ink);
    assert_eq!(at(&mut r, g.left - 5, notch_y + 1), look.desk_ink);
    assert_eq!(at(&mut r, g.left - 11, notch_y), look.desk);
    assert_eq!(at(&mut r, g.left - 10, notch_y + 2), look.desk);
    // The flag beside the TK's row: column 132, four wide and two high.
    let tk_line = (0..r.app.page().doc().line_count()).find(|&line| r.app.page().doc().line(line).contains("TK")).unwrap();
    let flag_y = g.y(r.app.page().layout().first_row(tk_line)).unwrap();
    let flag_x = g.area.x + 132 + 6;
    assert_eq!(r.app.grid().cell_at(flag_x, flag_y).0, 132);
    for (dx, dy, lit) in [(0, 0, true), (3, 1, true), (4, 0, false), (0, 2, false), (-1, 0, false)] {
        assert_eq!(at(&mut r, flag_x + dx, flag_y + dy) == look.desk_ink, lit, "{dx}, {dy}");
    }
    // A click on the narrow strip still goes to its row.
    r.click(g.left + 10, g.area.y + 100 * 2);
    assert_eq!(r.app.page().caret().row, 100);
}

/// A map that shows only part of a long text stops flush with its marks at
/// the end that is cut off, with no margin of paper there, so it does not
/// read as the whole text.
#[test]
fn a_map_that_scrolls_runs_off_its_cut_edge_with_no_margin() {
    let look = Look::manila();
    let mut r = rig(&long(400));
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    let total = r.app.page().layout().total_rows();
    assert!(g.first > 0 && g.first + g.shown == total, "the end of the text is shown and its start is not");
    // Cut at the top: paper starts with the first row of marks.
    assert_eq!(at(&mut r, g.left - 4, g.area.y), look.paper);
    assert_eq!(at(&mut r, g.left - 4, g.area.y - 1), look.desk);
    // Whole at the foot: four pixels of margin under the last marks.
    let foot = g.area.y + g.shown as i32 * g.pitch;
    assert_eq!(at(&mut r, g.left, foot + 2), look.paper);
    assert_eq!(at(&mut r, g.left, foot + 3), look.desk);
    // From the middle it is cut at both ends.
    put(&mut r, 400, 0);
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    assert!(g.first > 0 && g.first + g.shown < total);
    let foot = g.area.y + g.shown as i32 * g.pitch;
    assert_eq!(at(&mut r, g.left - 4, g.area.y - 1), look.desk);
    assert_eq!(at(&mut r, g.left - 4, foot - 1), look.paper);
    assert_eq!(at(&mut r, g.left - 4, foot), look.desk);
}

/// A text with no letter in it has no map: no strip of bare paper, and no
/// notch stuck to one.
#[test]
fn an_empty_text_has_no_map() {
    let look = Look::manila();
    let bare = |r: &mut Rig| {
        let grid = r.app.grid().clone();
        let cells = grid.cells(grid.map.unwrap().notch, 0, 14, 4);
        let frame = r.draw();
        (0..cells.w).all(|dx| (0..cells.h).all(|dy| frame.get(cells.x + dx, cells.y + dy) == Some(look.desk)))
    };
    let mut r = Rig::sized(1920, 1080);
    assert!(bare(&mut r));
    r.type_text("  ");
    assert!(bare(&mut r), "spaces are not letters");
    r.type_text("a");
    assert!(!bare(&mut r));
    let area = r.app.grid().map_area().unwrap();
    assert_eq!(at(&mut r, area.x - 4, area.y - 4), look.paper);
    r.press("backspace");
    assert!(bare(&mut r));
}

/// On one flat ground there is no strip to set the notch and the flags
/// apart from the marks, so they are as tall as a whole row of the map: a
/// block beside rows of dashes, and not one more dash.
#[test]
fn on_a_flat_ground_the_notch_and_the_flags_are_a_whole_row_tall() {
    let mut r = rig("One TK here.\n\nTwo.");
    let area = r.app.grid().map_area().unwrap();
    let notch = r.app.grid().x(120) + 2;
    let flag = area.x + 132 + 6;
    // On the desk: two pixels high, as the design has them.
    let look = Look::manila();
    assert_eq!((at(&mut r, notch, area.y + 8 + 1), at(&mut r, notch, area.y + 8 + 2)), (look.desk_ink, look.desk));
    assert_eq!((at(&mut r, flag, area.y + 1), at(&mut r, flag, area.y + 2)), (look.desk_ink, look.desk));
    // On white: four, the pitch of a row.
    assert!(r.app.set_look("sun"));
    let (ink, ground) = (Rgb(0, 0, 0), Rgb(255, 255, 255));
    assert_eq!((at(&mut r, notch, area.y + 8), at(&mut r, notch, area.y + 8 + 3), at(&mut r, notch, area.y + 8 + 4)), (ink, ink, ground));
    assert_eq!((at(&mut r, flag, area.y), at(&mut r, flag, area.y + 3), at(&mut r, flag, area.y + 4)), (ink, ink, ground));
    assert_eq!(at(&mut r, area.x, area.y + 2), ground, "the marks themselves are two pixels of the four");
}

// ───────────────────────── marks ─────────────────────────

/// A list inside a quote keeps its hang: its second row starts under the
/// item's first letter, four cells in for the quote and two for the dash.
#[test]
fn a_list_inside_a_quote_hangs_under_its_first_letter() {
    let mut r = rig("> - A list inside a quote, long enough to wrap so the hang can be seen under the item");
    assert_eq!(read(&mut r, 41, 112, 1), "   >      - A list inside a quote, long enough to wrap so the hang");
    assert_eq!(read(&mut r, 41, 112, 2), "            can be seen under the item");
    // The cursor on the space after the hung mark is in the empty column
    // before the text, never on the change bars'.
    put(&mut r, 0, 1);
    assert_eq!(r.screen().cursor.map(|at| at.col), Some(46));
    put(&mut r, 0, 0);
    assert_eq!(r.screen().cursor.map(|at| at.col), Some(44));
}

/// A text handed over with its place remembered is put back there; one
/// with none has its cursor's row on the platen, which is what "every
/// return: the same row on the platen" asks of a session that keeps no row.
#[test]
fn a_text_opened_with_no_remembered_row_has_its_cursor_on_the_platen() {
    let mut r = Rig::sized(1920, 1080);
    r.app.open_text(Opened { text: (0..10).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n"), cursor: Some(ok_text::Pos::new(4, 2)), ..Opened::default() });
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((49, 28)));
    assert_eq!(read(&mut r, 47, 112, 24), "line 0");
    assert_eq!(read(&mut r, 41, 118, 23), "");
}
