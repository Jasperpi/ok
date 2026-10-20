//! "Every cell of text is a character that is in the file, so arrow keys
//! never lie." The movement keys and the mouse, held to what the screen
//! shows: Up and Down go exactly one row, Home and End do nothing the
//! second time, a click puts the cursor on the cell that was clicked, and
//! the paper stays still while the cursor is above the platen.

mod attack_support;

use attack_support::{on_cluster_boundary, Rng};
use ok_host::{Key, Mods};
use ok_text::Pos;
use ok_ui::harness::Rig;

/// Lines of every kind, with the characters that are hard on a grid.
fn texts() -> Vec<String> {
    vec![
        String::from("# The orchard and a title long enough to take a second double-size row\n\nThe orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm.\n\n## What he did\n\n- thinned the fruit, and went on thinning it until the buckets were full and the light had gone\n  - [ ] and a nested one\n12. the twelfth\n\n> It felt like waste. I filled buckets with fruit that would never ripen and carried them to the compost.\n\n```\nlet rows = 12;\n\n\tindented with a tab\n```\n\nlast"),
        format!("e\u{0301}e\u{0301}\u{0308} \u{4E00}\u{4E8C}\u{4E09} tab\there \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} and \u{2764}\u{FE0F}. {}\n\n{}\n\n   spaces before and after   \n\n#### deep heading\n\n- \n", "wide\u{4E00} word ".repeat(14), "x".repeat(300)),
        String::from("\n\n\n"),
        format!("a{}b\n\tc", " ".repeat(200)),
    ]
}

/// The windows the checks are made in: a full screen, eighty by twenty
/// four, and one narrow enough to take the measure down.
const WINDOWS: [(usize, usize); 3] = [(1920, 1080), (960, 528), (600, 400)];

fn head(rig: &Rig) -> Pos {
    rig.app.page().cursor.head
}

/// Every place a cursor can rest on a line.
fn places(rig: &Rig) -> Vec<Pos> {
    let doc = rig.app.page().doc();
    let mut all = Vec::new();
    for (line, text) in doc.lines().iter().enumerate() {
        all.extend((0..=text.len()).filter(|&col| text.is_char_boundary(col) && on_cluster_boundary(text, col)).map(|col| Pos::new(line, col)));
    }
    all
}

/// The row of the whole text the cursor is drawn on, and how many grid rows
/// tall the row it belongs to is.
fn row_of(rig: &Rig) -> (usize, usize) {
    let page = rig.app.page();
    let (row, _) = page.layout().locate(page.doc(), page.cursor.head);
    (row, page.layout().shape(page.cursor.head.line).scale)
}

#[test]
fn home_and_end_do_nothing_the_second_time() {
    for (width, height) in WINDOWS {
        for text in texts() {
            let mut rig = Rig::with_text(&text, width, height);
            for place in places(&rig) {
                for chord in ["home", "end"] {
                    rig.app.page_mut().place(place, false);
                    rig.press(chord);
                    let once = head(&rig);
                    assert_eq!(once.line, place.line, "{chord} from {place:?} left the line");
                    rig.press(chord);
                    assert_eq!(head(&rig), once, "{chord} twice from {place:?} at {width} by {height} is not {chord} once");
                }
            }
        }
    }
}

/// Home and End stay on the row the cursor is drawn on.
#[test]
fn home_and_end_stay_on_the_row() {
    for (width, height) in WINDOWS {
        for text in texts() {
            let mut rig = Rig::with_text(&text, width, height);
            for place in places(&rig) {
                rig.app.page_mut().place(place, false);
                let (row, _) = row_of(&rig);
                for chord in ["home", "end", "home"] {
                    rig.press(chord);
                    assert_eq!(row_of(&rig).0, row, "{chord} from {place:?} at {width} by {height} went to another row");
                }
            }
        }
    }
}

/// Up goes to the row above and Down to the row below, from anywhere: one
/// row and no more, a double-size row counting as one.
#[test]
fn up_and_down_go_exactly_one_row() {
    for (width, height) in WINDOWS {
        for text in texts() {
            let mut rig = Rig::with_text(&text, width, height);
            let total = rig.app.page().layout().total_rows();
            for place in places(&rig) {
                rig.app.page_mut().place(place, false);
                let (row, tall) = row_of(&rig);
                rig.press("down");
                let (below, _) = row_of(&rig);
                if row + tall >= total {
                    assert_eq!(head(&rig), rig.app.page().doc().end(), "Down from the last row goes to the end");
                } else {
                    assert_eq!(below, row + tall, "Down from {place:?} at {width} by {height} went from row {row} to row {below}");
                }
                rig.app.page_mut().place(place, false);
                rig.press("up");
                let (above, above_tall) = row_of(&rig);
                if row == 0 {
                    assert_eq!(head(&rig), Pos::default(), "Up from the first row goes to the start");
                } else {
                    assert_eq!(above + above_tall, row, "Up from {place:?} at {width} by {height} went from row {row} to row {above}");
                }
            }
        }
    }
}

