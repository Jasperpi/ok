//! The demo, driven through the real window procedure and message loop in a
//! window that is cloaked and never activated. What these tests assert on is
//! the frame the demo drew: the typed text and the bottom row, which names
//! the last event the demo was given, are read back out of its pixels.
#![cfg(windows)]

mod support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ok_host::{Key, Mods};
use ok_win::{Icon, Options, SelfTest, Step, TickSource, EXIT_CRASH};
use support::demo::{Layout, CELL, DESK, MIN_CELLS, OK_AFTER_MS, PAPER};
use support::{folder, run, run_with, Screen, BLOCK, BOX};

const CTRL_SHIFT: Mods = Mods { ctrl: true, alt: false, shift: true };
const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false };

fn text(s: &str) -> Step {
    Step::Type(s.to_string())
}

#[test]
fn typed_characters_appear() {
    let report = run("typed", vec![text("Sphinx of black quartz, judge my vow.")]);
    let screen = Screen::of(&report);
    assert_eq!(screen.text_row(0), format!("Sphinx of black quartz, judge my vow.{BLOCK}"));
    assert_eq!(screen.last_event(), "Text .");
    assert_eq!(report.exit_code, 0);
    assert_eq!((report.width, report.height), (960, 528), "the frame is the client area");
    assert!(report.paints >= 1, "the first show was painted from the frame already drawn");
    println!("drawn {} times for 37 characters, {} paint messages", report.renders, report.paints);
    // Two colours and nothing else outside the letters: a corner of desk, a corner of sheet.
    assert_eq!(screen.pixel(0, 0), Some(DESK));
    let sheet = screen.layout.sheet();
    assert_eq!(screen.pixel(sheet.x + 1, sheet.y + 1), Some(PAPER));
}

#[test]
fn shifted_and_punctuation_keys_type_what_the_layout_says() {
    let report = run("shifted", vec![text("A1! b2@ {c} <d> \"e\" ~ | \\ ?")]);
    assert_eq!(Screen::of(&report).typed(), "A1! b2@ {c} <d> \"e\" ~ | \\ ?");
}

#[test]
fn control_codes_from_ctrl_letters_are_not_inserted() {
    // Each of these chords makes Windows post a control character as well:
    // Ctrl M is 0x0D, Ctrl H 0x08, Ctrl I 0x09, Ctrl J 0x0A, Ctrl [ 0x1B.
    let mut script = vec![text("ab")];
    for key in ['s', 'm', 'h', 'i', 'j', '[', ']', '\\', 'a', 'z'] {
        script.push(Step::Chord(Key::Char(key), Mods::CTRL));
    }
    script.push(Step::Chord(Key::Char('2'), CTRL_SHIFT));
    script.push(Step::Chord(Key::Char('6'), CTRL_SHIFT));
    script.push(text("c"));
    let report = run("control-codes", script);
    let screen = Screen::of(&report);
    assert_eq!(screen.text_row(0), format!("abc{BLOCK}"), "nothing but the three letters was inserted");
    assert_eq!(screen.text_row(1), "", "no line was broken by Ctrl M or Ctrl J");
}

#[test]
fn a_chord_arrives_as_a_key_with_its_modifiers() {
    let screen = Screen::of(&run("chord", vec![Step::Chord(Key::Char('s'), Mods::CTRL), Step::Hold(Mods::CTRL), Step::Press(Key::Char('s'))]));
    assert_eq!(screen.last_event(), "Key Ctrl S");
    let screen = Screen::of(&run("chord-shift", vec![Step::Hold(CTRL_SHIFT), Step::Press(Key::Char('z'))]));
    assert_eq!(screen.last_event(), "Key Ctrl Shift Z");
    let screen = Screen::of(&run("chord-equals", vec![Step::Hold(Mods::CTRL), Step::Press(Key::Char('='))]));
    assert_eq!(screen.last_event(), "Key Ctrl =");
    let screen = Screen::of(&run("chord-alt", vec![Step::Hold(Mods::ALT), Step::Press(Key::Left)]));
    assert_eq!(screen.last_event(), "Key Alt Left");
    assert_eq!(screen.typed(), "");
}

#[test]
fn ctrl_space_is_a_chord_and_types_no_space() {
    let report = run("ctrl-space", vec![text("a"), Step::Hold(Mods::CTRL), Step::Press(Key::Char(' ')), Step::Hold(Mods::NONE), text("b")]);
    assert_eq!(Screen::of(&report).typed(), "ab");
}

#[test]
fn an_alt_chord_is_a_key_and_its_character_is_swallowed() {
    let report = run("alt-chord", vec![text("a"), Step::Hold(Mods::ALT), Step::Press(Key::Char('x'))]);
    let screen = Screen::of(&report);
    assert_eq!(screen.last_event(), "Key Alt X");
    assert_eq!(screen.typed(), "a");
}

#[test]
fn ctrl_with_alt_sends_no_key() {
    let report = run("altgr", vec![text("a"), Step::Hold(CTRL_ALT), Step::Press(Key::Char('e')), Step::Press(Key::Left), Step::Press(Key::Enter)]);
    let screen = Screen::of(&report);
    assert_eq!(screen.last_event(), "Holding Ctrl Alt", "the modifiers were reported, and no key after them");
    assert_eq!(screen.text_row(0), format!("a{BLOCK}"));
}

