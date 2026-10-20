//! DESIGN.md, "Settings", "Looks" and "Faces": every option set, kept, put
//! back and restored to its default from the line, and the settings text
//! they are kept in.

mod line_support;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use line_support::*;
use ok_gfx::{AlphaGlyph, Frame, Rgb};
use ok_host::{Event, FaceCell, Host, Key, LocalTime, Mods, Program, Waker};
use ok_ui::flow::{Act, Flow, Part, Rest, Said};
use ok_ui::harness::{Rig, TestHost};
use ok_ui::line::settings::{self, OPTIONS};
use ok_ui::look::{Look, Strength, Surface};
use ok_ui::{line, App, Options};

/// The value an option has now, as the settings write it.
fn value(rig: &Rig, name: &str) -> String {
    settings::value(&rig.app, settings::find(name).unwrap())
}

/// What is typed for each option in these tests, and what that makes it.
const TRIED: [(&str, &str, &str); 19] = [
    ("look", "amber", "amber"),
    ("face", "spleen", "spleen"),
    ("scale", "2", "2"),
    ("paper", "fff", "FFFFFF"),
    ("ink", "000000", "000000"),
    ("desk", "#101010", "101010"),
    ("desk-ink", "c0c0c0", "C0C0C0"),
    ("mark", "ff0000", "FF0000"),
    ("half", "70", "70"),
    ("faint", "#888888", "888888"),
    ("measure", "72", "72"),
    ("platen", "off", "off"),
    ("scroll", "jump", "jump"),
    ("cursor", "underline", "underline"),
    ("focus", "off", "off"),
    ("echo", "off", "off"),
    ("enter", "line", "line"),
    ("limit", "650", "650"),
    ("window", "framed", "framed"),
];

/// The default of each, as DESIGN.md gives them.
const DEFAULTS: [(&str, &str); 19] = [
    ("look", "manila"),
    ("face", "gallant"),
    ("scale", "1"),
    ("paper", "EEE3C8"),
    ("ink", "221F1B"),
    ("desk", "35251A"),
    ("desk-ink", "E6D9BA"),
    ("mark", "ink"),
    ("half", "65"),
    ("faint", "35"),
    ("measure", "66"),
    ("platen", "on"),
    ("scroll", "smooth"),
    ("cursor", "block"),
    ("focus", "on"),
    ("echo", "on"),
    ("enter", "paragraph"),
    ("limit", "off"),
    ("window", "full"),
];

#[test]
fn every_option_starts_at_the_default_the_design_gives() {
    let (rig, _) = rig();
    for (name, default) in DEFAULTS {
        assert_eq!(value(&rig, name), default, "{name}");
        assert_eq!(settings::default(settings::find(name).unwrap(), &rig.host), default, "{name}");
    }
    assert_eq!(value(&rig, "folder"), "C:\\Users\\jaspe\\Documents\\ok");
    assert_eq!(OPTIONS.len(), DEFAULTS.len() + 1);
}

#[test]
fn every_option_is_tried_as_it_is_typed_and_esc_puts_it_back() {
    for (name, typed, becomes) in TRIED {
        let (mut rig, folder) = rig();
        let before = picture(&mut rig);
        let was = value(&rig, name);
        line_types(&mut rig, &format!("set {name} {typed}"));
        // The whole screen has changed already, before any Enter.
        assert_eq!(value(&rig, name), becomes, "{name}");
        let sentence = said(&mut rig);
        assert!(sentence.starts_with(&format!("Enter keeps {name} at {becomes}. Esc puts it back to {was}.")), "{name}: {sentence}");
        // In the table, the option's row carries the value being tried.
        let row = option_row(&mut rig, name).unwrap();
        assert!(row.ends_with(&format!(". {becomes}")), "{row}");
        rig.press("esc");
        assert_eq!(value(&rig, name), was, "{name}");
        assert_eq!(picture(&mut rig), before, "{name}: Esc puts the whole screen back");
        assert_eq!(rig.wants().fullscreen, true);
        assert_eq!(folder.borrow().settings, None, "{name}: nothing that was only tried is kept");
    }
}

