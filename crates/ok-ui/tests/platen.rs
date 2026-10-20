//! The platen and everything else that moves the paper: typing, the roll,
//! the arrow keys, the page keys, the wheel and the mouse.

mod common;

use common::{head, put, read, rig, selected};
use ok_gfx::Rgb;
use ok_host::{Event, Mods, MouseKind};
use ok_ui::harness::Rig;
use ok_ui::look::Look;

const PLATEN: i32 = 28;

/// The row of the screen the cursor is drawn on.
fn cursor_row(r: &mut Rig) -> i32 {
    r.screen().cursor.expect("a cursor on the sheet").row
}

/// A text of `n` short lines, each its own row.
fn lines(n: usize) -> String {
    (0..n).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n")
}

/// DESIGN.md, first run: "The cursor sits on the platen." A new text is
/// typed on the platen row from its first letter, with bare paper above,
/// and the paper climbs a row for every row written.
#[test]
fn a_new_text_is_typed_on_the_platen_with_bare_paper_above_it() {
    let mut r = Rig::sized(1920, 1080);
    assert_eq!(cursor_row(&mut r), PLATEN);
    assert_eq!(r.app.view().top(), -(PLATEN as i64));
    for typed in 1..=27 {
        r.type_text("x");
        assert_eq!(read(&mut r, 47, 112, PLATEN), "x");
        r.press("shift+enter");
        assert_eq!(cursor_row(&mut r), PLATEN, "after {typed} rows");
        assert_eq!(r.app.view().top(), typed - PLATEN as i64, "the paper has climbed a row");
        assert_eq!(read(&mut r, 41, 118, PLATEN - typed as i32 - 1), "", "bare paper above the first row");
        assert_eq!(read(&mut r, 47, 112, PLATEN - typed as i32), "x");
    }
    // The first row has reached the top of the sheet, under its head margin.
    assert_eq!((r.app.view().top(), read(&mut r, 47, 112, 1)), (-1, String::from("x")));
    assert_eq!(read(&mut r, 41, 118, PLATEN + 1), "", "and nothing is ever below the typing row");
}

/// A text shorter than the sheet opens the same way, and the wheel can roll
/// it up as far as the top of the sheet and no further. From there the
/// typing row comes down to the platen before the paper moves again.
#[test]
fn a_short_text_opens_with_its_last_row_at_the_platen_and_the_wheel_rolls_it_to_the_top() {
    let mut r = Rig::with_text("First.\n\nSecond.", 1920, 1080);
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, 2 - PLATEN as i64));
    assert_eq!(read(&mut r, 47, 112, PLATEN - 2), "First.");
    assert_eq!(read(&mut r, 41, 118, PLATEN - 3), "");
    r.wheel(-1000);
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (3, -1), "under one row of head margin, level with the contents and the labels");
    assert_eq!(read(&mut r, 47, 112, 1), "First.");
    for typed in 1..=25 {
        r.press("shift+enter");
        assert_eq!((cursor_row(&mut r), r.app.view().top()), (3 + typed, -1), "the paper has not moved yet");
    }
    assert_eq!(cursor_row(&mut r), PLATEN);
    r.press("shift+enter");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, 0), "and from the platen on it is the paper that moves");
    // The other way, the wheel takes the first row down as far as the platen.
    r.wheel(1000);
    assert_eq!(r.app.view().top(), -(PLATEN as i64));
    assert_eq!(read(&mut r, 47, 112, PLATEN), "First.");
}

