//! The Keys section of DESIGN.md, row by row, driven through the harness
//! as the shell would drive the window.

mod common;

use common::{head, presses, put, rig, said, selected};
use ok_host::{Event, Key, Mods, MouseKind};
use ok_ui::harness::Rig;

/// Two sentences, a blank line, and a second paragraph.
const PROSE: &str = "One two. Three four five.\n\nSecond paragraph here.";

/// One paragraph of twenty four-cell words: sixteen fill the first row of
/// a 66-cell measure and four are left for the second.
fn two_rows() -> String {
    (0..20).map(|n| format!("w{n:02} ")).collect()
}

// ───────────────────────── the table of sizes ─────────────────────────

#[test]
fn left_and_right_move_a_letter() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 0);
    r.press("right");
    assert_eq!(head(&r), (0, 1));
    r.press("left");
    r.press("left");
    assert_eq!(head(&r), (0, 0), "there is nowhere left of the start");
    put(&mut r, 0, 25);
    r.press("right");
    assert_eq!(head(&r), (1, 0), "past the end of a line is the start of the next");
    r.press("left");
    assert_eq!(head(&r), (0, 25));
    // A letter and the mark on it are one letter.
    let mut r = rig("n\u{0313}a");
    put(&mut r, 0, 0);
    r.press("right");
    assert_eq!(head(&r), (0, 3));
    r.press("right");
    assert_eq!(head(&r), (0, 4));
    r.press("left");
    r.press("left");
    assert_eq!(head(&r), (0, 0));
}

#[test]
fn ctrl_left_and_right_move_a_word() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 0);
    r.press("ctrl+right");
    assert_eq!(head(&r), (0, 4));
    r.press("ctrl+right");
    assert_eq!(head(&r), (0, 9));
    r.press("ctrl+left");
    assert_eq!(head(&r), (0, 4));
    r.press("ctrl+left");
    assert_eq!(head(&r), (0, 0));
    put(&mut r, 0, 21);
    r.press("ctrl+right");
    assert_eq!(head(&r), (0, 25), "from the last word, the end of the line");
    r.press("ctrl+right");
    assert_eq!(head(&r), (1, 0));
}

#[test]
fn alt_left_and_right_move_a_sentence() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 0);
    r.press("alt+right");
    assert_eq!(head(&r), (0, 9));
    r.press("alt+right");
    assert_eq!(head(&r), (2, 0), "the next sentence is in the next paragraph");
    r.press("alt+left");
    assert_eq!(head(&r), (0, 9));
    put(&mut r, 0, 12);
    r.press("alt+left");
    assert_eq!(head(&r), (0, 9), "from inside a sentence, its start");
    r.press("alt+left");
    assert_eq!(head(&r), (0, 0));
}

#[test]
fn up_and_down_move_a_row_and_keep_their_column() {
    let text = format!("{}\n\nshort", two_rows());
    let mut r = rig(&text);
    put(&mut r, 0, 70);
    r.press("up");
    assert_eq!(head(&r), (0, 6), "the same cell on the row above, which is the same line");
    r.press("down");
    assert_eq!(head(&r), (0, 70));
    r.press("down");
    assert_eq!(head(&r), (1, 0));
    r.press("down");
    assert_eq!(head(&r), (2, 5), "a shorter row takes the cursor to its end");
    r.press("down");
    assert_eq!(head(&r), (2, 5), "from the last row, the end of the text");
    presses(&mut r, &["up", "up"]);
    assert_eq!(head(&r), (0, 70), "the column it set out from is remembered across the short rows");
    presses(&mut r, &["up", "up"]);
    assert_eq!(head(&r), (0, 0), "from the first row, the start of the text");
}

#[test]
fn ctrl_up_and_down_move_a_paragraph() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 5);
    r.press("ctrl+down");
    assert_eq!(head(&r), (2, 0));
    r.press("ctrl+down");
    assert_eq!(head(&r), (2, 22), "from the last paragraph, the end of the text");
    put(&mut r, 2, 5);
    r.press("ctrl+up");
    assert_eq!(head(&r), (2, 0), "first to the start of this paragraph");
    r.press("ctrl+up");
    assert_eq!(head(&r), (0, 0));
}

