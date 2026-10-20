//! "Arrows try, Enter keeps, Esc puts back": after every kind of trying,
//! Esc leaves the text, the cursor, the paper, the look and every cell of
//! the screen exactly as they were, and nothing that was only looked at is
//! ever handed to the session as his text.

mod line_support;

use line_support::*;
use ok_host::Event;
use ok_text::{Pos, Span};
use ok_ui::harness::Rig;
use ok_ui::line::settings;

/// Something done on the open line that Esc must undo.
type Trying = (&'static str, fn(&mut Rig));

/// Types on the line and lets the bar rest.
fn rested(rig: &mut Rig, text: &str) {
    rig.type_text(text);
    rig.wait(250);
}

const EVERY_KIND: [Trying; 31] = [
    ("nothing at all", |_| {}),
    ("the list of what it understands", |rig| rig.wait(1500)),
    ("a title, before the bar has rested", |rig| rig.type_text("grand")),
    ("a title, with the text under the bar on the page", |rig| rested(rig, "grand")),
    ("a title, the bar walked up and down", |rig| {
        rested(rig, "o");
        rig.press("down");
        rig.wait(250);
        rig.press("down");
        rig.press("up");
        rig.wait(100);
    }),
    ("a title that finds nothing", |rig| rested(rig, "Rebuttal notes")),
    ("the bar moved on an empty line", |rig| {
        rig.press("down");
        rig.wait(250);
        rig.press("pagedown");
    }),
    ("a heading gone to", |rig| rig.type_text("#showed")),
    ("headings walked", |rig| {
        rig.type_text("#");
        rig.press("up");
        rig.press("up");
        rig.press("down");
    }),
    ("a phrase found", |rig| {
        rig.type_text("/apricots");
        rig.press("down");
        rig.press("f3");
    }),
    ("a phrase not found", |rig| rig.type_text("/plums")),
    ("a change typed and not made", |rig| rig.type_text("/apricots/plums")),
    ("a change of all typed and not made", |rig| rig.type_text("/apricots/plums/g")),
    ("a new title typed after rename", |rig| rig.type_text("rename Something else")),
    ("an earlier version on the page", |rig| {
        rested(rig, "earlier");
        rig.press("down");
        rig.wait(250);
    }),
    ("the table of options", |rig| {
        rig.type_text("set");
        rig.press("down");
        rig.press("down");
    }),
    ("a look walked to", |rig| {
        rig.type_text("set look ");
        for _ in 0..5 {
            rig.press("down");
        }
    }),
    ("a look typed", |rig| rig.type_text("set look wordperfect")),
    ("a face", |rig| rig.type_text("set face terminus")),
    ("a scale", |rig| rig.type_text("set scale 3")),
    ("a measure nudged", |rig| {
        rig.type_text("set measure ");
        for _ in 0..9 {
            rig.press("up");
        }
    }),
    ("colours set by hand", |rig| rig.type_text("set desk 000000")),
    ("a shade", |rig| rig.type_text("set faint 60")),
    ("the cursor", |rig| rig.type_text("set cursor underline")),
    ("the window", |rig| rig.type_text("set window framed")),
    ("a word limit", |rig| rig.type_text("set limit 400")),
    ("one option tried and then another", |rig| {
        rig.type_text("set look amber");
        for _ in 0..10 {
            rig.press("backspace");
        }
        rig.type_text("face spleen");
    }),
    ("the keys read", |rig| {
        rig.type_text("keys");
        rig.press("enter");
        rig.press("pagedown");
        rig.press("down");
        rig.wheel(-3);
    }),
    ("the notices read", |rig| {
        rig.type_text("notices");
        rig.press("enter");
        rig.press("pagedown");
        rig.press("end");
        rig.wait(2000);
    }),
    ("a word that acts, typed and not entered", |rig| {
        rig.type_text("discard");
        rig.type_text(" ");
        for _ in 0..8 {
            rig.press("backspace");
        }
        rig.type_text("quit");
    }),
    ("the wheel turned under the line", |rig| {
        rig.type_text("orch");
        rig.wheel(4);
        rig.wheel(-2);
    }),
];

/// Everything that must come back: what [`picture`] holds, and where the
/// selection began.
fn snapshot(rig: &mut Rig) -> ((String, (usize, usize), i64, ok_ui::look::Look, Vec<String>), Pos, Vec<String>) {
    let values = settings::OPTIONS.iter().map(|opt| settings::value(&rig.app, opt)).collect();
    (picture(rig), rig.app.page().cursor.anchor, values)
}

/// The desk with two kept versions of the open text behind it.
fn desk_rig(open: &str) -> (Rig, Folder) {
    let (rig, folder) = rig_at(open);
    folder.borrow_mut().versions = vec![(String::from("Today 13:40, kept"), String::from("The second draft.")), (String::from("Yesterday 22:10"), String::from("# First\n\nThe first draft of it."))];
    (rig, folder)
}

#[test]
fn esc_puts_back_exactly_what_was_there_after_every_kind_of_trying() {
    for (what, trying) in EVERY_KIND {
        // From three places: the end of a short text, the middle of a long
        // one with the paper moved, and with a stretch selected.
        for start in 0..3 {
            let (mut rig, folder) = desk_rig(if start == 1 { "College/Grandfather interview" } else { "College/The orchard" });
            match start {
                1 => {
                    rig.app.go(Pos::new(40, 12));
                    rig.wheel(-5);
                }
                2 => rig.app.select(Span { start: Pos::new(2, 4), end: Pos::new(6, 20) }),
                _ => {}
            }
            rig.send(Event::Tick);
            let text = rig.text();
            let before = snapshot(&mut rig);
            let handed = folder.borrow().handed.len();
            rig.press("esc");
            trying(&mut rig);
            rig.press("esc");
            assert_eq!(snapshot(&mut rig), before, "{what}, from start {start}");
            assert_eq!(rig.screen().kept, None, "{what}: the cursor is on the page again");
            // The folder is as it was, nothing was kept, and whatever the
            // session was handed on the way was his text.
            let state = folder.borrow();
            assert_eq!(state.text(state.open_name()), Some(text.as_str()), "{what}");
            assert_eq!((state.settings.as_ref(), state.limits.len(), state.names().len()), (None, 0, 6), "{what}");
            assert!(state.handed[handed..].iter().all(|(_, given)| *given == text), "{what}: the session was handed something else");
            drop(state);
            // And the next letter goes into the text, where the cursor is.
            if start != 2 {
                let at = head(&rig);
                rig.type_text("x");
                assert_eq!(rig.text().len(), text.len() + 1, "{what}");
                assert_eq!(head(&rig), (at.0, at.1 + 1), "{what}");
            }
        }
    }
}

#[test]
fn enter_keeps_what_was_tried_and_esc_afterwards_has_nothing_to_put_back() {
    // A heading gone to.
    let (mut rig, _) = rig();
    line_types(&mut rig, "#thin");
    rig.press("enter");
    assert_eq!(head(&rig), (4, 0));
    rig.press("esc");
    rig.press("esc");
    assert_eq!(head(&rig), (4, 0));
    // A hit stayed at.
    line_types(&mut rig, "/weight");
    rig.press("enter");
    let at = head(&rig);
    line_types(&mut rig, "#orchard");
    rig.press("esc");
    assert_eq!(head(&rig), at);
    // A value kept, and another only tried after it.
    line_types(&mut rig, "set look acme");
    rig.press("enter");
    rig.type_text(" measure 50");
    rig.press("esc");
    assert_eq!((rig.app.look().name.as_str(), rig.app.grid().measure), ("acme", 66));
    // A change made from the line stays made, and Esc goes back to it.
    rig.press("ctrl+home");
    line_types(&mut rig, "/apricots/plums");
    rig.press("enter");
    rig.press("esc");
    assert_eq!(rig.text().matches("plums").count(), 1);
    assert_eq!(head(&rig), (6, 37), "just after the word that was changed");
    assert_eq!(rig.screen().kept, None);
}

#[test]
fn a_text_that_is_only_shown_is_never_taken_for_his() {
    let shown: [Trying; 4] = [
        ("another text under the bar", |rig| rested(rig, "grand")),
        ("an earlier version", |rig| rested(rig, "earlier")),
        ("the keys", |rig| rig.press("f1")),
        ("the notices", |rig| {
            rig.type_text("notices");
            rig.press("enter");
        }),
    ];
    for (what, show) in shown {
        let (mut rig, folder) = desk_rig("College/The orchard");
        rig.type_text(" More.");
        let text = format!("{ORCHARD} More.");
        rig.press("esc");
        show(&mut rig);
        assert!(rig.app.showing(), "{what}");
        assert_ne!(rig.text(), text, "{what}: something else is on the page");
        assert_eq!(rig.app.text().doc().to_text("\n"), text);
        // Time passes, the window is asked to close, a second launch hands
        // a file over: each time the session is given his text.
        folder.borrow_mut().handed.clear();
        rig.wait(3000);
        rig.send(Event::Tick);
        rig.send(Event::Close);
        rig.send(Event::Open(std::path::PathBuf::from("C:/elsewhere/Other.md")));
        rig.press("ctrl+s");
        let state = folder.borrow();
        assert!(state.handed.iter().any(|(why, _)| *why == "tick") && state.handed.iter().any(|(why, _)| *why == "close"), "{what}");
        assert!(state.handed.iter().all(|(_, given)| *given == text), "{what}: {:?}", state.handed.iter().map(|(why, given)| (*why, given.len())).collect::<Vec<_>>());
        assert_eq!(state.text("College/The orchard"), Some(text.as_str()));
        drop(state);
        // Typing does nothing to what is shown; it goes to the line.
        let on_page = rig.text();
        rig.type_text("q");
        assert_ne!(rig.text(), format!("{on_page}q"));
        // Leaving from here closes with his text too.
        rig.press("ctrl+q");
        assert!(rig.wants().quit);
        assert_eq!(folder.borrow().handed.last().cloned(), Some(("close", text.clone())), "{what}");
    }
}

#[test]
fn what_the_session_refuses_leaves_the_line_waiting_and_esc_still_puts_back() {
    let attempts = ["grand", "A new text", "rename Other", "discard", "earlier", "set folder D:\\Essays"];
    for typed_on_line in attempts {
        let (mut rig, folder) = desk_rig("College/The orchard");
        let before = snapshot(&mut rig);
        folder.borrow_mut().refuse = Some(String::from("? Not saved. This folder cannot be written."));
        line_types(&mut rig, typed_on_line);
        rig.wait(250);
        rig.press("enter");
        assert_eq!(said(&mut rig), "? Not saved. This folder cannot be written.", "{typed_on_line}");
        assert_eq!(typed(&mut rig), typed_on_line);
        rig.press("esc");
        assert_eq!(snapshot(&mut rig), before, "{typed_on_line}");
        assert_eq!(folder.borrow().names().len(), 6);
    }
}
