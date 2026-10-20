//! The flow of DESIGN.md, "what the screen does while you write", driven
//! through the whole interface by the harness clock: every state of the
//! table with its trigger and its timing, read back from the strengths on
//! the screen; what the program asks of the shell at each moment; and the
//! word limit.

mod common;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use common::{head, read, said};
use ok_host::{Event, Host, Mods, MouseKind};
use ok_ui::flow::Rest;
use ok_ui::harness::Rig;
use ok_ui::line::Closed;
use ok_ui::look::{Look, Strength, Surface};
use ok_ui::session::{Opened, Session};
use ok_ui::Options;

const PLATEN: i32 = 28;

/// The orchard essay on its desk, as the program itself would open it: the
/// contents with the open text lit, a links block, the facts, and two
/// paragraphs under a title, the cursor at the end on the platen row.
fn desk() -> Rig {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    // The flow is tested with a line that does nothing, so that Esc is only
    // what the flow makes of it.
    let options = Options { file: Some(folder.join("College/The orchard.md")), folder: Some(folder), look: None, store: false };
    let session = Box::new(ok_ui::session::Memory::new(&options));
    let mut rig = Rig::with_seams(options, session, Box::new(Closed), ok_ui::flow::start());
    rig.size(1920, 1080);
    rig
}

fn strength(r: &mut Rig, col: i32, row: i32) -> Strength {
    r.screen().at(col, row).expect("a cell on the screen").strength
}

/// What is beside the page: the open text's name, another text's and its
/// leader dots in the contents, the links, the facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Sides {
    open: Strength,
    other: Strength,
    dots: Strength,
    links: Strength,
    facts: Strength,
}

fn sides(r: &mut Rig) -> Sides {
    Sides { open: strength(r, 4, 6), other: strength(r, 4, 2), dots: strength(r, 20, 2), links: strength(r, 134, 44), facts: strength(r, 112, 47) }
}

/// The strength of the label beside the map, which is level with the
/// notch wherever the typing has taken that. A paragraph still short enough
/// to be read whole has none.
fn label(r: &mut Rig) -> Option<Strength> {
    let row = (1..41).find(|&row| !read(r, 134, 157, row).is_empty())?;
    Some(strength(r, 134, row))
}

const AT_REST: Sides = Sides { open: Strength::INK, other: Strength::HALF, dots: Strength::FAINT, links: Strength::HALF, facts: Strength::HALF };

fn all(strength: Strength) -> Sides {
    Sides { open: strength, other: strength, dots: strength, links: strength, facts: strength }
}

/// The strength of the words of the title, of the first paragraph and of
/// the paragraph being written: rows of the text 0, 3 and the cursor's.
fn paragraphs(r: &mut Rig) -> (Strength, Strength, Strength) {
    let top = r.app.view().top();
    let own = r.screen().cursor.expect("a cursor").row;
    let row = |text_row: i64| (text_row - top) as i32;
    (strength(r, 47, row(0)), strength(r, 47, row(3)), strength(r, 47, own))
}

/// Twenty-one words, typed at whatever pace the rig is set to.
const WORDS_21: &str = " he let me think that for about a week and then he walked me to the one tree he had left";

/// Types until Writing has begun and its fade is over.
fn into_writing(r: &mut Rig) {
    r.pace = 60;
    r.press("enter");
    r.type_text("He let me");
    r.wait(900);
}

/// And on into a Run, its fade over.
fn into_a_run(r: &mut Rig) {
    into_writing(r);
    r.type_text(WORDS_21);
    r.wait(1500);
}

// ───────────────────────── Rest ─────────────────────────

#[test]
fn at_rest_everything_is_at_its_rest_strength_and_the_program_asks_for_nothing() {
    let mut r = desk();
    assert_eq!(sides(&mut r), AT_REST);
    assert_eq!(label(&mut r), Some(Strength::HALF));
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    assert_eq!(strength(&mut r, 44, PLATEN - 13), Strength::HALF, "the title's mark at half");
    assert_eq!(strength(&mut r, 47, 47), Strength::INK, "ok");
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    // An hour of nothing is an hour of nothing: no tick is asked for.
    let before = r.host.now;
    r.wait(3_600_000);
    assert_eq!(r.host.now, before + 3_600_000);
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    assert_eq!(sides(&mut r), AT_REST);
}