#[test]
fn shift_with_any_movement_selects() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 0);
    presses(&mut r, &["shift+right", "shift+right", "shift+right"]);
    assert_eq!(selected(&r), "One");
    r.press("shift+left");
    assert_eq!(selected(&r), "On", "going back gives a letter up");

    put(&mut r, 0, 0);
    r.press("ctrl+shift+right");
    assert_eq!(selected(&r), "One ");
    put(&mut r, 0, 0);
    r.press("alt+shift+right");
    assert_eq!(selected(&r), "One two. ");
    put(&mut r, 0, 9);
    r.press("alt+shift+left");
    assert_eq!(selected(&r), "One two. ");
    put(&mut r, 0, 0);
    r.press("ctrl+shift+down");
    assert_eq!(selected(&r), "One two. Three four five.\n\n");
    put(&mut r, 2, 22);
    r.press("ctrl+shift+up");
    assert_eq!(selected(&r), "Second paragraph here.");

    put(&mut r, 0, 12);
    r.press("shift+end");
    assert_eq!(selected(&r), "ee four five.");
    put(&mut r, 0, 12);
    r.press("shift+home");
    assert_eq!(selected(&r), "One two. Thr");
    put(&mut r, 0, 12);
    r.press("ctrl+shift+end");
    assert_eq!(selected(&r), "ee four five.\n\nSecond paragraph here.");
    put(&mut r, 0, 12);
    r.press("ctrl+shift+home");
    assert_eq!(selected(&r), "One two. Thr");

    let mut r = rig(&two_rows());
    put(&mut r, 0, 0);
    r.press("shift+down");
    assert_eq!(selected(&r).len(), 64, "one row");
    r.press("shift+up");
    assert_eq!(selected(&r), "");
}

#[test]
fn left_and_right_without_shift_fold_a_selection_to_its_end() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 4);
    presses(&mut r, &["shift+right", "shift+right", "shift+right", "left"]);
    assert_eq!((head(&r), selected(&r)), ((0, 4), String::new()));
    presses(&mut r, &["shift+right", "shift+right", "shift+right", "right"]);
    assert_eq!((head(&r), selected(&r)), ((0, 7), String::new()));
}

#[test]
fn backspace_and_delete_take_a_letter() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 3);
    r.press("backspace");
    assert_eq!(r.text().lines().next(), Some("On two. Three four five."));
    assert_eq!(head(&r), (0, 2));
    put(&mut r, 0, 0);
    r.press("delete");
    assert_eq!(r.text().lines().next(), Some("n two. Three four five."));
    assert_eq!(head(&r), (0, 0));
    // The mark goes with its letter, and Shift makes no difference.
    let mut r = rig("an\u{0313}");
    r.press("shift+backspace");
    assert_eq!(r.text(), "a");
    r.press("backspace");
    r.press("backspace");
    assert_eq!(r.text(), "", "nothing left to take is no error");
    r.press("delete");
    assert_eq!(r.text(), "");
}

#[test]
fn ctrl_backspace_and_delete_take_a_word() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 7);
    r.press("ctrl+backspace");
    assert_eq!(r.text().lines().next(), Some("One . Three four five."));
    assert_eq!(head(&r), (0, 4));
    let mut r = rig(PROSE);
    put(&mut r, 0, 4);
    r.press("ctrl+delete");
    assert_eq!(r.text().lines().next(), Some("One Three four five."));
    assert_eq!(head(&r), (0, 4));
}

#[test]
fn alt_backspace_and_delete_take_a_sentence_and_stay_in_the_paragraph() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 25);
    r.press("alt+backspace");
    assert_eq!(r.text().lines().next(), Some("One two. "));
    let mut r = rig(PROSE);
    put(&mut r, 0, 0);
    r.press("alt+delete");
    assert_eq!(r.text().lines().next(), Some("Three four five."));
    // The last sentence of a paragraph: to the end of the line and no
    // further, though the next sentence starts two lines down.
    r.press("alt+delete");
    assert_eq!(r.text(), "\n\nSecond paragraph here.");
    // From inside a sentence, back to its start.
    let mut r = rig(PROSE);
    put(&mut r, 0, 14);
    r.press("alt+backspace");
    assert_eq!(r.text().lines().next(), Some("One two.  four five."));
}