#[test]
fn every_option_is_kept_by_enter_and_restored_by_default() {
    for (name, typed, becomes) in TRIED {
        let (mut rig, folder) = rig();
        let default = DEFAULTS.iter().find(|(other, _)| *other == name).unwrap().1;
        line_types(&mut rig, &format!("set {name} {typed}"));
        rig.press("enter");
        // Kept, and the line stays at `ok set` for the next one, the bar
        // on the option just set.
        assert_eq!(value(&rig, name), becomes, "{name}");
        assert_eq!(typed_on(&mut rig), "set", "{name}");
        assert!(bar(&mut rig).unwrap().starts_with(&format!("{name} .")), "{name}");
        assert_eq!(said(&mut rig), format!("Enter changes {name}. It is {becomes} now."));
        // Esc from here puts nothing back: it was kept.
        rig.press("esc");
        assert_eq!(value(&rig, name), becomes, "{name}");
        // All but the text's own limit are written in the settings text.
        if name != "limit" {
            let written = folder.borrow().settings.clone().unwrap_or_default();
            assert!(written.lines().any(|line| line == format!("{name} {becomes}")), "{name} in:\n{written}");
        } else {
            assert_eq!(folder.borrow().limits.get("College/The orchard"), Some(&650));
        }
        // `default` restores.
        line_types(&mut rig, &format!("set {name} default"));
        assert_eq!(value(&rig, name), default, "{name}");
        let sentence = said(&mut rig);
        assert!(sentence.starts_with(&format!("Enter keeps {name} at {default}. Esc puts it back to {becomes}.")), "{name}: {sentence}");
        rig.press("enter");
        rig.press("esc");
        assert_eq!(value(&rig, name), default, "{name}");
    }
}

/// What is typed on the machine line.
fn typed_on(rig: &mut Rig) -> String {
    typed(rig)
}

#[test]
fn each_setting_does_what_it_says() {
    let (mut rig, folder) = rig();
    let set = |rig: &mut Rig, what: &str| {
        line_types(rig, &format!("set {what}"));
        rig.press("enter");
        rig.press("esc");
    };
    // look: the seven values of the look, on the glass.
    set(&mut rig, "look 5151");
    assert_eq!(rig.pixel(3, 3), Some(Rgb(0x10, 0x19, 0x13)));
    assert_eq!(rig.app.look(), &Look::named("5151").unwrap());
    // face: the cell comes from the face.
    set(&mut rig, "face spleen");
    assert_eq!((rig.app.grid().cell_w, rig.app.grid().cell_h), (12, 24));
    set(&mut rig, "face terminus");
    assert_eq!((rig.app.grid().cell_w, rig.app.grid().cell_h), (11, 22));
    set(&mut rig, "face gallant");
    assert_eq!((rig.app.grid().cell_w, rig.app.grid().cell_h), (12, 22));
    // scale and measure lay the screen out again.
    set(&mut rig, "scale 2");
    assert_eq!((rig.app.grid().scale, rig.app.grid().cols), (2, 80));
    set(&mut rig, "scale 1");
    set(&mut rig, "measure 50");
    assert_eq!(rig.app.grid().measure, 50);
    let row = row_of(&mut rig, "The orchard was eleven");
    assert_eq!(read(&mut rig, 55, 104, row), "The orchard was eleven acres when my grandfather");
    set(&mut rig, "measure 66");
    // window: the shell is asked for a frame.
    set(&mut rig, "window framed");
    assert!(!rig.wants().fullscreen);
    set(&mut rig, "window full");
    assert!(rig.wants().fullscreen);
    // platen and scroll are the two the paper reads.
    set(&mut rig, "platen off");
    set(&mut rig, "scroll jump");
    assert!(!rig.app.prefs().platen && !rig.app.prefs().smooth);
    // limit is this text's alone.
    set(&mut rig, "limit 650");
    assert_eq!(folder.borrow().limits.get("College/The orchard"), Some(&650));
    line_types(&mut rig, "essay plan");
    rig.press("enter");
    assert_eq!(value(&rig, "limit"), "off");
    set(&mut rig, "limit 0");
    assert_eq!(value(&rig, "limit"), "off");
}

#[test]
fn the_cursor_can_be_an_underscore_under_its_letter() {
    let (mut rig, _) = rig_with(&[("Notes", "abc")], "Notes", 1920, 1080);
    rig.press("left");
    let look = Look::manila();
    let row = rig.app.grid().platen;
    let cell = rig.app.grid().cells(47 + 2, row, 1, 1);
    // A block: the cell is ink from corner to corner.
    assert_eq!(rig.pixel(cell.x, cell.y), Some(look.ink));
    assert!(rig.screen().at(49, row).unwrap().inverse);
    line_types(&mut rig, "set cursor underline");
    rig.press("enter");
    rig.press("esc");
    // An underscore: the letter as it is printed, a stroke along the foot.
    assert_eq!(rig.pixel(cell.x, cell.y), Some(look.paper));
    assert_eq!(rig.pixel(cell.x, cell.y + cell.h - 1), Some(look.ink));
    assert_eq!(rig.pixel(cell.x + cell.w - 1, cell.y + cell.h - 2), Some(look.ink));
    assert_eq!(rig.pixel(cell.x, cell.y + cell.h - 3), Some(look.paper));
    let under = rig.screen().at(49, row).unwrap();
    assert!(!under.inverse && under.ch == 'c');
    assert_eq!(rig.screen().cursor.map(|at| (at.col, at.row, at.hollow)), Some((49, row, false)));
    // The line's own cursor is the same.
    rig.press("esc");
    let cell = rig.app.grid().cells(50, 47, 1, 1);
    assert_eq!(rig.pixel(cell.x, cell.y), Some(look.paper));
    assert_eq!(rig.pixel(cell.x, cell.y + cell.h - 1), Some(look.ink));
}