#[test]
fn seven_characters_are_not_writing_and_neither_are_eight_with_a_gap() {
    let mut r = desk();
    r.pace = 100;
    r.type_text(" Seven.");
    assert_eq!(sides(&mut r), AT_REST);
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none(), "counting characters needs no clock");
    // More than two seconds, and the count starts again from one.
    r.wait(2001);
    r.type_text(" And se");
    assert_eq!(sides(&mut r), AT_REST);
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    r.pace = 0;
    r.type_text("v");
    assert!(r.wants().animating, "the eighth with no gap over two seconds");
    // Exactly two seconds is not over two seconds.
    let mut r = desk();
    for ch in " Eight ch".chars() {
        r.type_text(&ch.to_string());
        r.wait(2000);
    }
    assert_ne!(sides(&mut r), AT_REST);
}

// ───────────────────────── Writing ─────────────────────────

#[test]
fn the_eighth_character_begins_writing_and_the_sides_go_to_faint_over_900_ms() {
    let mut r = desk();
    r.send(Event::Mouse { kind: MouseKind::Move, x: 5, y: 5, mods: Mods::NONE });
    assert!(!r.wants().pointer_hidden);
    r.type_text(" Eight c");
    assert!(r.wants().pointer_hidden, "the pointer hides");
    assert!(r.wants().animating);
    assert_eq!(r.wants().wake_in_ms, Some(4000), "the breath is the next thing on the clock");
    assert_eq!(sides(&mut r), AT_REST, "the fade starts from where things were");
    r.wait(448);
    let half_way = sides(&mut r);
    assert!(Strength::FAINT < half_way.open && half_way.open < Strength::INK);
    assert!(Strength::FAINT < half_way.other && half_way.other < Strength::HALF);
    assert!(Strength::FAINT < half_way.facts && half_way.facts < Strength::HALF);
    assert_eq!(half_way.dots, Strength::FAINT, "what is faint at rest has nowhere to go yet");
    r.wait(452);
    assert_eq!(sides(&mut r), all(Strength::FAINT));
    assert_eq!(label(&mut r), Some(Strength::FAINT));
    assert!(!r.wants().animating, "the fade is over and no more frames are asked for");
    assert_eq!(r.wants().wake_in_ms, Some(3100));
    // The text is untouched: every paragraph in ink, marks at half.
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    assert_eq!(strength(&mut r, 47, 47), Strength::INK, "ok is never dimmed");
}

#[test]
fn backspace_delete_and_movement_inside_the_paragraph_do_not_change_the_state() {
    let mut r = desk();
    into_writing(&mut r);
    assert_eq!(sides(&mut r), all(Strength::FAINT));
    for chord in ["backspace", "left", "left", "delete", "right", "home", "end", "shift+left", "right"] {
        r.wait(3000);
        r.press(chord);
        assert_eq!(sides(&mut r), all(Strength::FAINT), "{chord}");
        assert!(!r.wants().animating);
        assert_eq!(r.wants().wake_in_ms, Some(4000), "{chord} is a key: the pause starts again");
    }
    // Twenty-seven seconds have gone by without a breath, because no four
    // of them were without a key.
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
}

// ───────────────────────── Run ─────────────────────────

