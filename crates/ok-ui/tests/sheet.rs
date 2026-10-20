//! What is on the glass: the sheet, the machine's two lines and both sides,
//! read back from the screen as text with its strengths and attributes.

mod common;

use std::path::PathBuf;

use common::{put, read, rig, said};
use ok_gfx::Rgb;
use ok_host::Event;
use ok_text::{Pos, Span};
use ok_ui::canvas::{Cell, CursorBox};
use ok_ui::grid::Cols;
use ok_ui::harness::Rig;
use ok_ui::look::{Look, Strength, Surface};
use ok_ui::Options;

/// The first column of the text at 160 columns, and all its columns.
const TEXT: i32 = 47;
const SHEET_TEXT: Cols = Cols { first: 47, last: 112 };
/// The sheet from edge to edge.
const SHEET: Cols = Cols { first: 41, last: 118 };

fn cell(r: &mut Rig, col: i32, row: i32) -> Cell {
    r.screen().at(col, row).expect("a cell on the screen")
}

/// Where a piece of text is: on the sheet if it is there, and otherwise
/// anywhere on the screen.
fn find(r: &mut Rig, needle: &str) -> (i32, i32) {
    let screen = r.screen();
    let on_sheet = (0..screen.rows()).find_map(|row| {
        let line: Vec<char> = (SHEET.first..=SHEET.last).filter_map(|col| screen.at(col, row)).map(|cell| cell.ch).collect();
        let want: Vec<char> = needle.chars().collect();
        line.windows(want.len()).position(|window| window == want.as_slice()).map(|col| (SHEET.first + col as i32, row))
    });
    on_sheet.or_else(|| screen.find(needle)).unwrap_or_else(|| panic!("{needle:?} is not on the screen"))
}

/// The cells of a piece of text, wherever it is on the screen.
fn cells(r: &mut Rig, needle: &str) -> Vec<Cell> {
    let (col, row) = find(r, needle);
    (0..needle.chars().count() as i32).map(|along| cell(r, col + along, row)).collect()
}

fn strengths(r: &mut Rig, needle: &str) -> Vec<Strength> {
    cells(r, needle).into_iter().map(|cell| cell.strength).collect()
}

fn desk() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk")
}

/// The essay of the hand-drawn proof: two paragraphs from its file and the
/// rest typed in this sitting.
fn essay() -> Rig {
    let mut r = Rig::new(Options { file: Some(desk().join("College/The orchard.md")), folder: Some(desk()), look: None, store: false });
    r.size(1920, 1080);
    r.type_text("\nHe let me think that for about a week. Then he walked me to the one tree he had left alone on purpose, TK the year he planted it, and showed me what keeping all three looks like. The branches had split under the weight. The fruit was the size of walnuts and tasted of nothing. He did not say anything about it, which I have come to understand as a kind of teaching, the kind I got again in [[Grandfather interview]] when I asked what he would do differently and he said he would have **thinned harder**.");
    r.type_text("\nI think about that tree whenever I am asked to add one more thing to a week that is already full. For a long time I believed the answer was to keep all three and");
    r
}

// ───────────────────────── the proof, region by region ─────────────────────────

#[test]
fn the_essay_of_the_proof_is_set_as_the_proof_has_it() {
    let mut r = essay();
    // The heading: double size, its mark hung in the gutter at half strength
    // on the lower of its two rows.
    assert_eq!(r.screen().big(SHEET_TEXT, 1), "The orchard");
    let title = cell(&mut r, TEXT, 1);
    assert!(title.big && title.strength == Strength::INK && title.surface == Surface::Sheet);
    assert_eq!(read(&mut r, 41, 46, 1), "");
    assert_eq!(read(&mut r, 41, 46, 2), "   #");
    let mark = cell(&mut r, 44, 2);
    assert!(!mark.big && mark.strength == Strength::HALF);
    // The text, wrapped at 66 and set from column 47.
    assert_eq!(read(&mut r, TEXT, 112, 3), "");
    assert_eq!(read(&mut r, TEXT, 112, 4), "The orchard was eleven acres when my grandfather bought it and");
    assert_eq!(read(&mut r, TEXT, 112, 7), "in rows he paced out in boots, one long step and one short.");
    assert_eq!(read(&mut r, TEXT, 112, 14), "I remember thinking that a better farmer would have found a way to");
    assert_eq!(read(&mut r, TEXT, 112, 28), "answer was to keep all three and");
    assert_eq!(read(&mut r, 0, 159, 29), "", "nothing below the row being written");
    // The cursor: a solid block on the platen row, after the last letter.
    assert_eq!(r.app.grid().platen, 28);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 32, row: 28, wide: 1, tall: 1, hollow: false }));
    assert!(cell(&mut r, TEXT + 32, 28).inverse);
}