#[test]
fn enter_can_be_one_line_break_with_shift_enter_ending_the_paragraph() {
    let (mut rig, _) = rig_with(&[("Notes", "one")], "Notes", 1920, 1080);
    line_types(&mut rig, "set enter line");
    rig.press("enter");
    rig.press("esc");
    rig.press("enter");
    rig.type_text("two");
    assert_eq!(rig.text(), "one\ntwo");
    rig.press("shift+enter");
    rig.type_text("- three");
    assert_eq!(rig.text(), "one\ntwo\n\n- three");
    // In a list Enter goes on starting the next item.
    rig.press("enter");
    rig.type_text("four");
    assert_eq!(rig.text(), "one\ntwo\n\n- three\n- four");
    line_types(&mut rig, "set enter paragraph");
    rig.press("enter");
    rig.press("esc");
    rig.press("enter");
    rig.press("enter");
    rig.type_text("five");
    rig.press("enter");
    assert_eq!(rig.text(), "one\ntwo\n\n- three\n- four\n\nfive\n\n");
}

/// A flow that dims everything and has a sentence to say at every tick.
#[derive(Clone, Default)]
struct Dim {
    ticks: Rc<Cell<usize>>,
    acts: Rc<RefCell<Vec<Act>>>,
}

impl Flow for Dim {
    fn note(&mut self, act: Act, _host: &dyn Host) {
        self.acts.borrow_mut().push(act);
    }

    fn tick(&mut self, _host: &dyn Host) -> Option<Said> {
        self.ticks.set(self.ticks.get() + 1);
        Some(Said { text: String::from("143 words without stopping."), for_ms: Some(6000) })
    }

    fn animating(&self, _now_ms: u64) -> bool {
        true
    }

    fn wake_in_ms(&self, _now_ms: u64) -> Option<u64> {
        Some(2000)
    }

    fn part(&self, _part: Part, _rest: Strength, _now_ms: u64) -> Strength {
        Strength::FAINT
    }

    fn text(&self, _line: usize, own: bool, rest: Strength, _now_ms: u64) -> Strength {
        if own {
            rest
        } else {
            Strength::HALF
        }
    }
}

#[test]
fn focus_off_stops_the_fading_and_echo_off_what_the_machine_says_of_a_run() {
    let texts = desk();
    let borrowed: Vec<(&str, &str)> = texts.iter().map(|(name, text)| (*name, text.as_str())).collect();
    let (shelf, _) = Shelf::new(&borrowed, "College/The orchard");
    let flow = Dim::default();
    let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(flow.clone()));
    rig.size(1920, 1080);
    let row = row_of(&mut rig, "The orchard was eleven");
    let strengths = move |rig: &mut Rig| (rig.screen().at(47, row).unwrap().strength, rig.screen().at(4, 4).unwrap().strength);
    // With both on the flow fades the screen, speaks, and is given frames.
    rig.send(Event::Tick);
    assert_eq!(strengths(&mut rig), (Strength::HALF, Strength::FAINT));
    assert_eq!(said(&mut rig), "143 words without stopping.");
    assert!(rig.wants().animating && rig.wants().wake_in_ms.is_some());
    // echo off: it still fades, and is not heard.
    line_types(&mut rig, "set echo off");
    rig.press("enter");
    rig.press("esc");
    rig.send(Event::Tick);
    assert_eq!(said(&mut rig), "");
    assert_eq!(strengths(&mut rig), (Strength::HALF, Strength::FAINT));
    // focus off as well: everything at rest, and no frames asked for.
    line_types(&mut rig, "set focus off");
    assert_eq!(strengths(&mut rig).0, Strength::INK, "as soon as the value is typed");
    rig.press("enter");
    rig.press("esc");
    assert_eq!(strengths(&mut rig), (Strength::INK, Strength::HALF));
    assert!(!rig.wants().animating);
    assert_eq!(rig.wants().wake_in_ms, None, "a flow neither seen nor heard is not woken");
    // focus off alone: at rest, and heard.
    line_types(&mut rig, "set echo on");
    rig.press("enter");
    rig.press("esc");
    rig.send(Event::Tick);
    assert_eq!(said(&mut rig), "143 words without stopping.");
    assert_eq!(strengths(&mut rig), (Strength::INK, Strength::HALF));
    assert_eq!(rig.wants().wake_in_ms, Some(2000));
    // It is told what he does either way.
    flow.acts.borrow_mut().clear();
    rig.type_text("x");
    assert_eq!(*flow.acts.borrow(), [Act::Typed('x')]);
}

