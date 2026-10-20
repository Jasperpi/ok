//! The three seams, each driven through the whole interface with a
//! stand-in behind it, so that every call site the briefs promise is seen
//! to be in place: what the seam is told, when, and what the screen makes
//! of its answers.
//!
//! The stand-ins are small on purpose, and are the shortest worked example
//! of what each method of `Session`, `Line` and `Flow` is for.

mod common;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use common::{head, presses, read, said, selected};
use ok_host::{Event, Host, Key, Mods, MouseKind};
use ok_text::search::find_all;
use ok_text::{Change, Doc, Pos, Span};
use ok_ui::canvas::CursorBox;
use ok_ui::contents::Entry;
use ok_ui::flow::{Act, Flow, Part, Rest, Said};
use ok_ui::harness::Rig;
use ok_ui::line::{Answer, Closed, Line, Pane};
use ok_ui::look::{Look, Strength};
use ok_ui::page::Way;
use ok_ui::session::{LinkRow, Links, Memory, News, Opened, Session};
use ok_ui::{App, Options};

const ESSAY: &str = "# The orchard\n\nThe orchard was eleven acres when my grandfather bought it.\n\n## Thinning\n\nYou take off two of every three apricots, TK how many that is.\n\n## What he showed me\n\nThe branches had split under the weight.";

fn entry(depth: usize, name: &str, value: &str, lit: bool) -> Entry {
    Entry { depth, name: name.to_string(), value: value.to_string(), lit }
}

fn strengths(r: &mut Rig, first: i32, last: i32, row: i32) -> Vec<Strength> {
    (first..=last).filter_map(|col| r.screen().at(col, row)).filter(|cell| cell.ch != ' ').map(|cell| cell.strength).collect()
}

// ───────────────────────── a stand-in session ─────────────────────────

/// What the stand-in session was told.
#[derive(Default)]
struct Told {
    changes: Vec<Change>,
    calls: Vec<String>,
}

/// What the stand-in session answers, for a test to change as it goes.
#[derive(Default)]
struct Answers {
    off_disk: bool,
    failing: bool,
    words_today: usize,
    missing: Vec<String>,
    keep: Option<String>,
    follow: Option<Opened>,
    open: Option<Opened>,
    news: News,
    wake: Option<u64>,
}

struct Journal {
    first: Opened,
    name: String,
    contents: Vec<Entry>,
    links: Links,
    told: Rc<RefCell<Told>>,
    answers: Rc<RefCell<Answers>>,
}

impl Session for Journal {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        self.told.borrow_mut().calls.push(String::from("first"));
        self.first.clone()
    }

    fn open(&mut self, path: &Path, doc: &Doc, _host: &mut dyn Host) -> Option<Opened> {
        self.told.borrow_mut().calls.push(format!("open {} over {} bytes", path.display(), doc.len_bytes()));
        self.answers.borrow_mut().open.take()
    }

    fn changed(&mut self, _doc: &Doc, change: &Change, _host: &mut dyn Host) {
        self.told.borrow_mut().changes.push(change.clone());
    }

    fn on_disk(&self) -> bool {
        !self.answers.borrow().off_disk
    }

    fn failing(&self) -> bool {
        self.answers.borrow().failing
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> &[Entry] {
        &self.contents
    }

    fn links(&self) -> Option<&Links> {
        Some(&self.links)
    }

    fn exists(&self, target: &str) -> bool {
        !self.answers.borrow().missing.iter().any(|missing| missing == target)
    }

    fn words_today(&self) -> usize {
        self.answers.borrow().words_today
    }

    fn keep(&mut self, doc: &Doc, _host: &mut dyn Host) -> Option<String> {
        self.told.borrow_mut().calls.push(format!("keep {} bytes", doc.len_bytes()));
        self.answers.borrow().keep.clone()
    }

    fn follow(&mut self, _doc: &Doc, at: Pos, _host: &mut dyn Host) -> Option<Opened> {
        self.told.borrow_mut().calls.push(format!("follow {} {}", at.line, at.col));
        self.answers.borrow_mut().follow.take()
    }

    fn tick(&mut self, _doc: &Doc, _host: &mut dyn Host) -> News {
        std::mem::take(&mut self.answers.borrow_mut().news)
    }

    fn wake_in_ms(&self, _host: &dyn Host) -> Option<u64> {
        self.answers.borrow().wake
    }

    fn close(&mut self, doc: &Doc, _host: &mut dyn Host) {
        let seen = self.told.borrow().changes.len();
        self.told.borrow_mut().calls.push(format!("close with {} and {seen} changes", doc.to_text("\\n")));
    }
}

struct WithJournal {
    rig: Rig,
    told: Rc<RefCell<Told>>,
    answers: Rc<RefCell<Answers>>,
}

fn journal(first: Opened) -> WithJournal {
    let (told, answers) = (Rc::new(RefCell::new(Told::default())), Rc::new(RefCell::new(Answers::default())));
    let session = Journal {
        first,
        name: String::from("The orchard"),
        contents: vec![entry(0, "College", "", false), entry(1, "Essay plan", "210", false), entry(1, "The orchard", "412", true), entry(2, "A draft deep in a folder with a long name", "1,204", false)],
        links: Links { see_also: vec![LinkRow { name: String::from("Grandfather interview"), exists: true }, LinkRow { name: String::from("Thinning notes"), exists: false }], linked_from: (1..=8).map(|n| format!("Plan {n}")).collect() },
        told: told.clone(),
        answers: answers.clone(),
    };
    let mut rig = Rig::with_seams(Options::default(), Box::new(session), Box::new(Closed), Box::new(Rest));
    rig.size(1920, 1080);
    WithJournal { rig, told, answers }
}

fn opened(text: &str) -> Opened {
    Opened { text: text.to_string(), ..Opened::default() }
}