#[test]
fn the_typing_row_does_not_move_while_typing_past_the_platen() {
    let mut r = rig(&lines(28));
    assert_eq!(cursor_row(&mut r), PLATEN);
    assert_eq!(read(&mut r, 47, 112, PLATEN), "line 27");
    // Row after row, wrapped and broken, the cursor's row stays where it
    // is and the text climbs past it.
    let sentence = "The paper moves and the typing point does not. ";
    for round in 0..12 {
        for ch in sentence.chars() {
            r.type_text(&ch.to_string());
            assert_eq!(cursor_row(&mut r), PLATEN);
        }
        r.press(if round % 3 == 0 { "enter" } else { "shift+enter" });
        assert_eq!(cursor_row(&mut r), PLATEN);
        r.wait(100);
        assert_eq!(cursor_row(&mut r), PLATEN);
    }
    assert_eq!(r.app.view().top(), 15, "sixteen rows have climbed past");
    assert_eq!(read(&mut r, 41, 118, PLATEN + 1), "", "nothing is ever written below the typing row");
    assert_ne!(read(&mut r, 47, 112, PLATEN - 2), "");
}

#[test]
fn deleting_back_over_a_row_at_the_platen_brings_the_paper_back_down() {
    let mut r = rig(&lines(40));
    let top = r.app.view().top();
    assert_eq!((cursor_row(&mut r), top), (PLATEN, 39 - 28));
    r.press("shift+home");
    r.press("backspace");
    r.press("backspace");
    assert_eq!(r.text().lines().count(), 39);
    assert_eq!(cursor_row(&mut r), PLATEN, "the typing row is still the platen");
    assert_eq!(r.app.view().top(), top - 1);
    // Undo and redo are edits like any other.
    r.press("ctrl+z");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, top));
    // The typing row is held all the way back to the first row of the
    // text: the paper comes down, and bare paper comes onto the sheet above.
    let mut r = rig(&lines(29));
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, 0));
    for gone in 1..=28 {
        r.press("shift+home");
        r.press("backspace");
        r.press("backspace");
        assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, -gone), "after {gone} rows");
    }
    assert_eq!((r.text(), read(&mut r, 47, 112, PLATEN)), (String::from("line 0"), String::from("line 0")));
}

#[test]
fn with_scroll_smooth_the_paper_takes_four_frames_and_the_typing_row_is_held() {
    let look = Look::manila();
    let mut r = rig(&lines(40));
    r.type_text(" and more");
    // Nine characters are Writing, and the sides are fading: let that
    // finish, so that what animates from here on is the paper alone.
    r.wait(1000);
    assert!(!r.wants().animating);
    // The picture of a few cells: every pixel, left to right and top down.
    let picture = |r: &mut Rig, col: i32, row: i32, wide: i32| -> Vec<Rgb> {
        let cells = r.app.grid().cells(col, row, wide, 1);
        let frame = r.draw();
        (0..cells.h).flat_map(|dy| (0..cells.w).map(move |dx| (dx, dy))).filter_map(|(dx, dy)| frame.get(cells.x + dx, cells.y + dy)).collect()
    };

    r.press("shift+enter");
    // The text is already where it will rest, and so is the typing row.
    assert_eq!(read(&mut r, 47, 112, PLATEN - 1), "line 39 and more");
    assert_eq!(cursor_row(&mut r), PLATEN);
    // The paper is three quarters of a row short of it, then two, then
    // one, then there: four frames, each asked for by a tick.
    let mut shifts = Vec::new();
    for _ in 0..4 {
        shifts.push(r.app.view().shift());
        if r.wants().animating {
            r.send(Event::Tick);
        }
    }
    assert_eq!(shifts, [16, 11, 5, 0]);
    assert!(!r.wants().animating);

    // Mid-roll the typing row is held: it shows the cursor's block and
    // bare paper, and nothing of the rows sliding past behind it. The row
    // above is on its way up and not yet where it will rest.
    r.type_text("abc");
    r.press("shift+enter");
    assert_eq!(r.app.view().shift(), 16);
    assert!(picture(&mut r, 47, PLATEN, 1).iter().all(|&pixel| pixel == look.ink));
    assert!(picture(&mut r, 48, PLATEN, 20).iter().all(|&pixel| pixel == look.paper));
    let arriving = picture(&mut r, 47, PLATEN - 1, 3);
    for _ in 0..3 {
        r.send(Event::Tick);
    }
    let arrived = picture(&mut r, 47, PLATEN - 1, 3);
    assert_ne!(arriving, arrived);
    assert!(arrived.contains(&look.ink) && arrived.contains(&look.paper));
    assert_eq!(read(&mut r, 47, 112, PLATEN - 1), "abc");
    // A key that arrives mid-roll finishes it at once.
    r.press("shift+enter");
    assert_eq!(r.app.view().shift(), 16);
    r.type_text("x");
    assert_eq!(r.app.view().shift(), 0);
    assert!(!r.wants().animating);
    assert_eq!(read(&mut r, 47, 112, PLATEN), "x");
}