#[test]
fn marks_are_half_strength_and_the_words_they_govern_take_the_attribute() {
    let mut r = essay();
    let waste = cells(&mut r, "*waste*");
    assert_eq!(waste[0].strength, Strength::HALF);
    assert_eq!(waste[6].strength, Strength::HALF);
    assert!(waste[1..6].iter().all(|cell| cell.under && !cell.double && cell.strength == Strength::INK));
    assert!(!waste[0].under && !waste[6].under);

    let strong = cells(&mut r, "**thinned harder**.");
    assert!(strong[..2].iter().chain(&strong[16..18]).all(|cell| cell.strength == Strength::HALF && !cell.double));
    assert!(strong[2..16].iter().all(|cell| cell.double && !cell.under && cell.strength == Strength::INK));
    assert!(!strong[18].double, "the full stop after it is plain");

    let link = cells(&mut r, "[[Grandfather interview]]");
    assert!(link[..2].iter().chain(&link[23..]).all(|cell| cell.strength == Strength::HALF));
    assert!(link[2..23].iter().all(|cell| cell.strength == Strength::INK && !cell.inverse));

    let tk = cells(&mut r, " TK ");
    assert_eq!(tk.iter().map(|cell| cell.inverse).collect::<Vec<_>>(), [false, true, true, false]);
}

#[test]
fn change_bars_stand_beside_the_rows_written_in_this_sitting() {
    let mut r = essay();
    let bars: Vec<i32> = (0..49).filter(|&row| r.screen().bar(row)).collect();
    assert_eq!(bars, (17..=28).collect::<Vec<_>>(), "from the first row written to the row being written, and not the blank row Enter left under the older paragraph");
    // A two-pixel stroke down the middle of column 45, in the mark colour.
    let look = Look::manila();
    let (x, y) = r.cell_centre(45, 20);
    assert_eq!(r.pixel(x - 1, y), Some(look.ink));
    assert_eq!(r.pixel(x, y), Some(look.ink));
    assert_eq!(r.pixel(x - 2, y), Some(look.paper));
    assert_eq!(r.pixel(x + 1, y), Some(look.paper));
    assert_eq!(r.pixel(x, r.cell_centre(45, 10).1), Some(look.paper));
}

#[test]
fn a_new_day_begins_at_four_and_takes_the_change_bars_with_it() {
    let mut r = rig("An older line.");
    r.host.set_time(2026, 10, 11, 23, 30);
    r.type_text(" More.");
    assert!(r.screen().bar(1));
    r.host.set_time(2026, 10, 12, 3, 59);
    assert!(r.screen().bar(1), "the small hours belong to the evening before");
    r.host.set_time(2026, 10, 12, 4, 0);
    assert!(!r.screen().bar(1));
    r.type_text(" Again.");
    assert!(r.screen().bar(1));
}

#[test]
fn only_the_lines_that_read_differently_are_marked_as_changed() {
    let mut r = rig("First.\n\nSecond.");
    // Enter at the end of a paragraph leaves that paragraph as it was, and
    // the blank rows it writes are not something written: a bar stands
    // beside a blank row only between two rows that have one.
    r.type_text("\n");
    let bars: Vec<bool> = (1..=5).map(|row| r.screen().bar(row)).collect();
    assert_eq!(bars, [false; 5]);
    r.type_text("Third.");
    let bars: Vec<bool> = (1..=5).map(|row| r.screen().bar(row)).collect();
    assert_eq!(bars, [false, false, false, false, true]);
    r.type_text("\nFourth.");
    let bars: Vec<bool> = (1..=7).map(|row| r.screen().bar(row)).collect();
    assert_eq!(bars, [false, false, false, false, true, true, true], "the blank row between two paragraphs of today");
    // Enter at its start leaves it as it was, one line further down.
    let mut r = rig("First.\n\nSecond.");
    put(&mut r, 2, 0);
    r.press("enter");
    assert_eq!(r.text(), "First.\n\n\n\nSecond.");
    let bars: Vec<bool> = (1..=5).map(|row| r.screen().bar(row)).collect();
    assert_eq!(bars, [false; 5]);
    // Carrying a paragraph marks the one that moved and not the one passed.
    let mut r = rig("First.\n\nSecond.");
    r.press("alt+up");
    let bars: Vec<bool> = (1..=3).map(|row| r.screen().bar(row)).collect();
    assert_eq!(bars, [true, false, false]);
}