#[test]
fn modifiers_are_reported_going_down_and_coming_up() {
    assert_eq!(Screen::of(&run("mods-ctrl", vec![Step::Hold(Mods::CTRL)])).last_event(), "Holding Ctrl");
    assert_eq!(Screen::of(&run("mods-shift", vec![Step::Hold(Mods::SHIFT)])).last_event(), "Holding Shift");
    assert_eq!(Screen::of(&run("mods-both", vec![Step::Hold(CTRL_SHIFT)])).last_event(), "Holding Ctrl Shift");
    assert_eq!(Screen::of(&run("mods-up", vec![Step::Hold(Mods::CTRL), Step::Hold(Mods::NONE)])).last_event(), "Holding nothing");
}

#[test]
fn named_keys_arrive_as_keys_and_repeat_is_marked() {
    let named = [
        (Key::Left, "Key Left"),
        (Key::PageDown, "Key Page Down"),
        (Key::Home, "Key Home"),
        (Key::Delete, "Key Delete"),
        (Key::Tab, "Key Tab"),
        (Key::F(1), "Key F1"),
        (Key::F(12), "Key F12"),
    ];
    for (key, words) in named {
        let report = run(&format!("named-{words}"), vec![Step::Press(key)]);
        assert_eq!(Screen::of(&report).last_event(), words);
    }
    let report = run("named-repeat", vec![Step::Press(Key::Down), Step::Repeat(Key::Down)]);
    assert_eq!(Screen::of(&report).last_event(), "Key Down, repeating");
    let report = run("named-shift-tab", vec![Step::Chord(Key::Tab, Mods::SHIFT), Step::Hold(Mods::SHIFT), Step::Press(Key::Tab)]);
    assert_eq!(Screen::of(&report).last_event(), "Key Shift Tab");
}

#[test]
fn enter_tab_backspace_and_escape_come_from_the_key_not_the_character() {
    // Each of these also produces a character message (0x0D, 0x09, 0x08, 0x1B).
    let report = run("editing-keys", vec![text("abc"), Step::Press(Key::Backspace), Step::Press(Key::Tab), Step::Press(Key::Enter), text("d")]);
    let screen = Screen::of(&report);
    assert_eq!(screen.text_row(0), "ab");
    assert_eq!(screen.text_row(1), format!("d{BLOCK}"));
    let report = run("editing-escape", vec![text("abc"), Step::Press(Key::Escape), text("z")]);
    assert_eq!(Screen::of(&report).typed(), "z");
}

#[test]
fn a_surrogate_pair_arrives_as_one_character() {
    // U+1F600 as its two UTF-16 halves, then a letter.
    let report = run("surrogates", vec![Step::Chars(vec![0xD83D, 0xDE00]), Step::Wait(50), Step::Chars(vec![b'x' as u16])]);
    let screen = Screen::of(&report);
    assert_eq!(screen.text_row(0), format!("{BOX}x{BLOCK}"), "one cell for the pair, then the letter");
    let report = run("surrogates-said", vec![Step::Chars(vec![0xD83D, 0xDE00])]);
    assert_eq!(Screen::of(&report).last_event(), "Text U+1F600");
    assert_eq!(report.title, "ok demo, 1 character");
}

#[test]
fn half_a_surrogate_pair_is_dropped_and_typing_goes_on() {
    let report = run("orphans", vec![Step::Chars(vec![0xDE00, b'a' as u16, 0xD83D, b'b' as u16])]);
    assert_eq!(Screen::of(&report).typed(), "ab");
}

#[test]
fn characters_posted_directly_are_text_and_controls_among_them_are_not() {
    let units: Vec<u16> = vec![0x01, b'a' as u16, 0x08, 0x09, 0x1B, 0x7F, 0x85, b'b' as u16, 0x00E9];
    let report = run("posted", vec![Step::Chars(units)]);
    assert_eq!(Screen::of(&report).last_event(), "Text U+00E9");
    assert_eq!(report.title, "ok demo, 3 characters");
}

