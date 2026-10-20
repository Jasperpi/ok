//! DESIGN.md, "The line": every row of its table, read back from the
//! screen, with a writing folder that lives in memory behind it.

mod line_support;

use line_support::*;
use ok_host::Mods;
use ok_text::Pos;
use ok_ui::canvas::CursorBox;
use ok_ui::grid::Cols;
use ok_ui::look::Strength;

const TEXT: Cols = Cols { first: 47, last: 112 };

/// The orchard essay 120 lines down a text long enough for the paper to
/// move under it.
fn long_rig() -> (ok_ui::harness::Rig, Folder) {
    let long = format!("{}{ORCHARD}", "Nothing here.\n\n".repeat(60));
    rig_with(&[("Long", long.as_str())], "Long", 1920, 1080)
}

/// Columns and rows of the sheet that hold a cell in inverse video.
fn inverse_cells(rig: &mut ok_ui::harness::Rig) -> Vec<(i32, i32)> {
    let screen = rig.screen().clone();
    (0..45).flat_map(|row| (47..113).map(move |col| (col, row))).filter(|&(col, row)| screen.at(col, row).is_some_and(|cell| cell.inverse)).collect()
}

#[test]
fn esc_puts_the_cursor_after_ok_and_esc_again_returns() {
    let (mut rig, _) = rig();
    let on_page = rig.screen().cursor.unwrap();
    assert_eq!(said(&mut rig), "");
    rig.press("esc");
    // The solid cursor is after `ok` and a space; a hollow box keeps its
    // place on the page; the say row states what Enter will do.
    assert_eq!(rig.screen().cursor, Some(CursorBox { col: 50, row: 47, wide: 1, tall: 1, hollow: false }));
    assert_eq!(rig.screen().kept, Some(CursorBox { hollow: true, ..on_page }));
    assert_eq!(said(&mut rig), "Enter goes back to where you were.");
    assert!(said(&mut rig).starts_with("Enter "));
    // Nothing pops up: the contents are the contents.
    assert_eq!(contents(&mut rig).len(), 8);
    assert_eq!(bar(&mut rig), None);
    // Typing goes to the line and leaves the page alone.
    rig.type_text("orch");
    assert_eq!(typed(&mut rig), "orch");
    assert_eq!(rig.text(), ORCHARD);
    assert_eq!(rig.screen().cursor.map(|at| (at.col, at.row)), Some((54, 47)));
    rig.press("esc");
    assert_eq!(rig.screen().cursor, Some(on_page));
    assert_eq!(rig.screen().kept, None);
    assert_eq!((said(&mut rig), typed(&mut rig)), (String::new(), String::new()));
    // And it is the page that takes the next letter.
    rig.type_text("x");
    assert_eq!(rig.text(), format!("{ORCHARD}x"));
}

#[test]
fn the_line_is_edited_like_a_line() {
    let (mut rig, _) = rig();
    line_types(&mut rig, "orchrd");
    rig.press("left");
    rig.press("left");
    rig.type_text("a");
    assert_eq!(typed(&mut rig), "orchard");
    assert_eq!(rig.screen().cursor.map(|at| at.col), Some(55));
    rig.press("home");
    rig.press("delete");
    assert_eq!(typed(&mut rig), "rchard");
    rig.press("end");
    rig.press("backspace");
    assert_eq!(typed(&mut rig), "rchar");
    rig.type_text(" notes here");
    rig.press("ctrl+backspace");
    assert_eq!(typed(&mut rig), "rchar notes");
    rig.host.clipboard = Some(String::from("pasted\nsecond line"));
    rig.press("ctrl+v");
    assert_eq!(typed(&mut rig), "rchar notes pasted");
    for _ in 0..40 {
        rig.press("backspace");
    }
    assert_eq!(typed(&mut rig), "");
    assert_eq!(rig.text(), ORCHARD);
}

#[test]
fn left_empty_for_1200_ms_the_line_shows_what_it_understands() {
    let (mut rig, _) = rig();
    rig.press("esc");
    assert_eq!(rig.wants().wake_in_ms, Some(1200));
    rig.wait(1199);
    assert!(read(&mut rig, 47, 112, 47).starts_with("ok   "));
    rig.wait(1);
    // The default measure has 63 cells after `ok`; the list is 79, and is
    // cut between two of the things it lists.
    assert_eq!(read(&mut rig, 47, 112, 47), "ok a title   # heading   / phrase   rename   set   earlier");
    assert!((51..=105).filter_map(|col| rig.screen().at(col, 47)).filter(|cell| cell.ch != ' ').all(|cell| cell.strength == Strength::FAINT));
    assert_eq!(rig.wants().wake_in_ms, None);
    // The first character removes it.
    rig.type_text("x");
    assert_eq!(typed(&mut rig), "x");
    assert!(!read(&mut rig, 47, 112, 47).contains("heading"));
    // Left empty again, it waits again.
    rig.press("backspace");
    assert!(!read(&mut rig, 47, 112, 47).contains("heading"));
    rig.wait(1200);
    assert!(read(&mut rig, 47, 112, 47).contains("# heading"));
    rig.press("esc");
    // With room for it, the whole of what DESIGN.md gives.
    rig.app.set_measure(100);
    rig.press("esc");
    rig.wait(1200);
    let machine = rig.app.grid().machine.unwrap();
    let text = rig.app.grid().text;
    assert!(rig.screen().span(text, machine).starts_with("ok a title   # heading   / phrase   rename   set   earlier   discard   copy   keys  "));
}