#[test]
fn with_scroll_jump_or_animations_off_the_paper_moves_in_one_step() {
    let mut r = rig(&lines(40));
    r.app.prefs_mut().smooth = false;
    r.press("enter");
    assert_eq!((r.app.view().shift(), r.wants().animating), (0, false));
    assert_eq!(cursor_row(&mut r), PLATEN);
    let mut r = rig(&lines(40));
    r.host.reduced_motion = true;
    r.press("enter");
    assert_eq!((r.app.view().shift(), r.wants().animating), (0, false));
    // And through the script language.
    let r = ok_ui::harness::run("size 1920 1080\nscroll jump\ntype a\nkey shift+enter 40").unwrap();
    assert_eq!(r.app.view().shift(), 0);
    assert_eq!(r.app.view().top(), 40 - 28);
}

#[test]
fn the_arrow_keys_move_the_cursor_freely_and_the_paper_only_when_they_must() {
    let mut r = rig(&lines(200));
    let top = r.app.view().top();
    for step in 1..=5 {
        r.press("up");
        assert_eq!(cursor_row(&mut r), PLATEN - step);
    }
    assert_eq!(r.app.view().top(), top);
    // Typing above the platen moves nothing: the row is held where it is.
    r.type_text(" more");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN - 5, top));
    r.press("shift+enter");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN - 4, top), "and comes down toward the platen");
    // Past the top edge the paper comes down a row at a time.
    for _ in 0..PLATEN - 4 {
        r.press("up");
    }
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (0, top));
    r.press("up");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (0, top - 1));
    // Below the platen is free too, as far as the last row of the sheet.
    r.press("ctrl+home");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, -(PLATEN as i64)));
    for _ in 0..16 {
        r.press("down");
    }
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (44, -(PLATEN as i64)));
    r.press("down");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (44, 1 - PLATEN as i64));
    // But nothing is typed below the platen: the paper climbs to seat the row.
    r.type_text("x");
    assert_eq!(cursor_row(&mut r), PLATEN);
}