#[test]
fn a_line_break_posted_as_a_character_arrives_as_the_enter_key() {
    // Dictation, pasted-by-typing text and input methods have no Enter key
    // to press: their line breaks come as characters.
    let chars = |text: &str| Step::Chars(text.encode_utf16().collect());
    let report = run("break-return", vec![chars("one\rtwo")]);
    let screen = Screen::of(&report);
    assert_eq!((screen.text_row(0), screen.text_row(1)), (String::from("one"), format!("two{BLOCK}")));
    assert_eq!(report.title, "ok demo, 7 characters");

    let screen = Screen::of(&run("break-feed", vec![chars("one\ntwo")]));
    assert_eq!((screen.text_row(0), screen.text_row(1)), (String::from("one"), format!("two{BLOCK}")));

    // Windows line ends are one break, not two; two returns are two.
    let screen = Screen::of(&run("break-pair", vec![chars("one\r\ntwo\r\rthree")]));
    assert_eq!(screen.text_row(0), "one");
    assert_eq!(screen.text_row(1), "two");
    assert_eq!(screen.text_row(2), "");
    assert_eq!(screen.text_row(3), format!("three{BLOCK}"));

    // A line feed is half of a pair only straight after its return. With a
    // key pressed in between it is a line break of its own.
    let screen = Screen::of(&run("break-apart", vec![chars("one\r"), Step::Press(Key::Left), chars("\ntwo")]));
    assert_eq!(screen.text_row(0), "one");
    assert_eq!(screen.text_row(1), "");
    assert_eq!(screen.text_row(2), format!("two{BLOCK}"));

    // What the program is given is the key, said as a key.
    let screen = Screen::of(&run("break-said", vec![chars("a\r")]));
    assert_eq!(screen.last_event(), "Key Enter");
    let screen = Screen::of(&run("break-shift", vec![Step::Hold(Mods::SHIFT), chars("\n")]));
    assert_eq!(screen.last_event(), "Key Shift Enter");

    // It never turns into a command: with Ctrl or Alt held nothing is sent.
    for (name, held) in [("break-ctrl", Mods::CTRL), ("break-alt", Mods::ALT), ("break-altgr", CTRL_ALT)] {
        let report = run(name, vec![chars("https://example.com/"), Step::Hold(held), chars("\r\n"), Step::Hold(Mods::NONE), text("b")]);
        assert_eq!(Screen::of(&report).typed(), "https://example.com/b", "{name}");
        assert!(report.opened.is_empty(), "{name}: a character became Ctrl Enter");
    }
}

#[test]
fn the_enter_keys_own_character_is_not_a_second_line_break() {
    // Each of these keys also makes Windows post 0x0D or 0x0A as a character.
    let report = run("enter-once", vec![text("a"), Step::Press(Key::Enter), text("b"), Step::Press(Key::Enter), Step::Repeat(Key::Enter), text("c")]);
    let screen = Screen::of(&report);
    assert_eq!(screen.text_row(0), "a");
    assert_eq!(screen.text_row(1), "b");
    assert_eq!(screen.text_row(2), "", "the repeat is a second key, and breaks the line a second time");
    assert_eq!(screen.text_row(3), format!("c{BLOCK}"));
    assert_eq!(report.title, "ok demo, 6 characters");

    // Shift Enter is one key too, and Ctrl Enter (which posts 0x0A) is the
    // command alone: the demo breaks the line for the first and not the second.
    let report = run("enter-chords", vec![text("a"), Step::Chord(Key::Enter, Mods::SHIFT), Step::Chord(Key::Enter, Mods::CTRL), text("b")]);
    assert_eq!(report.title, "ok demo, 3 characters");
    let screen = Screen::of(&report);
    assert_eq!((screen.text_row(0), screen.text_row(1)), (String::from("a"), format!("b{BLOCK}")));
}

#[test]
fn a_burst_of_characters_is_drawn_once_at_its_end() {
    let burst: Vec<u16> = "the quick brown fox jumps ".repeat(8).encode_utf16().collect();
    let count = burst.len();
    let report = run("burst", vec![Step::Probe, Step::Chars(burst), Step::Probe]);
    let drawn = report.probes[1].renders - report.probes[0].renders;
    assert!(drawn <= 3, "{count} characters were drawn {drawn} times");
    assert_eq!(report.title, format!("ok demo, {count} characters"));
    assert_eq!(Screen::of(&report).text_row(0), "the quick brown fox jumps the quick brow");
}

#[test]
fn the_injected_text_key_is_never_a_command() {
    // Dictation and the emoji panel send their characters behind this key.
    let report = run("packet", vec![text("a"), Step::Hold(Mods::CTRL), Step::Vk(0xE7, true), Step::Vk(0xE7, false), Step::Hold(Mods::NONE)]);
    let screen = Screen::of(&report);
    assert_eq!(screen.last_event(), "Holding nothing", "no key was reported for it");
    assert_eq!(screen.typed(), "a");
}

#[test]
fn alt_alone_does_not_open_the_menu_or_eat_the_next_key() {
    let report = run("bare-alt", vec![text("a"), Step::Hold(Mods::ALT), Step::Hold(Mods::NONE), text("b"), Step::Press(Key::F(10)), text("c")]);
    assert_eq!(Screen::of(&report).typed(), "abc");
    assert_eq!(report.menus_refused, 2, "Windows asked for the menu once for Alt and once for F10");
}

#[test]
fn alt_space_asks_for_the_system_menu_only_while_the_window_has_one() {
    // A self-test never passes the request on to Windows, whose menu would
    // be a real one on the screen: it counts what a real run would have
    // done. As a window, that is to let Windows open the menu.
    let alt_space = [Step::Hold(Mods::ALT), Step::Press(Key::Char(' ')), Step::Hold(Mods::NONE)];
    let mut script = vec![text("a")];
    script.extend(alt_space.iter().cloned());
    script.push(text("b"));
    let report = run("alt-space", script);
    assert_eq!((report.system_menus, report.menus_refused), (1, 0), "asked for once, and not refused");
    let screen = Screen::of(&report);
    assert_eq!(screen.typed(), "ab", "no space was typed and the next key was not eaten");
    assert_eq!(screen.last_event(), "Text b");

    // Full screen the window has no system menu, and Windows answers a
    // request for a menu that is not there with its beep. The request stops
    // at the shell.
    let mut script = vec![Step::Press(Key::F(11)), text("a")];
    script.extend(alt_space.iter().cloned());
    script.push(text("b"));
    let report = run("alt-space-full-screen", script);
    assert!(report.fullscreen);
    assert_eq!((report.system_menus, report.menus_refused), (0, 1), "refused, and never passed on");
    assert_eq!(Screen::of(&report).typed(), "ab");

    // Back in a window the menu is there to ask for again.
    let mut script = vec![Step::Press(Key::F(11)), Step::Wait(100), Step::Press(Key::F(11))];
    script.extend(alt_space.iter().cloned());
    let report = run("alt-space-back", script);
    assert!(!report.fullscreen);
    assert_eq!((report.system_menus, report.menus_refused), (1, 0));
}