#[test]
fn the_machine_line_has_ok_and_the_facts_and_the_say_row_one_sentence() {
    let mut r = essay();
    let (say, machine) = (r.app.grid().say.unwrap(), r.app.grid().machine.unwrap());
    assert_eq!((say, machine), (46, 47));
    assert_eq!(read(&mut r, 41, 118, 45), "", "the row under the text is always empty");
    assert_eq!(read(&mut r, 0, 159, 48), "");
    assert_eq!(said(&mut r), "");
    assert_eq!(read(&mut r, TEXT, 49, machine), "ok");
    assert_eq!(strengths(&mut r, "ok"), [Strength::INK; 2]);
    let facts = "The orchard    263 words";
    assert_eq!(read(&mut r, 50, 112, machine).trim_start(), facts);
    assert_eq!(find(&mut r, facts), (113 - facts.len() as i32, machine), "right-aligned to column 112");
    assert!(strengths(&mut r, facts).iter().all(|&s| s == Strength::HALF || s == Strength::GROUND));

    r.app.say("Last here Tuesday at 23:41. That sitting added 388 words.");
    assert_eq!(said(&mut r), "Last here Tuesday at 23:41. That sitting added 388 words.");
    assert_eq!(cell(&mut r, TEXT, say).strength, Strength::HALF);
    assert_eq!(read(&mut r, 41, 118, 45), "", "one row is all it takes");
    // A sentence too long for the measure turns over between sentences or
    // between words, onto the row above. Only a word wider than the measure
    // is cut, and nothing ever goes over the text.
    r.app.say("Enter keeps measure at 66. Esc puts it back to 72. The default is 66.");
    assert_eq!(read(&mut r, TEXT, 112, 45), "Enter keeps measure at 66. Esc puts it back to 72.");
    assert_eq!(said(&mut r), "The default is 66.");
    r.app.say("x".repeat(200));
    assert_eq!(said(&mut r).len(), 66);
    assert_eq!(read(&mut r, TEXT, 112, 45).len(), 66);
    assert_eq!(read(&mut r, TEXT, 112, 44), "");
}

#[test]
fn the_facts_count_the_words_or_the_words_selected() {
    let mut r = Rig::sized(1920, 1080);
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "0 words");
    r.type_text("One");
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "1 word");
    r.type_text(" two three");
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "3 words");
    r.press("ctrl+shift+left");
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "1 word selected");
    r.press("ctrl+shift+left");
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "2 words selected");
    r.press("right");
    assert_eq!(read(&mut r, 50, 112, 47).trim_start(), "3 words");
}

#[test]
fn the_contents_are_a_contents_page_with_leader_dots() {
    let mut r = essay();
    assert_eq!(read(&mut r, 0, 40, 0), "");
    assert_eq!(read(&mut r, 0, 40, 1), "  College");
    assert_eq!(read(&mut r, 0, 40, 2), "    Essay plan .................. 210");
    assert_eq!(read(&mut r, 0, 40, 5), "    Orchard notes ............. 1,204");
    assert_eq!(read(&mut r, 0, 40, 6), "    The orchard ................. 263");
    // An empty row before each folder name after the first, as a contents
    // page sets its sections.
    assert_eq!(read(&mut r, 0, 40, 7), "    Why engineering ............. 351");
    assert_eq!(read(&mut r, 0, 40, 8), "");
    assert_eq!(read(&mut r, 0, 40, 9), "  Mock trial");
    assert_eq!(read(&mut r, 0, 40, 13), "");
    assert_eq!(read(&mut r, 0, 40, 14), "  Student government");
    assert_eq!(read(&mut r, 0, 40, 16), "    Homecoming run of show ...... 760");
    assert_eq!(read(&mut r, 0, 40, 17), "");
    // The open text in ink, everything else at half, the dots faint.
    let open = strengths(&mut r, "The orchard ................. 263");
    assert!(open[..11].iter().chain(&open[30..]).all(|&s| s == Strength::INK));
    assert!(open[12..29].iter().all(|&s| s == Strength::FAINT));
    let other = strengths(&mut r, "Essay plan .................. 210");
    assert!(other[..5].iter().chain(&other[30..]).all(|&s| s == Strength::HALF));
    assert!(strengths(&mut r, "College").iter().all(|&s| s == Strength::HALF));
    assert_eq!(cell(&mut r, 4, 6).surface, Surface::Desk);
}

#[test]
fn the_labels_name_the_headings_and_the_links_block_sits_at_the_foot() {
    let mut r = essay();
    // The title is this text's only heading, so it is not labelled; the
    // paragraph being written is, by its opening words, level with the
    // notch: its row is the 28th of the text, in the sixth row of cells.
    assert_eq!(read(&mut r, 134, 157, 1), "");
    assert_eq!(read(&mut r, 134, 157, 6), "I think about that tree");
    assert_eq!(cell(&mut r, 134, 6).strength, Strength::HALF);
    assert_eq!((2..42).filter(|&row| !read(&mut r, 134, 157, row).is_empty()).collect::<Vec<_>>(), [6]);
    assert_eq!(read(&mut r, 134, 157, 42), "See also");
    assert_eq!(read(&mut r, 134, 157, 43), "  Grandfather interview");
    assert_eq!(read(&mut r, 134, 157, 44), "Linked from");
    assert_eq!(read(&mut r, 134, 157, 45), "  Essay plan");
    // Captions and names are both words to be read: both at half.
    assert_eq!(cell(&mut r, 134, 42).strength, Strength::HALF);
    assert_eq!(cell(&mut r, 136, 43).strength, Strength::HALF);
    assert_eq!(read(&mut r, 134, 157, 41), "");
    // A link to a text not yet started is listed with a question mark, and
    // its name on the sheet is at half strength like its brackets.
    r.type_text(" See [[Nowhere yet]] too.");
    assert_eq!(read(&mut r, 134, 157, 43), "  Nowhere yet ?");
    assert_eq!(read(&mut r, 134, 157, 41), "See also");
    assert!(strengths(&mut r, "[[Nowhere yet]]").iter().all(|&s| s == Strength::HALF));
}