#[test]
fn nothing_then_enter_goes_back_to_where_he_last_was() {
    let (mut rig, folder) = rig();
    // With nowhere to go back to, Enter is Esc.
    rig.press("esc");
    rig.press("enter");
    assert_eq!((rig.text(), said(&mut rig)), (String::from(ORCHARD), String::new()));
    assert_eq!(rig.screen().kept, None);
    // Open another text, and this one is where he last was.
    line_types(&mut rig, "grandfather");
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "College/Grandfather interview");
    assert_eq!(rig.text(), interview());
    rig.press("esc");
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    // Answered by the contents: the bar is on the row Enter would open.
    assert!(bar(&mut rig).unwrap().starts_with("The orchard ."));
    assert_eq!(contents(&mut rig).len(), 8);
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "College/The orchard");
    assert_eq!(rig.text(), ORCHARD);
    assert_eq!(bar(&mut rig), None);
    // On an empty line Up and Down choose among all the texts.
    rig.press("esc");
    assert!(bar(&mut rig).unwrap().starts_with("Grandfather interview ."));
    rig.press("down");
    assert!(bar(&mut rig).unwrap().starts_with("Orchard notes ."));
    assert_eq!(said(&mut rig), "Enter opens College\\Orchard notes.");
    rig.press("up");
    rig.press("up");
    assert_eq!(said(&mut rig), "Enter opens Essay plan.");
    rig.press("up");
    assert_eq!(said(&mut rig), "Enter opens Essay plan.", "the bar stops at the first text");
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "Essay plan");
}

#[test]
fn a_title_finds_a_text_by_its_words_in_any_order_and_enter_opens_it() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "orch");
    // The contents answer with what was found, as a contents page, the
    // bar on the best of it.
    let rows = contents(&mut rig);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], "College");
    assert!(rows[1].starts_with("  Orchard notes .") && rows[1].ends_with(". 7"));
    assert!(rows[2].starts_with("  The orchard .") && rows[2].ends_with(". 48"));
    assert!(bar(&mut rig).unwrap().starts_with("Orchard notes ."));
    assert_eq!(said(&mut rig), "Enter opens College\\Orchard notes.");
    // Up and Down choose.
    rig.press("down");
    assert!(bar(&mut rig).unwrap().starts_with("The orchard ."));
    assert_eq!(said(&mut rig), "Enter opens College\\The orchard.");
    rig.press("down");
    assert!(bar(&mut rig).unwrap().starts_with("The orchard ."), "the bar stops at the last row");
    rig.press("up");
    assert_eq!(said(&mut rig), "Enter opens College\\Orchard notes.");
    // Words in any order, any case.
    for _ in 0..4 {
        rig.press("backspace");
    }
    rig.type_text("NOTES orchard");
    assert_eq!(contents(&mut rig).len(), 2);
    assert_eq!(said(&mut rig), "Enter opens College\\Orchard notes.");
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "College/Orchard notes");
    assert_eq!(rig.text(), "Notes on thinning.\n\nTwo of every three.");
    assert_eq!((said(&mut rig), typed(&mut rig)), (String::new(), String::new()));
    assert_eq!(rig.screen().kept, None, "the cursor is on the page again");
    // The text he left was handed to the session as it stood.
    assert_eq!(folder.borrow().text("College/The orchard"), Some(ORCHARD));
}

#[test]
fn the_page_shows_the_text_under_the_bar_once_it_has_rested_250_ms() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "grand");
    assert_eq!(rig.wants().wake_in_ms, Some(250));
    rig.wait(249);
    assert_eq!(title(&mut rig), "The orchard", "this text is still on the page");
    rig.wait(1);
    assert_eq!(title(&mut rig), "Grandfather interview");
    assert_eq!(read(&mut rig, 47, 112, 4), "Recorded at the kitchen table. See [[The orchard]] for what came");
    // The facts name what is on the page, and count it.
    assert!(read(&mut rig, 47, 112, 47).ends_with("Grandfather interview    455 words"));
    assert_eq!(rig.wants().wake_in_ms, None);
    // It is shown, not opened: the text is still his, and so the session
    // has been told on every tick.
    assert_eq!(rig.app.text().doc().to_text("\n"), ORCHARD);
    assert_eq!(folder.borrow().open_name(), "College/The orchard");
    rig.wait(5000);
    rig.send(ok_host::Event::Tick);
    assert!(folder.borrow().handed.iter().all(|(_, text)| text == ORCHARD));
    assert!(folder.borrow().handed.iter().any(|(what, _)| *what == "tick"));
    // Page Down and Page Up read what is shown.
    rig.press("pagedown");
    assert!(read(&mut rig, 47, 112, 1).starts_with("Answer "));
    rig.press("pageup");
    assert_eq!(title(&mut rig), "Grandfather interview");
    // An emptied line has the text itself on the page again, at once.
    for _ in 0..5 {
        rig.press("backspace");
    }
    assert_eq!(title(&mut rig), "The orchard");
    // A bar that moves to the open text shows the open text again, once
    // it has rested there.
    rig.type_text("orchard");
    rig.wait(250);
    assert_eq!(read(&mut rig, 47, 112, 1), "Notes on thinning.");
    rig.press("down");
    assert_eq!(said(&mut rig), "Enter opens College\\The orchard.");
    assert_eq!(read(&mut rig, 47, 112, 1), "Notes on thinning.", "until the bar has rested");
    rig.wait(250);
    assert_eq!(title(&mut rig), "The orchard");
    assert!(read(&mut rig, 47, 112, 47).ends_with("The orchard    48 words"));
    rig.press("up");
    rig.wait(250);
    assert_eq!(read(&mut rig, 47, 112, 1), "Notes on thinning.");
}