/// Down and then Up, or Up and then Down, comes back to the cell it left:
/// the column the cursor set out from is kept for as long as only Up and
/// Down are pressed.
#[test]
fn up_and_down_keep_the_column_they_set_out_from() {
    for (width, height) in WINDOWS {
        for text in texts() {
            let mut rig = Rig::with_text(&text, width, height);
            let total = rig.app.page().layout().total_rows();
            for place in places(&rig) {
                rig.app.page_mut().place(place, false);
                // A cursor on a mark hanging in the gutter is drawn beside
                // the text and comes back to the first letter.
                if place.col < rig.app.page().layout().shape(place.line).hung {
                    continue;
                }
                let before = rig.app.page().caret();
                let (row, tall) = row_of(&rig);
                if row + tall < total {
                    rig.press("down");
                    rig.press("up");
                    let now = rig.app.page().caret();
                    assert_eq!((now.row, now.cell), (before.row, before.cell), "Down then Up from {place:?} at {width} by {height}");
                }
                rig.app.page_mut().place(place, false);
                if row > 0 {
                    rig.press("up");
                    rig.press("down");
                    let now = rig.app.page().caret();
                    assert_eq!((now.row, now.cell), (before.row, before.cell), "Up then Down from {place:?} at {width} by {height}");
                }
            }
        }
    }
}

/// Left and then Right, and Right and then Left, come back to where they
/// started, from every place but the two ends of the text.
#[test]
fn left_and_right_undo_each_other() {
    for text in texts() {
        let mut rig = Rig::with_text(&text, 960, 528);
        let end = rig.app.page().doc().end();
        for place in places(&rig) {
            rig.app.page_mut().place(place, false);
            rig.press("left");
            rig.press("right");
            if place != Pos::default() {
                assert_eq!(head(&rig), place, "Left then Right from {place:?}");
            }
            rig.app.page_mut().place(place, false);
            rig.press("right");
            rig.press("left");
            if place != end {
                assert_eq!(head(&rig), place, "Right then Left from {place:?}");
            }
        }
    }
}

/// Right from the start reaches the end having stopped at every place a
/// cursor can rest, each once and in order; Left goes back the same way.
#[test]
fn right_visits_every_place_once_and_left_goes_back_through_them() {
    for text in texts() {
        let mut rig = Rig::with_text(&text, 960, 528);
        let all = places(&rig);
        rig.press("ctrl+home");
        let mut seen = vec![head(&rig)];
        for _ in 0..all.len() + 5 {
            rig.press("right");
            if seen.last() == Some(&head(&rig)) {
                break;
            }
            seen.push(head(&rig));
        }
        assert_eq!(seen, all, "Right through {:?}", text.chars().take(30).collect::<String>());
        let mut back = vec![head(&rig)];
        for _ in 0..all.len() + 5 {
            rig.press("left");
            if back.last() == Some(&head(&rig)) {
                break;
            }
            back.push(head(&rig));
        }
        back.reverse();
        assert_eq!(back, all);
    }
}

/// A click on a cell of the text puts the cursor in that cell: on the
/// character drawn there, or after the last one on its row.
#[test]
fn a_click_puts_the_cursor_on_the_cell_that_was_clicked() {
    for (width, height) in WINDOWS {
        for text in texts() {
            let mut rig = Rig::with_text(&text, width, height);
            rig.press("ctrl+home");
            rig.draw();
            let grid = rig.app.grid().clone();
            let top = rig.app.view().top();
            let total = rig.app.page().layout().total_rows() as i64;
            for row in 0..grid.text_rows {
                let at = top + row as i64;
                if at < 0 || at >= total {
                    continue;
                }
                for col in grid.text.first..=grid.text.last {
                    rig.click_cell(col, row);
                    let caret = rig.app.page().caret();
                    let page = rig.app.page();
                    let (line, within) = page.layout().line_of_row(at as usize);
                    let unit = page.layout().shape(line).scale as i64;
                    let row_top = page.layout().first_row(line) as i64 + within as i64 * unit;
                    assert_eq!(caret.row as i64, row_top, "a click on row {row} column {col} at {width} by {height} put the cursor on row {} of the text", caret.row);
                    // The cursor is on or before the cell clicked, and no
                    // further before it than one character can be wide.
                    let cell = col - grid.text.first;
                    let rows = page.layout().rows(line);
                    let text_row = rows[within];
                    // Whitespace at the end of a row is text too, as far
                    // as the measure shows it.
                    let cells = ok_text::str_width(page.doc().line(line).get(text_row.start..text_row.end).unwrap_or(""));
                    let shown = ((text_row.indent + cells) as i32 * unit as i32).min(grid.measure + 1);
                    let first = text_row.indent as i32 * unit as i32;
                    if cell >= first && cell < shown {
                        assert!(caret.cell <= cell && cell < caret.cell + 4 * unit as i32, "a click on column {col} row {row} at {width} by {height} put the cursor in cell {} (text column {cell})", caret.cell);
                    } else if cell >= shown {
                        assert!(caret.cell <= shown.max(first), "a click past the end of row {row} (column {col}, line {line} {:?}, its row {within}: {text_row:?}) at {width} by {height} put the cursor in cell {}, past the text's {shown}", page.doc().line(line).chars().take(40).collect::<String>(), caret.cell);
                    }
                }
            }
        }
    }
}

