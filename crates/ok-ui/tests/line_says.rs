//! Every sentence the line can say, as the say row shows it.
//!
//! Where DESIGN.md gives the sentence it is the sentence; the rest are in
//! the same voice. Each one is whole on a row of 66 cells, in sentence
//! case, ends with a full stop, and has no exclamation mark, arrow or
//! middle dot in it. While the line is open the sentence says what Enter
//! will do now; a mistake is `?` and one sentence.

mod line_support;

use line_support::*;
use ok_text::str_width;
use ok_ui::harness::Rig;

/// The rules every sentence keeps. The say row is as wide as the measure,
/// which is 66 cells unless a test has set another.
fn well_said(sentence: &str) {
    well_said_in(sentence, 66);
}

fn well_said_in(sentence: &str, room: usize) {
    assert!(str_width(sentence) <= room, "{} cells: {sentence}", str_width(sentence));
    assert!(sentence.ends_with('.'), "{sentence}");
    for banned in ['!', '\u{2192}', '\u{2190}', '\u{00B7}', '\u{2022}', '\u{2014}', '\u{2026}'] {
        assert!(!sentence.contains(banned), "{banned} in: {sentence}");
    }
    // Sentence case, in every sentence of it.
    let opening = sentence.strip_prefix("? ").unwrap_or(sentence);
    for part in opening.split(". ") {
        assert!(part.starts_with(|first: char| first.is_uppercase() || first.is_ascii_digit()), "{sentence}");
    }
}

/// What the say row reads after `typed` on a fresh line, checked against
/// the rules.
fn says(rig: &mut Rig, typed_on_line: &str) -> String {
    rig.press("esc");
    if rig.screen().kept.is_none() {
        rig.press("esc");
    }
    rig.type_text(typed_on_line);
    let sentence = said(rig);
    well_said_in(&sentence, rig.app.grid().measure as usize);
    sentence
}

/// The desk, with The orchard open, two kept versions behind it, and the
/// cursor at the top.
fn desk_rig() -> (Rig, Folder) {
    let (mut rig, folder) = rig();
    folder.borrow_mut().versions = vec![(String::from("Today 13:40, kept"), String::from("The second draft.")), (String::from("Yesterday 22:10"), String::from("The first."))];
    rig.press("ctrl+home");
    (rig, folder)
}

#[test]
fn what_enter_will_do_is_said_before_it_is_pressed() {
    let (mut rig, _) = desk_rig();
    let expected = [
        ("", "Enter goes back to where you were."),
        ("orch", "Enter opens College\\Orchard notes."),
        ("the orchard", "Enter opens College\\The orchard."),
        ("Rebuttal notes", "Enter starts a new text called Rebuttal notes."),
        ("#", "Enter goes to The orchard."),
        ("#showed", "Enter goes to What he showed me."),
        ("/apricots", "1 of 3. Up and Down step through them. Enter stays here."),
        ("/Apricots", "1 of 1. Enter stays here."),
        ("/apricots/plums", "1 of 3. Enter changes this one to plums and moves on."),
        ("/apricots/", "1 of 3. Enter takes this one out and moves on."),
        ("/apricots/plums/g", "Enter changes all 3 to plums. Ctrl Z takes them all back."),
        ("/apricots//g", "Enter takes all 3 out. Ctrl Z takes them all back."),
        ("/weight/load/g", "Enter changes the only one to load. Ctrl Z takes it back."),
        ("/weight//g", "Enter takes the only one out. Ctrl Z takes it back."),
        ("rename The trees", "Enter renames this text and mends 3 links to it."),
        ("set", "Enter changes look. It is manila now."),
        ("set fol", "Enter changes folder. It is C:\\Users\\jaspe\\Documents\\ok now."),
        // DESIGN.md's sentence for a value is 69 cells with two-figure
        // values in it. Tried at a measure that wide it is whole; at the
        // default it gives up its last part.
        ("set measure 72", "Enter keeps measure at 72. Esc puts it back to 66. The default is 66."),
        ("set measure 65", "Enter keeps measure at 65. Esc puts it back to 66."),
        ("set measure 66", "Enter keeps measure at 66. The default is 66."),
        ("set look amber", "Enter keeps look at amber. Esc puts it back to manila."),
        ("set look dusk", "Enter keeps these colours as dusk."),
        ("set folder D:\\Essays", "Enter writes in Essays from now on."),
        ("earlier", "Enter brings back Today 13:40, kept. The present one is kept too."),
        ("discard", "Enter moves this text to .discarded. Nothing is erased."),
        ("copy", "Enter copies 48 words with the marks removed."),
        ("keys", "Enter shows the keys on the page."),
        ("notices", "Enter shows the licences of the faces and code in the .exe."),
        ("quit", "Enter leaves. Nothing can be lost."),
    ];
    for (typed_on_line, sentence) in expected {
        assert_eq!(says(&mut rig, typed_on_line), sentence, "after {typed_on_line:?}");
        assert!(sentence.contains("Enter "), "{sentence}");
    }
    // With somewhere to go back to, the empty line names it.
    says(&mut rig, "grand");
    rig.press("enter");
    assert_eq!(says(&mut rig, ""), "Enter goes back to where you were, in The orchard.");
    // And under the keys or the notices it names the text he was in.
    rig.type_text("notices");
    rig.press("enter");
    let sentence = said(&mut rig);
    well_said(&sentence);
    assert_eq!(sentence, "Enter goes back to where you were, in Grandfather interview.");
}