#[test]
fn lists_are_walked_with_up_and_down_and_the_whole_screen_changes_as_the_bar_lands() {
    let (mut rig, _) = rig();
    let before = picture(&mut rig);
    line_types(&mut rig, "set look");
    rig.press("enter");
    // The values are set in under the option, the bar on the one in force.
    assert_eq!(typed(&mut rig), "set look");
    let rows = contents(&mut rig);
    assert_eq!(rows[..11], ["look ....................... manila", "  manila", "  sun", "  acme", "  register", "  register night", "  amber", "  vt100", "  5151", "  wordperfect", "face ...................... gallant"]);
    assert_eq!(bar(&mut rig).as_deref(), Some("manila"));
    assert_eq!(said(&mut rig), "Enter keeps look at manila. The default is manila.");
    // Down: the bar lands on sun and the whole screen is sun.
    rig.press("down");
    assert_eq!((bar(&mut rig).as_deref(), typed(&mut rig).as_str()), (Some("sun"), "set look sun"));
    assert_eq!(rig.pixel(3, 3), Some(Rgb(255, 255, 255)));
    assert_eq!(rig.app.look().name, "sun");
    assert_eq!(said(&mut rig), "Enter keeps look at sun. Esc puts it back to manila.");
    assert!(contents(&mut rig)[0].ends_with(". sun"));
    for _ in 0..4 {
        rig.press("down");
    }
    assert_eq!(bar(&mut rig).as_deref(), Some("amber"));
    assert_eq!(rig.pixel(3, 3), Some(Rgb(0x19, 0x13, 0x10)));
    for _ in 0..9 {
        rig.press("down");
    }
    assert_eq!(bar(&mut rig).as_deref(), Some("wordperfect"), "the bar stops at the last");
    rig.press("up");
    assert_eq!(bar(&mut rig).as_deref(), Some("5151"));
    // Esc puts back, exactly.
    rig.press("esc");
    assert_eq!(picture(&mut rig), before);
    assert_eq!(rig.pixel(3, 3), Some(Look::manila().desk));
    // The first letters of a value are enough.
    line_types(&mut rig, "set look reg");
    assert_eq!((bar(&mut rig).as_deref(), rig.app.look().name.as_str()), (Some("register"), "register"));
    rig.type_text("ister n");
    assert_eq!(bar(&mut rig).as_deref(), Some("register night"));
    assert_eq!(rig.app.look(), &Look::named("register night").unwrap());
    rig.press("esc");
    // A switch is a list of two.
    line_types(&mut rig, "set scroll ");
    assert_eq!(contents(&mut rig)[13..15], ["  smooth", "  jump"]);
    assert_eq!(bar(&mut rig).as_deref(), Some("smooth"));
    rig.press("down");
    assert_eq!((typed(&mut rig).as_str(), rig.app.prefs().smooth), ("set scroll jump", false));
    rig.press("esc");
    assert!(rig.app.prefs().smooth);
    // Up and Down walk the table of options too, the name following.
    line_types(&mut rig, "set");
    rig.press("down");
    rig.press("down");
    assert_eq!(typed(&mut rig), "set scale");
    assert!(bar(&mut rig).unwrap().starts_with("scale ."));
    rig.press("up");
    assert_eq!(typed(&mut rig), "set face");
    // A few letters of a name are enough there as well.
    for _ in 0..4 {
        rig.press("backspace");
    }
    rig.type_text("me");
    assert!(bar(&mut rig).unwrap().starts_with("measure ."));
    assert_eq!(said(&mut rig), "Enter changes measure. It is 66 now.");
}