#[test]
fn alt_f4_closes_the_window_and_is_never_a_key() {
    let alt_f4 = [Step::Hold(Mods::ALT), Step::Press(Key::F(4)), Step::Hold(Mods::NONE), Step::Wait(200), text(" never")];
    let dir = folder("alt-f4");
    let mut script = vec![text("kept")];
    script.extend(alt_f4.iter().cloned());
    let report = run_with(SelfTest::new(&dir, script));
    assert_eq!(report.exit_code, 0);
    let screen = Screen::of(&report);
    assert_eq!(screen.last_event(), "Holding Alt", "the F4 itself was not reported as a key");
    assert_eq!(screen.typed(), "kept", "the window was gone before anything more was typed");
    assert_eq!(std::fs::read_to_string(dir.join("Local").join("demo-words.txt")).unwrap(), "kept", "the program heard the close");

    // Full screen the window has no system menu, and it closes all the same.
    let dir = folder("alt-f4-full-screen");
    let mut script = vec![Step::Press(Key::F(11)), text("kept")];
    script.extend(alt_f4.iter().cloned());
    let report = run_with(SelfTest::new(&dir, script));
    assert!(report.fullscreen);
    assert_eq!(Screen::of(&report).typed(), "kept");
    assert_eq!(std::fs::read_to_string(dir.join("Local").join("demo-words.txt")).unwrap(), "kept");

    // F4 alone is an ordinary key.
    assert_eq!(Screen::of(&run("f4-alone", vec![Step::Press(Key::F(4))])).last_event(), "Key F4");
}

#[test]
fn the_close_command_closes_the_window_and_the_program_hears_first() {
    let dir = folder("close");
    let report = run_with(SelfTest::new(&dir, vec![text("kept"), Step::Close, Step::Wait(200), text(" never")]));
    assert_eq!(report.exit_code, 0);
    assert_eq!(Screen::of(&report).typed(), "kept");
    assert_eq!(std::fs::read_to_string(dir.join("Local").join("demo-words.txt")).unwrap(), "kept", "the close reached the program before the window went");
}

#[test]
fn ctrl_q_quits_and_nothing_after_it_is_typed() {
    let report = run("ctrl-q", vec![text("a"), Step::Chord(Key::Char('q'), Mods::CTRL), Step::Wait(200), text("zzz")]);
    assert_eq!(report.exit_code, 0);
    assert_eq!(Screen::of(&report).typed(), "a");
    assert_eq!(report.title, "ok demo, 1 character");
}

#[test]
fn the_mouse_reports_its_button_and_only_real_moves() {
    let screen = Screen::of(&run("mouse-down", vec![Step::MouseDown(120, 80)]));
    assert_eq!(screen.last_event(), "Mouse down at 120, 80");
    let screen = Screen::of(&run("mouse-drag", vec![Step::MouseDown(120, 80), Step::MouseMove(130, 95)]));
    assert_eq!(screen.last_event(), "Mouse at 130, 95");
    let screen = Screen::of(&run("mouse-up", vec![Step::MouseDown(120, 80), Step::MouseMove(130, 95), Step::MouseUp(131, 96)]));
    assert_eq!(screen.last_event(), "Mouse up at 131, 96");
    // A move message for where the pointer already is says nothing new.
    let screen = Screen::of(&run("mouse-still", vec![Step::MouseMove(40, 40), text("a"), Step::MouseMove(40, 40)]));
    assert_eq!(screen.last_event(), "Text a");
    let screen = Screen::of(&run("mouse-shift", vec![Step::Hold(Mods::SHIFT), Step::MouseDown(7, 9)]));
    assert_eq!(screen.last_event(), "Mouse down at 7, 9 with Shift");
    // Losing the focus mid-drag ends the drag where it was.
    let screen = Screen::of(&run("mouse-lost", vec![Step::MouseDown(50, 60), Step::MouseMove(55, 66), Step::Focus(false)]));
    assert_eq!(screen.last_event(), "Focus lost");
}

#[test]
fn the_wheel_arrives_in_rows_with_its_remainder_kept() {
    // One notch is three rows on this machine's settings; a third of a notch is one.
    let screen = Screen::of(&run("wheel-up", vec![Step::Wheel { x: 100, y: 200, delta: 120 }]));
    let words = screen.last_event();
    assert!(words.starts_with("Wheel ") && words.ends_with(" rows up at 100, 200"), "{words}");
    let screen = Screen::of(&run("wheel-down", vec![Step::Wheel { x: 5, y: 6, delta: -240 }]));
    assert!(screen.last_event().ends_with(" rows down at 5, 6"), "{}", screen.last_event());
    // Fine steps that are each less than a row add up instead of vanishing.
    let small = Step::Wheel { x: 1, y: 1, delta: 10 };
    let screen = Screen::of(&run("wheel-fine", vec![small.clone(), small.clone(), small.clone(), small.clone(), small.clone(), small]));
    assert!(screen.last_event().starts_with("Wheel 1 rows up"), "{}", screen.last_event());
}