#[test]
fn half_a_heading_at_the_edge_of_the_sheet_is_left_off_at_rest() {
    let text = format!("# First\n{}\n# Last\n{}", lines(100), lines(60));
    let text_cols = ok_ui::grid::Cols { first: 47, last: 112 };
    let mut r = rig(&text);
    r.app.prefs_mut().smooth = false;
    r.press("ctrl+home");
    put(&mut r, 5, 0);
    // The wheel never leaves a heading cut in two at the top of the sheet,
    // where it could not be drawn: it goes a row further the way it was
    // going, down the text or back up it.
    r.wheel(-27);
    assert_eq!(r.app.view().top(), 0);
    assert_eq!(r.screen().big(text_cols, 0), "First");
    r.wheel(-1);
    assert_eq!(r.app.view().top(), 2, "past the heading whole");
    assert_eq!(read(&mut r, 47, 112, 0), "line 0");
    r.wheel(1);
    assert_eq!(r.app.view().top(), 0, "and back onto it whole");
    assert_eq!(r.screen().big(text_cols, 0), "First");
    // At the foot a heading whose lower half would fall on the row kept
    // empty is not begun.
    let last = r.app.page().layout().first_row(101) as i64;
    r.wheel(-(last - 44) as i32);
    assert_eq!(r.app.view().top(), last - 44);
    assert_eq!(read(&mut r, 41, 118, 44), "");
    r.wheel(-1);
    assert_eq!(r.screen().big(text_cols, 43), "Last");
    assert_eq!(read(&mut r, 41, 46, 44), "   #");

    // Typing holds its row at the platen whatever that does to the top of
    // the sheet, so there a heading can still be cut in two. It is left off
    // whole, mark and all, and a cursor put on it is a bare block: the mark
    // or the letter under it, with no heading round it, would be read as
    // text that is there.
    let mut r = rig(&format!("# First\n{}", lines(28)));
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, 1));
    assert_eq!(read(&mut r, 41, 118, 0), "");
    assert_eq!(read(&mut r, 47, 112, 1), "line 0");
    put(&mut r, 0, 0);
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((44, 0)));
    assert_eq!(read(&mut r, 41, 118, 0), "", "the block holds no character");
    // The arrow keys bring the whole heading onto the sheet with them.
    r.press("right");
    r.press("left");
    assert_eq!(r.app.view().top(), -1);
    assert_eq!(r.screen().big(text_cols, 1), "First");
    assert_eq!(read(&mut r, 41, 46, 2), "   #");
}

#[test]
fn a_jump_brings_its_row_to_the_platen() {
    let mut r = rig(&lines(200));
    r.press("ctrl+home");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, -(PLATEN as i64)), "the first row of the text, with bare paper above it");
    assert_eq!(read(&mut r, 41, 118, PLATEN - 1), "");
    r.press("ctrl+end");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN, 199 - 28));
    r.app.go(ok_text::Pos::new(100, 2));
    assert_eq!((cursor_row(&mut r), head(&r)), (PLATEN, (100, 2)));
}

#[test]
fn the_page_keys_move_the_paper_a_screen_less_a_row() {
    let mut r = rig(&lines(300));
    r.press("ctrl+home");
    r.press("pagedown");
    assert_eq!((head(&r), cursor_row(&mut r), r.app.view().top()), ((44, 0), PLATEN, 44 - PLATEN as i64));
    r.press("pagedown");
    assert_eq!((head(&r), cursor_row(&mut r), r.app.view().top()), ((88, 0), PLATEN, 88 - PLATEN as i64));
    r.press("pageup");
    assert_eq!((head(&r), cursor_row(&mut r)), ((44, 0), PLATEN));
    r.press("pageup");
    assert_eq!((head(&r), cursor_row(&mut r), r.app.view().top()), ((0, 0), PLATEN, -(PLATEN as i64)));
    // From the end, where the last row is at the platen.
    r.press("ctrl+end");
    r.press("pageup");
    assert_eq!((head(&r).0, cursor_row(&mut r)), (299 - 44, PLATEN));
    r.press("pagedown");
    assert_eq!((head(&r).0, cursor_row(&mut r)), (299, PLATEN));
}

#[test]
fn the_wheel_moves_the_paper_and_leaves_the_cursor_in_the_text() {
    let mut r = rig(&lines(200));
    let top = r.app.view().top();
    r.wheel(3);
    assert_eq!(r.app.view().top(), top - 3);
    assert_eq!(head(&r), (199, 8));
    assert_eq!(cursor_row(&mut r), PLATEN + 3);
    r.wheel(-3);
    assert_eq!(r.app.view().top(), top);
    r.wheel(-50);
    assert_eq!(r.app.view().top(), top, "the last row goes no higher than the platen");
    r.wheel(10_000);
    assert_eq!(r.app.view().top(), -(PLATEN as i64), "and the first no lower than the platen");
    assert_eq!(r.screen().cursor, None, "the cursor has left the sheet with its row");
    assert_eq!(read(&mut r, 47, 112, PLATEN), "line 0");
    // The next letter is typed where the cursor is, and that row is
    // brought to the platen.
    r.type_text("!");
    assert_eq!((cursor_row(&mut r), r.text().ends_with("line 199!")), (PLATEN, true));
}

