//! What the attack found, each failure cut down to a few keys. All nine
//! were left failing by the attack and have since been mended, and the
//! flags that had the campaigns in `attack_model.rs` and `attack_sizes.rs`
//! pass over them (see `Known` in `attack_support`) have been taken out.
//!
//! Three of the tests named a row of the sheet that DESIGN.md does not:
//! they were written while a short text started at the top of the sheet,
//! and the design has its typing row on the platen. They now look for the
//! same thing on whichever row the platen puts it.

mod attack_support;
mod common;

use attack_support::on_cluster_boundary;
use common::{head, put, rig};
use ok_ui::harness::Rig;

/// The text columns of one row of the screen, read as text.
fn row_text(r: &mut Rig, row: i32) -> String {
    let text = r.app.grid().text;
    r.screen().span(text, row)
}

/// The row of the screen the cursor's line starts on.
fn caret_row(r: &Rig) -> i32 {
    (r.app.page().caret().row as i64 - r.app.view().top()) as i32
}

// ───────────────────────── the cursor inside a character ─────────────────────────

/// DESIGN.md: "every cell of text is a character that is in the file, so
/// arrow keys never lie." A heart typed from the emoji panel is two
/// characters, U+2764 and the selector U+FE0F that rides on it. Ctrl Left
/// and Ctrl Right stop between the two, where there is nothing to see and
/// the next letter typed takes the selector for itself.
#[test]
fn ctrl_left_and_right_never_stop_between_a_character_and_a_mark_on_it() {
    for text in ["I \u{2764}\u{FE0F} this", "the \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} family", "a\u{0301} . \u{0323}b"] {
        let mut r = rig(text);
        for _ in 0..12 {
            r.press("ctrl+left");
            let (_, col) = head(&r);
            assert!(on_cluster_boundary(text, col), "Ctrl Left in {text:?} stopped at byte {col}, inside a character");
        }
        for _ in 0..12 {
            r.press("ctrl+right");
            let (_, col) = head(&r);
            assert!(on_cluster_boundary(text, col), "Ctrl Right in {text:?} stopped at byte {col}, inside a character");
        }
    }
}

/// The same stop is where Ctrl Backspace cuts: it takes the selector and
/// leaves the heart, so the key seems to do nothing the first time.
#[test]
fn ctrl_backspace_takes_a_character_whole_never_only_the_mark_on_it() {
    let mut r = rig("ok \u{2764}\u{FE0F}");
    r.press("ctrl+backspace");
    assert!(!r.text().ends_with('\u{2764}'), "Ctrl Backspace left {:?}: the heart without its selector", r.text());
    // A letter with an accent typed as a separate mark, after a full stop.
    let mut r = rig("so.\u{0301}");
    r.press("ctrl+backspace");
    assert!(on_cluster_boundary("so.\u{0301}", r.text().len()), "Ctrl Backspace left {:?}: the mark went and its character stayed", r.text());
    // And forwards.
    let mut r = rig("\u{2764}\u{FE0F} ok");
    put(&mut r, 0, 0);
    r.press("ctrl+delete");
    assert!(!r.text().starts_with('\u{FE0F}'), "Ctrl Delete left {:?}: the selector without its heart", r.text());
}

/// Alt Right goes to the start of the next sentence. When a direction mark
/// follows the space between two sentences, as it does in Hebrew and
/// Arabic prose, the start it finds is between the space and its mark.
#[test]
fn alt_right_never_stops_between_a_space_and_a_mark_on_it() {
    let text = "\u{05E9}\u{05DC}\u{05D5}\u{05DD}. \u{200F}\u{05E9}\u{05DC}\u{05D5}\u{05DD}.";
    let mut r = rig(text);
    put(&mut r, 0, 0);
    r.press("alt+right");
    let (_, col) = head(&r);
    assert!(on_cluster_boundary(text, col), "Alt Right stopped at byte {col}, between the space and the mark on it");
}

// ───────────────────────── the letter under the cursor ─────────────────────────

/// Text copied from a web page often opens with a zero-width space. With
/// the cursor at the start of such a line the block is drawn holding only
/// that character, which has no picture, and the first letter of the line
/// is not on the screen at all.
#[test]
fn the_first_letter_of_a_line_is_still_drawn_when_the_cursor_is_on_a_mark_before_it() {
    let mut r = Rig::sized(1920, 1080);
    r.host.clipboard = Some(String::from("\u{200B}word"));
    r.press("ctrl+v");
    // A new text is typed on the platen row, wherever that is.
    let row = caret_row(&r);
    assert_eq!(row, r.app.grid().platen);
    assert_eq!(row_text(&mut r, row), "word");
    r.press("home");
    assert_eq!(head(&r), (0, 0));
    assert_eq!(row_text(&mut r, row), "word", "with the cursor at the start of the line the w is gone");
}