#[test]
fn twenty_words_begin_a_run_the_other_paragraphs_go_to_half_and_the_sides_to_ground() {
    let mut r = desk();
    into_writing(&mut r);
    r.pace = 0;
    // Nineteen words more are still Writing.
    let words: Vec<&str> = WORDS_21.split(' ').filter(|word| !word.is_empty()).collect();
    r.type_text(&format!(" {}", words[..19].join(" ")));
    assert!(!r.wants().animating);
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    // The first letter of the twentieth is the run.
    r.type_text(" h");
    assert!(r.wants().animating);
    r.wait(752);
    let (title, first, own) = paragraphs(&mut r);
    assert!(Strength::HALF < first && first < Strength::INK && title == first);
    assert_eq!(own, Strength::INK);
    let going = sides(&mut r);
    assert!(Strength::GROUND < going.open && going.open < Strength::FAINT && going.open == going.dots);
    r.wait(748);
    assert!(!r.wants().animating);
    assert_eq!(paragraphs(&mut r), (Strength::HALF, Strength::HALF, Strength::INK));
    assert_eq!(sides(&mut r), all(Strength::GROUND));
    // Gone means nothing is drawn where they were: bare desk, and bare
    // paper where the facts were.
    let look = Look::manila();
    for (col, row, ground) in [(4, 6, look.desk), (134, 44, look.desk), (100, 47, look.paper)] {
        let cells = r.app.grid().cells(col, row, 10, 1);
        let frame = r.draw();
        assert!((0..cells.w).all(|dx| (0..cells.h).all(|dy| frame.get(cells.x + dx, cells.y + dy) == Some(ground))), "column {col} row {row}");
    }
    // Never dimmed: his paragraph, the cursor, the map's marks, the notch,
    // the change bars, ok. Marks in the other paragraphs go down with them.
    assert_eq!(strength(&mut r, 47, 47), Strength::INK);
    assert!(r.screen().cursor.is_some() && r.screen().bar(PLATEN));
    let area = r.app.grid().map_area().unwrap();
    assert_eq!(r.pixel(area.x, area.y), Some(look.shade(Surface::Sheet, Strength::HALF)), "the map keeps its marks");
    let cursor_row = r.app.page().caret().row as i32;
    assert_eq!(r.pixel(r.app.grid().x(120) + 2, area.y + cursor_row * 4), Some(look.desk_ink), "and its notch");
    let top = r.app.view().top();
    assert_eq!(strength(&mut r, 44, (1 - top) as i32), Strength::HALF, "the title's mark was at half already");
}

#[test]
fn enter_in_a_run_leaves_the_paragraph_just_finished_in_ink_for_eight_characters() {
    let mut r = desk();
    into_a_run(&mut r);
    r.pace = 0;
    r.press("enter");
    // The paragraph just finished is two rows up from the new one's row.
    let finished = |r: &mut Rig| strength(r, 47, PLATEN - 2);
    let older = |r: &mut Rig| strength(r, 47, PLATEN - 6);
    assert_eq!((finished(&mut r), older(&mut r)), (Strength::INK, Strength::HALF));
    // Enter again before a word is typed has finished nothing new.
    r.type_text("Seven c");
    assert_eq!(finished(&mut r), Strength::INK);
    assert!(!r.wants().animating);
    r.type_text("h");
    assert!(r.wants().animating, "the eighth lets it go");
    r.wait(750);
    assert!(Strength::HALF < finished(&mut r) && finished(&mut r) < Strength::INK);
    r.wait(750);
    assert_eq!((finished(&mut r), older(&mut r)), (Strength::HALF, Strength::HALF));
    assert!(!r.wants().animating);
    assert_eq!(strength(&mut r, 47, PLATEN), Strength::INK, "and the one he is in now is his");
}

// ───────────────────────── Breath and Settle ─────────────────────────

#[test]
fn four_seconds_without_a_key_is_a_breath_and_ten_a_settle() {
    let mut r = desk();
    into_a_run(&mut r);
    let asked = r.wants().wake_in_ms.expect("the breath is on the clock");
    assert_eq!(asked, 4000 - 1500 - 60);
    r.wait(asked - 1);
    assert_eq!(paragraphs(&mut r), (Strength::HALF, Strength::HALF, Strength::INK));
    assert!(!r.wants().animating);
    // A breath: all paragraphs back to ink over 400 ms. Nothing else returns.
    r.wait(1);
    assert!(r.wants().animating);
    r.wait(200);
    let (_, first, _) = paragraphs(&mut r);
    assert!(Strength::HALF < first && first < Strength::INK);
    r.wait(200);
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    assert_eq!(sides(&mut r), all(Strength::GROUND));
    assert!(!r.wants().animating);
    assert_eq!(r.wants().wake_in_ms, Some(6000 - 400), "the settle is next");
    // Settle: everything back over 400 ms.
    r.wait(6000 - 400);
    assert!(r.wants().animating);
    r.wait(200);
    let coming = sides(&mut r);
    assert!(Strength::GROUND < coming.other && coming.other < Strength::HALF);
    r.wait(200);
    assert_eq!(sides(&mut r), AT_REST);
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    // Twenty-four words is not a hundred: nothing is said.
    assert_eq!(said(&mut r), "");
    // And then it is idle: no wake, no animation, for as long as he likes.
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    let before = r.host.now;
    r.wait(100_000);
    assert_eq!(r.host.now, before + 100_000);
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
}