#[test]
fn a_title_that_matches_nothing_starts_a_new_text() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "Rebuttal notes");
    assert_eq!(said(&mut rig), "Enter starts a new text called Rebuttal notes.");
    assert!(contents(&mut rig).is_empty(), "the contents show that nothing was found");
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "Rebuttal notes");
    assert_eq!(rig.text(), "");
    // A backslash makes or uses a folder.
    line_types(&mut rig, "Mock trial\\Rebuttal");
    assert_eq!(said(&mut rig), "Enter starts a new text called Mock trial\\Rebuttal.");
    rig.press("enter");
    assert_eq!(folder.borrow().open_name(), "Mock trial/Rebuttal");
    assert_eq!(folder.borrow().names().len(), 8);
    // What the session cannot do, it says, and the line waits.
    folder.borrow_mut().refuse = Some(String::from("? This folder cannot be written."));
    line_types(&mut rig, "One more");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? This folder cannot be written.");
    assert_eq!(typed(&mut rig), "One more");
    assert_eq!(folder.borrow().names().len(), 8);
}

#[test]
fn hash_and_letters_go_to_a_heading_and_the_page_follows() {
    let (mut rig, _) = rig();
    rig.press("ctrl+home");
    let before = picture(&mut rig);
    line_types(&mut rig, "#");
    // Answered by the labels: every heading, the bar on the section he is in.
    let cols = rig.app.grid().labels.unwrap();
    assert_eq!(labels(&mut rig), ["The orchard", "Thinning", "What he showed me"]);
    assert_eq!(bar_in(&mut rig, cols).as_deref(), Some("The orchard"));
    assert_eq!(said(&mut rig), "Enter goes to The orchard.");
    // The page follows as he types.
    rig.type_text("thin");
    assert_eq!(labels(&mut rig), ["Thinning"]);
    assert_eq!(bar_in(&mut rig, cols).as_deref(), Some("Thinning"));
    assert_eq!(said(&mut rig), "Enter goes to Thinning.");
    assert_eq!(head(&rig), (4, 0));
    let platen = rig.app.grid().platen;
    assert_eq!(rig.screen().kept.map(|at| at.row), Some(platen), "the hollow box is on the heading, held on the platen");
    // Up and Down choose, and the page follows them too.
    for _ in 0..4 {
        rig.press("backspace");
    }
    rig.press("down");
    assert_eq!((bar_in(&mut rig, cols).as_deref(), head(&rig)), (Some("Thinning"), (4, 0)));
    rig.press("down");
    assert_eq!((bar_in(&mut rig, cols).as_deref(), head(&rig)), (Some("What he showed me"), (8, 0)));
    assert_eq!(said(&mut rig), "Enter goes to What he showed me.");
    // Enter keeps: the cursor is there, on the page.
    rig.press("enter");
    assert_eq!(head(&rig), (8, 0));
    assert_eq!((said(&mut rig), rig.screen().kept), (String::new(), None));
    assert_eq!(labels(&mut rig)[0], "The orchard", "and the labels are the labels again");
    // Esc puts back instead.
    rig.press("ctrl+home");
    let before = picture(&mut rig);
    line_types(&mut rig, "#showed");
    assert_eq!(head(&rig), (8, 0));
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
    // A heading that is not there.
    line_types(&mut rig, "#zzz");
    assert_eq!(said(&mut rig), "Enter does nothing yet. No heading reads zzz.");
    assert!(labels(&mut rig).is_empty());
    assert_eq!(head(&rig), (0, 0), "the page is where it was");
    // In a text long enough for the paper to move, the heading comes to
    // the platen.
    let (mut rig, _) = long_rig();
    rig.press("ctrl+home");
    line_types(&mut rig, "#thin");
    assert_eq!(head(&rig), (124, 0));
    assert_eq!(rig.screen().kept.map(|at| at.row), Some(28));
    assert_eq!(read(&mut rig, 47, 112, 28), "Thinning");
    rig.press("enter");
    assert_eq!(rig.screen().cursor.map(|at| at.row), Some(28));
}

#[test]
fn hash_in_a_text_with_no_headings_goes_to_a_paragraph() {
    let (mut rig, _) = rig_with(&[("Notes", "First paragraph here.\n\nSecond one follows,\nand runs on.\n\n- Third is a list")], "Notes", 1920, 1080);
    line_types(&mut rig, "#");
    assert_eq!(labels(&mut rig), ["First paragraph here.", "Second one follows,", "Third is a list"]);
    assert_eq!(said(&mut rig), "Enter goes to Third is a list.", "the paragraph he is in");
    rig.type_text("sec");
    assert_eq!(labels(&mut rig), ["Second one follows,"]);
    assert_eq!(said(&mut rig), "Enter goes to Second one follows.");
    rig.press("enter");
    assert_eq!(head(&rig), (2, 0));
    line_types(&mut rig, "#nowhere");
    assert_eq!(said(&mut rig), "Enter does nothing yet. No paragraph starts nowhere.");
}