// ───────────────────────── a heading at the top of the sheet ─────────────────────────

/// Ctrl Home in a text that opens with a title lands on the title's `#`,
/// which hangs beside the lower half of the double-size row. When the paper
/// is only a row or two from the top it is rolled just far enough to show
/// that lower half, and a heading half off the sheet is not drawn: the
/// cursor sits on a `#` at the top edge with no title beside it.
#[test]
fn ctrl_home_shows_the_title_it_lands_on() {
    let body: String = (0..29).map(|n| format!("\nline {n}")).collect();
    let mut r = rig(&format!("# The orchard\n{body}"));
    assert_eq!(r.app.view().top(), 3, "the last row is at the platen, three rows of the text above the sheet");
    r.press("ctrl+home");
    assert_eq!(head(&r), (0, 0));
    // Where on the sheet the title lands is the platen's business (a jump
    // brings its row there); what matters here is that it is on the sheet,
    // whole, beside the cursor that is on its mark.
    let (grid_text, rows) = (r.app.grid().text, r.app.grid().text_rows);
    let shown: Vec<String> = (0..rows).map(|row| r.screen().big(grid_text, row)).collect();
    assert_eq!(shown.iter().position(|row| row == "The orchard"), Some(caret_row(&r) as usize - 1), "the sheet reads {shown:?} and the paper is at row {}", r.app.view().top());
    assert!(caret_row(&r) >= 1, "the cursor is on the lower half of the title, so the title's upper half is on the sheet");
}

/// The same from one row down with the arrow keys: Up, then Left onto the
/// mark.
#[test]
fn moving_onto_the_mark_of_a_title_keeps_the_title_on_the_sheet() {
    let body: String = (0..29).map(|n| format!("\nline {n}")).collect();
    let mut r = rig(&format!("# The orchard\n{body}"));
    r.press("ctrl+up");
    for _ in 0..40 {
        r.press("ctrl+up");
    }
    assert_eq!(head(&r), (0, 0));
    let (grid_text, rows) = (r.app.grid().text, r.app.grid().text_rows);
    let shown: Vec<String> = (0..rows).map(|row| r.screen().big(grid_text, row)).collect();
    assert_eq!(shown.iter().position(|row| row == "The orchard"), Some(caret_row(&r) as usize - 1), "the sheet reads {shown:?} and the paper is at row {}", r.app.view().top());
}

// ───────────────────────── Enter on an empty item ─────────────────────────

/// DESIGN.md: Enter "on an empty item, ends the list." With a selection
/// that starts on the empty item and reaches into the line below, Enter
/// takes the whole of that line, the part that was not selected included.
#[test]
fn enter_on_an_empty_item_takes_only_what_is_selected_from_the_line_below() {
    // Shift Right from the empty item selects the line break and nothing
    // else. The paragraph below is not selected and must still be there.
    let mut r = rig("- one\n- \nThe paragraph below.");
    put(&mut r, 1, 2);
    r.press("shift+right");
    assert_eq!(common::selected(&r), "\n");
    r.press("enter");
    assert!(r.text().contains("The paragraph below."), "Enter left {:?}: a paragraph that was not selected is gone", r.text());
}

/// The same with Shift Down, which is how a hand would select it: half of
/// the next line is selected, and all of it goes.
#[test]
fn enter_on_an_empty_item_keeps_the_rest_of_the_line_the_selection_ends_in() {
    let mut r = rig("- \nkept words");
    put(&mut r, 0, 2);
    r.press("shift+down");
    assert_eq!(common::selected(&r), "\nke");
    r.press("enter");
    assert!(r.text().ends_with("pt words"), "Enter left {:?}: \"pt words\" was not selected", r.text());
}

// ───────────────────────── Backspace after Enter ─────────────────────────

/// DESIGN.md, Backspace at the start of a paragraph: "Undoes what Enter
/// did: removes the blank line and the break together." Enter with the
/// cursor at the very start of a paragraph writes its two breaks above the
/// paragraph; the cursor is still at the start of that paragraph, and one
/// Backspace takes back only one of the two.
#[test]
fn backspace_undoes_an_enter_made_at_the_very_start_of_a_paragraph() {
    for text in ["First.\n\nSecond.", "Only."] {
        let line = text.split('\n').count() - 1;
        let mut r = rig(text);
        put(&mut r, line, 0);
        r.press("enter");
        assert_eq!(head(&r), (line + 2, 0));
        r.press("backspace");
        assert_eq!(r.text(), text, "Enter then Backspace at the start of the last paragraph of {text:?}");
        assert_eq!(head(&r), (line, 0));
    }
}