#[test]
fn focus_is_reported_and_a_hidden_caret_follows_the_cursor() {
    let report = run("focus", vec![Step::Focus(true), text("ab")]);
    let layout = Layout::new(report.width, report.height);
    let (x, y) = layout.at(layout.text_col + 2, layout.text_row);
    assert_eq!(report.caret, Some((x, y)), "the system caret sits in the cursor's cell");
    assert_eq!(Screen::of(&run("focus-gained", vec![Step::Focus(true)])).last_event(), "Focus gained");
    let report = run("focus-lost", vec![Step::Focus(true), Step::Hold(Mods::CTRL), Step::Focus(false)]);
    assert_eq!(Screen::of(&report).last_event(), "Focus lost");
    assert_eq!(report.caret, None, "the caret is given up with the focus");
}

#[test]
fn a_tiny_size_and_back_does_not_panic() {
    let sizes = [(1, 1), (0, 0), (13, 23), (0, 400), (400, 0), (12, 22), (65_535, 3), (5, 65_535), (300, 200)];
    let mut script = vec![text("small")];
    for (w, h) in sizes {
        script.push(Step::Size(w, h));
        script.push(text("."));
    }
    script.push(Step::Size(960, 528));
    script.push(text("back"));
    let report = run("tiny", script);
    assert_eq!(report.crashed, None);
    assert_eq!(report.exit_code, 0);
    assert_eq!((report.width, report.height), (960, 528));
    assert_eq!(Screen::of(&report).typed(), "small.........back");
}

#[test]
fn a_frame_too_small_for_a_single_cell_is_still_drawn() {
    let report = run("tiny-kept", vec![text("x"), Step::Size(7, 5)]);
    assert_eq!((report.width, report.height), (7, 5));
    assert_eq!(report.pixels.len(), 35);
    assert!(report.pixels.iter().all(|&p| p == DESK.px()), "no room for the sheet: all desk");
}

#[test]
fn resizing_for_real_respects_the_smallest_window_and_reports_the_size() {
    let least = (MIN_CELLS.0 as i32 * CELL.0, MIN_CELLS.1 as i32 * CELL.1);
    let report = run("client-small", vec![Step::Client(100, 50)]);
    assert_eq!(report.client, least);
    assert_eq!(Screen::of(&report).last_event(), format!("Resize to {} by {}", least.0, least.1));
    let report = run("client-grown", vec![Step::Client(1200, 660), text("wide")]);
    assert_eq!(report.client, (1200, 660));
    assert_eq!((report.width, report.height), (1200, 660));
    assert_eq!(Screen::of(&report).typed(), "wide");
}

#[test]
fn dragging_an_edge_snaps_to_whole_cells() {
    let report = run("snap", vec![Step::Drag(1000, 500), Step::Drag(1003, 517), Step::Drag(10, 10), Step::Drag(960, 528)]);
    assert_eq!(report.snaps[0], (996, 506), "83 by 23 cells is nearest");
    assert_eq!(report.snaps[1], (1008, 528), "84 by 24 cells is nearest");
    assert_eq!(report.snaps[2], (MIN_CELLS.0 as i32 * CELL.0, MIN_CELLS.1 as i32 * CELL.1));
    assert_eq!(report.snaps[3], (960, 528));
}

#[test]
fn f11_goes_full_screen_and_back() {
    let report = run("fullscreen", vec![text("a"), Step::Press(Key::F(11))]);
    assert!(report.fullscreen);
    let (w, h) = report.client;
    assert!(w >= 1024 && h >= 720, "the client area is a whole monitor: {w} by {h}");
    assert_eq!((report.width as i32, report.height as i32), (w, h), "and the frame is that size");
    let screen = Screen::of(&report);
    assert_eq!(screen.last_event(), format!("Resize to {w} by {h}"));
    assert_eq!(screen.typed(), "a");

    let report = run("fullscreen-back", vec![Step::Press(Key::F(11)), Step::Wait(100), Step::Press(Key::F(11)), text("b")]);
    assert!(!report.fullscreen);
    assert_eq!(report.client, (960, 528), "back to exactly where it was");
    assert_eq!(Screen::of(&report).typed(), "b");
}

#[test]
fn the_window_is_remembered_between_runs() {
    let dir = folder("placement");
    let first = run_with(SelfTest { client: Some((720, 440)), ..SelfTest::new(&dir, vec![text("one")]) });
    assert_eq!(first.client, (720, 440));
    let saved = std::fs::read_to_string(dir.join("Local").join("window.txt")).expect("the placement file");
    assert!(saved.contains("maximized no"), "{saved}");

    let second = run_with(SelfTest { client: None, ..SelfTest::new(&dir, vec![]) });
    assert_eq!(second.client, (720, 440), "opened where it was left");
    assert_eq!(Screen::of(&second).typed(), "one", "and the demo brought its words back");

    // Full screen is the program's setting; what is remembered is the window underneath.
    let third = run_with(SelfTest { client: None, ..SelfTest::new(&dir, vec![Step::Press(Key::F(11))]) });
    assert!(third.fullscreen);
    let fourth = run_with(SelfTest { client: None, ..SelfTest::new(&dir, vec![]) });
    assert_eq!(fourth.client, (720, 440));
}