#[test]
fn slash_and_a_phrase_finds_it_with_the_hits_in_inverse_video() {
    let (mut rig, _) = rig();
    rig.press("ctrl+home");
    let before = picture(&mut rig);
    let quiet = inverse_cells(&mut rig).len();
    line_types(&mut rig, "/apricots");
    // Case is ignored: three hits, each eight cells of inverse video.
    assert_eq!(said(&mut rig), "1 of 3. Up and Down step through them. Enter stays here.");
    // The one the cursor is on has its first letter in the hollow box
    // instead, which is how it is told from the others.
    assert_eq!(inverse_cells(&mut rig).len(), quiet + 3 * 8 - 1);
    // The cursor has gone to the nearest after where it was.
    assert_eq!(head(&rig), (6, 32));
    let kept = rig.screen().kept.unwrap();
    assert_eq!((kept.col, kept.hollow), (47 + 32, true));
    assert!(!rig.screen().at(kept.col, kept.row).unwrap().inverse && rig.screen().at(kept.col + 1, kept.row).unwrap().inverse);
    // Up and Down step, going round.
    rig.press("down");
    assert_eq!(said(&mut rig), "2 of 3. Up and Down step through them. Enter stays here.");
    assert_eq!(head(&rig), (6, 67));
    rig.press("down");
    assert_eq!(head(&rig).0, 10);
    rig.press("down");
    assert_eq!((said(&mut rig).as_str(), head(&rig)), ("1 of 3. Up and Down step through them. Enter stays here.", (6, 32)));
    rig.press("up");
    assert!(said(&mut rig).starts_with("3 of 3."));
    // A capital makes the case count.
    for _ in 0..8 {
        rig.press("backspace");
    }
    rig.type_text("Apricots");
    assert_eq!(said(&mut rig), "1 of 1. Enter stays here.");
    assert_eq!(head(&rig), (10, 47));
    // Esc puts everything back; Enter stays there.
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
    line_types(&mut rig, "/Apricots");
    rig.press("enter");
    assert_eq!(head(&rig), (10, 47));
    assert_eq!(inverse_cells(&mut rig).len(), 3, "with the line closed only the cursor and the TK are inverse");
    // The nearest after the cursor, not the first in the text.
    rig.app.go(Pos::new(8, 0));
    line_types(&mut rig, "/apricots");
    assert!(said(&mut rig).starts_with("3 of 3."));
    rig.press("esc");
    // Nothing found.
    line_types(&mut rig, "/plums");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Nothing here reads plums.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? Nothing here reads plums.");
    assert_eq!(typed(&mut rig), "/plums", "nothing blocks: the line is as it was");
    // In a text long enough for the paper to move, the hit comes to the
    // platen, and a tick beside the map marks every row that holds one.
    let (mut rig, _) = long_rig();
    rig.press("ctrl+home");
    line_types(&mut rig, "/apricots");
    assert_eq!(head(&rig), (126, 32));
    assert_eq!(rig.screen().kept.map(|at| (at.col, at.row)), Some((47 + 32, 28)));
    let look = rig.app.look().clone();
    let geometry = ok_ui::map::geometry(rig.app.grid(), rig.app.page()).unwrap();
    let hit_row = rig.app.page().layout().locate(rig.app.page().doc(), Pos::new(130, 47)).0;
    let flag_x = geometry.area.x + 132 + 6;
    assert_eq!(rig.pixel(flag_x, geometry.y(hit_row).unwrap()), Some(look.desk_ink));
    assert_eq!(rig.pixel(flag_x, geometry.y(10).unwrap()), Some(look.desk));
    rig.press("enter");
    assert_eq!(rig.screen().cursor.map(|at| at.row), Some(28));
    assert_eq!(rig.pixel(flag_x, geometry.y(hit_row).unwrap()), Some(look.desk), "the flags go when the find is closed");
}

#[test]
fn f3_and_shift_f3_step_through_the_hits_afterwards() {
    let (mut rig, _) = rig();
    rig.press("f3");
    assert_eq!(said(&mut rig), "Nothing has been looked for yet. Ctrl F finds a phrase.");
    rig.press("ctrl+home");
    line_types(&mut rig, "/apricots");
    rig.press("enter");
    assert_eq!(head(&rig), (6, 32));
    rig.press("f3");
    assert_eq!((head(&rig), said(&mut rig).as_str()), ((6, 67), "2 of 3. F3 and Shift F3 step through them."));
    rig.press("f3");
    assert_eq!(head(&rig), (10, 47));
    rig.press("f3");
    assert_eq!(head(&rig), (6, 32), "round to the first");
    rig.press("shift+f3");
    assert_eq!((head(&rig), said(&mut rig).as_str()), ((10, 47), "3 of 3. F3 and Shift F3 step through them."));
    // While the line is open they are Up and Down.
    rig.press("ctrl+home");
    rig.press("ctrl+f");
    rig.type_text("apricots");
    rig.press("f3");
    assert!(said(&mut rig).starts_with("2 of 3."));
    rig.press("shift+f3");
    assert!(said(&mut rig).starts_with("1 of 3."));
}

#[test]
fn ctrl_f_opens_the_line_with_a_slash_typed() {
    let (mut rig, _) = rig();
    rig.press("ctrl+f");
    assert_eq!(typed(&mut rig), "/");
    assert_eq!(rig.screen().cursor.map(|at| (at.col, at.row)), Some((51, 47)));
    assert_eq!(said(&mut rig), "Enter does nothing yet. Type the phrase to find after the slash.");
    rig.type_text("weight");
    assert_eq!(said(&mut rig), "1 of 1. Enter stays here.");
    // And from the line itself it starts the phrase again.
    rig.press("ctrl+f");
    assert_eq!(typed(&mut rig), "/");
}