#[test]
fn numbers_are_typed_or_nudged() {
    let (mut rig, _) = rig();
    line_types(&mut rig, "set measure ");
    assert_eq!(said(&mut rig), "Enter keeps measure at 66. The default is 66.");
    rig.press("up");
    assert_eq!((typed(&mut rig).as_str(), rig.app.grid().measure), ("set measure 67", 67));
    rig.press("down");
    rig.press("down");
    assert_eq!((typed(&mut rig).as_str(), rig.app.grid().measure), ("set measure 65", 65));
    assert_eq!(said(&mut rig), "Enter keeps measure at 65. Esc puts it back to 66.");
    // Typed: it is tried as soon as it is a value the option can take.
    for _ in 0..2 {
        rig.press("backspace");
    }
    assert_eq!(rig.app.grid().measure, 66, "nothing typed is the value it had");
    rig.type_text("4");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Measure takes 40 to 100.");
    assert_eq!(rig.app.grid().measure, 66);
    rig.type_text("0");
    assert_eq!(rig.app.grid().measure, 40);
    rig.press("down");
    assert_eq!(typed(&mut rig), "set measure 40", "it stops at its least");
    rig.type_text("0");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Measure takes 40 to 100.");
    assert_eq!(rig.app.grid().measure, 66, "what cannot be is not half applied");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? Measure takes 40 to 100.");
    rig.press("esc");
    // Scale stops at 1 and 3; a share of the ink at 0 and 100.
    line_types(&mut rig, "set scale ");
    for _ in 0..5 {
        rig.press("up");
    }
    assert_eq!((typed(&mut rig).as_str(), rig.app.grid().scale), ("set scale 3", 3));
    rig.press("esc");
    assert_eq!(rig.app.grid().scale, 1);
    line_types(&mut rig, "set half ");
    rig.press("up");
    assert_eq!(value(&rig, "half"), "66");
    rig.press("esc");
    // The limit is nudged by fifties, up from nothing.
    line_types(&mut rig, "set limit ");
    rig.press("up");
    assert_eq!(typed(&mut rig), "set limit 50");
    rig.press("down");
    assert_eq!((typed(&mut rig).as_str(), value(&rig, "limit")), ("set limit off", String::from("off")));
}

#[test]
fn what_an_option_cannot_take_is_said_and_changes_nothing() {
    let cases = [
        ("scale 4", "? Scale takes 1 to 3."),
        ("paper cream", "? Paper takes a colour as six hex digits."),
        ("mark none", "? Mark takes six hex digits, or ink."),
        ("half 101", "? Half takes 0 to 100, or six hex digits."),
        ("platen maybe", "? Platen takes on or off."),
        ("scroll fast", "? Scroll takes smooth or jump."),
        ("cursor bar", "? Cursor takes block or underline."),
        ("enter both", "? The enter setting takes paragraph or line."),
        ("window tiled", "? Window takes full or framed."),
        ("limit many", "? Limit takes a number of words, or off."),
        ("face helvetica", "? Face takes one of the faces listed."),
        ("look custom", "? Look takes a name of 24 letters at most."),
        ("folder ", "? Folder takes the path of a folder."),
    ];
    for (typed_after_set, mistake) in cases {
        let (mut rig, folder) = rig();
        let before = picture(&mut rig);
        line_types(&mut rig, &format!("set {typed_after_set}"));
        let waiting = said(&mut rig);
        assert_eq!(waiting, mistake.replacen("? ", "Enter does nothing yet. ", 1), "{typed_after_set}");
        rig.press("enter");
        assert_eq!(said(&mut rig), mistake);
        assert_eq!(typed(&mut rig), format!("set {typed_after_set}").trim_end(), "nothing blocks");
        rig.press("esc");
        assert_eq!(picture(&mut rig), before, "{typed_after_set}");
        assert_eq!(folder.borrow().settings, None);
    }
}