#[test]
fn a_click_places_the_cursor_and_a_drag_selects() {
    let mut r = rig("First paragraph of words.\n\nSecond paragraph here.");
    r.click_cell(47 + 6, 1);
    assert_eq!((head(&r), selected(&r)), ((0, 6), String::new()));
    // Past the end of a row is the end of that row; below the text, its end.
    r.click_cell(47 + 60, 1);
    assert_eq!(head(&r), (0, 25));
    r.click_cell(47 + 3, 20);
    assert_eq!(head(&r), (2, 22));
    r.click_cell(47, 2);
    assert_eq!(head(&r), (1, 0));
    // In the gutter and the margins of the sheet, the nearest of the row.
    r.click_cell(42, 3);
    assert_eq!(head(&r), (2, 0));
    r.click_cell(117, 3);
    assert_eq!(head(&r), (2, 22));
    // On the desk, nothing.
    r.click_cell(10, 3);
    r.click_cell(150, 30);
    assert_eq!(head(&r), (2, 22));

    // Shift and a click stretch the selection; a drag makes one.
    r.click_cell(47 + 6, 1);
    r.hold(Mods::SHIFT);
    r.click_cell(47 + 15, 1);
    r.release();
    assert_eq!(selected(&r), "paragraph");
    let (from, to) = (r.cell_centre(47, 1), r.cell_centre(47 + 6, 3));
    r.send(Event::Mouse { kind: MouseKind::Down, x: from.0, y: from.1, mods: Mods::NONE });
    r.send(Event::Mouse { kind: MouseKind::Move, x: to.0, y: to.1, mods: Mods::NONE });
    assert_eq!(selected(&r), "First paragraph of words.\n\nSecond");
    r.send(Event::Mouse { kind: MouseKind::Up, x: to.0, y: to.1, mods: Mods::NONE });
    r.send(Event::Mouse { kind: MouseKind::Move, x: from.0, y: from.1, mods: Mods::NONE });
    assert_eq!(selected(&r), "First paragraph of words.\n\nSecond", "the button is up: moving selects nothing more");
    // On a double-size heading either row and either half of a letter.
    let mut r = rig("# Title\n\nBody.");
    for (col, row) in [(47 + 4, 1), (47 + 5, 1), (47 + 4, 2), (47 + 5, 2)] {
        r.click_cell(col, row);
        assert_eq!(head(&r), (0, 4), "cell {col}, {row}");
    }
}

#[test]
fn a_click_on_the_map_goes_there() {
    let mut r = rig(&lines(200));
    let area = r.app.grid().map_area().unwrap();
    r.click(area.x + 20, area.y + 50 * 4 + 1);
    assert_eq!(head(&r), (50, 0));
    assert_eq!(cursor_row(&mut r), PLATEN);
    r.click(area.x - 10, area.y + 2);
    assert_eq!(head(&r), (0, 0), "the notch column counts as the map");
    r.click(area.x + 20, area.y + 200 * 4 + 40);
    assert_eq!(head(&r), (0, 0), "below the last row of the map there is nothing to go to");
}

#[test]
fn with_the_platen_off_the_paper_only_keeps_the_cursor_in_sight() {
    let mut r = rig(&lines(28));
    r.app.prefs_mut().platen = false;
    for typed in 1..=16 {
        r.press("shift+enter");
        assert_eq!((cursor_row(&mut r), r.app.view().top()), (PLATEN + typed, -1));
    }
    r.press("shift+enter");
    assert_eq!((cursor_row(&mut r), r.app.view().top()), (44, 0));
    put(&mut r, 0, 0);
    r.type_text("x");
    assert_eq!(r.app.view().top(), 0);
}