#[test]
fn every_change_reaches_the_session_and_replays_to_the_text_on_the_page() {
    let WithJournal { mut rig, told, .. } = journal(opened("First.\n\n- one\n- two\n\nLast TK."));
    let mut replayed = Doc::from_text("First.\n\n- one\n- two\n\nLast TK.");
    let mut seen = 0;
    let mut check = |rig: &mut Rig, what: &str| {
        let told = told.borrow();
        for change in &told.changes[seen..] {
            let made = replayed.replace(Span { start: change.start, end: change.old_end }, &change.inserted);
            assert_eq!(&made, change, "after {what}: the change as told is the change the text had");
        }
        seen = told.changes.len();
        assert_eq!(replayed.to_text("\n"), rig.text(), "after {what}");
    };
    rig.type_text(" More words");
    check(&mut rig, "typing");
    let steps = [
        "enter", "backspace", "ctrl+backspace", "alt+backspace", "ctrl+z", "ctrl+z", "ctrl+y", "ctrl+home", "delete", "ctrl+delete", "alt+delete", "ctrl+down", "ctrl+down", "tab", "end", "enter",
        "enter", "ctrl+a", "tab", "shift+tab", "ctrl+end", "alt+up", "alt+up", "alt+down", "ctrl+t", "ctrl+x", "ctrl+v", "ctrl+v", "shift+up", "ctrl+c", "ctrl+end", "ctrl+v", "ctrl+z", "ctrl+z",
        "ctrl+z", "ctrl+z", "ctrl+z", "ctrl+y", "ctrl+shift+z", "shift+enter", "shift+home", "enter", "ctrl+a", "backspace", "ctrl+z",
    ];
    for step in steps {
        rig.press(step);
        check(&mut rig, step);
        rig.type_text("x y");
        check(&mut rig, step);
    }
    rig.wait(3000);
    rig.click_cell(50, 3);
    rig.type_text("clicked ");
    check(&mut rig, "a click and typing");
    assert!(seen > 120, "{seen} changes were told");
}

#[test]
fn ok_is_printed_only_while_the_session_says_everything_is_on_disk() {
    let WithJournal { mut rig, answers, .. } = journal(opened(ESSAY));
    assert_eq!(read(&mut rig, 47, 49, 47), "ok");
    answers.borrow_mut().off_disk = true;
    rig.type_text("x");
    assert_eq!(read(&mut rig, 47, 49, 47), "");
    answers.borrow_mut().off_disk = false;
    rig.send(Event::Tick);
    assert_eq!(read(&mut rig, 47, 49, 47), "ok");
    // A failed write: the whole machine line goes inverse until it mends.
    let all_inverse = |rig: &mut Rig| (47..=112).all(|col| rig.screen().at(col, 47).is_some_and(|cell| cell.inverse));
    assert!(!all_inverse(&mut rig));
    answers.borrow_mut().failing = true;
    answers.borrow_mut().off_disk = true;
    rig.send(Event::Tick);
    assert!(all_inverse(&mut rig));
    assert_eq!(read(&mut rig, 47, 49, 47), "");
    assert!(read(&mut rig, 47, 112, 47).ends_with("words"));
    answers.borrow_mut().failing = false;
    rig.send(Event::Tick);
    assert!(!all_inverse(&mut rig));
}

#[test]
fn the_facts_carry_the_name_the_count_and_the_words_typed_today() {
    let WithJournal { mut rig, answers, .. } = journal(opened(ESSAY));
    assert_eq!(rig.wants().title, "The orchard");
    assert_eq!(rig.app.page().words(), 37);
    assert_eq!(read(&mut rig, 50, 112, 47).trim_start(), "The orchard    37 words");
    answers.borrow_mut().words_today = 1630;
    rig.send(Event::Tick);
    assert_eq!(read(&mut rig, 50, 112, 47).trim_start(), "The orchard    37 words    1,630 today");
    rig.press("ctrl+a");
    assert_eq!(read(&mut rig, 50, 112, 47).trim_start(), "The orchard    37 words selected    1,630 today");
}

#[test]
fn the_contents_and_the_links_are_the_session_s_rows() {
    let WithJournal { mut rig, .. } = journal(opened(ESSAY));
    assert_eq!(read(&mut rig, 0, 40, 1), "  College");
    assert_eq!(read(&mut rig, 0, 40, 2), "    Essay plan .................. 210");
    // The open text's count is the page's own, whatever the row was given.
    assert_eq!(read(&mut rig, 0, 40, 3), format!("    The orchard {} 37", ".".repeat(18)));
    assert_eq!(strengths(&mut rig, 4, 14, 3), [Strength::INK; 10]);
    // A name too long for its row is not cut: it turns over, set in by
    // two, and its count stands on the last of its rows.
    assert_eq!(read(&mut rig, 0, 40, 4), "      A draft deep in a folder");
    assert_eq!(read(&mut rig, 0, 40, 5), "        with a long name ...... 1,204");
    assert_eq!(read(&mut rig, 0, 40, 6), "");
    // Up to six rows of each kind of link, then how many more; the block's
    // last row is the row under the text.
    let block: Vec<String> = (34..=45).map(|row| read(&mut rig, 134, 157, row)).collect();
    assert_eq!(block, ["", "See also", "  Grandfather interview", "  Thinning notes ?", "Linked from", "  Plan 1", "  Plan 2", "  Plan 3", "  Plan 4", "  Plan 5", "  Plan 6", "  and 2 more"]);
}

#[test]
fn a_link_the_session_does_not_know_is_at_half_strength() {
    let WithJournal { mut rig, answers, .. } = journal(opened("See [[Essay plan]] and [[Thinning notes]]."));
    rig.press("ctrl+home");
    rig.wheel(-1000);
    assert_eq!(strengths(&mut rig, 47 + 6, 47 + 15, 1), [Strength::INK; 9]);
    assert_eq!(strengths(&mut rig, 47 + 25, 47 + 38, 1), [Strength::INK; 13]);
    answers.borrow_mut().missing = vec![String::from("Thinning notes")];
    rig.send(Event::Tick);
    assert_eq!(strengths(&mut rig, 47 + 6, 47 + 15, 1), [Strength::INK; 9]);
    assert_eq!(strengths(&mut rig, 47 + 25, 47 + 38, 1), [Strength::HALF; 13]);
}