#[test]
fn several_headings_are_labelled_where_their_lines_fall_in_the_map() {
    let mut text = String::from("# Title\n\n");
    for part in ["Opening", "Where it is weak", "Closing"] {
        text.push_str(&format!("## {part}\n\n"));
        for _ in 0..12 {
            text.push_str("A paragraph of plain words that is long enough to take up a few rows of the sheet, so that the map has something to show.\n\n");
        }
    }
    let mut r = rig(&text);
    // Each heading's line, as a row of the map, falls in a row of cells:
    // four pixels to a row of text, twenty-two to a row of cells.
    let rows: Vec<usize> = (0..r.app.page().doc().line_count()).filter(|&line| r.app.page().doc().line(line).starts_with('#')).map(|line| r.app.page().layout().first_row(line)).collect();
    // The first two fall in the same row of cells, so the later moves down.
    let wanted: Vec<i32> = rows.iter().map(|&row| 1 + (row as i32 * 4) / 22).collect();
    assert_eq!(wanted[..2], [1, 1]);
    let expected = [1, 2, wanted[2], wanted[3]];
    assert!(wanted[2] > 2 && wanted[3] > wanted[2]);
    for (title, row) in ["Title", "Opening", "Where it is weak", "Closing"].into_iter().zip(expected) {
        assert_eq!(read(&mut r, 134, 157, row), title);
    }
    // The cursor is at the end, in the last section: that label is in ink.
    assert_eq!(strengths(&mut r, "Closing")[0], Strength::INK);
    let labels = r.screen().lines().iter().filter(|row| row.get(134..).is_some_and(|side| side.trim() == "Opening")).count();
    assert_eq!(labels, 1);
    let opening = find(&mut r, "Opening");
    assert_eq!(cell(&mut r, opening.0, opening.1).strength, Strength::HALF);
    // Two labels that want the same row: the later moves down.
    let mut r = rig("# One\n## Two\n### Three\n\nBody.");
    assert_eq!(read(&mut r, 134, 157, 1), "One");
    assert_eq!(read(&mut r, 134, 157, 2), "Two");
    assert_eq!(read(&mut r, 134, 157, 3), "Three");
}

#[test]
fn a_text_with_no_headings_labels_the_paragraph_he_is_in_and_no_other() {
    let mut r = rig("The first paragraph opens with these words and then goes on for a while longer than the label has room for.\n\n- A list comes second, and its one item is a long one\n\n> And a quote third, which runs on as well\n\nShort.");
    let labels = |r: &mut Rig| -> Vec<String> { (0..45).map(|row| read(r, 134, 157, row)).filter(|label| !label.is_empty()).collect() };
    put(&mut r, 4, 6);
    assert_eq!(labels(&mut r), ["And a quote third, which"]);
    assert_eq!(read(&mut r, 134, 157, 2), "And a quote third, which", "level with the notch");
    assert_eq!(cell(&mut r, 134, 2).strength, Strength::HALF);
    // Cut at a whole word, and not left ending on a comma or a joining word.
    put(&mut r, 2, 4);
    assert_eq!(labels(&mut r), ["A list comes second"]);
    put(&mut r, 0, 0);
    assert_eq!(labels(&mut r), ["The first paragraph"]);
    assert_eq!(read(&mut r, 134, 157, 1), "The first paragraph");
    // On a blank line there is no paragraph to name, and one short enough
    // to be read whole in a label has no need of one: as he began it the
    // label would only echo him letter by letter.
    put(&mut r, 1, 0);
    assert!(labels(&mut r).is_empty());
    put(&mut r, 6, 6);
    assert!(labels(&mut r).is_empty());
}

