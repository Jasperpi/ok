//! Enter, Backspace, Tab and the paragraph carry as exact pairs: the text
//! and the cursor before a key, and the text and the cursor after it.
//!
//! A pair is written with `|` where the cursor is and `^` where a selection
//! began. Each table holds what DESIGN.md's Keys section says for one key,
//! at the first and the last line, in lists, quotes, fenced code and
//! headings, and with a selection. A table reports every row that fails,
//! not only the first. The rows that fail today have been moved to
//! `attack_found.rs`, each as a test of its own.

mod attack_support;

use ok_text::{Pos, Span};
use ok_ui::harness::Rig;

/// Reads a marked text into the text itself, the cursor and where the
/// selection began.
fn unmark(marked: &str) -> (String, Pos, Pos) {
    let mut text = String::new();
    let (mut line, mut col) = (0, 0);
    let (mut head, mut anchor) = (None, None);
    for ch in marked.chars() {
        match ch {
            '|' => head = Some(Pos::new(line, col)),
            '^' => anchor = Some(Pos::new(line, col)),
            '\n' => {
                text.push(ch);
                line += 1;
                col = 0;
            }
            ch => {
                text.push(ch);
                col += ch.len_utf8();
            }
        }
    }
    let head = head.expect("a pair marks the cursor with |");
    (text, head, anchor.unwrap_or(head))
}

/// A full-screen rig holding a marked text.
fn load(marked: &str) -> Rig {
    let (text, head, anchor) = unmark(marked);
    let mut rig = Rig::with_text(&text, 1920, 1080);
    rig.app.page_mut().select(Span { start: anchor, end: head });
    rig
}

/// The rig's text, marked the same way.
fn marked(rig: &Rig) -> String {
    let cursor = rig.app.page().cursor;
    let mut out = String::new();
    for (number, line) in rig.app.page().doc().lines().iter().enumerate() {
        if number > 0 {
            out.push('\n');
        }
        let mut marks = vec![];
        if cursor.head.line == number {
            marks.push((cursor.head.col, '|'));
        }
        if cursor.anchor.line == number && cursor.anchor != cursor.head {
            marks.push((cursor.anchor.col, '^'));
        }
        marks.sort();
        let mut from = 0;
        for (at, mark) in marks {
            out.push_str(&line[from..at]);
            out.push(mark);
            from = at;
        }
        out.push_str(&line[from..]);
    }
    out
}

/// Presses the chords of `keys`, separated by spaces, and gives the result.
fn after(before: &str, keys: &str) -> String {
    let mut rig = load(before);
    for chord in keys.split_whitespace() {
        rig.press(chord);
    }
    marked(&rig)
}