#[test]
fn typing_after_a_breath_is_writing_again_with_the_sides_left_where_they_were() {
    let mut r = desk();
    into_a_run(&mut r);
    r.wait(4400);
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    r.pace = 0;
    r.type_text(" and");
    assert_eq!(sides(&mut r), all(Strength::GROUND), "nothing comes back because he typed");
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    assert!(!r.wants().animating);
    assert_eq!(r.wants().wake_in_ms, Some(4000));
    // Twenty words more, with no gap of four seconds, and it is a run again.
    r.type_text(&WORDS_21[..WORDS_21.len() - 5]);
    assert!(r.wants().animating);
    r.wait(1500);
    assert_eq!(paragraphs(&mut r), (Strength::HALF, Strength::HALF, Strength::INK));
    // A breath taken in Writing, before any run, changes nothing on the
    // page, since nothing there had dimmed.
    let mut r = desk();
    into_writing(&mut r);
    r.wait(4000);
    assert!(!r.wants().animating);
    assert_eq!(sides(&mut r), all(Strength::FAINT));
    assert_eq!(r.wants().wake_in_ms, Some(10_000 - 60 - 900 - 4000), "the settle is next");
}

#[test]
fn a_settle_after_a_hundred_words_says_how_many_for_six_seconds() {
    let mut r = desk();
    r.pace = 20;
    r.press("enter");
    let sentence = "the paper moves and the typing point does not ";
    r.type_text(&sentence.repeat(15));
    r.type_text("and then eight words more to finish i");
    r.pace = 0;
    r.type_text("t");
    assert_eq!(said(&mut r), "");
    r.wait(9_999);
    assert_eq!(said(&mut r), "");
    r.wait(1);
    assert_eq!(said(&mut r), "143 words without stopping.");
    assert_eq!(strength(&mut r, 47, 46), Strength::HALF);
    r.wait(400);
    assert_eq!(sides(&mut r), AT_REST, "the numbers current");
    assert!(read(&mut r, 50, 112, 47).ends_with("273 words"));
    assert_eq!(label(&mut r), Some(Strength::HALF));
    assert!(!r.wants().animating);
    assert_eq!(r.wants().wake_in_ms, Some(5600), "only the sentence is still on the clock");
    r.wait(5599);
    assert_eq!(said(&mut r), "143 words without stopping.");
    r.wait(1);
    assert_eq!(said(&mut r), "");
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    // Ninety-nine words earn nothing. Breaths on the way do not break it:
    // without stopping means without a settle.
    let mut r = desk();
    r.pace = 20;
    r.press("enter");
    r.type_text(&sentence.repeat(6));
    r.wait(5000);
    r.type_text(&sentence.repeat(5));
    r.pace = 0;
    r.wait(10_000);
    assert_eq!(said(&mut r), "");
    r.wait(10_000);
    r.type_text(&" x".repeat(101));
    r.wait(4500);
    r.type_text(" y");
    r.wait(10_000);
    assert_eq!(said(&mut r), "102 words without stopping.");
}

// ───────────────────────── Taking the wheel ─────────────────────────