#[test]
fn a_label_too_long_for_its_row_turns_over_or_is_cut_at_a_word() {
    // The row below is free: the title takes it.
    let mut r = rig(&format!("# One\n\n## If he changes his account of that evening\n\n{}## Last\n\nMore.", "Body.\n\n".repeat(12)));
    assert_eq!(read(&mut r, 134, 157, 1), "One");
    assert_eq!(read(&mut r, 134, 157, 2), "If he changes his");
    assert_eq!(read(&mut r, 134, 157, 3), "account of that evening");
    assert_eq!(read(&mut r, 134, 157, 4), "");
    assert_eq!(read(&mut r, 134, 157, 6), "Last", "where its line falls in the map");
    // Another label wants it: one row, cut where it does not end on a
    // joining word or a comma.
    let mut r = rig("# One\n## If he changes his account of that evening\n## A third-level heading, plain\n## Last");
    assert_eq!(read(&mut r, 134, 157, 2), "If he changes");
    assert_eq!(read(&mut r, 134, 157, 3), "A third-level heading");
    assert_eq!(read(&mut r, 134, 157, 4), "Last");
}

// ───────────────────────── every kind of markup ─────────────────────────

#[test]
fn a_second_level_heading_is_double_struck_and_deeper_ones_plain() {
    let mut r = rig("## Second\n\n### Third\n\n#### Fourth\n\n##### Fifth");
    assert_eq!(read(&mut r, 41, 112, 1), "  ##  Second");
    assert!(cells(&mut r, "Second").iter().all(|cell| cell.double && !cell.big && cell.strength == Strength::INK));
    assert!(cells(&mut r, "##")[0].strength == Strength::HALF && !cells(&mut r, "##")[0].double);
    assert_eq!(read(&mut r, 41, 112, 3), " ###  Third");
    assert!(cells(&mut r, "Third").iter().all(|cell| !cell.double && !cell.big));
    // The gutter is three cells. Four marks and more do not fit beside the
    // text, so they stay in it as typed, at half strength like any mark,
    // and nothing is ever put in the sheet's edge column.
    assert_eq!(read(&mut r, 41, 112, 5), "      #### Fourth");
    assert_eq!(strengths(&mut r, "#### Fourth")[..4], [Strength::HALF; 4]);
    assert_eq!(strengths(&mut r, "#### Fourth")[5], Strength::INK);
    assert_eq!(read(&mut r, 41, 112, 7), "      ##### Fifth");
    assert_eq!(strengths(&mut r, "##### Fifth")[..5], [Strength::HALF; 5]);
    assert_eq!(strengths(&mut r, "##### Fifth")[6], Strength::INK);
    for row in 0..45 {
        assert_eq!(read(&mut r, 41, 41, row), "", "row {row}");
    }
}

#[test]
fn a_heading_wraps_at_half_the_measure_and_takes_two_rows_a_row() {
    let mut r = rig("# A title long enough that it cannot be set on one double row\n\nBody.");
    assert_eq!(r.screen().big(SHEET_TEXT, 1), "A title long enough that it");
    assert_eq!(r.screen().big(SHEET_TEXT, 3), "cannot be set on one double row");
    assert_eq!(read(&mut r, 41, 46, 2), "   #", "the mark hangs beside the first row only");
    assert_eq!(read(&mut r, 41, 46, 4), "");
    assert_eq!(read(&mut r, TEXT, 112, 6), "Body.");
}

#[test]
fn a_quote_hangs_its_mark_and_is_set_in_by_four_from_both_edges() {
    let mut r = rig("> He did not say anything about it, which I have come to understand as a kind of teaching.\n\nAfter.");
    assert_eq!(read(&mut r, 41, 112, 1), "   >      He did not say anything about it, which I have come to");
    assert_eq!(read(&mut r, 41, 112, 2), "          understand as a kind of teaching.");
    assert_eq!(cell(&mut r, 44, 1).strength, Strength::HALF);
    assert_eq!(cell(&mut r, 51, 1).strength, Strength::INK);
    let widest = (1..=2).map(|row| read(&mut r, TEXT, 112, row).len()).max().unwrap();
    assert!(widest <= 4 + 58, "no row reaches into the last four cells of the measure");
}

#[test]
fn list_rows_hang_under_the_first_letter_of_the_item() {
    let mut r = rig("- An item with enough words in it that it has to wrap onto a second row of the sheet\n- [ ] A box, shown as typed\n12. Twelve");
    assert_eq!(read(&mut r, TEXT, 112, 1), "- An item with enough words in it that it has to wrap onto a");
    assert_eq!(read(&mut r, TEXT, 112, 2), "  second row of the sheet");
    assert_eq!(read(&mut r, TEXT, 112, 3), "- [ ] A box, shown as typed");
    assert_eq!(read(&mut r, TEXT, 112, 4), "12. Twelve");
    assert_eq!(cell(&mut r, TEXT, 1).strength, Strength::HALF);
    assert_eq!(cell(&mut r, TEXT + 2, 1).strength, Strength::INK);
    assert_eq!(strengths(&mut r, "- [ ] A")[..6], [Strength::HALF; 6]);
    assert_eq!(strengths(&mut r, "12. T")[..4], [Strength::HALF; 4]);
    assert_eq!(read(&mut r, 41, 46, 1), "", "a list marker does not hang in the gutter");
}