#[test]
fn a_colour_set_by_hand_makes_the_look_custom_and_a_new_name_keeps_it() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "set paper 10");
    assert_eq!(rig.app.look(), &Look::manila(), "not a colour yet");
    rig.type_text("1");
    assert_eq!(rig.app.look().paper, Rgb(0x11, 0x00, 0x11), "three digits are a colour");
    rig.type_text("010");
    // Tried: the sheet is that colour, and the look reads custom.
    assert_eq!(rig.app.look().paper, Rgb(0x10, 0x10, 0x10));
    assert_eq!(rig.pixel(700, 300), Some(Rgb(0x10, 0x10, 0x10)));
    assert!(contents(&mut rig)[0].ends_with(". custom"));
    assert_eq!(said(&mut rig), "Enter keeps paper at 101010. Esc puts it back to EEE3C8.");
    rig.press("enter");
    rig.type_text(" ink e0e0e0");
    rig.press("enter");
    assert_eq!(value(&rig, "look"), "custom");
    // The list of looks has custom in it while that is what is in force.
    rig.type_text(" look ");
    assert_eq!(contents(&mut rig)[10], "  custom");
    assert_eq!(bar(&mut rig).as_deref(), Some("custom"));
    // A new name after `set look` keeps the present colours under it.
    rig.type_text("dusk");
    assert_eq!(said(&mut rig), "Enter keeps these colours as dusk.");
    assert_eq!(value(&rig, "look"), "custom", "not yet");
    rig.press("enter");
    assert_eq!(value(&rig, "look"), "dusk");
    assert_eq!(said(&mut rig), "Enter changes look. It is dusk now.");
    assert_eq!(rig.app.looks().len(), 1);
    // It is a look like any other from then on.
    rig.type_text(" look sun");
    rig.press("enter");
    assert_eq!(rig.app.look(), &Look::named("sun").unwrap());
    rig.type_text(" look ");
    assert_eq!(contents(&mut rig)[10], "  dusk", "listed after the built-in ones");
    rig.type_text("du");
    assert_eq!((rig.app.look().paper, rig.app.look().ink), (Rgb(0x10, 0x10, 0x10), Rgb(0xE0, 0xE0, 0xE0)));
    rig.press("enter");
    rig.press("esc");
    assert_eq!(rig.app.look().name, "dusk");
    // The settings text makes the look the way the line did, then names it.
    let written = folder.borrow().settings.clone().unwrap();
    assert!(written.starts_with("paper 101010\nink E0E0E0\ndesk 35251A\ndesk-ink E6D9BA\nmark ink\nhalf 65\nfaint 35\nlook dusk\nlook dusk\nface gallant\n"), "{written}");
    // Setting a colour back by hand to what a look has makes it that look.
    line_types(&mut rig, "set look manila");
    rig.press("enter");
    rig.type_text(" desk 000");
    rig.press("enter");
    assert_eq!(value(&rig, "look"), "custom");
    rig.type_text(" desk default");
    assert_eq!(value(&rig, "look"), "manila");
    rig.press("enter");
    rig.press("esc");
    assert_eq!(rig.app.look(), &Look::manila());
}

#[test]
fn the_settings_are_kept_through_the_session_and_read_at_launch() {
    let (mut rig, folder) = rig();
    assert_eq!(folder.borrow().settings_written, 0, "nothing is written until something is set");
    for what in ["look vt100", "face terminus", "scale 2", "measure 58", "platen off", "scroll jump", "cursor underline", "focus off", "echo off", "enter line", "window framed"] {
        line_types(&mut rig, &format!("set {what}"));
        rig.press("enter");
        rig.press("esc");
    }
    assert_eq!(folder.borrow().settings_written, 11, "once for each");
    let written = folder.borrow().settings.clone().unwrap();
    assert_eq!(written, "look vt100\nface terminus\nscale 2\nmeasure 58\nplaten off\nscroll jump\ncursor underline\nfocus off\necho off\nenter line\nwindow framed\n");
    // A launch with that text behind it comes up the same.
    let launch = |text: &str| {
        let texts = desk();
        let borrowed: Vec<(&str, &str)> = texts.iter().map(|(name, text)| (*name, text.as_str())).collect();
        let (shelf, folder) = Shelf::new(&borrowed, "College/The orchard");
        folder.borrow_mut().settings = Some(text.to_string());
        let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
        rig.size(1920, 1080);
        (rig, folder)
    };
    let (mut again, folder) = launch(&written);
    for (name, typed, _) in TRIED {
        let expected = written.lines().find_map(|line| line.strip_prefix(&format!("{name} "))).map(str::to_string);
        if let Some(expected) = expected {
            assert_eq!(value(&again, name), expected, "{name} (tried with {typed})");
        }
    }
    assert_eq!(again.app.look(), &Look::named("vt100").unwrap());
    assert_eq!((again.app.grid().scale, again.app.grid().measure, again.app.grid().cell_w), (2, 58, 22));
    assert!(!again.wants().fullscreen);
    assert_eq!(folder.borrow().settings_written, 0, "reading them writes nothing");
    assert_eq!(again.pixel(3, 3), Some(Rgb(0x15, 0x18, 0x19)));
    // The chords that change a setting keep it too.
    again.press("ctrl+-");
    assert!(folder.borrow().settings.clone().unwrap().contains("scale 1\n"));
    again.press("f11");
    assert!(folder.borrow().settings.clone().unwrap().contains("window full\n"));
    // A text with slips in it starts the program all the same: each line
    // that makes sense is taken and the rest are passed over.
    let (odd, _) = launch("mesure 70\nmeasure 700\nmeasure 70\n\nlook\npaper nothing\nscale 9\n# a note to self\nlimit 20\nfolder C:\\nowhere\ncursor underline\nface no such face\n   echo   off  \n");
    assert_eq!((value(&odd, "measure"), value(&odd, "cursor"), value(&odd, "echo")), (String::from("70"), String::from("underline"), String::from("off")));
    assert_eq!((value(&odd, "scale"), value(&odd, "face"), value(&odd, "limit")), (String::from("1"), String::from("gallant"), String::from("off")));
    assert_eq!(odd.app.look(), &Look::manila());
    // A look of his own is made by the text exactly as the line makes one.
    let (mut own, _) = launch("paper 101010\nink E0E0E0\ndesk 000000\ndesk-ink C0C0C0\nmark ink\nhalf 70\nfaint 40\nlook dusk\nlook sun\n");
    assert_eq!(own.app.look(), &Look::named("sun").unwrap());
    assert_eq!(own.app.looks().len(), 1);
    line_types(&mut own, "set look dusk");
    assert_eq!((own.app.look().paper, own.app.look().desk_ink, value(&own, "half")), (Rgb(0x10, 0x10, 0x10), Rgb(0xC0, 0xC0, 0xC0), String::from("70")));
    // And colours with no name come back as custom.
    let (unnamed, _) = launch("paper 101010\nink E0E0E0\n");
    assert_eq!(value(&unnamed, "look"), "custom");
    assert_eq!(unnamed.app.look().desk, Look::manila().desk);
    // What a launch is asked for by hand comes on top of the settings.
    let texts = desk();
    let borrowed: Vec<(&str, &str)> = texts.iter().map(|(name, text)| (*name, text.as_str())).collect();
    let (shelf, folder) = Shelf::new(&borrowed, "College/The orchard");
    folder.borrow_mut().settings = Some(String::from("look amber\n"));
    let asked = Rig::with_seams(Options { look: Some(String::from("sun")), ..Options::default() }, Box::new(shelf), line::start(), Box::new(Rest));
    assert_eq!(asked.app.look().name, "sun");
}