#[test]
fn backspace_and_delete_take_a_selection_whatever_their_size() {
    for chord in ["backspace", "delete", "ctrl+backspace", "alt+delete"] {
        let mut r = rig(PROSE);
        put(&mut r, 0, 4);
        presses(&mut r, &["shift+right", "shift+right", "shift+right", chord]);
        assert_eq!(r.text().lines().next(), Some("One . Three four five."), "{chord}");
    }
}

// ───────────────────────── carrying the paragraph ─────────────────────────

#[test]
fn alt_up_and_down_carry_the_paragraph_and_the_cursor_with_it() {
    let mut r = rig("First.\n\nSecond.\n\nThird.");
    put(&mut r, 2, 3);
    r.press("alt+up");
    assert_eq!(r.text(), "Second.\n\nFirst.\n\nThird.");
    assert_eq!(head(&r), (0, 3));
    r.press("alt+up");
    assert_eq!(r.text(), "Second.\n\nFirst.\n\nThird.", "nothing above to pass");
    r.press("alt+down");
    assert_eq!(r.text(), "First.\n\nSecond.\n\nThird.");
    assert_eq!(head(&r), (2, 3));
    r.press("alt+down");
    assert_eq!(r.text(), "First.\n\nThird.\n\nSecond.");
    assert_eq!(head(&r), (4, 3));
    r.press("alt+down");
    assert_eq!(r.text(), "First.\n\nThird.\n\nSecond.", "nothing below to pass");
    // Each carry is one step of undo.
    r.press("ctrl+z");
    assert_eq!(r.text(), "First.\n\nSecond.\n\nThird.");
    assert_eq!(head(&r), (2, 3));
}

#[test]
fn a_paragraph_of_several_lines_is_carried_whole_and_blank_lines_are_not_paragraphs() {
    let mut r = rig("One\nstill one\n\n\nTwo");
    put(&mut r, 4, 1);
    r.press("alt+up");
    assert_eq!(r.text(), "Two\n\n\nOne\nstill one");
    assert_eq!(head(&r), (0, 1));
    put(&mut r, 4, 2);
    r.press("alt+up");
    assert_eq!(r.text(), "One\nstill one\n\n\nTwo");
    assert_eq!(head(&r), (1, 2), "the cursor keeps its line of the paragraph");
    put(&mut r, 2, 0);
    presses(&mut r, &["alt+up", "alt+down"]);
    assert_eq!(r.text(), "One\nstill one\n\n\nTwo", "a blank line is not carried");
    let mut r = rig("\n\nonly");
    presses(&mut r, &["alt+up", "alt+down"]);
    assert_eq!(r.text(), "\n\nonly", "blank lines are nothing to pass");
}

// ───────────────────────── Enter ─────────────────────────

#[test]
fn enter_ends_the_paragraph_and_writes_the_blank_line() {
    let mut r = rig("abcdef");
    put(&mut r, 0, 3);
    r.press("enter");
    assert_eq!(r.text(), "abc\n\ndef");
    assert_eq!(head(&r), (2, 0));
    // A heading and a quote are paragraphs too.
    for text in ["# Title", "> quoted", "---"] {
        let mut r = rig(text);
        r.press("enter");
        assert_eq!(r.text(), format!("{text}\n\n"));
        assert_eq!(head(&r), (2, 0));
    }
}

#[test]
fn enter_in_a_list_starts_the_next_item() {
    for (item, next) in [("- one", "- "), ("* one", "* "), ("  + one", "  + "), ("9. nine", "10. "), ("3) three", "4) "), ("- [x] done", "- [ ] "), ("- [ ] to do", "- [ ] ")] {
        let mut r = rig(item);
        r.press("enter");
        assert_eq!(r.text(), format!("{item}\n{next}"));
        assert_eq!(head(&r), (1, next.len()));
    }
    // In the middle of an item, the rest goes to the new one.
    let mut r = rig("- onetwo");
    put(&mut r, 0, 5);
    r.press("enter");
    assert_eq!(r.text(), "- one\n- two");
    assert_eq!(head(&r), (1, 2));
}

