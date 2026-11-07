//! Blocks on the sheet, from the line and with the mouse: plain words that
//! box, size and set things side by side, frames dragged by their edges on
//! a board and in the flow, and every one of them one step for Ctrl Z.

use ok_host::{Event, MouseKind, Mods};
use ok_ui::harness::Rig;

fn rig(text: &str) -> Rig {
    let mut rig = Rig::with_text(text, 1600, 900);
    rig.command("platen off").unwrap();
    rig
}

fn on_line(rig: &mut Rig, typed: &str) {
    rig.press("esc");
    rig.type_text(typed);
}

/// Presses at one cell, moves to another, and lets go there.
fn drag(rig: &mut Rig, from: (i32, i32), to: (i32, i32)) {
    let (x, y) = rig.cell_centre(from.0, from.1);
    rig.send(Event::Mouse { kind: MouseKind::Down, x, y, mods: Mods::NONE });
    let (x, y) = rig.cell_centre(to.0, to.1);
    rig.send(Event::Mouse { kind: MouseKind::Move, x, y, mods: Mods::NONE });
    rig.send(Event::Mouse { kind: MouseKind::Up, x, y, mods: Mods::NONE });
}

/// Where the screen shows `needle`: its first cell.
fn find(rig: &mut Rig, needle: &str) -> (i32, i32) {
    rig.screen().find(needle).unwrap_or_else(|| panic!("{needle:?} is not on the screen:\n{}", rig.screen().lines().join("\n")))
}

#[test]
fn words_on_the_line_say_what_they_will_do_and_enter_does_it() {
    let mut rig = rig("one\n\ntwo");
    rig.command("cursor 0 1").unwrap();
    on_line(&mut rig, "put this in a box called Plan");
    find(&mut rig, "Enter puts this paragraph in a box called Plan.");
    assert_eq!(rig.text(), "one\n\ntwo", "nothing changes before Enter");
    rig.press("enter");
    assert_eq!(rig.text(), "::: box Plan\none\n:::\n\ntwo");
    find(&mut rig, "::: box Plan");
    // The cursor is still on the letter it was on.
    rig.type_text("X");
    assert_eq!(rig.text(), "::: box Plan\noXne\n:::\n\ntwo");
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    assert_eq!(rig.text(), "one\n\ntwo", "the box is one step");
}

#[test]
fn a_title_is_still_a_title() {
    let mut rig = rig("words");
    on_line(&mut rig, "Grandfather interview");
    assert!(rig.screen().find("Enter puts").is_none());
    rig.press("esc");
    assert_eq!(rig.text(), "words");
}

#[test]
fn what_cannot_be_done_is_said_and_nothing_changes() {
    let mut rig = rig("alone");
    rig.command("cursor 0 0").unwrap();
    on_line(&mut rig, "side by side");
    find(&mut rig, "? There is nothing after this to set beside it.");
    rig.press("enter");
    assert_eq!(rig.text(), "alone");
}

#[test]
fn a_note_on_a_board_is_dragged_by_its_edge() {
    let text = "top\n:::: board\n::: Note at 4,2\nwords on it\n:::\n::::\nfoot";
    let mut rig = rig(text);
    rig.command("cursor 0 0").unwrap();
    let (col, row) = find(&mut rig, "::: Note at 4,2");
    // The fence's words sit two cells in from the frame's left edge.
    let edge = (col - 2, row + 1);
    drag(&mut rig, edge, (edge.0 + 5, edge.1 + 3));
    assert_eq!(rig.text(), text.replace("at 4,2", "at 9,5"));
    // The cursor did not move into the note.
    rig.type_text("Y");
    assert!(rig.text().starts_with("Ytop"));
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    assert_eq!(rig.text(), text);
}

#[test]
fn a_frame_is_made_wider_by_its_right_edge() {
    let text = ":::: board\n::: Note at 0,0\nwords\n:::\n::::";
    let mut rig = rig(text);
    let (col, row) = find(&mut rig, "::: Note at 0,0");
    // A note is 24 cells wide when its fence does not say.
    let right = (col - 2 + 23, row + 1);
    drag(&mut rig, right, (right.0 + 6, right.1));
    assert_eq!(rig.text(), ":::: board\n::: Note at 0,0 width 30\nwords\n:::\n::::");
}

#[test]
fn a_box_in_the_flow_is_dragged_past_the_parts_below() {
    let text = "::: box A\na\n:::\n\nmiddle\n\n::: box B\nb\n:::\n\nend";
    let mut rig = rig(text);
    let (col, row) = find(&mut rig, "::: box A");
    let (_, below) = find(&mut rig, "::: box B");
    drag(&mut rig, (col - 2, row + 1), (col - 2, below + 2));
    assert_eq!(rig.text(), "middle\n\n::: box B\nb\n:::\n\n::: box A\na\n:::\n\nend");
}

#[test]
fn a_click_on_a_fence_s_words_is_a_click_in_text() {
    let text = "::: box Plan\nx\n:::";
    let mut rig = rig(text);
    let (col, row) = find(&mut rig, "::: box Plan");
    rig.click_cell(col + 4, row);
    rig.type_text("Z");
    assert_eq!(rig.text(), "::: Zbox Plan\nx\n:::");
}

#[test]
fn up_and_down_stay_in_a_column() {
    let text = "::: columns\nleft one\nleft two\n\nright one\nright two\n:::";
    let mut rig = rig(text);
    rig.command("cursor 4 2").unwrap();
    rig.press("down");
    rig.type_text("!");
    assert_eq!(rig.text(), "::: columns\nleft one\nleft two\n\nright one\nri!ght two\n:::");
}

#[test]
fn small_type_is_made_from_the_line_and_typed_in_like_any_line() {
    let mut rig = rig("a note in small type that goes on\n\nnext");
    rig.command("cursor 0 2").unwrap();
    on_line(&mut rig, "make it small");
    find(&mut rig, "Enter sets this paragraph in small type.");
    rig.press("enter");
    assert_eq!(rig.text(), "::: small\na note in small type that goes on\n:::\n\nnext");
    // The cursor kept its letter, and moves and types a letter at a time.
    rig.press("right");
    rig.type_text("X");
    assert_eq!(rig.text(), "::: small\na nXote in small type that goes on\n:::\n\nnext");
    let cursor = rig.screen().cursor.expect("a cursor");
    assert_eq!(cursor.wide, 1);
    // Down leaves the small line for the fence under it.
    rig.press("down");
    rig.type_text("Y");
    assert!(rig.text().contains("\n:::Y\n") || rig.text().contains("\nY:::\n") || rig.text().contains("\n::Y:\n") || rig.text().contains("\n:Y::\n"), "{}", rig.text());
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    assert_eq!(rig.text(), "a note in small type that goes on\n\nnext");
}

#[test]
fn smaller_steps_down_through_the_sizes_to_small() {
    let mut rig = rig("::: big\nwords\n:::");
    rig.command("cursor 1 0").unwrap();
    on_line(&mut rig, "smaller");
    rig.press("enter");
    assert_eq!(rig.text(), "words");
    on_line(&mut rig, "smaller");
    rig.press("enter");
    assert_eq!(rig.text(), "::: small\nwords\n:::");
    on_line(&mut rig, "smaller");
    find(&mut rig, "? Small is the smallest size.");
}