#[test]
fn a_dead_end_says_so_and_enter_answers_with_a_question_mark() {
    let (mut rig, folder) = desk_rig();
    folder.borrow_mut().versions.clear();
    let expected = [
        ("#zzz", "No heading reads zzz."),
        ("/", "Type the phrase to find after the slash."),
        ("/plums", "Nothing here reads plums."),
        ("/plums/pears", "Nothing here reads plums."),
        ("/apricots/plums/x", "Only g may follow the last slash."),
        ("rename", "Type the new title after rename."),
        ("earlier", "Nothing earlier is kept of this text."),
        ("set measure 7", "Measure takes 40 to 100."),
        ("set scale 0", "Scale takes 1 to 3."),
        ("set ink black", "Ink takes a colour as six hex digits."),
        ("set desk-ink 12", "Desk-ink takes a colour as six hex digits."),
        ("set mark dark", "Mark takes six hex digits, or ink."),
        ("set faint 400", "Faint takes 0 to 100, or six hex digits."),
        ("set focus maybe", "Focus takes on or off."),
        ("set echo x", "Echo takes on or off."),
        ("set enter x", "The enter setting takes paragraph or line."),
        ("set limit lots", "Limit takes a number of words, or off."),
        ("set face comic", "Face takes one of the faces listed."),
        ("set folder ", "Folder takes the path of a folder."),
        ("set colour red", "No option is called colour."),
    ];
    for (typed_on_line, reason) in expected {
        assert_eq!(says(&mut rig, typed_on_line), format!("Enter does nothing yet. {reason}"), "after {typed_on_line:?}");
        rig.press("enter");
        let answer = said(&mut rig);
        well_said(&answer);
        if typed_on_line == "set colour red" {
            assert_eq!(answer, "? No option called colour. The nearest is folder.");
        } else {
            assert_eq!(answer, format!("? {reason}"), "after {typed_on_line:?}");
        }
        // The next key clears it, whatever the key.
        rig.press("right");
        assert_eq!(said(&mut rig), format!("Enter does nothing yet. {reason}"));
    }
    // DESIGN.md's own example of a mistake.
    says(&mut rig, "set mesure 70");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? No option called mesure. The nearest is measure.");
    // A text with no headings and one with nothing in it.
    let (mut rig, _) = rig_with(&[("Notes", "One paragraph."), ("Empty", "")], "Notes", 1920, 1080);
    assert_eq!(says(&mut rig, "#two"), "Enter does nothing yet. No paragraph starts two.");
    assert_eq!(says(&mut rig, "empty"), "Enter opens Empty.");
    rig.press("enter");
    assert_eq!(says(&mut rig, "#"), "Enter does nothing yet. This text has nothing to go to.");
    assert_eq!(says(&mut rig, "copy"), "Enter copies 0 words with the marks removed.");
}