#[test]
fn the_link_the_cursor_is_in_is_inverse_from_bracket_to_bracket() {
    let mut r = rig("See [[Essay plan]] now.");
    assert!(cells(&mut r, "[[Essay plan]]").iter().all(|cell| !cell.inverse));
    put(&mut r, 0, 8);
    let link = cells(&mut r, "[[Essay plan]]");
    // Every cell, the cursor's too: the bar is unbroken and the cursor is
    // still a solid block.
    assert!(link.iter().all(|cell| cell.inverse));
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 8, row: 1, wide: 1, tall: 1, hollow: false }));
    // It is told from its neighbours by a line of paper ruled round it one
    // pixel inside its edge, which no other cell of the bar has.
    let look = Look::manila();
    let ring = |r: &mut Rig, col: i32| -> (Option<Rgb>, Option<Rgb>, Option<Rgb>) {
        let block = r.app.grid().cells(col, 1, 1, 1);
        (r.pixel(block.x, block.y), r.pixel(block.x + 1, block.y + 1), r.pixel(block.x + 6, block.y + 20))
    };
    assert_eq!(ring(&mut r, TEXT + 8), (Some(look.ink), Some(look.paper), Some(look.paper)));
    assert_eq!(ring(&mut r, TEXT + 7), (Some(look.ink), Some(look.ink), Some(look.ink)));
    assert!(!cell(&mut r, TEXT + 3, 1).inverse && !cell(&mut r, TEXT + 18, 1).inverse);
    // Just before it and just after it the cursor is outside.
    for col in [3, 18] {
        put(&mut r, 0, col);
        assert_eq!(cells(&mut r, "[[Essay plan]]").iter().filter(|cell| cell.inverse).count(), 0, "column {col}");
    }
}

#[test]
fn tk_is_inverse_only_standing_alone_in_capitals() {
    let mut r = rig("TK and TKs and tk and (TK) and ATK\n\n```\nTK in code\n```");
    put(&mut r, 4, 0);
    let row: Vec<bool> = (0..34).map(|along| cell(&mut r, TEXT + along, 1).inverse).collect();
    let lit: Vec<usize> = row.iter().enumerate().filter(|(_, &on)| on).map(|(at, _)| at).collect();
    assert_eq!(lit, [0, 1, 23, 24]);
    assert!(!cell(&mut r, TEXT, 4).inverse, "nothing in fenced code is a TK");
}

#[test]
fn code_tables_and_rules_are_shown_as_typed() {
    let mut r = rig("Use `wrap()` here.\n\n| a | b |\n\n---\n\n```\n# not a heading\n```\n\n***both*** and _under_ and [text](http://example.com)");
    let code = cells(&mut r, "`wrap()`");
    assert_eq!((code[0].strength, code[7].strength), (Strength::HALF, Strength::HALF));
    assert!(code[1..7].iter().all(|cell| cell.strength == Strength::INK && !cell.double && !cell.under));
    assert_eq!(read(&mut r, TEXT, 112, 3), "| a | b |");
    assert_eq!(read(&mut r, TEXT, 112, 5), "---");
    assert_eq!(strengths(&mut r, "---"), [Strength::HALF; 3]);
    // Fenced code is plain ink and nothing in it is markup.
    assert_eq!(read(&mut r, 41, 112, 8), "      # not a heading");
    assert!(cells(&mut r, "# not a heading").iter().all(|cell| cell.strength == Strength::INK && !cell.big));
    let both = cells(&mut r, "***both***");
    assert!(both[3..7].iter().all(|cell| cell.double && cell.under));
    assert!(both[..3].iter().all(|cell| cell.strength == Strength::HALF && !cell.under));
    assert!(cells(&mut r, "under").iter().all(|cell| cell.under && !cell.double));
    let link = cells(&mut r, "[text](http://example.com)");
    assert_eq!(link[0].strength, Strength::HALF);
    assert_eq!(link[1].strength, Strength::INK);
    assert_eq!((link[5].strength, link[6].strength), (Strength::HALF, Strength::HALF));
}

#[test]
fn marked_wide_and_control_characters_and_tabs_each_keep_to_their_cells() {
    let mut r = rig("an\u{0313}\u{4E00}b\tc\u{1}d\u{200B}e");
    r.app.page_mut().read_only = false;
    let row: Vec<char> = (0..12).map(|along| cell(&mut r, TEXT + along, 1).ch).collect();
    assert_eq!(row, ['a', 'n', '\u{4E00}', ' ', 'b', ' ', ' ', ' ', ' ', 'c', '\u{1}', 'd']);
    assert_eq!(cell(&mut r, TEXT + 12, 1).ch, 'e', "a zero-width space takes no cell");
    assert_eq!(r.screen().cursor.map(|at| at.col), Some(TEXT + 13));
    // The cursor on a wide character is two cells wide; on a tab, one.
    put(&mut r, 0, 4);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 2, row: 1, wide: 2, tall: 1, hollow: false }));
    put(&mut r, 0, 8);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 5, row: 1, wide: 1, tall: 1, hollow: false }));
    // A line that is nothing but marks, and one that starts with them.
    let mut r = rig("\u{0301}\u{0301}\n\u{0301}x\n\u{FEFF}\u{202E}");
    r.draw();
    r.type_text("\u{0301}");
    r.draw();
}