#[test]
fn the_title_follows_the_program() {
    assert_eq!(run("title-none", vec![]).title, "ok demo, 0 characters");
    assert_eq!(run("title-some", vec![text("abc")]).title, "ok demo, 3 characters");
}

#[test]
fn the_window_wears_the_icon_it_is_given_as_pixels() {
    // Half opaque paper, half ink at half strength: the alpha is not all or nothing.
    let image = |side: u32| Icon {
        width: side,
        height: side,
        argb: (0..side * side).map(|i| if i % side < side / 2 { 0xFF00_0000 | PAPER.px() } else { 0x8024_1E19 }).collect(),
    };
    let options = Options { icons: vec![image(16), image(32), image(48)], ..support::options() };
    let report = ok_win::selftest(options, SelfTest::new(folder("icons"), vec![]), |host| Box::new(support::demo::Demo::new(host)));
    assert!(report.has_icons && report.cloaked && !report.was_foreground);

    // With no pixels the window falls back to an icon stamped into the exe, and a test's exe has none.
    assert!(!run("icons-none", vec![]).has_icons);

    // A malformed image is passed over, not drawn from.
    let short = Icon { width: 32, height: 32, argb: vec![0xFFFF_FFFF; 10] };
    let options = Options { icons: vec![short], ..support::options() };
    let report = ok_win::selftest(options, SelfTest::new(folder("icons-short"), vec![]), |host| Box::new(support::demo::Demo::new(host)));
    assert!(!report.has_icons);
    assert_eq!(report.exit_code, 0);
}

#[test]
fn a_second_launch_hands_over_its_file() {
    let path = PathBuf::from("C:\\Users\\jaspe\\Documents\\ok\\The orchard.md");
    let report = run("open", vec![Step::Open(path)]);
    assert_eq!(Screen::of(&report).last_event(), "Open C:\\Users\\jaspe\\Documents\\ok\\The orchard.md");
}

#[test]
fn the_file_a_launch_is_given_is_opened_before_the_first_frame() {
    let options = Options { open: Some(PathBuf::from("C:\\Users\\jaspe\\Documents\\ok\\Essay plan.md")), ..support::options() };
    let test = SelfTest::new(folder("open-own"), vec![]);
    let report = ok_win::selftest(options, test, |host| Box::new(support::demo::Demo::new(host)));
    assert_eq!(Screen::of(&report).last_event(), "Open C:\\Users\\jaspe\\Documents\\ok\\Essay plan.md");

    // A name with no folder is made whole against this launch's directory.
    let options = Options { open: Some(PathBuf::from("notes.txt")), ..support::options() };
    let report = ok_win::selftest(options, SelfTest::new(folder("open-relative"), vec![]), |host| Box::new(support::demo::Demo::new(host)));
    let whole = std::env::current_dir().unwrap().join("notes.txt");
    let said = Screen::of(&report).last_event();
    let expected = format!("Open {}", whole.display());
    assert!(expected.starts_with(&said) && said.len() > 10, "{said:?} is the start of {expected:?}");
}

#[test]
fn the_end_of_a_session_saves_first_and_a_cancelled_one_carries_on() {
    let dir = folder("end-session");
    let words = dir.join("Local").join("demo-words.txt");
    let report = run_with(SelfTest::new(&dir, vec![text("one"), Step::EndSession(false), Step::Shot(dir.join("after-cancel.png")), text(" two")]));
    assert_eq!(report.exit_code, 0);
    assert_eq!(Screen::of(&report).typed(), "one two", "a cancelled shutdown leaves the window working");
    assert_eq!(std::fs::read_to_string(&words).unwrap(), "one two");

    let dir = folder("end-session-real");
    let report = run_with(SelfTest::new(&dir, vec![text("one"), Step::EndSession(true), Step::Wait(200), text(" never")]));
    assert_eq!(report.exit_code, 0);
    assert_eq!(std::fs::read_to_string(dir.join("Local").join("demo-words.txt")).unwrap(), "one");
    assert_eq!(Screen::of(&report).typed(), "one");
}

#[test]
fn the_clipboard_and_the_browser_are_stand_ins_and_work() {
    let report = run("stand-ins", vec![
        text("https://example.com/a"),
        Step::Chord(Key::Char('c'), Mods::CTRL),
        Step::Chord(Key::Enter, Mods::CTRL),
        Step::Chord(Key::Char('v'), Mods::CTRL),
    ]);
    assert_eq!(report.clipboard.as_deref(), Some("https://example.com/a"));
    assert_eq!(report.opened, vec!["https://example.com/a".to_string()]);
    assert_eq!(report.title, "ok demo, 42 characters", "the paste doubled the text");

    // Only web addresses are ever opened.
    for (name, words) in [("opened-exe", "calc.exe"), ("opened-file", "file:///C:/Windows/win.ini"), ("opened-flag", "https://example.com/ --flag")] {
        let report = run(name, vec![Step::Chars(words.encode_utf16().collect()), Step::Chord(Key::Enter, Mods::CTRL)]);
        assert!(report.opened.is_empty(), "{words} reached the browser");
    }
}