#[test]
fn enter_on_an_empty_item_ends_the_list() {
    let mut r = rig("- one\n- ");
    r.press("enter");
    assert_eq!(r.text(), "- one\n\n");
    assert_eq!(head(&r), (2, 0));
    // Typed from nothing: the marker once, two items, then Enter twice.
    let mut r = Rig::sized(1920, 1080);
    r.type_text("- one\ntwo\n\nAfter");
    assert_eq!(r.text(), "- one\n- two\n\nAfter");
}

#[test]
fn enter_on_a_blank_line_or_in_fenced_code_is_one_line_break() {
    let mut r = rig("abc\n");
    r.press("enter");
    assert_eq!(r.text(), "abc\n\n");
    assert_eq!(head(&r), (2, 0));
    let mut r = rig("```\nlet x = 1;");
    r.press("enter");
    assert_eq!(r.text(), "```\nlet x = 1;\n");
    assert_eq!(head(&r), (2, 0));
    // On the fence itself, and on a line that only looks like a list.
    let mut r = rig("```");
    r.press("enter");
    assert_eq!(r.text(), "```\n");
    let mut r = rig("```\n- not an item");
    r.press("enter");
    assert_eq!(r.text(), "```\n- not an item\n");
}

#[test]
fn shift_enter_is_a_line_break_inside_the_paragraph() {
    let mut r = rig("abcdef");
    put(&mut r, 0, 3);
    r.press("shift+enter");
    assert_eq!(r.text(), "abc\ndef");
    assert_eq!(head(&r), (1, 0));
    let mut r = rig("- one");
    r.press("shift+enter");
    assert_eq!(r.text(), "- one\n", "and in a list it starts no item");
}

#[test]
fn enter_replaces_a_selection() {
    let mut r = rig("abcXXdef");
    put(&mut r, 0, 3);
    presses(&mut r, &["shift+right", "shift+right", "enter"]);
    assert_eq!(r.text(), "abc\n\ndef");
}

// ───────────────────────── Backspace at the start of a paragraph ─────────────────────────

#[test]
fn backspace_at_the_start_of_a_paragraph_undoes_what_enter_did() {
    let mut r = rig("abc\n\ndef");
    put(&mut r, 2, 0);
    r.press("backspace");
    assert_eq!(r.text(), "abcdef");
    assert_eq!(head(&r), (0, 3));
    // Enter and then Backspace is where it started, at every kind of line.
    for text in ["abc", "# Title", "> quoted", "- one", "9. nine", "- [x] done"] {
        let mut r = rig(text);
        presses(&mut r, &["enter", "backspace"]);
        assert_eq!(r.text(), text);
        assert_eq!(head(&r), (0, text.len()));
    }
    // Word and sentence do the same thing there.
    for chord in ["ctrl+backspace", "alt+backspace"] {
        let mut r = rig("abc\n\ndef");
        put(&mut r, 2, 0);
        r.press(chord);
        assert_eq!(r.text(), "abcdef", "{chord}");
    }
}

#[test]
fn backspace_at_the_start_of_an_item_joins_it_to_the_item_above() {
    let mut r = rig("- one\n- two");
    put(&mut r, 1, 2);
    r.press("backspace");
    assert_eq!(r.text(), "- onetwo");
    assert_eq!(head(&r), (0, 5));
    // The first item of a list has nothing to join: its marker is text.
    let mut r = rig("- one");
    put(&mut r, 0, 2);
    r.press("backspace");
    assert_eq!(r.text(), "-one");
}

#[test]
fn backspace_at_other_line_starts_takes_one_break() {
    let mut r = rig("abc\ndef");
    put(&mut r, 1, 0);
    r.press("backspace");
    assert_eq!(r.text(), "abcdef");
    // More blank lines than Enter writes: one goes at a time.
    let mut r = rig("abc\n\n\ndef");
    put(&mut r, 3, 0);
    r.press("backspace");
    assert_eq!(r.text(), "abc\n\ndef");
    assert_eq!(head(&r), (2, 0));
    r.press("backspace");
    assert_eq!(r.text(), "abcdef");
}