/// Checks a table of (before, keys, after) and reports every row that is
/// not as written.
fn table(what: &str, rows: &[(&str, &str, &str)]) {
    let mut wrong = Vec::new();
    for &(before, keys, want) in rows {
        let got = after(before, keys);
        if got != want {
            wrong.push(format!("  {before:?} then {keys}\n      gives {got:?}\n      wants {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{what}: {} of {} pairs are wrong\n{}", wrong.len(), rows.len(), wrong.join("\n"));
}

// ───────────────────────── Enter ─────────────────────────

/// "Ends the paragraph and writes the blank line between paragraphs."
#[test]
fn enter_in_a_paragraph_a_heading_a_quote_and_a_rule() {
    table(
        "Enter",
        &[
            ("abc|def", "enter", "abc\n\n|def"),
            ("abc|", "enter", "abc\n\n|"),
            ("|abc", "enter", "\n\n|abc"),
            // The first line of several, the last line, and one between.
            ("one|\n\ntwo\n\nthree", "enter", "one\n\n|\n\ntwo\n\nthree"),
            ("one\n\ntwo\n\nthree|", "enter", "one\n\ntwo\n\nthree\n\n|"),
            ("one\n\ntw|o\n\nthree", "enter", "one\n\ntw\n\n|o\n\nthree"),
            // A paragraph of two lines is cut where the cursor is.
            ("one\ntw|o", "enter", "one\ntw\n\n|o"),
            // No whitespace is trimmed on either side.
            ("abc |def", "enter", "abc \n\n|def"),
            ("abc| def", "enter", "abc\n\n| def"),
            ("# Title|", "enter", "# Title\n\n|"),
            ("# Ti|tle", "enter", "# Ti\n\n|tle"),
            ("#| Title", "enter", "#\n\n| Title"),
            ("## Deeper|\n\nbody", "enter", "## Deeper\n\n|\n\nbody"),
            ("> quoted|", "enter", "> quoted\n\n|"),
            ("> quo|ted", "enter", "> quo\n\n|ted"),
            ("---|", "enter", "---\n\n|"),
            ("\u{4E00}|\u{4E8C}", "enter", "\u{4E00}\n\n|\u{4E8C}"),
            ("e\u{0301}|x", "enter", "e\u{0301}\n\n|x"),
        ],
    );
}

/// "In a list, the next item; on an empty item, ends the list."
#[test]
fn enter_in_a_list() {
    table(
        "Enter in a list",
        &[
            ("- one|", "enter", "- one\n- |"),
            ("* one|", "enter", "* one\n* |"),
            ("+ one|", "enter", "+ one\n+ |"),
            ("- one| two", "enter", "- one\n- | two"),
            ("- one\n- two|\n- three", "enter", "- one\n- two\n- |\n- three"),
            ("  - nested|", "enter", "  - nested\n  - |"),
            ("1. first|", "enter", "1. first\n2. |"),
            ("9) nine|", "enter", "9) nine\n10) |"),
            ("- [ ] to do|", "enter", "- [ ] to do\n- [ ] |"),
            ("- [x] done|", "enter", "- [x] done\n- [ ] |"),
            ("- [X] done|", "enter", "- [X] done\n- [ ] |"),
            // The first and the last line of the text.
            ("- one|\n\nafter", "enter", "- one\n- |\n\nafter"),
            ("before\n\n- one|", "enter", "before\n\n- one\n- |"),
            // An empty item ends the list: the marker goes.
            ("- one\n- |", "enter", "- one\n\n|"),
            ("- one\n- |\nafter", "enter", "- one\n\n|\nafter"),
            ("- one\n  - |", "enter", "- one\n\n|"),
            ("1. one\n2. |", "enter", "1. one\n\n|"),
            ("- [ ] one\n- [ ] |", "enter", "- [ ] one\n\n|"),
            // Before the marker and inside it the line is only broken.
            ("|- one", "enter", "\n|- one"),
            ("-| one", "enter", "-\n| one"),
        ],
    );
}

/// "On a blank line or in fenced code, one line break."
#[test]
fn enter_on_a_blank_line_and_in_fenced_code() {
    table(
        "Enter on a blank line or in code",
        &[
            ("|", "enter", "\n|"),
            ("abc\n|", "enter", "abc\n\n|"),
            ("abc\n|\ndef", "enter", "abc\n\n|\ndef"),
            ("abc\n\n|\n\ndef", "enter", "abc\n\n\n|\n\ndef"),
            ("   |", "enter", "   \n|"),
            ("```\ncode|\n```", "enter", "```\ncode\n|\n```"),
            ("```\nco|de\n```", "enter", "```\nco\n|de\n```"),
            ("```|", "enter", "```\n|"),
            ("```\ncode\n```|", "enter", "```\ncode\n```\n|"),
            ("```\n- not an item|\n```", "enter", "```\n- not an item\n|\n```"),
            ("```\n# not a heading|\n```", "enter", "```\n# not a heading\n|\n```"),
            ("~~~\ncode|", "enter", "~~~\ncode\n|"),
            // A fence left open turns everything below it into code.
            ("```\none\n\ntwo|", "enter", "```\none\n\ntwo\n|"),
            // After the closing fence it is prose again.
            ("```\ncode\n```\nprose|", "enter", "```\ncode\n```\nprose\n\n|"),
        ],
    );
}

/// Enter with a selection: what is selected goes, and the key then does
/// what it does where the selection began.
#[test]
fn enter_with_a_selection() {
    table(
        "Enter with a selection",
        &[
            ("ab^cd|ef", "enter", "ab\n\n|ef"),
            ("ab|cd^ef", "enter", "ab\n\n|ef"),
            ("one ^two\n\nthree| four", "enter", "one \n\n| four"),
            ("^abc|", "enter", "\n\n|"),
            ("- a^bc|d", "enter", "- a\n- |d"),
            ("- one\n- ^two|", "enter", "- one\n- \n- |"),
            ("- ^one\n- two|", "enter", "- \n- |"),
            ("# T^itl|e", "enter", "# T\n\n|e"),
            ("> q^uo|te", "enter", "> q\n\n|te"),
            ("```\na^b|c\n```", "enter", "```\na\n|c\n```"),
            ("^\nabc|def", "enter", "\n|def"),
            ("abc^def|", "shift+enter", "abc\n|"),
        ],
    );
}

/// "A line break inside the paragraph."
#[test]
fn shift_enter_everywhere() {
    table(
        "Shift Enter",
        &[
            ("abc|def", "shift+enter", "abc\n|def"),
            ("|abc", "shift+enter", "\n|abc"),
            ("abc|", "shift+enter", "abc\n|"),
            ("# Ti|tle", "shift+enter", "# Ti\n|tle"),
            ("> quo|ted", "shift+enter", "> quo\n|ted"),
            ("- one|", "shift+enter", "- one\n|"),
            ("- one\n- |", "shift+enter", "- one\n- \n|"),
            ("```\ncode|", "shift+enter", "```\ncode\n|"),
            ("abc\n|\ndef", "shift+enter", "abc\n\n|\ndef"),
        ],
    );
}

// ───────────────────────── Backspace ─────────────────────────

/// "Undoes what Enter did: removes the blank line and the break together."
#[test]
fn backspace_at_the_start_of_a_paragraph() {
    table(
        "Backspace at the start of a paragraph",
        &[
            ("abc\n\n|def", "backspace", "abc|def"),
            ("abc\n\n|", "backspace", "abc|"),
            ("one\n\ntwo\n\n|three", "backspace", "one\n\ntwo|three"),
            ("one\n\n|two\n\nthree", "backspace", "one|two\n\nthree"),
            ("# Title\n\n|body", "backspace", "# Title|body"),
            ("> quoted\n\n|body", "backspace", "> quoted|body"),
            ("- one\n\n|body", "backspace", "- one|body"),
            ("body\n\n|# Title", "backspace", "body|# Title"),
            ("body\n\n|- item", "backspace", "body|- item"),
            // With Ctrl and with Alt it is the same act.
            ("abc\n\n|def", "ctrl+backspace", "abc|def"),
            ("abc\n\n|def", "alt+backspace", "abc|def"),
            // The first line has nothing before it.
            ("|abc", "backspace", "|abc"),
            ("|abc", "ctrl+backspace", "|abc"),
            ("|", "backspace", "|"),
            // A break inside a paragraph is one break.
            ("abc\n|def", "backspace", "abc|def"),
            // More blank lines than Enter writes go one at a time.
            ("abc\n\n\n|def", "backspace", "abc\n\n|def"),
            // An item under an item: the break and the marker Enter wrote.
            ("- one\n- |", "backspace", "- one|"),
            ("- one\n- |two", "backspace", "- one|two"),
            ("1. one\n2. |", "backspace", "1. one|"),
            ("- [ ] one\n- [ ] |", "backspace", "- [ ] one|"),
            ("  - one\n  - |two", "backspace", "  - one|two"),
            // The first item of a list has nothing to join: its marker is text.
            ("- |one", "backspace", "-|one"),
            ("para\n- |one", "backspace", "para\n-|one"),
            // In fenced code a line break is a line break.
            ("```\none\n|two\n```", "backspace", "```\none|two\n```"),
            ("```\none\n\n|two\n```", "backspace", "```\none\n|two\n```"),
            // A selection is what goes, wherever it is.
            ("abc\n\n^def|", "backspace", "abc\n\n|"),
            ("a^bc\n\nde|f", "backspace", "a|f"),
        ],
    );
}

/// Enter and then Backspace is where it started, text and cursor, wherever
/// in a paragraph, a heading, a quote or an item the cursor was. (The very
/// start of a paragraph is a case of its own; see `attack_found.rs`.)
#[test]
fn enter_then_backspace_is_where_it_started() {
    let texts = ["The orchard was eleven acres.", "# The orchard", "## What he did", "> It felt like waste.", "- thinned the fruit", "  - [x] a nested one", "12. the twelfth", "one\ntwo words here\nthree", "caf\u{00E9} \u{4E00}\u{4E8C} e\u{0301}x"];
    let mut wrong = Vec::new();
    for text in texts {
        let lines: Vec<&str> = text.split('\n').collect();
        for (line, content) in lines.iter().enumerate() {
            let first = attack_support::marker(content).map_or(1, |m| m.text);
            for col in (first..=content.len()).filter(|&col| content.is_char_boundary(col)) {
                if !attack_support::on_cluster_boundary(content, col) || content[..col].trim().is_empty() {
                    continue;
                }
                let mut rig = Rig::with_text(text, 1920, 1080);
                rig.app.page_mut().place(Pos::new(line, col), false);
                let before = marked(&rig);
                rig.press("enter");
                let between = marked(&rig);
                rig.press("backspace");
                if marked(&rig) != before {
                    wrong.push(format!("  {before:?} then enter gives {between:?} then backspace gives {:?}", marked(&rig)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} places where Backspace does not undo Enter\n{}", wrong.len(), wrong.join("\n"));
}

// ───────────────────────── Tab ─────────────────────────

/// "In a list, nests the item or brings it back by two spaces. ...
/// Otherwise nothing: no tab characters."
#[test]
fn tab_and_shift_tab() {
    table(
        "Tab",
        &[
            ("- one\n- tw|o", "tab", "- one\n  - tw|o"),
            ("- one\n  - tw|o", "tab", "- one\n    - tw|o"),
            ("- one\n  - tw|o", "shift+tab", "- one\n- tw|o"),
            ("- one\n- tw|o", "shift+tab", "- one\n- tw|o"),
            (" - on|e", "shift+tab", "- on|e"),
            // The first and the last line.
            ("- on|e\n\nafter", "tab", "  - on|e\n\nafter"),
            ("before\n\n- on|e", "tab", "before\n\n  - on|e"),
            ("1. fir|st", "tab", "  1. fir|st"),
            ("- [ ] to d|o", "tab", "  - [ ] to d|o"),
            ("- |", "tab", "  - |"),
            // The cursor in the indentation and on the marker.
            ("|- one", "tab", "  |- one"),
            ("  |- one", "shift+tab", "|- one"),
            (" | - one", "shift+tab", "|- one"),
            // Every item a selection touches, and only the items.
            ("- ^one\n- two\n- thr|ee", "tab", "  - ^one\n  - two\n  - thr|ee"),
            ("pa^ra\n- one\n- tw|o", "tab", "pa^ra\n  - one\n  - tw|o"),
            ("  - ^one\n- two\n    - thr|ee", "shift+tab", "- ^one\n- two\n  - thr|ee"),
            // Outside a list: nothing, and never a tab character.
            ("pla|in", "tab", "pla|in"),
            ("pla|in", "shift+tab", "pla|in"),
            ("# Ti|tle", "tab", "# Ti|tle"),
            ("> quo|te", "tab", "> quo|te"),
            ("|", "tab", "|"),
            ("  indented| prose", "shift+tab", "  indented| prose"),
            ("```\n- not an it|em\n```", "tab", "```\n- not an it|em\n```"),
            ("```\n  - not an it|em\n```", "shift+tab", "```\n  - not an it|em\n```"),
            ("a^b|c", "tab", "a^b|c"),
            // With Ctrl or Alt it is not this key.
            ("- on|e", "ctrl+tab", "- on|e"),
            ("- on|e", "alt+tab", "- on|e"),
        ],
    );
}

/// Tab and then Shift Tab is where it started, at every place in an item.
#[test]
fn tab_then_shift_tab_is_where_it_started() {
    let mut wrong = Vec::new();
    for text in ["- one", "  - [ ] nested box", "10. tenth", "para\n- one\n- two\nmore"] {
        let lines: Vec<&str> = text.split('\n').collect();
        for (line, content) in lines.iter().enumerate() {
            for col in 0..=content.len() {
                let mut rig = Rig::with_text(text, 1920, 1080);
                rig.app.page_mut().place(Pos::new(line, col), false);
                let before = marked(&rig);
                rig.press("tab");
                rig.press("shift+tab");
                if marked(&rig) != before {
                    wrong.push(format!("  {before:?} gives {:?}", marked(&rig)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} places where Shift Tab does not undo Tab\n{}", wrong.len(), wrong.join("\n"));
}

// ───────────────────────── carrying the paragraph ─────────────────────────

/// "Alt ... carries the paragraph up or down."
#[test]
fn alt_up_and_down_carry_the_paragraph() {
    table(
        "Alt Up and Alt Down",
        &[
            ("First.\n\nSec|ond.\n\nThird.", "alt+up", "Sec|ond.\n\nFirst.\n\nThird."),
            ("First.\n\nSec|ond.\n\nThird.", "alt+down", "First.\n\nThird.\n\nSec|ond."),
            // The first paragraph has nothing above it, the last nothing below.
            ("Fir|st.\n\nSecond.", "alt+up", "Fir|st.\n\nSecond."),
            ("First.\n\nSec|ond.", "alt+down", "First.\n\nSec|ond."),
            ("on|ly", "alt+up", "on|ly"),
            ("on|ly", "alt+down", "on|ly"),
            // A paragraph of several lines goes whole and the cursor keeps
            // its line of it.
            ("one\n\ntwo a\ntwo| b\ntwo c\n\nthree", "alt+up", "two a\ntwo| b\ntwo c\n\none\n\nthree"),
            ("one\n\ntwo a\ntwo| b\ntwo c\n\nthree", "alt+down", "one\n\nthree\n\ntwo a\ntwo| b\ntwo c"),
            // The gap between two paragraphs stays where it is.
            ("one\n\n\n\ntw|o", "alt+up", "tw|o\n\n\n\none"),
            ("on|e\n \ntwo", "alt+down", "two\n \non|e"),
            // A heading, a quote and a list are each carried like a paragraph.
            ("body\n\n# Tit|le", "alt+up", "# Tit|le\n\nbody"),
            ("> quo|te\n\nbody", "alt+down", "body\n\n> quo|te"),
            ("body\n\n- one\n- tw|o\n- three", "alt+up", "- one\n- tw|o\n- three\n\nbody"),
            ("- one\n- tw|o\n\nbody", "alt+down", "body\n\n- one\n- tw|o"),
            // A heading with its paragraph straight under it is one paragraph.
            ("first\n\n# Title\nbod|y", "alt+up", "# Title\nbod|y\n\nfirst"),
            // A whole fenced block with no blank line in it is one paragraph.
            ("body\n\n```\nco|de\n```", "alt+up", "```\nco|de\n```\n\nbody"),
            // On a blank line there is no paragraph to carry.
            ("one\n|\ntwo", "alt+up", "one\n|\ntwo"),
            ("one\n|\ntwo", "alt+down", "one\n|\ntwo"),
            ("|\n\nonly", "alt+down", "|\n\nonly"),
            // Blank lines at the ends of the text stay at the ends.
            ("\none\n\ntw|o\n", "alt+up", "\ntw|o\n\none\n"),
            // The cursor at the very start and the very end of its paragraph.
            ("one\n\n|two", "alt+up", "|two\n\none"),
            ("one|\n\ntwo", "alt+down", "two\n\none|"),
            // With Shift too it is the same key.
            ("one\n\ntw|o", "alt+shift+up", "tw|o\n\none"),
        ],
    );
}

/// Carrying a paragraph up and then down, or down and then up, is where it
/// started, and each carry is taken back by one Ctrl Z.
#[test]
fn a_carry_goes_back_the_way_it_came_and_is_one_step_of_undo() {
    let text = "# Title\n\nFirst paragraph\nof two lines.\n\n\n- an item\n- another\n\n> a quote\n\nLast.";
    let lines: Vec<&str> = text.split('\n').collect();
    let mut wrong = Vec::new();
    for (line, content) in lines.iter().enumerate() {
        for col in [0, content.len() / 2, content.len()] {
            for (there, back) in [("alt+up", "alt+down"), ("alt+down", "alt+up")] {
                let mut rig = Rig::with_text(text, 1920, 1080);
                rig.app.page_mut().place(Pos::new(line, col), false);
                let before = marked(&rig);
                rig.press(there);
                let carried = marked(&rig);
                if carried == before {
                    continue;
                }
                rig.press(back);
                if marked(&rig) != before {
                    wrong.push(format!("  {before:?} then {there} then {back} gives {:?}", marked(&rig)));
                }
                rig.press("ctrl+z");
                if marked(&rig) != carried {
                    wrong.push(format!("  {before:?} then {there} {back} ctrl+z gives {:?}, wants {carried:?}", marked(&rig)));
                }
                rig.press("ctrl+z");
                if marked(&rig) != before {
                    wrong.push(format!("  {before:?} then {there} {back} ctrl+z ctrl+z gives {:?}", marked(&rig)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} carries that do not go back\n{}", wrong.len(), wrong.join("\n"));
}

// ───────────────────────── each key is one step of undo ─────────────────────────

/// Whatever Enter, Backspace, Delete, Tab or a carry did at any place in a
/// text of every kind of line, with or without a selection, one Ctrl Z
/// puts the text, the cursor and the selection back exactly, and Ctrl Y
/// makes it again.
#[test]
fn every_one_of_these_keys_is_undone_exactly_by_one_ctrl_z() {
    let text = "# Title\n\nA paragraph. Two sentences.\nsecond line\n\n- item\n- \n  - [ ] nested\n1. numbered\n\n> quote\n\n```\ncode\n\n- x\n```\n\n\nlast";
    let lines: Vec<&str> = text.split('\n').collect();
    let places: Vec<Pos> = lines.iter().enumerate().flat_map(|(line, content)| (0..=content.len()).map(move |col| Pos::new(line, col))).collect();
    let mut wrong = Vec::new();
    let mut tried = 0;
    for (number, &head) in places.iter().enumerate() {
        // No selection, and two selections: a short one and one that
        // reaches a few lines on.
        for anchor in [head, places[(number + 3) % places.len()], places[(number + 41) % places.len()]] {
            for key in ["enter", "shift+enter", "backspace", "delete", "ctrl+backspace", "alt+backspace", "ctrl+delete", "alt+delete", "tab", "shift+tab", "alt+up", "alt+down"] {
                let mut rig = Rig::with_text(text, 1920, 1080);
                rig.app.page_mut().select(Span { start: anchor, end: head });
                let before = marked(&rig);
                rig.press(key);
                let done = marked(&rig);
                if rig.text() == text {
                    continue;
                }
                tried += 1;
                rig.press("ctrl+z");
                if marked(&rig) != before {
                    wrong.push(format!("  {before:?} then {key} then ctrl+z gives {:?}", marked(&rig)));
                    continue;
                }
                rig.press("ctrl+y");
                if marked(&rig) != done {
                    wrong.push(format!("  {before:?} then {key} ctrl+z ctrl+y gives {:?}, wants {done:?}", marked(&rig)));
                }
            }
        }
    }
    assert!(tried > 2000, "only {tried} edits were tried");
    wrong.truncate(12);
    assert!(wrong.is_empty(), "edits that one Ctrl Z does not take back exactly\n{}", wrong.join("\n"));
}