#[test]
fn what_the_session_hands_over_at_launch_is_put_on_the_page_as_it_was_left() {
    let first = Opened {
        text: (0..100).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n"),
        cursor: Some(Pos::new(40, 3)),
        top: Some(30),
        stamps: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, u32::MAX],
        read_only: false,
        say: Some(String::from("Last here Tuesday at 23:41. That sitting added 388 words.")),
    };
    let WithJournal { mut rig, told, .. } = journal(first);
    assert_eq!(told.borrow().calls, ["first"]);
    assert_eq!(head(&rig), (40, 3));
    assert_eq!(rig.app.view().top(), 30, "the same row at the top of the sheet");
    assert_eq!(read(&mut rig, 47, 112, 0), "line 30");
    assert_eq!(rig.screen().cursor, Some(CursorBox { col: 50, row: 10, wide: 1, tall: 1, hollow: false }));
    assert_eq!(said(&mut rig), "Last here Tuesday at 23:41. That sitting added 388 words.");
    // No line carries today's stamp, so nothing has a change bar yet.
    assert!((0..45).all(|row| !rig.screen().bar(row)));
    rig.type_text("x");
    assert_eq!((0..45).filter(|&row| rig.screen().bar(row)).collect::<Vec<_>>(), [10]);
    assert_eq!(said(&mut rig), "", "the first key takes the sentence down");

    // A text for reading only is shown and not changed.
    let WithJournal { mut rig, told, .. } = journal(Opened { read_only: true, say: Some(String::from("This file is not UTF-8, so it is open for reading only.")), ..opened("caf\u{FFFD}") });
    assert_eq!(said(&mut rig), "This file is not UTF-8, so it is open for reading only.");
    rig.type_text("x");
    rig.press("backspace");
    assert_eq!(rig.text(), "caf\u{FFFD}");
    assert!(told.borrow().changes.is_empty());
}

#[test]
fn ctrl_s_ctrl_enter_ticks_and_a_second_launch_all_reach_the_session() {
    let WithJournal { mut rig, told, answers } = journal(opened(ESSAY));
    // Ctrl S: the session keeps a version and its sentence goes on the say row.
    answers.borrow_mut().keep = Some(String::from("Kept 14:02. 2,481 bytes on disk."));
    rig.type_text("!");
    rig.press("ctrl+s");
    assert_eq!(told.borrow().calls.last().cloned(), Some(format!("keep {} bytes", ESSAY.len() + 1)));
    assert_eq!(told.borrow().changes.len(), 1, "the change came before the keep");
    assert_eq!(said(&mut rig), "Kept 14:02. 2,481 bytes on disk.");

    // Ctrl Enter: the session is told where the cursor is, and what it
    // hands back takes the page.
    answers.borrow_mut().follow = Some(opened("# Grandfather interview\n\nRecorded at the kitchen table."));
    rig.press("ctrl+enter");
    assert_eq!(told.borrow().calls.last().map(String::as_str), Some("follow 10 41"));
    assert_eq!(rig.text(), "# Grandfather interview\n\nRecorded at the kitchen table.");
    assert_eq!(head(&rig), (2, 30));
    rig.press("ctrl+z");
    assert_eq!(rig.text(), "# Grandfather interview\n\nRecorded at the kitchen table.", "a new text has nothing to undo");
    rig.press("ctrl+enter");
    assert_eq!(rig.text(), "# Grandfather interview\n\nRecorded at the kitchen table.", "nothing handed back: nothing changes");

    // A tick: the session asked to be woken, and brings news.
    answers.borrow_mut().wake = Some(700);
    rig.send(Event::Tick);
    assert_eq!(rig.wants().wake_in_ms, Some(700));
    answers.borrow_mut().wake = None;
    answers.borrow_mut().news = News { say: Some(String::from("Saved again. Nothing was lost.")), replaced: None };
    rig.wait(700);
    assert_eq!(said(&mut rig), "Saved again. Nothing was lost.");
    answers.borrow_mut().news = News { say: Some(String::from("This text changed on disk. Showing the newer one; yours is in earlier.")), replaced: Some(opened("Newer.")) };
    rig.send(Event::Tick);
    assert_eq!(rig.text(), "Newer.");
    // A sentence longer than the measure is never cut in a word: it turns
    // over, its first sentence going up onto the row kept empty under the
    // text.
    assert_eq!(read(&mut rig, 47, 112, 45), "This text changed on disk.");
    assert_eq!(said(&mut rig), "Showing the newer one; yours is in earlier.");

    // A second launch hands this one a file.
    answers.borrow_mut().open = Some(Opened { cursor: Some(Pos::new(0, 2)), ..opened("From the second launch.") });
    rig.send(Event::Open(PathBuf::from("C:/Users/jaspe/Documents/ok/Other.md")));
    assert_eq!(told.borrow().calls.last().map(String::as_str), Some("open C:/Users/jaspe/Documents/ok/Other.md over 6 bytes"));
    assert_eq!((rig.text(), head(&rig)), (String::from("From the second launch."), (0, 2)));
}

#[test]
fn leaving_hands_the_session_everything_first() {
    for leave in ["ctrl+q", "alt+f4", "close"] {
        let WithJournal { mut rig, told, .. } = journal(opened("abc"));
        rig.type_text("d");
        match leave {
            "close" => rig.send(Event::Close),
            chord => rig.press(chord),
        }
        assert_eq!(told.borrow().calls.last().map(String::as_str), Some("close with abcd and 1 changes"), "{leave}");
        assert_eq!(rig.wants().quit, leave != "close");
    }
}

#[test]
fn the_memory_session_reads_a_folder_once_and_keeps_see_also_true_to_the_text() {
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let options = Options { file: Some(desk.join("College/Essay plan.md")), folder: Some(desk.clone()), look: None, store: false };
    let session = Memory::new(&options);
    assert_eq!(session.name(), "Essay plan");
    let names: Vec<(usize, &str, &str, bool)> = session.contents().iter().map(|entry| (entry.depth, entry.name.as_str(), entry.value.as_str(), entry.lit)).collect();
    assert_eq!(names[..4], [(0, "College", "", false), (1, "Essay plan", "210", true), (1, "Grandfather interview", "630", false), (1, "Leadership essay", "348", false)]);
    assert_eq!(names.len(), 14);
    assert_eq!(session.links().map(|links| links.see_also.clone()), Some(vec![LinkRow { name: String::from("The orchard"), exists: true }]));
    assert!(session.exists("the orchard") && session.exists("College/Orchard notes") && !session.exists("Rebuttal notes"));
    assert!(session.on_disk() && !session.failing());
    // With no folder, nothing is listed and every link is taken on trust.
    let mut alone = Memory::new(&Options { file: Some(desk.join("no such file.md")), ..Options::default() });
    let mut host = ok_ui::harness::TestHost::default();
    assert_eq!(alone.first(&mut host), Opened::default());
    assert!(alone.contents().is_empty() && alone.exists("anything"));
    assert_eq!(session.today(&host), alone.today(&host));
}