#[test]
fn delete_at_the_end_of_a_paragraph_is_the_same_act_from_the_other_side() {
    let mut r = rig("abc\n\ndef");
    put(&mut r, 0, 3);
    r.press("delete");
    assert_eq!(r.text(), "abcdef");
    assert_eq!(head(&r), (0, 3));
    let mut r = rig("- one\n- two");
    put(&mut r, 0, 5);
    r.press("delete");
    assert_eq!(r.text(), "- onetwo");
    let mut r = rig("abc\ndef");
    put(&mut r, 0, 3);
    r.press("ctrl+delete");
    assert_eq!(r.text(), "abcdef");
}

// ───────────────────────── Tab ─────────────────────────

#[test]
fn tab_and_shift_tab_nest_an_item_by_two_spaces() {
    let mut r = rig("- one\n- two");
    put(&mut r, 1, 5);
    r.press("tab");
    assert_eq!(r.text(), "- one\n  - two");
    assert_eq!(head(&r), (1, 7), "the cursor stays on its letter");
    r.press("tab");
    assert_eq!(r.text(), "- one\n    - two");
    presses(&mut r, &["shift+tab", "shift+tab"]);
    assert_eq!(r.text(), "- one\n- two");
    assert_eq!(head(&r), (1, 5));
    r.press("shift+tab");
    assert_eq!(r.text(), "- one\n- two", "nothing left to bring back");
    // Every item a selection touches, as one step.
    presses(&mut r, &["ctrl+a", "tab"]);
    assert_eq!(r.text(), "  - one\n  - two");
    r.press("ctrl+z");
    assert_eq!(r.text(), "- one\n- two");
}

#[test]
fn tab_outside_a_list_does_nothing_and_no_tab_character_is_ever_typed() {
    let mut r = rig("plain words");
    presses(&mut r, &["tab", "shift+tab"]);
    assert_eq!(r.text(), "plain words");
    r.send(Event::Text('\t'));
    r.send(Event::Text('\u{7}'));
    assert_eq!(r.text(), "plain words");
}

// ───────────────────────── Home, End and the page keys ─────────────────────────

#[test]
fn home_and_end_go_to_the_ends_of_the_row() {
    let mut r = rig(&two_rows());
    put(&mut r, 0, 70);
    r.press("home");
    assert_eq!(head(&r), (0, 64));
    r.press("end");
    assert_eq!(head(&r), (0, 80));
    put(&mut r, 0, 10);
    r.press("end");
    assert_eq!(head(&r), (0, 63), "after the last word of a row that goes on");
    r.press("home");
    assert_eq!(head(&r), (0, 0));
    // The mark of a heading hangs outside the row.
    let mut r = rig("# Title");
    r.press("home");
    assert_eq!(head(&r), (0, 2));
    presses(&mut r, &["left", "left"]);
    assert_eq!(head(&r), (0, 0), "the mark is still text the arrows reach");
}

#[test]
fn ctrl_home_and_end_go_to_the_ends_of_the_text() {
    let mut r = rig(PROSE);
    r.press("ctrl+home");
    assert_eq!(head(&r), (0, 0));
    r.press("ctrl+end");
    assert_eq!(head(&r), (2, 22));
}

#[test]
fn page_up_and_down_move_one_screen_less_a_row() {
    let text: String = (0..300).map(|n| format!("line {n}\n")).collect();
    let mut r = rig(&text);
    put(&mut r, 0, 0);
    assert_eq!(r.app.grid().text_rows, 45);
    r.press("pagedown");
    assert_eq!(head(&r), (44, 0));
    r.press("pagedown");
    assert_eq!(head(&r), (88, 0));
    r.press("pageup");
    assert_eq!(head(&r), (44, 0));
    presses(&mut r, &["pageup", "pageup"]);
    assert_eq!(head(&r), (0, 0));
    r.press("shift+pagedown");
    assert_eq!(selected(&r).lines().count(), 44);
}