#[test]
fn a_one_shot_wake_brings_ok_back_after_typing_stops() {
    let report = run("wake", vec![Step::Wait(100), text("a")]);
    assert_eq!(Screen::of(&report).machine_line(), "", "ok is erased by the keystroke");
    let report = run("wake-after", vec![text("a"), Step::Wait(OK_AFTER_MS + 500)]);
    assert_eq!(Screen::of(&report).machine_line(), "ok", "and printed again once typing has stopped");
    // One tick, and not before its time: woken early, the demo would have had to ask again.
    assert_eq!(report.ticks.iter().filter(|t| t.source == TickSource::Wake).count(), 1);
}

/// A program that does nothing but use its waker. `at_start` is run once,
/// with the waker, when the program is told its size; the waker is also left
/// in `kept` for the test to use after the window has gone.
struct Woken<F: FnMut(ok_host::Waker)> {
    at_start: Option<F>,
    kept: Arc<Mutex<Option<ok_host::Waker>>>,
}

impl<F: FnMut(ok_host::Waker)> ok_host::Program for Woken<F> {
    fn event(&mut self, event: ok_host::Event, host: &mut dyn ok_host::Host) {
        if let (ok_host::Event::Resize { .. }, Some(mut at_start)) = (&event, self.at_start.take()) {
            *self.kept.lock().unwrap() = Some(host.waker());
            at_start(host.waker());
        }
    }
    fn wants(&mut self, _: &dyn ok_host::Host) -> ok_host::Wants {
        ok_host::Wants { fullscreen: false, ..ok_host::Wants::default() }
    }
    fn render(&mut self, frame: &mut ok_gfx::Frame, _: &mut dyn ok_host::Host) {
        frame.clear(DESK);
    }
}

/// Runs a [`Woken`] unseen and returns the report and the waker it kept.
fn run_woken(name: &str, script: Vec<Step>, at_start: impl FnMut(ok_host::Waker) + 'static) -> (ok_win::Report, ok_host::Waker) {
    let kept = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&kept);
    let report = ok_win::selftest(support::options(), SelfTest::new(folder(name), script), move |_| {
        Box::new(Woken { at_start: Some(at_start), kept: slot })
    });
    assert!(report.cloaked && !report.was_foreground && !report.timed_out);
    assert_eq!(report.crashed, None);
    let waker = kept.lock().unwrap().take().expect("the program was told its size");
    (report, waker)
}

fn waker_ticks(report: &ok_win::Report) -> Vec<f64> {
    report.ticks.iter().filter(|t| t.source == TickSource::Waker).map(|t| t.at_ms).collect()
}

#[test]
fn a_waker_called_many_times_costs_one_tick() {
    let (report, _) = run_woken("waker-many", vec![Step::Wait(200)], |wake| {
        for _ in 0..100 {
            wake();
        }
    });
    assert_eq!(waker_ticks(&report).len(), 1);
    assert_eq!(report.ticks.len(), 1, "and nothing else ticked");
}

#[test]
fn a_waker_on_another_thread_wakes_the_idle_window_each_time_there_is_news() {
    let (report, _) = run_woken("waker-thread", vec![Step::Wait(700), Step::Probe], |wake| {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            wake();
            std::thread::sleep(std::time::Duration::from_millis(200));
            wake();
            wake();
        });
    });
    let ticks = waker_ticks(&report);
    assert_eq!(ticks.len(), 2, "one tick for each time there was news: {ticks:?}");
    assert!(ticks[0] >= 140.0, "the first came when it was asked for, not before: {ticks:?}");
    assert!(ticks[1] - ticks[0] >= 100.0, "and the second in its own time: {ticks:?}");
    assert!(ticks[1] < report.probes[0].at_ms - 100.0, "neither waited for the end of the pause: {ticks:?}");
}