// ───────────────────────── a stand-in line ─────────────────────────

/// A little line that really works: `/phrase` finds, `set` lists options in
/// the contents, `#` lists headings in the labels, `show` previews another
/// text. Esc puts back, Enter keeps.
#[derive(Default)]
struct Little {
    open: bool,
    typed: String,
    hits: Vec<Span>,
    rows: Vec<Entry>,
    pane: Option<Pane>,
    bar: usize,
    offer: Option<String>,
    /// The moment the page is to show what is under the bar.
    wake_at: Option<u64>,
    log: Rc<RefCell<Vec<String>>>,
}

impl Little {
    fn close(&mut self) {
        let log = self.log.clone();
        *self = Little { log, ..Little::default() };
    }

    /// Works out what the typed text asks for and has the page follow.
    fn understand(&mut self, app: &mut App, now_ms: u64) {
        self.hits.clear();
        self.rows.clear();
        self.pane = None;
        self.bar = 0;
        self.wake_at = None;
        if let Some(phrase) = self.typed.strip_prefix('/') {
            self.hits = find_all(app.page().doc(), phrase, false);
            self.pane = Some(Pane::Page);
            match self.hits.first() {
                Some(first) => app.go(first.start),
                // Nothing found: back to where the line was opened from,
                // still held for the next try.
                None => {
                    app.restore();
                    app.hold_place();
                }
            }
        } else if self.typed == "set" {
            self.rows = vec![entry(0, "look", &app.look().name, false), entry(0, "measure", &app.prefs().measure.to_string(), false), entry(0, "scale", &app.prefs().scale.to_string(), false)];
            self.rows.extend((0..60).map(|n| entry(0, &format!("option {n}"), "off", false)));
            self.pane = Some(Pane::Contents);
        } else if self.typed.starts_with('#') {
            let doc = app.page().doc();
            self.rows = (0..doc.line_count()).filter(|&line| doc.line(line).starts_with('#')).map(|line| entry(0, doc.line(line).trim_start_matches(['#', ' ']), "", false)).collect();
            self.pane = Some(Pane::Labels);
        } else if self.typed == "show" {
            self.wake_at = Some(now_ms + 250);
        }
    }
}

impl Line for Little {
    fn is_open(&self) -> bool {
        self.open
    }

    fn open(&mut self, with: &str, app: &mut App, _host: &mut dyn Host) {
        self.log.borrow_mut().push(format!("open {with:?}"));
        self.open = true;
        self.typed = with.to_string();
        app.hold_place();
    }

    fn key(&mut self, key: Key, mods: Mods, app: &mut App, host: &mut dyn Host) -> bool {
        self.log.borrow_mut().push(format!("key {key:?}{}", if mods.any() { " with a modifier" } else { "" }));
        if mods.ctrl || mods.alt {
            return false;
        }
        match key {
            Key::Escape => {
                app.restore();
                self.close();
            }
            Key::Enter => {
                app.keep_place();
                let kept = format!("Kept {}.", self.typed);
                self.close();
                app.say(kept);
            }
            Key::Backspace => {
                self.typed.pop();
                self.understand(app, host.now_ms());
            }
            Key::Up => self.bar = self.bar.saturating_sub(1),
            Key::Down => self.bar = (self.bar + 1).min(self.rows.len().saturating_sub(1)),
            _ => return false,
        }
        true
    }

    fn text(&mut self, ch: char, app: &mut App, host: &mut dyn Host) {
        self.typed.push(ch);
        self.understand(app, host.now_ms());
    }

    fn step(&mut self, way: Way, _app: &mut App, _host: &mut dyn Host) {
        self.log.borrow_mut().push(format!("step {way:?}"));
    }

    fn keys(&mut self, _app: &mut App, _host: &mut dyn Host) {
        self.log.borrow_mut().push(String::from("keys"));
    }

    fn typed_on_page(&mut self, app: &mut App, _host: &mut dyn Host) {
        let head = app.page().cursor.head;
        let before = app.page().doc().line(head.line).get(..head.col).unwrap_or("");
        self.offer = before.ends_with("[[").then(|| String::from("Essay plan"));
    }

    fn tab(&mut self, app: &mut App, host: &mut dyn Host) -> bool {
        let Some(offer) = self.offer.take() else { return false };
        let clock = app.clock(host);
        app.page_mut().insert(&format!("{offer}]]"), clock)
    }

    fn tick(&mut self, app: &mut App, host: &mut dyn Host) {
        self.log.borrow_mut().push(String::from("tick"));
        if self.wake_at.is_some_and(|at| host.now_ms() >= at) {
            self.wake_at = None;
            app.preview("Another text.\n\nShown, and not opened.");
        }
    }

    fn wake_in_ms(&self, now_ms: u64) -> Option<u64> {
        self.wake_at.map(|at| at.saturating_sub(now_ms))
    }

    fn typed(&self) -> &str {
        &self.typed
    }

    fn caret(&self) -> usize {
        self.typed.chars().count()
    }

    fn hint(&self) -> Option<&str> {
        Some("a title   # heading   / phrase   set")
    }

    fn say(&self) -> Option<&str> {
        self.open.then_some(if self.hits.is_empty() { "Enter goes back to where you were, in The orchard." } else { "Enter stays here." })
    }

    fn answer(&self) -> Option<Answer<'_>> {
        self.pane.map(|pane| Answer { pane, rows: &self.rows, bar: (!self.rows.is_empty()).then_some(self.bar) })
    }

    fn hits(&self) -> &[Span] {
        &self.hits
    }

    fn offer(&self) -> Option<&str> {
        self.offer.as_deref()
    }
}

fn little(text: &str, width: usize, height: usize) -> (Rig, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let line = Little { log: log.clone(), ..Little::default() };
    let mut rig = Rig::with_seams(Options::default(), Box::new(Memory::default()), Box::new(line), Box::new(Rest));
    rig.size(width, height);
    rig.app.open_text(opened(text));
    // Rolled up as far as the wheel takes it: a short text from row 1.
    rig.wheel(-1000);
    (rig, log)
}