#[test]
fn the_folder_is_changed_by_enter_and_never_only_tried() {
    let (mut rig, folder) = rig();
    line_types(&mut rig, "set folder");
    assert_eq!(said(&mut rig), "Enter changes folder. It is C:\\Users\\jaspe\\Documents\\ok now.");
    rig.press("enter");
    assert_eq!(typed(&mut rig), "set folder");
    assert_eq!(said(&mut rig), "Enter does nothing yet. Folder takes the path of a folder.");
    rig.type_text("D:\\Essays\\College");
    assert_eq!(said(&mut rig), "Enter writes in College from now on.");
    assert_eq!(folder.borrow().folder, PathBuf::from("C:\\Users\\jaspe\\Documents\\ok"), "nothing has moved yet");
    rig.press("enter");
    assert_eq!(folder.borrow().folder, PathBuf::from("D:\\Essays\\College"));
    assert_eq!(rig.text(), "Written in the other folder.");
    // The line stays at `ok set`, and the table reads the new folder.
    assert_eq!(typed(&mut rig), "set");
    assert!(contents(&mut rig)[19].ends_with(". D:\\Essays\\College"));
    // Esc now goes back to the text that is open, not the one that was.
    rig.press("esc");
    assert_eq!(rig.text(), "Written in the other folder.");
    assert_eq!(rig.screen().kept, None);
    // A folder that cannot be used is the session's to refuse.
    line_types(&mut rig, "set folder C:\\nowhere");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? There is no folder at C:\\nowhere.");
    assert_eq!(typed(&mut rig), "set folder C:\\nowhere");
    assert_eq!(folder.borrow().folder, PathBuf::from("D:\\Essays\\College"));
}

/// A host with two fixed-pitch faces installed.
#[derive(Default)]
struct WithFaces {
    inner: TestHost,
    asked: Vec<(String, u32)>,
}

impl Host for WithFaces {
    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    fn local_time(&self) -> LocalTime {
        self.inner.local_time()
    }

    fn waker(&self) -> Waker {
        self.inner.waker()
    }

    fn clipboard_get(&mut self) -> Option<String> {
        self.inner.clipboard_get()
    }

    fn clipboard_set(&mut self, text: &str) {
        self.inner.clipboard_set(text)
    }

    fn documents_dir(&self) -> PathBuf {
        PathBuf::from("C:\\Users\\jaspe\\Documents")
    }

    fn local_data_dir(&self) -> PathBuf {
        PathBuf::from("C:\\Users\\jaspe\\AppData\\Local\\ok")
    }

    fn roaming_data_dir(&self) -> PathBuf {
        PathBuf::from("C:\\Users\\jaspe\\AppData\\Roaming\\ok")
    }

    fn system_faces(&mut self) -> Vec<String> {
        vec![String::from("Consolas"), String::from("Courier New")]
    }

    fn load_face_file(&mut self, _path: &Path) -> Option<String> {
        None
    }