#[test]
fn taking_the_wheel_goes_straight_to_rest_in_120_ms() {
    let ways: [(&str, fn(&mut Rig)); 6] = [
        ("the cursor leaves its paragraph", |r| r.press("up")),
        ("Esc", |r| r.press("esc")),
        ("a chord", |r| r.press("ctrl+left")),
        ("a chord bound to nothing", |r| r.press("alt+w")),
        ("the mouse", |r| r.send(Event::Mouse { kind: MouseKind::Move, x: 900, y: 500, mods: Mods::NONE })),
        ("the wheel", |r| r.wheel(1)),
    ];
    for (what, take) in ways {
        let mut r = desk();
        into_a_run(&mut r);
        // "up" must leave the paragraph: from its first row it does.
        r.press("home");
        r.press("up");
        r.press("up");
        r.press("up");
        assert_eq!(sides(&mut r), all(Strength::GROUND), "{what}: moving inside the paragraph is not taking the wheel");
        take(&mut r);
        assert!(r.wants().animating, "{what}");
        r.wait(60);
        let coming = sides(&mut r);
        assert!(Strength::GROUND < coming.other && coming.other < Strength::HALF, "{what}");
        r.wait(60);
        assert_eq!(sides(&mut r), AT_REST, "{what}");
        let top = r.app.view().top();
        assert_eq!(strength(&mut r, 47, (3 - top) as i32), Strength::INK, "{what}");
        // At rest, and idle.
        assert!(!r.wants().animating && r.wants().wake_in_ms.is_none(), "{what}");
    }
    // From Rest there is nowhere to go, and nothing to animate.
    let mut r = desk();
    r.send(Event::Mouse { kind: MouseKind::Move, x: 900, y: 500, mods: Mods::NONE });
    r.press("esc");
    r.press("ctrl+left");
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    // And a count of characters under way is dropped.
    r.type_text("1234567");
    r.press("ctrl+right");
    r.type_text("8");
    assert_eq!(sides(&mut r), AT_REST);
    assert!(!r.wants().animating);
}

// ───────────────────────── animations turned off ─────────────────────────

#[test]
fn with_animations_turned_off_every_fade_is_a_single_step() {
    let mut r = desk();
    r.host.reduced_motion = true;
    r.press("enter");
    r.type_text("Eight ch");
    assert_eq!(sides(&mut r), all(Strength::FAINT));
    assert!(!r.wants().animating);
    r.type_text(WORDS_21);
    assert_eq!(paragraphs(&mut r), (Strength::HALF, Strength::HALF, Strength::INK));
    assert_eq!(sides(&mut r), all(Strength::GROUND));
    assert!(!r.wants().animating);
    r.press("enter");
    r.type_text("Eight ch");
    assert_eq!(strength(&mut r, 47, PLATEN - 2), Strength::HALF);
    assert!(!r.wants().animating);
    r.wait(4000);
    assert_eq!(paragraphs(&mut r), (Strength::INK, Strength::INK, Strength::INK));
    assert!(!r.wants().animating);
    r.wait(6000);
    assert_eq!(sides(&mut r), AT_REST);
    assert!(!r.wants().animating && r.wants().wake_in_ms.is_none());
    // Taking the wheel is one step too.
    r.type_text(" Eight ch");
    r.press("esc");
    assert_eq!(sides(&mut r), AT_REST);
    assert!(!r.wants().animating);
}

// ───────────────────────── the word limit ─────────────────────────

/// A session with one text and a word limit that a test can change.
struct Limited {
    text: String,
    limit: Rc<Cell<Option<usize>>>,
}

impl Session for Limited {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        Opened { text: self.text.clone(), ..Opened::default() }
    }

    fn name(&self) -> &str {
        "The orchard"
    }

    fn limit(&self) -> Option<usize> {
        self.limit.get()
    }

    fn set_limit(&mut self, limit: Option<usize>) {
        self.limit.set(limit);
    }
}

/// Fifteen words on five rows of three, in one paragraph.
const FIFTEEN: &str = "one two three\nfour five six\nseven eight nine\nten eleven twelve\nthirteen fourteen fifteen";

fn limited(text: &str, limit: Option<usize>) -> (Rig, Rc<Cell<Option<usize>>>) {
    let limit = Rc::new(Cell::new(limit));
    let session = Limited { text: text.to_string(), limit: limit.clone() };
    let mut rig = Rig::with_seams(Options::default(), Box::new(session), Box::new(Closed), Box::new(Rest));
    rig.size(1920, 1080);
    (rig, limit)
}