#[test]
fn a_waker_that_outlives_its_window_does_nothing() {
    let (stop, stopped) = (Arc::new(Mutex::new(false)), Arc::new(Mutex::new(false)));
    let (stop_thread, stopped_thread) = (Arc::clone(&stop), Arc::clone(&stopped));
    // A thread that keeps calling while the window opens, runs and closes.
    let (report, waker) = run_woken("waker-outlives", vec![Step::Wait(100)], move |wake| {
        let (stop, stopped) = (Arc::clone(&stop_thread), Arc::clone(&stopped_thread));
        std::thread::spawn(move || {
            while !*stop.lock().unwrap() {
                wake();
                std::thread::yield_now();
            }
            *stopped.lock().unwrap() = true;
        });
    });
    assert!(!waker_ticks(&report).is_empty());
    assert_eq!(report.exit_code, 0);
    // The window is gone. Both the thread's waker and this one are still callable.
    std::thread::sleep(std::time::Duration::from_millis(50));
    waker();
    waker();
    *stop.lock().unwrap() = true;
    for _ in 0..200 {
        if *stopped.lock().unwrap() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(*stopped.lock().unwrap(), "the thread that kept waking came to no harm");
}

#[test]
fn a_panic_is_written_down_and_the_words_survive_it() {
    let dir = folder("panic");
    let report = run_with(SelfTest::new(&dir, vec![text("kept"), Step::Press(Key::F(9)), Step::Wait(200), text(" never")]));
    assert_eq!(report.exit_code, EXIT_CRASH);
    let message = report.crashed.expect("the panic's message");
    assert!(message.starts_with("the demo was asked to panic with F9"), "{message}");
    assert!(message.contains("demo"), "the place it happened is noted: {message}");

    let note = std::fs::read_to_string(dir.join("Local").join("crash.txt")).expect("crash.txt");
    assert!(note.contains("the demo was asked to panic with F9"), "{note}");
    assert!(note.contains("while handling Key { key: F(9)"), "{note}");
    // The program was given one last close, and used it.
    assert_eq!(std::fs::read_to_string(dir.join("Local").join("demo-words.txt")).unwrap(), "kept");
    // The window was shown and cloaked, which is how a real one stands for
    // a moment during its first show. A dialog it owned would be cloaked
    // with it, so the dialog a real run raises would have stood alone.
    assert_eq!(report.crash_dialog_owned, Some(false));

    let again = run_with(SelfTest::new(&dir, vec![]));
    assert_eq!(again.exit_code, 0);
    let screen = Screen::of(&again);
    assert_eq!(screen.typed(), "kept");
    assert_eq!(screen.last_event(), "Resize to 960 by 528");
}

#[test]
fn a_program_that_panics_while_starting_or_drawing_ends_cleanly_too() {
    use ok_gfx::Frame;
    use ok_host::{Event, Host, Program, Wants};

    struct BadDraw;
    impl Program for BadDraw {
        fn event(&mut self, _: Event, _: &mut dyn Host) {}
        fn wants(&mut self, _: &dyn Host) -> Wants {
            Wants { redraw: true, fullscreen: false, ..Wants::default() }
        }
        fn render(&mut self, _: &mut Frame, _: &mut dyn Host) {
            panic!("the ribbon ran out");
        }
    }

    let dir = folder("panic-draw");
    let report = ok_win::selftest(support::options(), SelfTest::new(&dir, vec![text("a")]), |_| Box::new(BadDraw));
    assert_eq!(report.exit_code, EXIT_CRASH);
    assert!(report.crashed.unwrap().starts_with("the ribbon ran out"));
    assert!(std::fs::read_to_string(dir.join("Local").join("crash.txt")).unwrap().contains("while drawing."));
    assert!(report.cloaked && !report.was_foreground);

    let dir = folder("panic-start");
    let report = ok_win::selftest(support::options(), SelfTest::new(&dir, vec![text("a")]), |_| -> Box<dyn Program> { panic!("no paper") });
    assert_eq!(report.exit_code, EXIT_CRASH);
    assert!(std::fs::read_to_string(dir.join("Local").join("crash.txt")).unwrap().contains("while starting."));
}

#[test]
fn the_last_frame_is_written_as_a_png() {
    let dir = folder("png");
    let png = dir.join("shots").join("last.png");
    let report = run_with(SelfTest { png: Some(png.clone()), ..SelfTest::new(&dir, vec![text("ok")]) });
    let bytes = std::fs::read(&png).expect("the PNG");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&bytes[12..16], b"IHDR");
    let size = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
    assert_eq!((size(16), size(20)), (report.width, report.height));
}

#[test]
fn opening_full_screen_is_the_default_a_program_gets() {
    use ok_gfx::Frame;
    use ok_host::{Event, Host, Program, Wants};

    /// Wants nothing but the defaults, which ask for full screen, and never
    /// asks for a redraw. Notes every size it is told.
    struct Plain(Arc<Mutex<Vec<(usize, usize)>>>);
    impl Program for Plain {
        fn event(&mut self, event: Event, _: &mut dyn Host) {
            if let Event::Resize { width, height } = event {
                self.0.lock().unwrap().push((width, height));
            }
        }
        fn wants(&mut self, _: &dyn Host) -> Wants {
            Wants::default()
        }
        fn render(&mut self, frame: &mut Frame, _: &mut dyn Host) {
            frame.clear(PAPER);
        }
    }

    let dir = folder("default-fullscreen");
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::clone(&sizes);
    let report = ok_win::selftest(support::options(), SelfTest::new(&dir, vec![]), move |_| Box::new(Plain(told)));
    assert!(report.fullscreen && report.cloaked && !report.was_foreground);
    assert_eq!((report.width as i32, report.height as i32), report.client);
    assert!(report.client.0 >= 1024);
    assert_eq!(*sizes.lock().unwrap(), vec![(report.width, report.height)], "told its size once: the monitor's, not the window's on the way there");
    assert!(report.pixels.iter().all(|&p| p == PAPER.px()), "a new size is drawn even when the program does not ask");
    assert_eq!(report.on_glass, Some(true));
    assert_eq!(report.title, "ok");
    // The window underneath is what was remembered, not the monitor-sized one.
    let saved = std::fs::read_to_string(dir.join("Local").join("window.txt")).unwrap();
    assert!(saved.contains(&format!("right {}", 960 + 16 + saved_left(&saved))), "{saved}");
}

fn saved_left(saved: &str) -> i32 {
    saved.lines().find_map(|l| l.strip_prefix("left ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}