// ───────────────────────── undo and redo ─────────────────────────

#[test]
fn undo_takes_typing_back_a_word_at_a_time_and_redo_puts_it_back() {
    let mut r = Rig::sized(1920, 1080);
    r.type_text("one two three");
    r.press("ctrl+z");
    assert_eq!(r.text(), "one two ");
    r.press("ctrl+z");
    assert_eq!(r.text(), "one ");
    r.press("ctrl+y");
    assert_eq!(r.text(), "one two ");
    r.press("ctrl+shift+z");
    assert_eq!(r.text(), "one two three");
    assert_eq!(head(&r), (0, 13));
    r.press("ctrl+y");
    assert_eq!(r.text(), "one two three", "nothing more to redo");
    presses(&mut r, &["ctrl+z", "ctrl+z", "ctrl+z", "ctrl+z"]);
    assert_eq!(r.text(), "", "and nothing more to undo");
    // Typing after an undo forgets what could have been redone.
    r.press("ctrl+y");
    r.type_text("x");
    presses(&mut r, &["ctrl+z", "ctrl+z", "ctrl+y", "ctrl+y", "ctrl+y"]);
    assert_eq!(r.text(), "one x");
}

#[test]
fn undo_puts_the_cursor_back_where_the_edit_began() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 3);
    r.type_text("XX");
    put(&mut r, 2, 0);
    r.press("ctrl+z");
    assert_eq!(r.text(), PROSE);
    assert_eq!(head(&r), (0, 3));
    r.press("ctrl+y");
    assert_eq!(head(&r), (0, 5));
    // A pause of more than a second starts a new step.
    let mut r = Rig::sized(1920, 1080);
    r.type_text("ab");
    r.wait(1500);
    r.type_text("cd");
    r.press("ctrl+z");
    assert_eq!(r.text(), "ab");
}

#[test]
fn enter_and_its_undoing_each_undo_as_one_step() {
    let mut r = rig("abc");
    presses(&mut r, &["enter", "ctrl+z"]);
    assert_eq!(r.text(), "abc");
    presses(&mut r, &["ctrl+y", "backspace", "ctrl+z"]);
    assert_eq!(r.text(), "abc\n\n");
}

// ───────────────────────── the clipboard ─────────────────────────

#[test]
fn cut_copy_and_paste_go_through_the_clipboard_as_plain_text() {
    let mut r = rig("alpha beta");
    put(&mut r, 0, 0);
    for _ in 0..5 {
        r.press("shift+right");
    }
    r.press("ctrl+c");
    assert_eq!(r.host.clipboard.as_deref(), Some("alpha"));
    assert_eq!(r.text(), "alpha beta");
    r.host.clipboard = None;
    r.press("ctrl+x");
    assert_eq!(r.host.clipboard.as_deref(), Some("alpha"));
    assert_eq!(r.text(), " beta");
    presses(&mut r, &["ctrl+end", "ctrl+v", "ctrl+v"]);
    assert_eq!(r.text(), " betaalphaalpha");
    // Paste is one step of undo, and goes over a selection.
    r.press("ctrl+z");
    assert_eq!(r.text(), " betaalpha");
    presses(&mut r, &["ctrl+a", "ctrl+v"]);
    assert_eq!(r.text(), "alpha");
}

#[test]
fn line_breaks_cross_the_clipboard_both_ways() {
    let mut r = rig("a\nb");
    presses(&mut r, &["ctrl+a", "ctrl+c"]);
    assert_eq!(r.host.clipboard.as_deref(), Some("a\r\nb"), "as Windows writes them");
    // Whatever kind comes back is one kind in the text, tabs are kept, and
    // characters no key could type are left out.
    r.host.clipboard = Some(String::from("x\r\ny\rz\tw\u{0}\u{1b}!"));
    presses(&mut r, &["ctrl+a", "ctrl+v"]);
    assert_eq!(r.text(), "x\ny\nz\tw!");
    r.host.clipboard = None;
    r.press("ctrl+v");
    assert_eq!(r.text(), "x\ny\nz\tw!", "an empty clipboard pastes nothing");
}