fn facts(r: &mut Rig) -> String {
    read(r, 50, 112, 47).trim_start().to_string()
}

/// The rows of the sheet from the first of the text to the cursor's.
fn sheet(r: &mut Rig) -> Vec<String> {
    let first = (-r.app.view().top()) as i32;
    (first..=PLATEN).map(|row| read(r, 47, 112, row)).collect()
}

#[test]
fn under_its_limit_and_at_it_a_text_has_no_rule_and_the_facts_say_how_far() {
    let (mut r, _) = limited(FIFTEEN, Some(650));
    assert_eq!(facts(&mut r), "The orchard    15 of 650 words");
    assert_eq!(sheet(&mut r), FIFTEEN.split('\n').collect::<Vec<_>>());
    // Exactly at the limit nothing is past it.
    let (mut r, _) = limited(FIFTEEN, Some(15));
    assert_eq!(facts(&mut r), "The orchard    15 of 15 words");
    assert_eq!(sheet(&mut r), FIFTEEN.split('\n').collect::<Vec<_>>());
    assert_eq!(r.app.page().rule(), None);
    assert!((47..=112).all(|col| r.screen().at(col, PLATEN).is_some_and(|cell| cell.strength == Strength::INK || cell.ch == ' ')));
    // With a selection the count is the selection's, limit or no limit.
    r.press("ctrl+shift+left");
    assert_eq!(facts(&mut r), "The orchard    1 word selected");
    // No limit, no "of".
    let (mut r, _) = limited(FIFTEEN, None);
    assert_eq!(facts(&mut r), "The orchard    15 words");
}

#[test]
fn past_its_limit_a_text_has_a_rule_where_the_limit_falls_and_the_rest_at_half() {
    let (mut r, _) = limited(FIFTEEN, Some(7));
    assert_eq!(facts(&mut r), "The orchard    15 of 7 words");
    // The eighth word is on the third row: the rule is set above that row.
    let rule = format!("{} 7 words", "=".repeat(58));
    assert_eq!(sheet(&mut r), ["one two three", "four five six", rule.as_str(), "seven eight nine", "ten eleven twelve", "thirteen fourteen fifteen"]);
    assert_eq!(r.app.page().rule(), Some((7, 2)));
    let rule_row = PLATEN - 3;
    // Equals signs across the measure, faint as leader dots are; the
    // limit at its right end at half, as a count is. Nothing boxed.
    assert_eq!(strength(&mut r, 47, rule_row), Strength::FAINT);
    assert_eq!(strength(&mut r, 104, rule_row), Strength::FAINT);
    assert_eq!(r.screen().at(105, rule_row).map(|cell| cell.ch), Some(' '));
    assert_eq!(strength(&mut r, 106, rule_row), Strength::HALF);
    assert_eq!(strength(&mut r, 112, rule_row), Strength::HALF);
    assert!(!r.screen().bar(rule_row));
    // Up to the limit in ink; from the first word past it, at half.
    assert_eq!(strength(&mut r, 47, rule_row - 1), Strength::INK);
    let limit_row: Vec<Strength> = (47..47 + 16).map(|col| strength(&mut r, col, rule_row + 1)).collect();
    assert_eq!(limit_row[..5], [Strength::INK; 5], "seven is the seventh word");
    assert_eq!(limit_row[6..11], [Strength::HALF; 5], "eight is past the limit");
    assert_eq!(limit_row[12..], [Strength::HALF; 4]);
    assert_eq!(strength(&mut r, 47, PLATEN), Strength::HALF, "his own paragraph too: the limit is not the flow");
    // The cursor is where the page says, on the platen row, one row lower
    // on the paper than in the text.
    assert_eq!(r.screen().cursor.map(|at| (at.col, at.row)), Some((47 + 25, PLATEN)));
    assert_eq!(r.wants().caret, Some(r.app.grid().cells(47 + 25, PLATEN, 1, 1)));
    // In the map the rows past the limit are faint, word by word.
    let look = Look::manila();
    let area = r.app.grid().map_area().unwrap();
    let (half, faint) = (look.shade(Surface::Sheet, Strength::HALF), look.shade(Surface::Sheet, Strength::FAINT));
    assert_eq!(r.pixel(area.x, area.y + 4), Some(half));
    assert_eq!(r.pixel(area.x, area.y + 8), Some(half), "seven");
    assert_eq!(r.pixel(area.x + 12, area.y + 8), Some(faint), "eight");
    assert_eq!(r.pixel(area.x, area.y + 16), Some(faint));
}