#[test]
fn esc_puts_the_cursor_on_the_line_and_a_hollow_box_keeps_its_place() {
    let (mut rig, log) = little(ESSAY, 1920, 1080);
    let on_page = rig.screen().cursor.unwrap();
    assert_eq!(rig.screen().kept, None);
    rig.press("esc");
    assert_eq!(*log.borrow(), ["open \"\""]);
    // The solid cursor is after `ok` and a space; the page keeps a hollow
    // box where it was.
    assert_eq!(rig.screen().cursor, Some(CursorBox { col: 50, row: 47, wide: 1, tall: 1, hollow: false }));
    assert_eq!(rig.screen().kept, Some(CursorBox { hollow: true, ..on_page }));
    assert_eq!(rig.wants().caret, Some(rig.app.grid().cells(50, 47, 1, 1)));
    // The empty line shows what it understands, faint, and the say row
    // what Enter will do.
    assert!(read(&mut rig, 47, 112, 47).starts_with("ok a title   # heading   / phrase   set  "));
    assert!(read(&mut rig, 47, 112, 47).ends_with("  37 words"));
    assert_eq!(strengths(&mut rig, 51, 60, 47)[0], Strength::FAINT);
    assert!(rig.screen().at(50, 47).is_some_and(|cell| cell.inverse && cell.ch == 'a'), "the cursor sits on its first letter");
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    // Typing goes to the line and leaves the page alone.
    rig.type_text("orch");
    assert_eq!(rig.text(), ESSAY);
    assert_eq!(read(&mut rig, 47, 60, 47), "ok orch");
    assert_eq!(strengths(&mut rig, 50, 53, 47), [Strength::INK; 4]);
    assert_eq!(rig.screen().cursor.map(|at| (at.col, at.row)), Some((54, 47)));
    // Esc again and the cursor is back on the page.
    rig.press("esc");
    assert_eq!(rig.screen().cursor, Some(on_page));
    assert_eq!(rig.screen().kept, None);
    assert_eq!(read(&mut rig, 47, 60, 47), "ok");
}

#[test]
fn the_facts_give_way_to_what_is_typed_on_the_line() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let session = Journal { first: opened(ESSAY), name: String::from("The orchard"), contents: Vec::new(), links: Links::default(), told: Rc::default(), answers: Rc::default() };
    let mut rig = Rig::with_seams(Options::default(), Box::new(session), Box::new(Little { log, ..Little::default() }), Box::new(Rest));
    rig.size(1920, 1080);
    assert!(read(&mut rig, 47, 112, 47).ends_with("The orchard    37 words"));
    rig.press("esc");
    rig.type_text(&"long ".repeat(8));
    assert!(read(&mut rig, 47, 112, 47).ends_with("    37 words"), "the name goes first");
    assert!(!read(&mut rig, 47, 112, 47).contains("The orchard"));
    rig.type_text(&"long ".repeat(3));
    assert_eq!(read(&mut rig, 47, 112, 47), format!("ok {}", "long ".repeat(11)).trim_end());
}

#[test]
fn a_phrase_found_from_the_line_shows_its_hits_and_esc_puts_everything_back() {
    let long = format!("{}\n\n{}", "Nothing here.\n\n".repeat(60), ESSAY);
    let (mut rig, _) = little(&long, 1920, 1080);
    rig.press("ctrl+home");
    let before = (head(&rig), rig.app.view().top(), rig.screen().lines());
    rig.press("ctrl+f");
    assert_eq!(read(&mut rig, 47, 60, 47), "ok /");
    rig.type_text("the");
    // The first hit has come to the platen and every hit is inverse.
    assert_eq!(rig.screen().big(ok_ui::grid::Cols { first: 47, last: 112 }, 28), "The orchard");
    let inverse: Vec<bool> = (0..5).map(|along| rig.screen().at(47 + along * 2, 28).unwrap().inverse).collect();
    assert_eq!(inverse, [true, true, true, false, false], "The, in the double-size heading");
    assert_eq!(said(&mut rig), "Enter stays here.");
    let hits = (0..45).flat_map(|row| (47..113).map(move |col| (col, row))).filter(|&(col, row)| rig.screen().at(col, row).is_some_and(|cell| cell.inverse)).count();
    assert!(hits >= 3 * 2 + 3 * 4, "{hits} inverse cells");
    // A flag beside each row of the map that holds a hit.
    let look = Look::manila();
    let geometry = ok_ui::map::geometry(rig.app.grid(), rig.app.page()).unwrap();
    let hit_row = rig.app.page().layout().locate(rig.app.page().doc(), Pos::new(122, 2)).0;
    let flag_x = geometry.area.x + 132 + 6;
    assert_eq!(rig.pixel(flag_x, geometry.y(hit_row).unwrap()), Some(look.desk_ink));
    assert_eq!(rig.pixel(flag_x, geometry.y(10).unwrap()), Some(look.desk));
    // Esc: the cursor, the paper and the picture are as they were.
    rig.press("esc");
    assert_eq!((head(&rig), rig.app.view().top(), rig.screen().lines()), before);
    // Enter keeps the place instead, and the line may speak as it closes.
    rig.press("ctrl+f");
    rig.type_text("apricots");
    rig.press("enter");
    assert_eq!(head(&rig), (128, 32));
    assert_eq!(said(&mut rig), "Kept /apricots.");
    assert_eq!(rig.screen().cursor.map(|at| at.row), Some(28));
    // With the line closed nothing is inverse but the TK and the cursor.
    let screen = rig.screen().clone();
    let inverse = (0..45).flat_map(|row| (47..113).map(move |col| (col, row))).filter(|&(col, row)| screen.at(col, row).is_some_and(|cell| cell.inverse)).count();
    assert_eq!(inverse, 3);
}