#[test]
fn slash_old_new_changes_this_one_and_moves_on() {
    let (mut rig, folder) = rig();
    rig.press("ctrl+home");
    line_types(&mut rig, "/apricots/plums");
    assert_eq!(said(&mut rig), "1 of 3. Enter changes this one to plums and moves on.");
    rig.press("enter");
    assert!(rig.text().contains("every three plums, TK") && rig.text().contains("The apricots that stay"));
    assert_eq!(said(&mut rig), "1 of 2. Enter changes this one to plums and moves on.");
    assert_eq!(head(&rig), (6, 64), "on to the next");
    // Down passes one over.
    rig.press("down");
    assert!(said(&mut rig).starts_with("2 of 2."));
    rig.press("enter");
    assert!(rig.text().contains("weight of the plums he kept") && rig.text().contains("The apricots that stay"));
    assert_eq!(said(&mut rig), "1 of 1. Enter changes this one to plums and moves on.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "Changed 3. That was the last one.");
    assert_eq!(typed(&mut rig), "");
    let changed = ORCHARD.replace("apricots", "plums").replace("Apricots", "plums");
    assert_eq!(rig.text(), changed);
    assert_eq!(folder.borrow().text("College/The orchard"), Some(changed.as_str()));
    // Each was a step of its own.
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    assert_eq!(rig.text().matches("plums").count(), 1);
    rig.press("ctrl+z");
    assert_eq!(rig.text(), ORCHARD);
    // An empty new takes the phrase out; Ctrl Z works from the line too.
    line_types(&mut rig, "/ TK how many that is/");
    assert_eq!(said(&mut rig), "1 of 1. Enter takes this one out and moves on.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "Changed 1. That was the only one.");
    assert!(rig.text().contains("three apricots,. The"));
    rig.press("ctrl+z");
    line_types(&mut rig, "/apricots/plums");
    rig.press("enter");
    assert_eq!(rig.text().matches("plums").count(), 1);
    rig.press("ctrl+z");
    assert_eq!(rig.text(), ORCHARD);
    assert!(said(&mut rig).ends_with("of 3. Enter changes this one to plums and moves on."));
    // Only g may follow the last slash.
    rig.type_text("/x");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Only g may follow the last slash.");
}

#[test]
fn slash_old_new_g_changes_all_and_one_ctrl_z_takes_them_all_back() {
    let (mut rig, folder) = rig();
    let at = head(&rig);
    line_types(&mut rig, "/apricots/plums/g");
    assert_eq!(said(&mut rig), "Enter changes all 3 to plums. Ctrl Z takes them all back.");
    rig.press("enter");
    let changed = ORCHARD.replace("apricots", "plums").replace("Apricots", "plums");
    assert_eq!(rig.text(), changed);
    assert_eq!(said(&mut rig), "Changed 3. Ctrl Z takes them all back.");
    assert_eq!(folder.borrow().text("College/The orchard"), Some(changed.as_str()));
    // The cursor kept its place at the end of the text, three letters in.
    assert_eq!(head(&rig), (at.0, at.1 - 3));
    assert_eq!(rig.screen().kept, None);
    // Only the rows that held a hit are marked as written today.
    let barred: Vec<i32> = (0..45).filter(|&row| rig.screen().bar(row)).collect();
    let rows: Vec<usize> = [6, 10].iter().flat_map(|&line| { let first = rig.app.page().layout().first_row(line); first..first + rig.app.page().layout().rows(line).len() }).collect();
    let top = rig.app.view().top();
    assert_eq!(barred, rows.iter().map(|&row| (row as i64 - top) as i32).collect::<Vec<_>>());
    // One Ctrl Z takes them all back, and Ctrl Y makes them again.
    rig.press("ctrl+z");
    assert_eq!(rig.text(), ORCHARD);
    assert_eq!(head(&rig), at);
    assert_eq!(folder.borrow().text("College/The orchard"), Some(ORCHARD));
    rig.press("ctrl+y");
    assert_eq!(rig.text(), changed);
    rig.press("ctrl+z");
    // One alone.
    line_types(&mut rig, "/weight/load/g");
    assert_eq!(said(&mut rig), "Enter changes the only one to load. Ctrl Z takes it back.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "Changed 1. Ctrl Z takes it back.");
    // And a text that takes no changes says so.
    let (mut rig, _) = rig_with(&[("Notes", "one two one")], "Notes", 1920, 1080);
    rig.app.page_mut().read_only = true;
    line_types(&mut rig, "/one/1/g");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? This text is open for reading only.");
    assert_eq!(rig.text(), "one two one");
}

#[test]
fn rename_renames_the_text_and_mends_every_link_to_it() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "rename");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Type the new title after rename.");
    // Answered by the contents: the bar on this text's row.
    assert!(bar(&mut rig).unwrap().starts_with("The orchard ."));
    rig.type_text(" The trees");
    assert_eq!(said(&mut rig), "Enter renames this text and mends 3 links to it.");
    assert!(bar(&mut rig).unwrap().starts_with("The trees ."), "the row reads as it will");
    assert_eq!(contents(&mut rig).len(), 8);
    rig.press("enter");
    assert_eq!(said(&mut rig), "Renamed to The trees and mended 3 links.");
    assert_eq!(folder.borrow().open_name(), "College/The trees");
    assert_eq!(folder.borrow().text("Essay plan"), Some("Plan.\n\n- Open with [[The trees]]\n- Then [[The trees|the trees]] again\n- End on [[Grandfather interview]]"));
    assert!(folder.borrow().text("College/Grandfather interview").unwrap().contains("See [[The trees]] for"));
    assert!(read(&mut rig, 47, 112, 47).ends_with("The trees    48 words"));
    assert_eq!(rig.text(), ORCHARD);
    // A name that is taken is refused by the session, in its own words.
    line_types(&mut rig, "rename Orchard notes");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? There is a text called Orchard notes already.");
    assert_eq!(typed(&mut rig), "rename Orchard notes");
    rig.press("esc");
    // A text nothing links to.
    line_types(&mut rig, "grand");
    rig.press("enter");
    line_types(&mut rig, "rename Interview");
    assert_eq!(said(&mut rig), "Enter renames this text and mends 1 link to it.");
    rig.press("esc");
    line_types(&mut rig, "closing");
    rig.press("enter");
    line_types(&mut rig, "rename Close");
    assert_eq!(said(&mut rig), "Enter renames this text.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "Renamed to Close.");
}

#[test]
fn set_answers_with_every_option_as_a_table_in_the_contents() {
    let (mut rig, _) = rig();
    line_types(&mut rig, "set");
    let rows = contents(&mut rig);
    let names: Vec<&str> = rows.iter().map(|row| row.split(' ').next().unwrap()).collect();
    assert_eq!(names, ["look", "face", "scale", "paper", "ink", "desk", "desk-ink", "mark", "half", "faint", "measure", "platen", "scroll", "cursor", "focus", "echo", "enter", "limit", "window", "folder"]);
    assert!(rows[0].ends_with(". manila") && rows[10].ends_with(". 66") && rows[3].ends_with(". EEE3C8") && rows[19].ends_with(". jaspe\\Documents\\ok"));
    assert!(bar(&mut rig).unwrap().starts_with("look ."));
    assert_eq!(said(&mut rig), "Enter changes look. It is manila now.");
}

#[test]
fn earlier_shows_this_text_as_it_was_and_enter_brings_one_back() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "earlier");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Nothing earlier is kept of this text.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? Nothing earlier is kept of this text.");
    rig.press("esc");
    folder.borrow_mut().versions = vec![(String::from("Today 13:40, kept"), String::from("The second draft.")), (String::from("Yesterday 22:10"), String::from("# First\n\nThe first draft of it."))];
    let before = picture(&mut rig);
    line_types(&mut rig, "earlier");
    // Versions newest first, in the contents.
    let rows = contents(&mut rig);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].starts_with("Today 13:40, kept .") && rows[0].ends_with(". 3"));
    assert!(rows[1].starts_with("Yesterday 22:10 .") && rows[1].ends_with(". 6"));
    assert!(bar(&mut rig).unwrap().starts_with("Today 13:40, kept"));
    assert_eq!(said(&mut rig), "Enter brings back Today 13:40, kept. The present one is kept too.");
    // The page shows the one under the bar once it has rested.
    rig.wait(250);
    assert_eq!(read(&mut rig, 47, 112, 1), "The second draft.");
    rig.press("down");
    assert_eq!(said(&mut rig), "Enter brings back Yesterday 22:10. The present one is kept too.");
    rig.wait(250);
    assert_eq!(title(&mut rig), "First");
    assert!(read(&mut rig, 47, 112, 47).ends_with("Yesterday 22:10    6 words"));
    // Esc puts this text back.
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
    // Enter brings one back, and the present one is kept too.
    line_types(&mut rig, "earlier");
    rig.press("down");
    rig.press("enter");
    assert_eq!(rig.text(), "# First\n\nThe first draft of it.");
    assert_eq!(said(&mut rig), "Brought back Yesterday 22:10. The present one is kept too.");
    assert_eq!(folder.borrow().versions.len(), 3);
    assert_eq!(folder.borrow().versions[0].1, ORCHARD);
    assert_eq!(folder.borrow().text("College/The orchard"), Some("# First\n\nThe first draft of it."));
}