#[test]
fn cut_and_copy_with_nothing_selected_say_so() {
    for chord in ["ctrl+c", "ctrl+x"] {
        let mut r = rig("alpha");
        r.host.clipboard = Some(String::from("kept"));
        r.press(chord);
        assert_eq!(r.host.clipboard.as_deref(), Some("kept"));
        assert_eq!(r.text(), "alpha");
        assert_eq!(said(&mut r), "Nothing selected. Shift with an arrow selects.");
        // The next key clears it.
        r.press("left");
        assert_eq!(said(&mut r), "");
    }
}

#[test]
fn ctrl_a_selects_everything() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 4);
    r.press("ctrl+a");
    assert_eq!(selected(&r), PROSE);
    assert_eq!(head(&r), (2, 22));
}

#[test]
fn typing_goes_over_a_selection_as_one_step() {
    let mut r = rig("alpha beta");
    put(&mut r, 0, 0);
    for _ in 0..5 {
        r.press("shift+right");
    }
    r.type_text("X");
    assert_eq!(r.text(), "X beta");
    assert_eq!(head(&r), (0, 1));
    r.press("ctrl+z");
    assert_eq!(r.text(), "alpha beta");
    assert_eq!(selected(&r), "alpha", "and the selection comes back with it");
}

// ───────────────────────── the rest of the table ─────────────────────────

#[test]
fn ctrl_t_goes_to_the_next_tk_and_round_again() {
    let mut r = rig("a TK b\n\nTKs are not it, but this is: TK");
    put(&mut r, 0, 0);
    r.press("ctrl+t");
    assert_eq!((head(&r), selected(&r)), ((0, 4), String::from("TK")));
    r.press("ctrl+t");
    assert_eq!((head(&r), selected(&r)), ((2, 31), String::from("TK")));
    r.press("ctrl+t");
    assert_eq!(head(&r), (0, 4), "round to the first");
    // It is selected, so what is typed next takes its place.
    r.type_text("1962");
    assert_eq!(r.text().lines().next(), Some("a 1962 b"));

    let mut r = rig("nothing to come here");
    r.press("ctrl+t");
    assert_eq!(said(&mut r), "Nothing still to come.");
    assert_eq!(head(&r), (0, 20));
}

#[test]
fn ctrl_equals_and_minus_step_the_scale_between_one_and_three() {
    let mut r = rig(PROSE);
    assert_eq!((r.app.grid().scale, r.app.grid().cols), (1, 160));
    r.press("ctrl+=");
    assert_eq!((r.app.grid().scale, r.app.grid().cols, r.app.grid().rows), (2, 80, 24));
    assert_eq!(r.wants().cell, (24, 44));
    r.press("ctrl+shift+=");
    assert_eq!(r.app.grid().scale, 3);
    r.press("ctrl+=");
    assert_eq!(r.app.grid().scale, 3);
    presses(&mut r, &["ctrl+-", "ctrl+-", "ctrl+-"]);
    assert_eq!((r.app.grid().scale, r.wants().cell), (1, (12, 22)));
    assert_eq!(r.text(), PROSE);
}

#[test]
fn f11_asks_for_a_window_and_ctrl_q_asks_to_leave() {
    let mut r = rig(PROSE);
    assert!(r.wants().fullscreen && !r.wants().quit);
    r.press("f11");
    assert!(!r.wants().fullscreen);
    r.press("f11");
    assert!(r.wants().fullscreen);
    r.press("ctrl+q");
    assert!(r.wants().quit);
    let mut r = rig(PROSE);
    r.press("alt+f4");
    assert!(r.wants().quit);
    // Closing the window from outside asks nothing either.
    let mut r = rig(PROSE);
    r.send(Event::Close);
    assert_eq!(r.text(), PROSE);
}