#[test]
fn a_list_from_the_line_takes_the_place_of_the_contents_with_a_bar_on_one_row() {
    let (mut rig, _) = little(ESSAY, 1920, 1080);
    rig.press("esc");
    rig.type_text("set");
    assert_eq!(read(&mut rig, 0, 40, 1), format!("  look {} manila", ".".repeat(23)));
    assert_eq!(read(&mut rig, 0, 40, 2), format!("  measure {} 66", ".".repeat(24)));
    assert_eq!(read(&mut rig, 0, 40, 3), format!("  scale {} 1", ".".repeat(27)));
    let bar = |rig: &mut Rig, row: i32| (2..=36).all(|col| rig.screen().at(col, row).is_some_and(|cell| cell.inverse));
    assert!(bar(&mut rig, 1) && !bar(&mut rig, 2));
    presses(&mut rig, &["down", "down"]);
    assert!(!bar(&mut rig, 1) && bar(&mut rig, 3));
    // A list longer than the pane scrolls to keep the bar in view.
    for _ in 0..60 {
        rig.press("down");
    }
    assert_eq!(read(&mut rig, 0, 40, 44), format!("  option 59 {} off", ".".repeat(21)));
    assert!(bar(&mut rig, 44));
    assert_eq!(read(&mut rig, 0, 40, 1), format!("  option 16 {} off", ".".repeat(21)));
    assert_eq!(read(&mut rig, 0, 40, 45), "");
    rig.press("esc");
    assert_eq!(read(&mut rig, 0, 40, 1), "", "and the pane goes back to its own rows");
}

#[test]
fn a_list_for_the_labels_is_drawn_in_their_place() {
    let (mut rig, _) = little(ESSAY, 1920, 1080);
    assert_eq!(read(&mut rig, 134, 157, 2), "Thinning");
    rig.press("esc");
    rig.type_text("#");
    assert_eq!(read(&mut rig, 134, 157, 1), "The orchard");
    assert_eq!(read(&mut rig, 134, 157, 2), "Thinning");
    assert_eq!(read(&mut rig, 134, 157, 3), "What he showed me");
    assert!((134..=157).all(|col| rig.screen().at(col, 1).is_some_and(|cell| cell.inverse)));
    rig.press("down");
    assert!((134..=157).all(|col| rig.screen().at(col, 2).is_some_and(|cell| cell.inverse)));
    assert!(!rig.screen().at(134, 1).unwrap().inverse);
}

#[test]
fn when_its_pane_is_not_on_screen_a_list_rises_from_the_say_row() {
    let (mut rig, _) = little(ESSAY, 960, 528);
    assert_eq!(rig.app.grid().contents, None);
    assert_eq!(rig.app.grid().say, Some(21));
    rig.press("esc");
    rig.type_text("set");
    // Eight rows at most, over the foot of the sheet, the bar in inverse.
    assert_eq!(read(&mut rig, 7, 72, 12), "The branches had split under the weight.", "above the eight rows the sheet is as it was");
    assert_eq!(read(&mut rig, 7, 72, 13), format!("look {} manila", ".".repeat(54)));
    assert_eq!(read(&mut rig, 7, 72, 14), format!("measure {} 66", ".".repeat(55)));
    assert_eq!(read(&mut rig, 7, 72, 20), format!("option 4 {} off", ".".repeat(53)));
    assert!((7..=72).all(|col| rig.screen().at(col, 13).is_some_and(|cell| cell.inverse)));
    assert_eq!(said(&mut rig), "Enter goes back to where you were, in The orchard.");
    assert_eq!(read(&mut rig, 7, 20, 22), "ok set");
    for _ in 0..20 {
        rig.press("down");
    }
    assert_eq!(read(&mut rig, 7, 72, 17), format!("option 17 {} off", ".".repeat(52)));
    assert!((7..=72).all(|col| rig.screen().at(col, 17).is_some_and(|cell| cell.inverse)));
    // The same list on a screen with a contents pane does not rise.
    let (mut rig, _) = little(ESSAY, 1920, 1080);
    rig.press("esc");
    rig.type_text("set");
    assert_eq!(read(&mut rig, 47, 112, 44), "");
    assert_eq!(read(&mut rig, 47, 112, 38), "");
}

#[test]
fn the_line_tries_another_text_on_the_page_and_puts_this_one_back() {
    let (mut rig, log) = little(ESSAY, 1920, 1080);
    rig.press("ctrl+home");
    rig.press("down");
    let before = (head(&rig), rig.app.view().top(), rig.screen().lines());
    rig.press("esc");
    rig.type_text("show");
    // The bar has to rest 250 ms before the page shows what is under it.
    assert_eq!(rig.wants().wake_in_ms, Some(250));
    rig.wait(249);
    assert_eq!(rig.screen().big(ok_ui::grid::Cols { first: 47, last: 112 }, 1), "The orchard", "this text is still on the page");
    rig.wait(1);
    assert!(log.borrow().contains(&String::from("tick")));
    assert_eq!(read(&mut rig, 47, 112, 1), "Another text.");
    assert_eq!(read(&mut rig, 47, 112, 3), "Shown, and not opened.");
    assert_eq!(rig.wants().wake_in_ms, None);
    // Esc: the text, the cursor and the paper that were there.
    rig.press("esc");
    assert_eq!(rig.text(), ESSAY);
    rig.draw();
    assert_eq!((head(&rig), rig.app.view().top()), (before.0, before.1));
    assert_eq!(rig.screen().lines(), before.2);
    // And it is this text that takes the next letter.
    rig.type_text("x");
    assert!(rig.text().starts_with("# The orchard\nx\n"));
}

#[test]
fn keys_go_to_the_open_line_first_and_only_the_window_s_chords_come_back() {
    let (mut rig, log) = little(ESSAY, 1920, 1080);
    // Closed: F3, Shift F3, F1 and Ctrl F are the line's and arrive as calls.
    presses(&mut rig, &["f3", "shift+f3", "f1"]);
    assert_eq!(*log.borrow(), ["step On", "step Back", "keys"]);
    rig.press("ctrl+f");
    log.borrow_mut().clear();
    // Open: everything arrives as a key, and what the line hands back is
    // dropped unless it is one of the window's own chords.
    presses(&mut rig, &["left", "ctrl+z", "f3", "pagedown", "tab", "ctrl+a", "alt+up", "delete"]);
    assert_eq!(log.borrow().len(), 8);
    assert_eq!((rig.text(), selected(&rig)), (String::from(ESSAY), String::new()));
    assert_eq!(read(&mut rig, 47, 60, 47), "ok /");
    rig.press("ctrl+=");
    assert_eq!(rig.app.grid().scale, 2);
    rig.press("ctrl+-");
    rig.press("f11");
    assert!(!rig.wants().fullscreen);
    rig.press("ctrl+q");
    assert!(rig.wants().quit);
}