#[test]
fn discard_moves_this_text_out_of_the_folder() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "discard");
    assert_eq!(said(&mut rig), "Enter moves this text to .discarded. Nothing is erased.");
    assert_eq!(folder.borrow().names().len(), 6, "nothing has happened yet");
    rig.press("enter");
    assert_eq!(said(&mut rig), "Moved to .discarded. Nothing was erased.");
    assert_eq!(folder.borrow().discarded, [(String::from("College/The orchard"), String::from(ORCHARD))]);
    assert_eq!(folder.borrow().names().len(), 5);
    // The session says what is on the page in its place.
    assert_eq!(folder.borrow().open_name(), "Mock trial/Closing argument");
    assert_eq!(rig.text(), "# Closing\n\nMembers of the jury.");
    assert_eq!(rig.screen().kept, None);
}

#[test]
fn copy_puts_the_whole_text_on_the_clipboard_with_the_marks_removed() {
    let (mut rig, _) = rig_with(&[("Essay", "# The orchard\n\nThe *orchard* was **eleven** acres\nwhen he bought it.\n\n- one\n- two")], "Essay", 1920, 1080);
    line_types(&mut rig, "copy");
    assert_eq!(said(&mut rig), "Enter copies 13 words with the marks removed.");
    assert_eq!(rig.host.clipboard, None);
    rig.press("enter");
    assert_eq!(said(&mut rig), "Copied 13 words with the marks removed.");
    // Each paragraph one line, line breaks as Windows writes them.
    assert_eq!(rig.host.clipboard.as_deref(), Some("The orchard\r\n\r\nThe orchard was eleven acres when he bought it.\r\n\r\none\r\ntwo"));
    assert_eq!(rig.text(), "# The orchard\n\nThe *orchard* was **eleven** acres\nwhen he bought it.\n\n- one\n- two", "the text itself is untouched");
    assert_eq!(rig.screen().kept, None);
}