// ───────────────────────── the cursor and the selection ─────────────────────────

#[test]
fn the_cursor_on_a_heading_is_two_cells_wide_and_two_rows_tall() {
    let mut r = rig("# Title");
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 10, row: 1, wide: 2, tall: 2, hollow: false }));
    put(&mut r, 0, 3);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: TEXT + 2, row: 1, wide: 2, tall: 2, hollow: false }));
    let under = cell(&mut r, TEXT + 2, 1);
    assert!(under.inverse && under.big && under.ch == 'i');
    assert!(cell(&mut r, TEXT + 3, 2).inverse, "all four cells of the letter");
    // Its pixels: the block in ink, the letter left as paper inside it.
    let look = Look::manila();
    let block = r.app.grid().cells(TEXT + 2, 1, 2, 2);
    let colours: Vec<Rgb> = (0..block.w).flat_map(|dx| (0..block.h).map(move |dy| (dx, dy))).filter_map(|(dx, dy)| r.pixel(block.x + dx, block.y + dy)).collect();
    assert!(colours.iter().all(|&colour| colour == look.ink || colour == look.paper));
    assert!(colours.contains(&look.paper) && colours.iter().filter(|&&colour| colour == look.ink).count() > colours.len() / 2);
    // On the mark in the gutter it is one cell again, on the mark's row.
    put(&mut r, 0, 0);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: 44, row: 2, wide: 1, tall: 1, hollow: false }));
    assert_eq!(cell(&mut r, 44, 2).ch, '#');
    // The space after the mark has no cell beside it, the next column
    // being the change bars': the cursor on it rests in the empty column
    // before the text, and never on a bar.
    put(&mut r, 0, 1);
    assert_eq!(r.screen().cursor, Some(CursorBox { col: 46, row: 2, wide: 1, tall: 1, hollow: false }));
}

#[test]
fn the_cursor_rests_one_cell_past_a_full_row() {
    let mut r = rig(&"x".repeat(66));
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((113, 1)));
    r.type_text("y");
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((48, 2)));
    // Spaces typed at the end of a full row hang past the measure, and the
    // cursor waits at the edge for the next word.
    let mut r = rig(&"x".repeat(66));
    r.type_text("    ");
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((113, 1)));
    r.type_text("y");
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((48, 2)));
}

#[test]
fn a_selection_is_inverse_video_across_paragraphs_and_has_no_block() {
    let mut r = rig("First paragraph.\n\nSecond paragraph.\n\nThird.");
    r.app.page_mut().select(Span { start: Pos::new(0, 6), end: Pos::new(4, 3) });
    assert_eq!(r.screen().cursor, None, "the selection is the cursor");
    let lit = |r: &mut Rig, row: i32| -> Vec<i32> { (41..119).filter(|&col| r.screen().at(col, row).is_some_and(|cell| cell.inverse)).collect() };
    // From the seventh letter to the end of the line and the break after it.
    assert_eq!(lit(&mut r, 1), (TEXT + 6..=TEXT + 16).collect::<Vec<_>>());
    assert_eq!(lit(&mut r, 2), [TEXT], "a selected blank line shows as one cell");
    assert_eq!(lit(&mut r, 3), (TEXT..=TEXT + 17).collect::<Vec<_>>());
    assert_eq!(lit(&mut r, 4), [TEXT]);
    assert_eq!(lit(&mut r, 5), [TEXT, TEXT + 1, TEXT + 2]);
    // A selected heading takes its mark with it.
    let mut r = rig("# Title\n\nBody.");
    r.press("ctrl+a");
    assert!(cell(&mut r, 44, 2).inverse && cell(&mut r, TEXT, 1).inverse);
}

#[test]
fn a_hollow_box_keeps_the_place_while_the_keyboard_is_elsewhere() {
    let mut r = rig("abc");
    put(&mut r, 0, 1);
    r.send(Event::Focus(false));
    assert_eq!(r.screen().kept, Some(CursorBox { col: TEXT + 1, row: 1, wide: 1, tall: 1, hollow: true }));
    assert_eq!(r.screen().cursor, None);
    assert!(!cell(&mut r, TEXT + 1, 1).inverse);
    r.send(Event::Focus(true));
    assert_eq!(r.screen().cursor.map(|at| at.hollow), Some(false));
    assert_eq!(r.screen().kept, None);
}