#[test]
fn an_offer_is_drawn_after_the_cursor_and_tab_takes_it() {
    let (mut rig, _) = little("See ", 1920, 1080);
    rig.type_text("[[");
    assert_eq!(read(&mut rig, 47, 112, 1), "See [[ Essay plan");
    assert_eq!(strengths(&mut rig, 54, 64, 1), [Strength::HALF; 9]);
    assert_eq!(rig.screen().cursor.map(|at| at.col), Some(53));
    assert_eq!(rig.text(), "See [[", "the offer is not in the text");
    rig.press("tab");
    assert_eq!(rig.text(), "See [[Essay plan]]");
    assert_eq!(read(&mut rig, 47, 112, 1), "See [[Essay plan]]");
    // Ctrl Z takes it back like any other edit; typing on dismisses it.
    rig.press("ctrl+z");
    assert_eq!(rig.text(), "See [[");
    rig.type_text("x");
    assert_eq!(read(&mut rig, 47, 112, 1), "See [[x");
    rig.press("tab");
    assert_eq!(rig.text(), "See [[x");
    // In a list Tab nests the item and the line is not asked.
    let (mut rig, _) = little("- see ", 1920, 1080);
    rig.type_text("[[");
    rig.press("tab");
    assert_eq!(rig.text(), "  - see [[");
}

// ───────────────────────── a stand-in flow ─────────────────────────

/// A flow that writes down what it is told and dims what it is asked to.
#[derive(Default, Clone)]
struct Dimmer {
    acts: Rc<RefCell<Vec<Act>>>,
    /// The most any part beside the page may be.
    parts: Rc<Cell<Option<Strength>>>,
    /// The most any paragraph but his own may be.
    others: Rc<Cell<Option<Strength>>>,
    /// The lines that were asked about with `own` set, in one frame.
    own: Rc<RefCell<Vec<usize>>>,
    animating: Rc<Cell<bool>>,
    wake: Rc<Cell<Option<u64>>>,
    to_say: Rc<RefCell<Option<Said>>>,
    ticks: Rc<Cell<usize>>,
}

impl Flow for Dimmer {
    fn note(&mut self, act: Act, _host: &dyn Host) {
        self.acts.borrow_mut().push(act);
    }

    fn tick(&mut self, _host: &dyn Host) -> Option<Said> {
        self.ticks.set(self.ticks.get() + 1);
        self.to_say.borrow_mut().take()
    }

    fn animating(&self, _now_ms: u64) -> bool {
        self.animating.get()
    }

    fn wake_in_ms(&self, _now_ms: u64) -> Option<u64> {
        self.wake.get()
    }

    fn part(&self, _part: Part, rest: Strength, _now_ms: u64) -> Strength {
        self.parts.get().map_or(rest, |most| rest.min(most))
    }

    fn text(&self, line: usize, own: bool, rest: Strength, _now_ms: u64) -> Strength {
        if own {
            self.own.borrow_mut().push(line);
            return rest;
        }
        self.others.get().map_or(rest, |most| rest.min(most))
    }
}

fn dimmer(text: &str) -> (Rig, Dimmer) {
    let flow = Dimmer::default();
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let options = Options { file: Some(desk.join("College/Essay plan.md")), folder: Some(desk), look: None, store: false };
    let mut rig = Rig::with_seams(Options::default(), Box::new(Memory::new(&options)), Box::new(Closed), Box::new(flow.clone()));
    rig.size(1920, 1080);
    rig.app.open_text(opened(text));
    rig.wheel(-1000);
    (rig, flow)
}

#[test]
fn the_flow_is_told_what_he_does_in_the_order_he_does_it() {
    let (mut rig, flow) = dimmer("One.\n\nTwo.");
    let told = |rig: &mut Rig, act: &mut dyn FnMut(&mut Rig)| -> Vec<Act> {
        flow.acts.borrow_mut().clear();
        act(rig);
        flow.acts.borrow().clone()
    };
    assert_eq!(told(&mut rig, &mut |rig| rig.type_text("ab")), [Act::Typed('a'), Act::Typed('b')]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("backspace")), [Act::Erased]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("delete")), [], "nothing was there to take");
    assert_eq!(told(&mut rig, &mut |rig| rig.press("left")), [Act::Moved { left: false }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("up")), [Act::Moved { left: true }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("ctrl+up")), [Act::Chord, Act::Moved { left: true }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("ctrl+end")), [Act::Chord, Act::Moved { left: true }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("enter")), [Act::Entered { first: 2, last: 2 }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("ctrl+backspace")), [Act::Chord, Act::Erased]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("ctrl+z")), [Act::Chord]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("ctrl+c")), [Act::Chord]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("alt+up")), [Act::Chord]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("esc")), [Act::Escape]);
    assert_eq!(told(&mut rig, &mut |rig| rig.press("pageup")), [Act::Moved { left: true }]);
    assert_eq!(told(&mut rig, &mut |rig| rig.wheel(2)), [Act::Mouse]);
    assert_eq!(told(&mut rig, &mut |rig| rig.click(700, 30)), [Act::Mouse, Act::Mouse]);
    assert_eq!(told(&mut rig, &mut |rig| rig.send(Event::Mouse { kind: MouseKind::Move, x: 5, y: 5, mods: Mods::NONE })), [Act::Mouse]);
    assert_eq!(told(&mut rig, &mut |rig| rig.hold(Mods::CTRL)), [], "a modifier alone is not yet a chord");
    rig.release();
}