#[test]
fn the_keys_of_the_line_and_the_session_arrive_without_touching_the_text() {
    for chord in ["esc", "ctrl+f", "f3", "shift+f3", "f1", "ctrl+s", "ctrl+enter"] {
        let mut r = rig(PROSE);
        put(&mut r, 0, 4);
        r.press(chord);
        assert_eq!(r.own_text(), PROSE, "{chord}");
    }
    // Esc goes to the line and Esc comes back to the same cell, and a
    // letter is a letter again: the line is a place, not a mode.
    let mut r = rig(PROSE);
    put(&mut r, 0, 4);
    r.press("esc");
    r.type_text("q");
    assert_eq!(r.own_text(), PROSE);
    r.press("esc");
    assert_eq!(head(&r), (0, 4));
    assert_eq!(selected(&r), "");
    r.type_text("x");
    assert_eq!(r.own_text().lines().next(), Some("One xtwo. Three four five."));
}

#[test]
fn nothing_is_bound_on_ctrl_with_alt() {
    let mut r = rig(PROSE);
    put(&mut r, 0, 4);
    let both = Mods { ctrl: true, alt: true, shift: false };
    for key in [Key::Left, Key::Backspace, Key::Enter, Key::Char('q'), Key::Char('z'), Key::Up, Key::F(4)] {
        r.send(Event::Key { key, mods: both, repeat: false });
    }
    assert_eq!(r.text(), PROSE);
    assert_eq!(head(&r), (0, 4));
    assert!(!r.wants().quit);
}

#[test]
fn a_letter_is_a_letter_from_the_first_frame() {
    let mut r = Rig::new(Default::default());
    // Before the window even has a size.
    r.type_text("a");
    r.size(1920, 1080);
    r.type_text("b");
    assert_eq!(r.text(), "ab");
    for ch in ['j', 'k', ':', 'i', '/', '#', ' ', 'q'] {
        r.send(Event::Text(ch));
    }
    assert_eq!(r.text(), "abjk:i/# q");
}

// ───────────────────────── holding a key, and the pointer ─────────────────────────

#[test]
fn holding_ctrl_or_alt_alone_for_half_a_second_explains_it() {
    let mut r = rig(PROSE);
    r.hold(Mods::CTRL);
    assert_eq!(r.wants().wake_in_ms, Some(500));
    r.wait(499);
    assert_eq!(said(&mut r), "");
    r.wait(1);
    assert_eq!(said(&mut r), "Ctrl moves by word and by paragraph, and deletes by word.");
    r.wait(5000);
    assert_eq!(said(&mut r), "Ctrl moves by word and by paragraph, and deletes by word.", "until it is let go");
    r.release();
    assert_eq!(said(&mut r), "");
    assert_eq!(r.wants().wake_in_ms, None);

    r.hold(Mods::ALT);
    r.wait(500);
    assert_eq!(said(&mut r), "Alt moves and deletes by sentence, and carries the paragraph.");
    r.release();
    // Shift is not asked about, and neither is a chord.
    r.hold(Mods::SHIFT);
    r.wait(2000);
    assert_eq!(said(&mut r), "");
    r.release();
    r.hold(Mods::CTRL);
    r.wait(200);
    r.press("right");
    r.wait(2000);
    assert_eq!(said(&mut r), "", "a key was pressed with it, so it was not a question");
    r.release();
}

#[test]
fn the_pointer_hides_at_the_first_keystroke_and_returns_when_moved() {
    let mut r = rig(PROSE);
    assert!(!r.wants().pointer_hidden);
    r.type_text("a");
    assert!(r.wants().pointer_hidden);
    r.send(Event::Mouse { kind: MouseKind::Move, x: 10, y: 10, mods: Mods::NONE });
    assert!(!r.wants().pointer_hidden);
    r.press("left");
    assert!(r.wants().pointer_hidden);
    r.wheel(1);
    assert!(!r.wants().pointer_hidden);
}

#[test]
fn a_text_open_for_reading_only_takes_no_edits() {
    let mut r = rig(PROSE);
    r.app.page_mut().read_only = true;
    r.type_text("x\n");
    presses(&mut r, &["backspace", "delete", "ctrl+backspace", "alt+up", "tab", "ctrl+v", "ctrl+z", "ctrl+y", "ctrl+a", "ctrl+x"]);
    assert_eq!(r.text(), PROSE);
    r.press("ctrl+home");
    assert_eq!(head(&r), (0, 0), "but the cursor still moves");
}