// ───────────────────────── the map ─────────────────────────

#[test]
fn the_map_is_a_strip_of_paper_with_a_square_for_every_letter() {
    let mut r = essay();
    let look = Look::manila();
    let area = r.app.grid().map_area().unwrap();
    // The strip: four pixels of margin round the marks on every side, its
    // top edge on the grid at the top of row 1, and no further down than
    // the marks go (the 28th row's are two pixels tall).
    assert_eq!(area.y - 4, r.app.grid().y(1));
    assert_eq!(r.pixel(area.x - 4, area.y - 4), Some(look.paper));
    assert_eq!(r.pixel(area.x - 4, area.y - 5), Some(look.desk));
    assert_eq!(r.pixel(area.x - 5, area.y), Some(look.desk));
    assert_eq!(r.pixel(area.x + 135, area.y), Some(look.paper));
    assert_eq!(r.pixel(area.x + 136, area.y), Some(look.desk));
    assert_eq!(r.pixel(area.x, area.y + 27 * 4 + 2 + 3), Some(look.paper));
    assert_eq!(r.pixel(area.x, area.y + 27 * 4 + 2 + 4), Some(look.desk));
    // The heading is twice as big: "The" is three squares of four pixels.
    let half = look.shade(Surface::Sheet, Strength::HALF);
    assert_eq!(r.pixel(area.x, area.y), Some(half));
    assert_eq!(r.pixel(area.x + 11, area.y + 3), Some(half));
    assert_eq!(r.pixel(area.x + 12, area.y), Some(look.paper), "the space after it is left open");
    assert_eq!(r.pixel(area.x, area.y + 4), Some(look.paper), "and so is the rest of its row");
    // An older row at half strength, one written today in ink: two pixels
    // a letter, four a row.
    let older = area.y + 3 * 4;
    assert_eq!(r.pixel(area.x, older), Some(half));
    assert_eq!(r.pixel(area.x + 5, older + 1), Some(half));
    assert_eq!(r.pixel(area.x + 6, older), Some(look.paper), "after The, a space");
    assert_eq!(r.pixel(area.x, older + 2), Some(look.paper), "between rows");
    let today = area.y + 16 * 4;
    assert_eq!(r.pixel(area.x, today), Some(look.ink));
    // The notch, level with the cursor's row, and a flag beside the TK.
    let notch = r.app.grid().x(120) + 2;
    let cursor_row = area.y + 27 * 4;
    assert_eq!(r.pixel(notch, cursor_row), Some(look.desk_ink));
    assert_eq!(r.pixel(notch + 5, cursor_row + 1), Some(look.desk_ink));
    assert_eq!(r.pixel(notch, cursor_row - 1), Some(look.desk));
    let flag = area.x + 132 + 6;
    assert_eq!(r.pixel(flag, area.y + 17 * 4), Some(look.desk_ink));
    assert_eq!(r.pixel(flag, area.y + 16 * 4), Some(look.desk));
}

#[test]
fn the_map_never_shrinks_until_it_must_and_then_only_once() {
    let paragraph = "A paragraph of plain words that is long enough to take up a few rows.\n\n";
    let r = rig(&paragraph.repeat(80));
    let total = r.app.page().layout().total_rows();
    assert!(total > 200 && total <= 253);
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    assert_eq!((g.letter, g.pitch, g.first, g.shown), (2, 4, 0, total));
    // Past 253 rows: one pixel a letter and two a row.
    let r = rig(&paragraph.repeat(100));
    let total = r.app.page().layout().total_rows();
    assert!(total > 253 && total <= 506);
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    assert_eq!((g.letter, g.pitch, g.first, g.shown), (1, 2, 0, total));
    // Past that it scrolls, keeping the cursor's row in view.
    let mut r = rig(&paragraph.repeat(400));
    let total = r.app.page().layout().total_rows();
    let g = ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap();
    assert_eq!((g.letter, g.shown, g.first), (1, 506, total - 506));
    r.press("ctrl+home");
    assert_eq!(ok_ui::map::geometry(r.app.grid(), r.app.page()).unwrap().first, 0);
    r.draw();
}

#[test]
fn on_one_flat_ground_the_map_has_no_strip() {
    let mut r = essay();
    assert!(r.app.set_look("sun"));
    let area = r.app.grid().map_area().unwrap();
    assert_eq!(r.pixel(area.x - 4, area.y - 4), Some(Rgb(255, 255, 255)));
    assert_eq!(r.pixel(area.x, area.y + 16 * 4), Some(Rgb(0, 0, 0)));
    assert_eq!(r.pixel(area.x, area.y + 3 * 4), Some(Rgb(89, 89, 89)));
    assert!(!r.app.set_look("no such look"));
    assert_eq!(r.app.look().name, "sun");
}