#[test]
fn the_parts_beside_the_page_are_as_strong_as_the_flow_says() {
    let (mut rig, flow) = dimmer("# The orchard\n\nSee [[Essay plan]] for what is to come of it.");
    let parts = |rig: &mut Rig| -> [Vec<Strength>; 4] {
        [strengths(rig, 2, 36, 2), strengths(rig, 134, 157, 1), strengths(rig, 134, 157, 45), strengths(rig, 80, 112, 47)]
    };
    let [contents, labels, links, facts] = parts(&mut rig);
    assert!(contents.contains(&Strength::INK) && contents.contains(&Strength::FAINT), "the open text in ink, its dots faint");
    assert_eq!((labels[0], links[0], facts[0]), (Strength::HALF, Strength::HALF, Strength::HALF), "the label of his own paragraph, the title being this text's only heading");
    // Writing: all four to faint. The open text's name is no exception.
    flow.parts.set(Some(Strength::FAINT));
    let [contents, labels, links, facts] = parts(&mut rig);
    for part in [contents, labels, links, facts] {
        assert!(!part.is_empty() && part.iter().all(|&strength| strength == Strength::FAINT));
    }
    // A run: all four to ground, and nothing is drawn where they were.
    flow.parts.set(Some(Strength::GROUND));
    let look = Look::manila();
    for (col, row) in [(4, 2), (134, 1), (136, 45)] {
        let cell = rig.app.grid().cells(col, row, 8, 1);
        let frame = rig.draw();
        assert!((0..cell.w).all(|dx| (0..cell.h).all(|dy| frame.get(cell.x + dx, cell.y + dy) == Some(look.desk))), "column {col} row {row}");
    }
    // Part of the way through a fade is a strength like any other.
    flow.parts.set(Some(Strength::HALF.toward(Strength::FAINT, 128)));
    let between = rig.screen().at(134, 1).unwrap().strength;
    assert!(Strength::FAINT < between && between < Strength::HALF);
}

#[test]
fn every_paragraph_but_his_own_is_as_strong_as_the_flow_says() {
    let (mut rig, flow) = dimmer("First *one*.\n\nSecond, where\nthe cursor is.\n\n- Third");
    rig.press("ctrl+home");
    rig.press("ctrl+down");
    flow.own.borrow_mut().clear();
    flow.others.set(Some(Strength::HALF));
    assert_eq!(strengths(&mut rig, 47, 112, 1), [Strength::HALF; 11]);
    assert_eq!(strengths(&mut rig, 47, 112, 3)[1..], [Strength::INK; 11], "his own paragraph is never dimmed");
    assert_eq!(strengths(&mut rig, 47, 112, 4), [Strength::INK; 12]);
    assert_eq!(strengths(&mut rig, 47, 112, 6), [Strength::HALF; 6]);
    let mut own = flow.own.borrow().clone();
    own.dedup();
    assert!(own.iter().all(|line| (2..=3).contains(line)) && own.contains(&2) && own.contains(&3));
    // Weaker than half, the marks go down with the words.
    flow.others.set(Some(Strength::FAINT));
    assert_eq!(strengths(&mut rig, 47, 112, 1), [Strength::FAINT; 11]);
    // An emphasis keeps its underscore at whatever strength the words have.
    assert!((47 + 7..47 + 10).all(|col| rig.screen().at(col, 1).is_some_and(|cell| cell.under)));
    // Moving into another paragraph makes that one his.
    rig.press("ctrl+up");
    rig.press("ctrl+up");
    assert_eq!(strengths(&mut rig, 47, 112, 1)[0], Strength::INK);
    assert_eq!(strengths(&mut rig, 47, 112, 3)[1], Strength::FAINT);
}

#[test]
fn nothing_the_design_says_is_never_dimmed_asks_the_flow_anything() {
    let (mut rig, flow) = dimmer("Older.\n\nA TK here.");
    rig.type_text(" New.");
    flow.parts.set(Some(Strength::GROUND));
    flow.others.set(Some(Strength::GROUND));
    let look = Look::manila();
    // `ok`, the cursor, the change bar, the map's marks, the notch, a flag.
    assert_eq!(strengths(&mut rig, 47, 48, 47), [Strength::INK; 2]);
    assert!(rig.screen().cursor.is_some() && rig.screen().bar(3));
    let area = rig.app.grid().map_area().unwrap();
    assert_eq!(rig.pixel(area.x, area.y), Some(look.shade(ok_ui::look::Surface::Sheet, Strength::HALF)));
    assert_eq!(rig.pixel(area.x, area.y + 8), Some(look.ink));
    assert_eq!(rig.pixel(rig.app.grid().x(120) + 2, area.y + 8), Some(look.desk_ink));
    assert_eq!(rig.pixel(area.x + 138, area.y + 8), Some(look.desk_ink));
    // The other paragraph has gone, and his own has not.
    assert_eq!(strengths(&mut rig, 47, 112, 1), [Strength::GROUND; 6]);
    assert_eq!(strengths(&mut rig, 47, 112, 3)[0], Strength::INK);
}

#[test]
fn the_flow_asks_for_its_ticks_and_speaks_on_the_say_row() {
    let (mut rig, flow) = dimmer("One.");
    assert!(!rig.wants().animating && rig.wants().wake_in_ms.is_none());
    flow.wake.set(Some(2000));
    rig.type_text("x");
    assert_eq!(rig.wants().wake_in_ms, Some(2000));
    let before = flow.ticks.get();
    rig.wait(2000);
    assert_eq!(flow.ticks.get(), before + 1, "woken once, when it asked");
    flow.wake.set(None);
    // While a fade runs it is ticked every frame.
    flow.animating.set(true);
    rig.send(Event::Tick);
    assert!(rig.wants().animating);
    let before = flow.ticks.get();
    rig.wait(160);
    assert_eq!(flow.ticks.get(), before + 10);
    flow.animating.set(false);
    rig.send(Event::Tick);
    assert!(!rig.wants().animating);
    // A sentence for the say row, taken down by the clock.
    *flow.to_say.borrow_mut() = Some(Said { text: String::from("143 words without stopping."), for_ms: Some(6000) });
    rig.send(Event::Tick);
    assert_eq!(said(&mut rig), "143 words without stopping.");
    assert_eq!(rig.wants().wake_in_ms, Some(6000));
    rig.wait(5999);
    assert_eq!(said(&mut rig), "143 words without stopping.");
    rig.wait(1);
    assert_eq!(said(&mut rig), "");
    // Or left until the next key.
    *flow.to_say.borrow_mut() = Some(Said { text: String::from("Start typing."), for_ms: None });
    rig.send(Event::Tick);
    rig.wait(60_000);
    assert_eq!(said(&mut rig), "Start typing.");
    rig.type_text("x");
    assert_eq!(said(&mut rig), "");
}