#[test]
fn what_was_done_is_said_when_the_line_has_closed() {
    let after = |typed_on_line: &str, enters: usize| -> String {
        let (mut rig, _) = desk_rig();
        line_types(&mut rig, typed_on_line);
        for _ in 0..enters {
            rig.press("enter");
        }
        assert_eq!(rig.screen().kept, None, "{typed_on_line}: the line has closed");
        let sentence = said(&mut rig);
        well_said(&sentence);
        sentence
    };
    assert_eq!(after("/apricots/plums/g", 1), "Changed 3. Ctrl Z takes them all back.");
    assert_eq!(after("/weight/load/g", 1), "Changed 1. Ctrl Z takes it back.");
    assert_eq!(after("/apricots/plums", 3), "Changed 3. That was the last one.");
    assert_eq!(after("/weight/load", 1), "Changed 1. That was the only one.");
    assert_eq!(after("rename The trees", 1), "Renamed to The trees and mended 3 links.");
    assert_eq!(after("rename Essays\\The trees", 1), "Renamed to The trees and mended 3 links.");
    assert_eq!(after("earlier", 1), "Brought back Today 13:40, kept. The present one is kept too.");
    assert_eq!(after("discard", 1), "Moved to .discarded. Nothing was erased.");
    assert_eq!(after("copy", 1), "Copied 48 words with the marks removed.");
    // What F3 says on the page.
    let (mut rig, _) = desk_rig();
    rig.press("f3");
    let nothing_yet = said(&mut rig);
    well_said(&nothing_yet);
    assert_eq!(nothing_yet, "Nothing has been looked for yet. Ctrl F finds a phrase.");
    line_types(&mut rig, "/weight");
    rig.press("enter");
    rig.press("f3");
    assert_eq!(said(&mut rig), "1 of 1. F3 and Shift F3 step through them.");
    rig.press("ctrl+a");
    rig.type_text("All gone.");
    rig.press("f3");
    let gone = said(&mut rig);
    well_said(&gone);
    assert_eq!(gone, "Nothing here reads weight.");
    // And the two mistakes a change can be.
    line_types(&mut rig, "/gone/gone/g");
    assert_eq!(said(&mut rig), "Enter does nothing yet. That would change nothing.");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? That would change nothing.");
    rig.press("esc");
    // The case of what is found is not looked at, so this one is a change.
    line_types(&mut rig, "/all/all/g");
    assert_eq!(said(&mut rig), "Enter changes the only one to all. Ctrl Z takes it back.");
    rig.press("esc");
    rig.app.page_mut().read_only = true;
    line_types(&mut rig, "/gone/here");
    rig.press("enter");
    assert_eq!(said(&mut rig), "? This text is open for reading only.");
    well_said("? This text is open for reading only.");
    well_said("? That would change nothing.");
}

#[test]
fn every_sentence_is_whole_at_the_narrowest_the_default_measure_is_shown() {
    // DESIGN.md: each fits 66 cells. A window too narrow for 66 has a
    // shorter say row, and the sentence for a value gives way from its end
    // rather than being cut inside a word.
    let (mut rig, _) = desk_rig();
    rig.size(960, 528);
    assert_eq!(rig.app.grid().measure, 66);
    assert_eq!(says(&mut rig, "set look register night"), "Enter keeps look at register night. Esc puts it back to manila.");
    rig.press("enter");
    assert_eq!(says(&mut rig, "set look wordperfect"), "Enter keeps look at wordperfect.");
    rig.press("esc");
    rig.size(720, 528);
    assert!(rig.app.grid().measure < 66);
    let narrow = says_cut(&mut rig, "set measure 44");
    assert_eq!(narrow, "Enter keeps measure at 44.");
}

/// What the say row reads after `typed`, on a screen narrower than the
/// sentences were written for.
fn says_cut(rig: &mut Rig, typed_on_line: &str) -> String {
    rig.press("esc");
    rig.type_text(typed_on_line);
    said(rig)
}