/// While the cursor is above the platen, typing leaves the paper where it
/// is: the row comes down to the platen and only then does the paper climb.
#[test]
fn typing_above_the_platen_does_not_move_the_paper() {
    let mut rng = Rng::new(4);
    for (width, height) in WINDOWS {
        let mut rig = Rig::sized(width, height);
        let platen = rig.app.grid().platen as i64;
        let mut climbed = false;
        for _ in 0..4000 {
            let before = (rig.app.view().top(), rig.app.page().caret().row as i64 - rig.app.view().top());
            match rng.below(30) {
                0 => rig.key(Key::Enter, Mods::NONE),
                1 => rig.key(Key::Enter, Mods::SHIFT),
                2 => rig.send(ok_host::Event::Text(' ')),
                3 => rig.send(ok_host::Event::Text('\u{4E00}')),
                _ => rig.send(ok_host::Event::Text('a')),
            }
            let after = (rig.app.view().top(), rig.app.page().caret().row as i64 - rig.app.view().top());
            assert!(after.1 <= platen, "the typing row went below the platen at {width} by {height}: row {} of the sheet", after.1);
            if before.1 < platen {
                assert_eq!(after.0, before.0, "the paper moved while the cursor was above the platen, on row {} of the sheet", before.1);
            } else {
                assert_eq!(after.1, platen, "typing at the platen moved the typing row");
                climbed |= after.0 > before.0;
            }
        }
        assert!(climbed, "the text never reached the platen at {width} by {height}");
    }
}

/// "Ctrl T: To the next TK, wrapping." Pressed as many times as there are
/// TKs, from anywhere, it selects each of them once, in the order they
/// stand in the text, and the next press is the first one again.
#[test]
fn ctrl_t_visits_every_tk_once_and_comes_round() {
    let text = "TK at the start, TK in the middle and TK\n\nATK is not one, nor TKs, but (TK) is.\n\n```\nTK in code is code\n```\n\n- TK\n> TK TK\n# TK";
    let mut rig = Rig::with_text(text, 1920, 1080);
    let count = 8;
    assert_eq!(rig.app.page().tk_count(), count);
    for place in places(&rig) {
        rig.app.page_mut().place(place, false);
        let mut visited = Vec::new();
        for _ in 0..count {
            rig.press("ctrl+t");
            let cursor = rig.app.page().cursor;
            assert_eq!(rig.app.page().selection().as_deref(), Some("TK"), "from {place:?}");
            assert_eq!((cursor.anchor.line, cursor.anchor.col + 2), (cursor.head.line, cursor.head.col));
            visited.push(cursor.anchor);
        }
        let first = visited[0];
        let mut in_order = visited.clone();
        in_order.sort();
        in_order.dedup();
        assert_eq!(in_order.len(), count, "from {place:?} Ctrl T visited {visited:?}");
        // Document order, starting wherever the cursor was and wrapping once.
        let wraps = visited.windows(2).filter(|pair| pair[1] < pair[0]).count();
        assert!(wraps <= 1, "from {place:?} Ctrl T went {visited:?}");
        // The first TK it finds is the nearest at or after the cursor.
        let nearest = in_order.iter().copied().find(|&tk| tk >= place).unwrap_or(in_order[0]);
        assert_eq!(first, nearest, "from {place:?} the first Ctrl T went to {first:?}");
        rig.press("ctrl+t");
        assert_eq!(rig.app.page().cursor.anchor, first, "from {place:?} the press after the last comes round to the first");
        assert_eq!(rig.text(), text);
    }
}