#[test]
fn keys_puts_the_keys_on_the_page_for_reading() {
    let (mut rig, folder) = rig();
    let before = picture(&mut rig);
    line_types(&mut rig, "keys");
    assert_eq!(said(&mut rig), "Enter shows the keys on the page.");
    rig.press("enter");
    assert_eq!(title(&mut rig), "Keys");
    assert_eq!(read(&mut rig, 47, 112, 10), "plain    letter        row                   letter");
    assert_eq!(rig.screen().kept, None, "there is no place to keep in a text that is only shown");
    // The cursor waits on the empty line, and Enter and Esc both go back.
    assert_eq!(rig.screen().cursor.map(|at| (at.col, at.row)), Some((50, 47)));
    assert_eq!(typed(&mut rig), "");
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    assert!(read(&mut rig, 47, 112, 47).contains("  Keys    ") && read(&mut rig, 47, 112, 47).ends_with(" words"));
    assert!(!read(&mut rig, 47, 112, 47).contains("The orchard"));
    // It is read with the arrows and by the screen, and takes no typing.
    rig.press("down");
    rig.press("down");
    assert_eq!(read(&mut rig, 47, 112, 8), "plain    letter        row                   letter");
    rig.press("pagedown");
    assert!(rig.app.view().top() > 30);
    rig.press("home");
    assert_eq!(title(&mut rig), "Keys");
    rig.press("end");
    let foot = rig.app.grid().platen;
    assert_eq!(read(&mut rig, 47, 112, foot), "quit                    Leaves.");
    rig.wait(60_000);
    assert!(folder.borrow().handed.iter().all(|(_, text)| text == ORCHARD), "the session never sees it");
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
    // F1 is the same, from the page, and F1 again goes back.
    rig.press("f1");
    assert_eq!(title(&mut rig), "Keys");
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    rig.press("f1");
    assert_eq!(picture(&mut rig), before);
    rig.press("f1");
    rig.press("enter");
    assert_eq!(picture(&mut rig), before);
    // Typing on the line under it takes the text back and carries on.
    rig.press("f1");
    rig.type_text("#thin");
    assert_eq!(head(&rig), (4, 0));
    let platen = rig.app.grid().platen;
    assert_eq!(read(&mut rig, 47, 112, platen), "Thinning");
}

#[test]
fn notices_puts_the_licences_on_the_page_without_markup() {
    let (mut rig, _) = rig();
    let before = picture(&mut rig);
    line_types(&mut rig, "notices");
    assert_eq!(said(&mut rig), "Enter shows the licences of the faces and code in the .exe.");
    rig.press("enter");
    assert_eq!(read(&mut rig, 47, 112, 0), "");
    assert_eq!(read(&mut rig, 47, 112, 1), "ok is a writing tool for Windows: one window, one .exe, and a page");
    assert!(read(&mut rig, 47, 112, 47).ends_with(" words") && read(&mut rig, 47, 112, 47).contains("Notices    "));
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    // Its sections are listed where the labels are.
    let sections = labels(&mut rig);
    assert_eq!(sections[..6], ["Gallant", "  from Jens Schweikhardt", "  from the FreeBSD", "  from the University of", "Spleen", "Terminus"]);
    assert_eq!(sections.len(), 21);
    // Nothing in it is read as markup, on any screen of it: no double
    // size, no double strike, no underscore, no inverse video, nothing in
    // the gutter. The line that starts with a # is shown as typed.
    let mut seen = String::new();
    for _ in 0..40 {
        let screen = rig.screen().clone();
        for row in 0..45 {
            for col in 41..=118 {
                let cell = screen.at(col, row).unwrap();
                assert!(!cell.big && !cell.double && !cell.under && !cell.inverse, "row {row} column {col}: {}", screen.span(TEXT, row));
                assert!(col >= 47 || cell.ch == ' ', "something hangs beside row {row}");
            }
            seen.push_str(&screen.span(TEXT, row));
            seen.push('\n');
        }
        rig.press("pagedown");
    }
    assert!(seen.contains("\n# @(#)COPYRIGHT 8.2 (Berkeley) 3/21/94\n"));
    assert!(seen.contains("1. Redistributions of source code must retain the above copyright"));
    assert!(seen.contains("GCC runtime library"));
    assert!(!seen.contains("```"), "the fence that keeps the markup out is never on the sheet");
    // It cannot be scrolled above its first line, by key or by wheel.
    rig.press("home");
    rig.press("up");
    rig.press("pageup");
    rig.wheel(30);
    assert_eq!(read(&mut rig, 47, 112, 0), "");
    assert_eq!(read(&mut rig, 47, 112, 1), "ok is a writing tool for Windows: one window, one .exe, and a page");
    rig.size(1280, 720);
    rig.size(1920, 1080);
    assert_eq!(read(&mut rig, 47, 112, 1), "ok is a writing tool for Windows: one window, one .exe, and a page");
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
}

#[test]
fn quit_leaves_with_everything_handed_to_the_session() {
    let (mut rig, folder) = rig();
    rig.type_text(" More.");
    line_types(&mut rig, "quit");
    assert_eq!(said(&mut rig), "Enter leaves. Nothing can be lost.");
    assert!(!rig.wants().quit);
    rig.press("enter");
    assert!(rig.wants().quit);
    assert_eq!(folder.borrow().handed.last().cloned(), Some(("close", format!("{ORCHARD} More."))));
    // The chords that leave work from the line too.
    let (mut rig, _) = line_support::rig();
    line_types(&mut rig, "half a tit");
    rig.press("ctrl+q");
    assert!(rig.wants().quit);
}

#[test]
fn a_mistake_gets_a_question_mark_and_one_sentence_and_the_next_key_clears_it() {
    let (mut rig, _) = rig();
    line_types(&mut rig, "set mesure 70");
    assert_eq!(said(&mut rig), "Enter does nothing yet. No option is called mesure.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? No option called mesure. The nearest is measure.");
    // Nothing blocks: the line is as it was, and the next key clears it.
    assert_eq!(typed(&mut rig), "set mesure 70");
    rig.press("left");
    assert_eq!(said(&mut rig), "Enter does nothing yet. No option is called mesure.");
    rig.press("enter");
    rig.type_text("0");
    assert!(!said(&mut rig).starts_with('?'));
    rig.press("esc");
    // The same while the option is still being named.
    line_types(&mut rig, "set platten");
    assert_eq!(said(&mut rig), "Enter does nothing yet. No option is called platten.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? No option called platten. The nearest is platen.");
}