    fn face_cell(&mut self, family: &str, height: u32) -> Option<FaceCell> {
        self.asked.push((family.to_string(), height));
        match family {
            "Courier New" => Some(FaceCell { width: 12, height: 23, ascent: 18 }),
            "Consolas" => Some(FaceCell { width: 11, height: 23, ascent: 18 }),
            _ => None,
        }
    }

    fn rasterize(&mut self, family: &str, height: u32, ch: char) -> Option<AlphaGlyph> {
        self.inner.rasterized.push(ch);
        (family == "Courier New" && height == 20 && ch.is_ascii_graphic()).then(|| AlphaGlyph { width: 6, height: 10, left: 3, top: 8, coverage: vec![255; 60] })
    }

    fn open_in_browser(&mut self, url: &str) {
        self.inner.open_in_browser(url)
    }

    fn reduced_motion(&self) -> bool {
        false
    }
}

#[test]
fn the_system_s_fixed_pitch_faces_are_listed_and_rasterized_into_the_grid() {
    let mut host = WithFaces::default();
    let (shelf, folder) = Shelf::new(&[("Notes", "Typed in Courier.")], "Notes");
    let mut app = App::with_seams(&mut host, Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    let mut frame = Frame::new(1920, 1080);
    app.event(Event::Resize { width: 1920, height: 1080 }, &mut host);
    let key = |app: &mut App, host: &mut WithFaces, key: Key| app.event(Event::Key { key, mods: Mods::NONE, repeat: false }, host);
    let types = |app: &mut App, host: &mut WithFaces, text: &str| text.chars().for_each(|ch| app.event(Event::Text(ch), host));
    key(&mut app, &mut host, Key::Escape);
    types(&mut app, &mut host, "set face ");
    app.render(&mut frame, &mut host);
    // The three inside the program, then the system's.
    let cols = app.grid().contents.unwrap();
    let rows: Vec<String> = (3..8).map(|row| app.screen().span(cols, row)).collect();
    assert_eq!(rows, ["  gallant", "  spleen", "  terminus", "  Consolas", "  Courier New"]);
    // A few letters land the bar, and the whole screen is in that face:
    // the cell is the face's, and every glyph on it came from the host.
    types(&mut app, &mut host, "cou");
    assert_eq!(app.face_name(), "Courier New");
    assert_eq!(host.asked, [(String::from("Consolas"), 20), (String::from("Courier New"), 20)], "the host measured each as the bar landed on it");
    assert_eq!((app.grid().cell_w, app.grid().cell_h, app.grid().rows), (12, 23, 46));
    host.inner.rasterized.clear();
    app.render(&mut frame, &mut host);
    for ch in ['T', 'y', 'p', 'C', 'o', 'u', 'r', '.', 'k'] {
        assert!(host.inner.rasterized.contains(&ch), "{ch} was asked of the host");
    }
    let text = app.grid().text;
    let row = app.screen().find("Typed in Courier.").map(|(_, row)| row).unwrap();
    assert_eq!(app.screen().span(text, row), "Typed in Courier.");
    let say = app.grid().say.unwrap();
    assert_eq!(app.screen().span(text, say), "Enter keeps face at Courier New. Esc puts it back to gallant.");
    // A glyph is where the host put it in its cell: six pixels in, eight
    // down, solid.
    let cell = app.grid().cells(text.first, row, 1, 1);
    let look = Look::manila();
    assert_eq!(frame.get(cell.x + 3, cell.y + 8), Some(look.shade(Surface::Sheet, Strength::INK)));
    assert_eq!(frame.get(cell.x, cell.y), Some(look.paper));
    // Asked once: a second frame costs the host nothing.
    host.inner.rasterized.clear();
    app.render(&mut frame, &mut host);
    assert!(host.inner.rasterized.is_empty());
    // Esc puts Gallant back; Enter keeps the face, and it is written.
    key(&mut app, &mut host, Key::Escape);
    assert_eq!((app.face_name(), app.grid().cell_h), ("gallant", 22));
    key(&mut app, &mut host, Key::Escape);
    types(&mut app, &mut host, "set face courier new");
    key(&mut app, &mut host, Key::Enter);
    key(&mut app, &mut host, Key::Escape);
    assert_eq!(app.face_name(), "Courier New");
    assert!(folder.borrow().settings.clone().unwrap().contains("face Courier New\n"));
    // And a launch with that written comes up in it.
    let written = folder.borrow().settings.clone().unwrap();
    let (shelf, folder) = Shelf::new(&[("Notes", "Typed in Courier.")], "Notes");
    folder.borrow_mut().settings = Some(written);
    let app = App::with_seams(&mut host, Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    assert_eq!(app.face_name(), "Courier New");
}