#[test]
fn the_rule_comes_and_goes_with_the_words_and_the_typing_row_holds_still() {
    let (mut r, limit) = limited("one two three\nfour five six", Some(7));
    r.type_text(" seven");
    assert_eq!(r.app.page().rule(), None);
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(PLATEN));
    // The eighth word: the rule goes in above the row it is on, the paper
    // above climbs a row to make room, and the typing row stays put.
    r.type_text(" e");
    assert_eq!(r.app.page().rule(), Some((7, 1)));
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(PLATEN));
    assert_eq!(read(&mut r, 47, 112, PLATEN), "four five six seven e");
    assert!(read(&mut r, 47, 112, PLATEN - 1).ends_with("= 7 words"));
    assert_eq!(read(&mut r, 47, 112, PLATEN - 2), "one two three");
    assert_eq!(facts(&mut r), "The orchard    8 of 7 words");
    // Taken back, the rule goes and the paper comes down again.
    r.press("backspace");
    assert_eq!(r.app.page().rule(), None);
    assert_eq!(read(&mut r, 47, 112, PLATEN - 1), "one two three");
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(PLATEN));
    // Undo and redo carry it with them.
    r.press("ctrl+z");
    r.press("ctrl+y");
    assert_eq!(r.app.page().rule(), None);
    r.type_text("eight nine");
    assert_eq!(r.app.page().rule(), Some((7, 1)));
    // A limit changed or taken away is seen at the next thing that happens.
    limit.set(Some(9));
    r.send(Event::Tick);
    assert_eq!((r.app.page().rule(), facts(&mut r)), (None, String::from("The orchard    9 of 9 words")));
    limit.set(Some(2));
    r.send(Event::Tick);
    assert_eq!(r.app.page().rule(), Some((2, 0)));
    assert!(read(&mut r, 47, 112, PLATEN - 2).ends_with("= 2 words"), "above the very first row");
    limit.set(None);
    r.send(Event::Tick);
    assert_eq!((r.app.page().rule(), facts(&mut r)), (None, String::from("The orchard    9 words")));
}

#[test]
fn clicks_and_arrows_pass_over_the_rule_as_if_it_were_not_there() {
    let (mut r, _) = limited(FIFTEEN, Some(7));
    let rule_row = PLATEN - 3;
    // A click on a row above the rule, on a row below it, and on the rule
    // itself, which answers as the row under it.
    r.click_cell(47 + 4, rule_row - 1);
    assert_eq!(head(&r), (1, 4));
    r.click_cell(47 + 6, rule_row + 1);
    assert_eq!(head(&r), (2, 6));
    r.click_cell(47 + 2, rule_row);
    assert_eq!(head(&r), (2, 2));
    r.click_cell(47 + 30, PLATEN);
    assert_eq!(head(&r), (4, 25), "the last row is still the last row");
    // Up and Down step from the row under the rule to the row over it.
    r.click_cell(47 + 2, rule_row + 1);
    r.press("up");
    assert_eq!(head(&r), (1, 2));
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(rule_row - 1));
    r.press("down");
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(rule_row + 1));
    // A click on the map goes to the row of the text, rule or no rule.
    let area = r.app.grid().map_area().unwrap();
    r.click(area.x + 6, area.y + 3 * 4 + 1);
    assert_eq!(head(&r), (3, 0));
    assert_eq!(r.screen().cursor.map(|at| at.row), Some(PLATEN), "and brings it to the platen");
}