#[test]
fn typing_two_brackets_wakes_the_contents_and_tab_takes_the_offer() {
    let (mut rig, _) = rig();
    rig.type_text(" See [");
    assert_eq!(contents(&mut rig).len(), 8);
    assert_eq!(bar(&mut rig), None);
    // The second bracket wakes the contents: every text, the first offered.
    rig.type_text("[");
    assert!(bar(&mut rig).unwrap().starts_with("Essay plan ."));
    let row = rig.screen().cursor.unwrap().row;
    assert!(read(&mut rig, 47, 112, row).ends_with("See [[ Essay plan"));
    // Filtered to the letters typed since, the rest of the best match
    // after the cursor at half strength.
    rig.type_text("gr");
    let rows = contents(&mut rig);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].starts_with("  Grandfather interview ."));
    assert!(bar(&mut rig).unwrap().starts_with("Grandfather interview ."));
    assert!(read(&mut rig, 47, 112, row).ends_with("See [[gr andfather interview"));
    let cursor = rig.screen().cursor.unwrap().col;
    let offered: Vec<Strength> = (cursor + 1..cursor + 20).filter_map(|col| rig.screen().at(col, row)).filter(|cell| cell.ch != ' ').map(|cell| cell.strength).collect();
    assert_eq!(offered, [Strength::HALF; 18]);
    assert!(rig.text().ends_with("See [[gr"), "the offer is not in the text");
    // Tab takes it and closes the brackets.
    rig.press("tab");
    assert!(rig.text().ends_with("See [[Grandfather interview]]"));
    assert_eq!(contents(&mut rig).len(), 8);
    assert_eq!(bar(&mut rig), None);
    // Ctrl Z takes it back like any other edit.
    rig.press("ctrl+z");
    assert!(rig.text().ends_with("See [[gr"));
    assert_eq!(bar(&mut rig), None, "and the contents stay asleep until he types there again");
    // Letters from the middle of a name are answered with the whole name.
    rig.press("backspace");
    rig.press("backspace");
    rig.type_text("interv");
    assert!(read(&mut rig, 47, 112, row).ends_with("See [[interv  Grandfather interview"));
    rig.press("tab");
    assert!(rig.text().ends_with("See [[Grandfather interview]]"));
    // Typing the closing brackets himself keeps exactly what he typed.
    rig.type_text(" and [[Not yet a text");
    assert!(contents(&mut rig).len() == 8 && bar(&mut rig).is_none(), "nothing matches, so nothing is offered");
    rig.type_text("]]");
    rig.press("tab");
    assert!(rig.text().ends_with("and [[Not yet a text]]"));
    // Going somewhere else puts the contents back.
    rig.type_text(" [[orch");
    assert!(bar(&mut rig).is_some());
    rig.press("backspace");
    assert!(bar(&mut rig).is_some(), "the offer follows a letter taken back");
    rig.press("ctrl+home");
    assert_eq!((bar(&mut rig), contents(&mut rig).len()), (None, 8));
    // And Esc takes the cursor to the line as ever.
    rig.press("ctrl+end");
    rig.type_text("[[");
    rig.press("esc");
    assert_eq!(bar(&mut rig), None);
    assert_eq!(said(&mut rig), "Enter goes back to where you were.");
}

#[test]
fn holding_ctrl_or_alt_for_500_ms_prints_its_row_of_the_table() {
    let (mut rig, _) = rig();
    rig.hold(Mods::CTRL);
    rig.wait(499);
    assert_eq!(said(&mut rig), "");
    rig.wait(1);
    assert_eq!(said(&mut rig), "Ctrl moves by word and by paragraph, and deletes by word.");
    rig.wait(5000);
    assert_eq!(said(&mut rig), "Ctrl moves by word and by paragraph, and deletes by word.", "until he lets go");
    rig.release();
    assert_eq!(said(&mut rig), "");
    rig.hold(Mods::ALT);
    rig.wait(500);
    assert_eq!(said(&mut rig), "Alt moves and deletes by sentence, and carries the paragraph.");
    rig.release();
    // A chord is not a question.
    rig.press("ctrl+left");
    rig.hold(Mods::CTRL);
    rig.press("left");
    rig.wait(600);
    assert_eq!(said(&mut rig), "");
    rig.release();
    // Both at once are AltGr, and Shift is not asked about.
    rig.hold(Mods { ctrl: true, alt: true, shift: false });
    rig.wait(600);
    assert_eq!(said(&mut rig), "");
    rig.release();
}

#[test]
fn enter_on_the_text_that_is_open_goes_back_to_it_with_all_it_can_undo() {
    let (mut rig, folder) = rig();
    rig.type_text(" More.");
    rig.press("ctrl+home");
    line_types(&mut rig, "the orchard");
    assert_eq!(said(&mut rig), "Enter opens College\\The orchard.");
    rig.press("enter");
    // Nothing was opened afresh: the cursor is where it was, and Ctrl Z
    // still takes back what was typed before.
    assert_eq!(head(&rig), (0, 0));
    assert_eq!(rig.screen().kept, None);
    assert_eq!(folder.borrow().previous, None);
    assert_eq!(rig.text(), format!("{ORCHARD} More."));
    rig.press("ctrl+z");
    rig.press("ctrl+z");
    assert_eq!(rig.text(), ORCHARD);
}
