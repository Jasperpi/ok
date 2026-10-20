//! The real session, driven through the whole interface with no window: a
//! save layer on a writing folder made for the test under the build
//! directory, typing, pauses, other programs changing files, leaving and
//! coming back.
//!
//! The host's clock moves only when told, but the save layer's two threads
//! run in real time, so wherever an answer is awaited from them the test
//! ticks until it has come.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::said;
use ok_host::{Event, Host};
use ok_text::Pos;
use ok_ui::harness::Rig;
use ok_ui::session::Opened;
use ok_ui::{App, Options};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A folder of its own for one test, under the build directory and nowhere
/// else, gone again when the test is done.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/folder-tests").join(format!("{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("writing")).expect("a folder for the test can be made");
        Scratch(fs::canonicalize(&path).expect("the folder is there"))
    }

    /// The writing folder.
    fn folder(&self) -> PathBuf {
        self.0.join("writing")
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.folder().join(rel);
        fs::create_dir_all(path.parent().expect("a file has a folder")).expect("the folder can be made");
        fs::write(path, text).expect("the file can be written");
    }

    fn read(&self, rel: &str) -> String {
        String::from_utf8_lossy(&fs::read(self.folder().join(rel)).unwrap_or_default()).into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for _ in 0..50 {
            match fs::remove_dir_all(&self.0) {
                Ok(()) => return,
                Err(_) if !self.0.exists() => return,
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

fn options(folder: &Path, file: Option<&str>) -> Options {
    Options { store: true, folder: Some(folder.to_path_buf()), file: file.map(|file| folder.join(file)), ..Options::default() }
}

/// Starts the program full screen.
fn start(options: Options) -> Rig {
    let mut rig = Rig::new(options);
    rig.size(1920, 1080);
    rig
}

/// Ticks until `done`, for at most twenty seconds of real time.
fn until(rig: &mut Rig, what: &str, mut done: impl FnMut(&mut Rig) -> bool) {
    let started = Instant::now();
    loop {
        rig.send(Event::Tick);
        if done(rig) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "waited twenty seconds for {what}; the say row reads {:?}", said(rig));
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets the pause after typing pass and waits for `ok`.
fn saved(rig: &mut Rig) {
    rig.wait(2_100);
    until(rig, "ok", |rig| rig.app.session().on_disk());
}

/// Waits until the folder has been read: this many texts listed.
fn listed(rig: &mut Rig, texts: usize) {
    until(rig, "the folder to be listed", |rig| rig.app.session().names().len() == texts);
}

/// The program goes without its window being closed first, as when it
/// faults: nothing is asked of the session before it is dropped.
fn crash(rig: &mut Rig) {
    rig.app = App::new(&mut rig.host, Options::default());
}

/// The window is closed and the program ends.
fn leave(rig: &mut Rig) {
    rig.send(Event::Close);
    crash(rig);
}

/// The program is started again, by the same person on the same machine.
fn again(rig: &mut Rig, options: Options) {
    rig.app = App::new(&mut rig.host, options);
    rig.size(1920, 1080);
}

/// The sentence DESIGN.md gives for a text changed by another program. It
/// is 70 cells long, and the say row shows as much of a sentence as the
/// measure of 66 holds.
const NEWER: &str = "This text changed on disk. The newer one is shown; yours is kept.";

/// True when the say row reads `sentence`, or its first 66 cells.
fn says(rig: &mut Rig, sentence: &str) -> bool {
    let shown = said(rig);
    shown == sentence || (shown.chars().count() == 66 && sentence.starts_with(&shown))
}

/// What the machine line reads.
fn machine(rig: &mut Rig) -> String {
    let (text, row) = (rig.app.grid().text, rig.app.grid().machine.unwrap_or(-1));
    rig.screen().span(text, row)
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder).map(|listing| listing.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
}

/// Sets the host's clock to the real one, read as UTC, so that what the
/// save layer dates with the real clock falls on the host's today.
fn real_time(rig: &mut Rig) {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs() as i64).unwrap_or(0);
    let (days, of_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let shifted = days + 719_468;
    let (era, of_era) = (shifted.div_euclid(146_097), shifted.rem_euclid(146_097));
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    rig.host.set_time(year as i32, month as u8, day as u8, (of_day / 3_600) as u8, (of_day % 3_600 / 60) as u8);
    rig.send(Event::Tick);
}

fn open_named(rig: &mut Rig, name: &str, start: bool) -> Result<(), String> {
    let doc = rig.app.page().doc().clone();
    let opened = rig.app.session_mut().open_named(name, start, &doc, &mut rig.host)?;
    rig.app.open_text(opened);
    Ok(())
}

#[test]
fn typing_reaches_the_file_and_ok_says_so() {
    let scratch = Scratch::new("typing");
    scratch.write("Notes.md", "First line.\r\n");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    assert_eq!(rig.text(), "First line.\n");
    assert_eq!(rig.app.session().name(), "Notes");
    assert!(rig.app.session().on_disk() && !rig.app.session().failing());
    assert!(machine(&mut rig).starts_with("ok "), "{}", machine(&mut rig));

    rig.type_text("More words");
    // The first keystroke that changes the text takes ok away.
    assert!(!rig.app.session().on_disk());
    assert!(!machine(&mut rig).starts_with("ok"));
    assert_eq!(scratch.read("Notes.md"), "First line.\r\n");

    saved(&mut rig);
    // The file's own line endings are kept.
    assert_eq!(scratch.read("Notes.md"), "First line.\r\nMore words");
    assert!(machine(&mut rig).starts_with("ok "));
    assert!(machine(&mut rig).ends_with("Notes    4 words    2 today"), "{}", machine(&mut rig));

    // An undo is a change like any other: ok goes, the file follows.
    rig.press("ctrl+z");
    assert!(!rig.app.session().on_disk());
    saved(&mut rig);
    assert_eq!(scratch.read("Notes.md").replace("\r\n", "\n"), rig.text());

    leave(&mut rig);
    // Nothing of ok's own is ever put in the writing folder.
    assert_eq!(names(&scratch.folder()), ["Notes.md"]);
}

#[test]
fn a_text_is_left_by_link_and_by_name_and_both_files_are_right() {
    let scratch = Scratch::new("switching");
    scratch.write("Essay plan.md", "See [[The orchard]] and [[Not yet#Opening]].");
    scratch.write("The orchard.md", "Orchard text.");
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("Essay plan.md")));
    listed(&mut rig, 2);
    until(&mut rig, "the other text to be read", |rig| rig.app.session().exists("The orchard"));
    assert!(!rig.app.session().exists("Not yet"));
    let links = rig.app.session().links().cloned().unwrap_or_default();
    assert_eq!(links.see_also.iter().map(|row| (row.name.as_str(), row.exists)).collect::<Vec<_>>(), [("The orchard", true), ("Not yet", false)]);

    // Left while what was typed is still unsaved.
    rig.type_text(" x");
    rig.app.go(Pos::new(0, 8));
    rig.press("ctrl+enter");
    assert_eq!(rig.text(), "Orchard text.");
    assert_eq!(rig.app.session().name(), "The orchard");
    assert_eq!(rig.app.session().previous(), Some("Essay plan"));
    assert_eq!(rig.app.session().links().map(|links| links.linked_from.clone()), Some(vec!["Essay plan".to_string()]));
    until(&mut rig, "the text left behind to be written", |_| scratch.read("Essay plan.md") == "See [[The orchard]] and [[Not yet#Opening]]. x");

    rig.type_text(" y");
    open_named(&mut rig, "Essay plan", false).unwrap();
    assert_eq!(rig.text(), "See [[The orchard]] and [[Not yet#Opening]]. x");
    assert_eq!(rig.app.session().previous(), Some("The orchard"));
    until(&mut rig, "the orchard to be written", |_| scratch.read("The orchard.md") == "Orchard text. y");
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    // The text already on the page is not opened over itself.
    assert_eq!(open_named(&mut rig, "essay PLAN", false), Err("? Essay plan is already on the page.".to_string()));
    assert_eq!(open_named(&mut rig, "No such text", false), Err("? No text called No such text.".to_string()));

    // A link to a text that does not exist starts it, and no file is made
    // until something is written.
    rig.app.go(Pos::new(0, 27));
    rig.press("ctrl+enter");
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("", "Not yet"));
    assert!(!folder.join("Not yet.md").exists());
    rig.type_text("Started.");
    saved(&mut rig);
    assert_eq!(scratch.read("Not yet.md"), "Started.");
    assert_eq!(rig.app.session().names(), ["Essay plan", "Not yet", "The orchard"]);

    // A title typed on the line starts a text too; a backslash says which folder.
    open_named(&mut rig, "College\\Why engineering", true).unwrap();
    rig.type_text("Because.");
    saved(&mut rig);
    assert_eq!(scratch.read("College/Why engineering.md"), "Because.");
    assert_eq!(rig.app.session_mut().peek("Not yet").as_deref(), Some("Started."));
    leave(&mut rig);
}

#[test]
fn renaming_mends_the_links_in_every_other_text() {
    let scratch = Scratch::new("renaming");
    scratch.write("College/The orchard.md", "# The orchard\n\nSee [[The orchard#Thinning]] below.\n");
    scratch.write("College/Essay plan.md", "Plan: [[The orchard]] first, then [[College/The orchard|the essay]].\n");
    scratch.write("Notes.md", "[[the orchard#Thinning]] and `[[The orchard]]` in code.\n");
    scratch.write("Mock trial/Opening.md", "Compare [[college/the orchard]].\r\n");
    let mut rig = start(options(&scratch.folder(), Some("College/The orchard.md")));
    listed(&mut rig, 4);
    until(&mut rig, "every text to be read", |rig| rig.app.session().links_here() == 4);

    let doc = rig.app.page().doc().clone();
    assert_eq!(rig.app.session_mut().rename("Apricots", &doc, &mut rig.host), Ok(4));
    assert_eq!(rig.app.session().name(), "Apricots");
    assert!(scratch.folder().join("College/Apricots.md").exists() && !scratch.folder().join("College/The orchard.md").exists());
    assert_eq!(scratch.read("College/Essay plan.md"), "Plan: [[Apricots]] first, then [[College/Apricots|the essay]].\n");
    assert_eq!(scratch.read("Notes.md"), "[[Apricots#Thinning]] and `[[The orchard]]` in code.\n");
    assert_eq!(scratch.read("Mock trial/Opening.md"), "Compare [[College/Apricots]].\r\n");
    assert_eq!(rig.app.session().links_here(), 4);
    // The link the text holds to itself is mended on the page at the next tick.
    rig.send(Event::Tick);
    assert_eq!(rig.text(), "# The orchard\n\nSee [[Apricots#Thinning]] below.\n");
    saved(&mut rig);
    assert_eq!(scratch.read("College/Apricots.md"), "# The orchard\n\nSee [[Apricots#Thinning]] below.\n");

    // A title with a slash in it moves the text, and the links follow.
    let doc = rig.app.page().doc().clone();
    assert_eq!(rig.app.session_mut().rename("Drafts/Apricot essay", &doc, &mut rig.host), Ok(4));
    assert!(scratch.folder().join("Drafts/Apricot essay.md").exists());
    assert_eq!(scratch.read("College/Essay plan.md"), "Plan: [[Apricot essay]] first, then [[Drafts/Apricot essay|the essay]].\n");
    assert_eq!(scratch.read("Mock trial/Opening.md"), "Compare [[Drafts/Apricot essay]].\r\n");
    rig.send(Event::Tick);
    assert_eq!(rig.text(), "# The orchard\n\nSee [[Apricot essay#Thinning]] below.\n");

    // A name that is taken is refused, and nothing moves.
    let doc = rig.app.page().doc().clone();
    assert_eq!(rig.app.session_mut().rename("\\Notes", &doc, &mut rig.host), Err("? A text called Notes is already there.".to_string()));
    assert_eq!(rig.app.session().name(), "Apricot essay");
    leave(&mut rig);
    assert_eq!(scratch.read("Drafts/Apricot essay.md"), "# The orchard\n\nSee [[Apricot essay#Thinning]] below.\n");
}

#[test]
fn a_discarded_text_is_moved_and_nothing_is_erased() {
    let scratch = Scratch::new("discard");
    scratch.write("A.md", "Kept text.");
    scratch.write("B.md", "Going");
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("B.md")));
    listed(&mut rig, 2);
    rig.type_text(" away");
    let doc = rig.app.page().doc().clone();
    let opened: Opened = rig.app.session_mut().discard(&doc, &mut rig.host).unwrap();
    rig.app.open_text(opened);
    // What was typed and not yet saved went with it.
    assert_eq!(fs::read_to_string(folder.join(".discarded/B.md")).unwrap(), "Going away");
    assert!(!folder.join("B.md").exists());
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("Kept text.", "A"));
    assert_eq!(said(&mut rig), "Moved to .discarded. Nothing was erased.");
    assert_eq!(rig.app.session().names(), ["A"]);
    assert!(rig.app.session().on_disk());

    // The last text of all leaves an empty one in its place.
    let doc = rig.app.page().doc().clone();
    let opened = rig.app.session_mut().discard(&doc, &mut rig.host).unwrap();
    rig.app.open_text(opened);
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("", "Untitled"));
    assert_eq!(fs::read_to_string(folder.join(".discarded/A.md")).unwrap(), "Kept text.");
    leave(&mut rig);
}

#[test]
fn versions_are_listed_and_one_is_brought_back() {
    let scratch = Scratch::new("versions");
    scratch.write("Draft.md", "");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    rig.type_text("one");
    rig.press("ctrl+s");
    until(&mut rig, "the first version", |rig| said(rig).starts_with("Kept "));
    // The answer is the truth: the size is read back from the disk.
    assert_eq!(said(&mut rig), "Kept 14:02. 3 bytes on disk.");
    assert!(rig.app.session().on_disk());
    rig.type_text(" two");
    rig.press("ctrl+s");
    until(&mut rig, "the second version", |rig| said(rig) == "Kept 14:02. 7 bytes on disk.");

    real_time(&mut rig);
    let rows = rig.app.session_mut().versions();
    assert!(rows.len() >= 2, "{rows:?}");
    assert!(rows[0].name.starts_with("Today ") && rows[0].name.ends_with(", kept"), "{rows:?}");
    assert_eq!(rows[0].name.len(), "Today 14:02, kept".len());
    assert_eq!(rows[0].value, "2");
    let earlier = rows.iter().position(|row| row.value == "1" && row.name.ends_with(", kept")).expect("the first version is listed");
    assert_eq!(rig.app.session_mut().version(earlier).as_deref(), Some("one"));

    let doc = rig.app.page().doc().clone();
    let opened = rig.app.session_mut().bring_back(earlier, &doc, &mut rig.host).unwrap();
    rig.app.open_text(opened);
    assert_eq!(rig.text(), "one");
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert_eq!(scratch.read("Draft.md"), "one");
    // The text it took the place of is kept too.
    let rows = rig.app.session_mut().versions();
    let replaced = rows.iter().position(|row| row.name.ends_with(", replaced")).expect("the present text was kept");
    assert_eq!(rig.app.session_mut().version(replaced).as_deref(), Some("one two"));
    leave(&mut rig);
}

#[test]
fn coming_back_is_the_same_text_row_and_cell() {
    let scratch = Scratch::new("return");
    // Long enough that the row he is on can be held at the platen.
    let filler: String = (0..40).map(|n| format!("\n\nParagraph {n} stands here.")).collect();
    let essay = format!("First paragraph here.\n\nSecond paragraph here.{filler}");
    scratch.write("Essay.md", &essay);
    scratch.write("Other.md", "Another text.");
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("Essay.md")));
    listed(&mut rig, 2);
    rig.host.set_time(2026, 10, 13, 23, 41);
    // Paragraph 30 is line 64 of the text.
    rig.app.go(Pos::new(64, 9));
    rig.type_text(" new");
    let row = rig.app.view().cursor_row(rig.app.page());
    assert_eq!(row, rig.app.grid().platen as i64);
    assert_eq!(rig.app.session().words_today(), 1);
    rig.app.session_mut().set_limit(Some(650));
    let today = rig.app.session().today(&rig.host);
    assert_eq!(rig.app.page().stamp(64), today);
    leave(&mut rig);
    assert_eq!(scratch.read("Essay.md"), essay.replace("Paragraph 30", "Paragraph new 30"));

    // The same evening: the text he was in, the cursor in its cell, its row
    // on the platen, today's lines still marked and today's words counted.
    again(&mut rig, options(&folder, None));
    assert_eq!(rig.app.session().name(), "Essay");
    assert_eq!(rig.app.page().cursor.head, Pos::new(64, 13));
    assert_eq!(rig.app.view().cursor_row(rig.app.page()), row);
    assert_eq!(said(&mut rig), "Last here today at 23:41. That sitting added 1 word.");
    assert_eq!((rig.app.page().stamp(64), rig.app.page().stamp(62), rig.app.page().stamp(0)), (today, 0, 0));
    assert_eq!(rig.app.session().words_today(), 1);
    assert_eq!(rig.app.session().limit(), Some(650));
    assert!(rig.app.session().on_disk());
    // The sentence stays for as long as he does not type.
    rig.wait(5_000);
    assert_eq!(said(&mut rig), "Last here today at 23:41. That sitting added 1 word.");
    leave(&mut rig);

    // Two mornings later.
    rig.host.set_time(2026, 10, 15, 9, 0);
    again(&mut rig, options(&folder, None));
    assert_eq!(said(&mut rig), "Last here Tuesday at 23:41. That sitting added 1 word.");
    assert_eq!(rig.app.page().cursor.head, Pos::new(64, 13));
    assert_eq!(rig.app.session().words_today(), 0);
    assert_ne!(rig.app.page().stamp(64), rig.app.session().today(&rig.host));
    assert!(machine(&mut rig).ends_with("Essay    167 of 650 words"), "{}", machine(&mut rig));
    rig.type_text("x");
    assert_eq!(said(&mut rig), "");
    leave(&mut rig);
}

#[test]
fn words_typed_after_the_last_save_come_back_after_a_crash() {
    let scratch = Scratch::new("crash");
    scratch.write("Essay.md", "Saved words.");
    let folder = scratch.folder();
    let mut rig = start(options(&folder, None));
    listed(&mut rig, 1);
    rig.type_text(" Three more words");
    // Long enough for the journal thread to have them, not for the file.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(scratch.read("Essay.md"), "Saved words.");
    crash(&mut rig);

    again(&mut rig, options(&folder, None));
    assert_eq!(said(&mut rig), "Brought back 3 words typed after the last save.");
    assert_eq!(rig.text(), "Saved words. Three more words");
    assert_eq!(rig.app.page().cursor.head, Pos::new(0, 29));
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert_eq!(scratch.read("Essay.md"), "Saved words. Three more words");
    leave(&mut rig);
    again(&mut rig, options(&folder, None));
    assert!(!said(&mut rig).starts_with("Brought back"));
    leave(&mut rig);
}

#[test]
fn a_file_changed_by_another_program_while_nothing_is_unsaved_is_shown() {
    let scratch = Scratch::new("outside-clean");
    scratch.write("A.md", "one\n");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    rig.type_text("mine");
    saved(&mut rig);
    scratch.write("A.md", "one\ntheirs\n");
    until(&mut rig, "the newer text", |rig| rig.text() == "one\ntheirs\n");
    assert!(says(&mut rig, NEWER), "{}", said(&mut rig));
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert!(!rig.app.session().failing());
    assert_eq!(scratch.read("A.md"), "one\ntheirs\n");
    // His is in earlier, as the sentence says.
    let rows = rig.app.session_mut().versions();
    let his = rows.iter().position(|row| row.name.ends_with(", set aside")).expect("what he had saved is a version");
    assert_eq!(rig.app.session_mut().version(his).as_deref(), Some("one\nmine"));
    // And typing goes on in the newer text.
    rig.type_text("x");
    saved(&mut rig);
    assert_eq!(scratch.read("A.md").replace("\r\n", "\n"), rig.text());
    leave(&mut rig);
}

#[test]
fn a_file_changed_by_another_program_while_he_has_unsaved_words_keeps_both() {
    let scratch = Scratch::new("outside-dirty");
    scratch.write("A.md", "one\n");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    rig.type_text("words of his own");
    assert!(!rig.app.session().on_disk());
    scratch.write("A.md", "theirs\n");
    until(&mut rig, "the newer text", |rig| rig.text() == "theirs\n");
    assert!(says(&mut rig, NEWER), "{}", said(&mut rig));
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert!(!rig.app.session().failing());
    assert_eq!(scratch.read("A.md"), "theirs\n");
    let rows = rig.app.session_mut().versions();
    let his = rows.iter().position(|row| row.name.ends_with(", yours")).expect("his unsaved text is a version");
    assert_eq!(rig.app.session_mut().version(his).as_deref(), Some("one\nwords of his own"));
    assert_eq!(rows[his].value, "5");
    leave(&mut rig);
    assert_eq!(names(&scratch.folder()), ["A.md"]);
}

#[test]
fn a_read_only_file_opens_for_reading_and_can_be_written_once_it_may_be() {
    let scratch = Scratch::new("read-only");
    scratch.write("Locked.md", "Not to be changed.");
    let path = scratch.folder().join("Locked.md");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();

    let mut rig = start(options(&scratch.folder(), None));
    assert_eq!(said(&mut rig), "This file is marked read-only, so it is open for reading only.");
    rig.type_text("x");
    assert_eq!(rig.text(), "Not to be changed.");
    assert!(rig.app.session().on_disk() && !rig.app.session().failing());
    rig.press("ctrl+s");
    assert_eq!(said(&mut rig), "This text is open for reading only.");

    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    until(&mut rig, "the page to take typing", |rig| !rig.app.page().read_only);
    rig.type_text(" Now it is.");
    saved(&mut rig);
    assert_eq!(scratch.read("Locked.md"), "Not to be changed. Now it is.");
    leave(&mut rig);
}

#[test]
fn a_write_that_fails_is_said_and_said_again_when_it_succeeds() {
    let scratch = Scratch::new("held");
    scratch.write("Essay.md", "Before.");
    let path = scratch.folder().join("Essay.md");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    rig.type_text(" After.");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();
    rig.wait(2_100);
    until(&mut rig, "the failure to be said", |rig| rig.app.session().failing());
    assert_eq!(said(&mut rig), "Not saved. This file is marked read-only. Still trying.");
    assert!(!rig.app.session().on_disk());
    assert!(!machine(&mut rig).starts_with("ok"));
    assert_eq!(scratch.read("Essay.md"), "Before.");

    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    until(&mut rig, "the write to succeed", |rig| rig.app.session().on_disk());
    assert_eq!(said(&mut rig), "Saved again. Nothing was lost.");
    assert!(!rig.app.session().failing());
    assert_eq!(scratch.read("Essay.md"), "Before. After.");
    leave(&mut rig);
}

#[test]
fn a_file_that_is_not_utf_8_is_shown_and_never_rewritten() {
    let scratch = Scratch::new("not-utf-8");
    let bytes = [0xff, 0xfe, b'h', 0x00, b'i', 0x00, 0xe9, b'\n'];
    fs::write(scratch.folder().join("Old.txt"), bytes).unwrap();
    let mut rig = start(options(&scratch.folder(), None));
    assert_eq!(said(&mut rig), "This file is not UTF-8, so it is open for reading only.");
    assert_eq!(rig.app.session().name(), "Old");
    let shown = rig.text();
    rig.type_text("typed");
    rig.press("backspace");
    assert_eq!(rig.text(), shown);
    assert!(!rig.app.session().failing());
    rig.wait(2_100);
    leave(&mut rig);
    assert_eq!(fs::read(scratch.folder().join("Old.txt")).unwrap(), bytes);
    assert_eq!(names(&scratch.folder()), ["Old.txt"]);
}

#[test]
fn the_first_run_makes_the_folder_and_the_first_line_names_the_text() {
    // No folder is named, so this is `ok` in the host's Documents, which
    // for this host is a folder of its own under the build directory.
    let run = Options { store: true, ..Options::default() };
    let mut rig = start(run.clone());
    let folder = rig.host.documents_dir().join("ok");
    assert_eq!(said(&mut rig), "Start typing. Each word is saved as you finish it.");
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("", "Untitled"));
    until(&mut rig, "the empty text to be made", |_| folder.join("Untitled.md").exists());
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert_eq!(rig.app.session().names(), ["Untitled"]);

    rig.type_text("# The orchard: a *draft*?");
    assert_eq!(said(&mut rig), "");
    assert_eq!(rig.app.session().name(), "Untitled");
    rig.type_text("\n");
    rig.send(Event::Tick);
    assert_eq!(rig.app.session().name(), "The orchard a draft");
    assert!(folder.join("The orchard a draft.md").exists() && !folder.join("Untitled.md").exists());
    assert_eq!(rig.wants().title, "The orchard a draft");

    rig.type_text("The orchard was eleven acres.");
    rig.wait(2_100);
    assert_eq!(said(&mut rig), "Esc goes to the line. Hold Ctrl or Alt to see what they do.");
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    assert_eq!(fs::read_to_string(folder.join("The orchard a draft.md")).unwrap(), "# The orchard: a *draft*?\n\nThe orchard was eleven acres.");
    assert!(machine(&mut rig).ends_with("The orchard a draft    9 words    9 today"), "{}", machine(&mut rig));
    assert_eq!(names(&folder), ["The orchard a draft.md"]);

    // A later first line does not rename a text that has a name.
    rig.app.go(Pos::new(0, 0));
    rig.type_text("Changed\n");
    rig.send(Event::Tick);
    assert_eq!(rig.app.session().name(), "The orchard a draft");
    leave(&mut rig);

    // Neither sentence is ever said again.
    again(&mut rig, run);
    assert_eq!(said(&mut rig), "Last here today at 14:02. That sitting added 10 words.");
    rig.type_text("x");
    rig.wait(2_100);
    assert_eq!(said(&mut rig), "");
    leave(&mut rig);
}

#[test]
fn the_days_words_never_fall_and_pasted_text_does_not_count() {
    let scratch = Scratch::new("today");
    scratch.write("A.md", "");
    scratch.write("B.md", "");
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("A.md")));
    listed(&mut rig, 2);
    rig.type_text("one two three");
    assert_eq!(rig.app.session().words_today(), 3);
    rig.press("ctrl+a");
    rig.press("ctrl+x");
    assert_eq!((rig.text().as_str(), rig.app.session().words_today()), ("", 3));
    rig.host.clipboard = Some("these pasted words".to_string());
    rig.press("ctrl+v");
    assert_eq!((rig.text().as_str(), rig.app.session().words_today()), ("these pasted words", 3));
    rig.press("ctrl+z");
    rig.press("ctrl+y");
    assert_eq!(rig.app.session().words_today(), 3);
    rig.type_text(" four");
    rig.press("backspace");
    rig.type_text("r");
    assert_eq!(rig.app.session().words_today(), 4);
    // The count is the folder's, not the text's.
    open_named(&mut rig, "B", false).unwrap();
    rig.type_text("five");
    assert_eq!(rig.app.session().words_today(), 5);
    saved(&mut rig);
    assert!(machine(&mut rig).ends_with("B    1 word    5 today"), "{}", machine(&mut rig));

    // The day begins at 04:00.
    rig.host.set_time(2026, 10, 12, 3, 59);
    rig.send(Event::Tick);
    assert_eq!(rig.app.session().words_today(), 5);
    rig.wait(61_000);
    assert_eq!(rig.app.session().words_today(), 0);
    assert!(machine(&mut rig).ends_with("B    1 word"), "{}", machine(&mut rig));
    leave(&mut rig);
}

#[test]
fn a_text_only_shown_on_the_page_is_never_written_into_the_open_one() {
    let scratch = Scratch::new("preview");
    scratch.write("Mine.md", "My own words.");
    scratch.write("Other.md", "Some other text, which the line is showing.");
    let mut rig = start(options(&scratch.folder(), Some("Mine.md")));
    listed(&mut rig, 2);
    rig.type_text(" And more.");
    // The line shows another text on the page while his is still unsaved,
    // and the pause passes with that other text in front.
    let other = rig.app.session_mut().peek("Other").unwrap_or_default();
    rig.app.preview(&other);
    assert_eq!(rig.text(), "Some other text, which the line is showing.");
    saved(&mut rig);
    assert_eq!(scratch.read("Mine.md"), "My own words. And more.");
    assert!(!rig.app.session().failing());
    // The window is closed with the other text still in front.
    rig.app.restore();
    rig.type_text(" Last.");
    rig.app.preview(&other);
    rig.send(Event::Close);
    assert_eq!(scratch.read("Mine.md"), "My own words. And more. Last.");
    assert_eq!(scratch.read("Other.md"), "Some other text, which the line is showing.");
    crash(&mut rig);
}

#[test]
fn a_close_that_is_called_off_leaves_everything_working() {
    let scratch = Scratch::new("close-twice");
    scratch.write("Essay.md", "");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    rig.type_text("a");
    rig.send(Event::Close);
    // Everything is on disk before the close returns.
    assert_eq!(scratch.read("Essay.md"), "a");
    assert!(rig.app.session().on_disk());
    // The sign-out was cancelled: the window stays and he types on.
    rig.type_text("b");
    assert!(!rig.app.session().on_disk());
    rig.send(Event::Close);
    assert_eq!(scratch.read("Essay.md"), "ab");
    rig.type_text("c");
    saved(&mut rig);
    assert_eq!(scratch.read("Essay.md"), "abc");
    rig.press("ctrl+s");
    until(&mut rig, "a version", |rig| said(rig).starts_with("Kept "));
    leave(&mut rig);
    assert_eq!(scratch.read("Essay.md"), "abc");
}

#[test]
fn settings_and_the_choice_of_folder_are_remembered() {
    let scratch = Scratch::new("settings");
    scratch.write("Here.md", "In the first folder.");
    let elsewhere = scratch.0.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("There.md"), "In the second folder.").unwrap();
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    assert_eq!(rig.app.session_mut().settings(), None);
    rig.app.session_mut().write_settings("look sun\nmeasure 72\n");
    assert_eq!(rig.app.session_mut().settings().as_deref(), Some("look sun\nmeasure 72\n"));
    assert!(!rig.app.session().folder().unwrap().to_string_lossy().starts_with(r"\\?\"));

    rig.type_text(" Typed before leaving.");
    let doc = rig.app.page().doc().clone();
    let opened = rig.app.session_mut().use_folder(&elsewhere, &doc, &mut rig.host).unwrap();
    rig.app.open_text(opened);
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("In the second folder.", "There"));
    assert_eq!(scratch.read("Here.md"), "In the first folder. Typed before leaving.");
    let folder = rig.app.session().folder().unwrap().to_path_buf();
    assert!(folder.ends_with("elsewhere"), "{}", folder.display());
    let doc = rig.app.page().doc().clone();
    assert_eq!(rig.app.session_mut().use_folder(&elsewhere, &doc, &mut rig.host).err().as_deref(), Some("? That is the writing folder already."));
    leave(&mut rig);

    // With no folder named, the next launch is where he chose to write.
    again(&mut rig, Options { store: true, ..Options::default() });
    assert_eq!(rig.app.session().name(), "There");
    assert_eq!(rig.app.session().folder(), Some(folder.as_path()));
    let settings = rig.app.session_mut().settings().unwrap_or_default();
    assert!(settings.starts_with("look sun\nmeasure 72\nfolder "), "{settings}");
    leave(&mut rig);
}

#[test]
fn typing_that_does_not_stop_is_still_written_every_twenty_seconds() {
    let scratch = Scratch::new("no-pause");
    scratch.write("Run.md", "");
    let mut rig = start(options(&scratch.folder(), None));
    listed(&mut rig, 1);
    // A letter a second for half a minute: never a pause of two seconds.
    rig.pace = 1_000;
    rig.type_text("abcdefghijklmnopqrstuvwxyz ab");
    until(&mut rig, "a save made while he was still typing", |_| scratch.read("Run.md").len() >= 20);
    assert!(scratch.read("Run.md").len() < 29, "{}", scratch.read("Run.md"));
    assert!(!rig.app.session().on_disk());
    rig.pace = 0;
    saved(&mut rig);
    assert_eq!(scratch.read("Run.md"), "abcdefghijklmnopqrstuvwxyz ab");
    leave(&mut rig);
}

#[test]
fn a_file_handed_over_by_a_second_launch_is_opened_where_it_lies() {
    let scratch = Scratch::new("handed");
    scratch.write("Here.md", "Here.");
    scratch.write("College/Plan.md", "The plan.");
    let away = scratch.0.join("away");
    fs::create_dir_all(&away).unwrap();
    fs::write(away.join("Letter.txt"), "A letter.").unwrap();
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("Here.md")));
    listed(&mut rig, 2);
    // This launch's own file comes round again and changes nothing.
    rig.type_text(" More.");
    rig.send(Event::Open(folder.join("Here.md")));
    assert_eq!(rig.text(), "Here. More.");
    // A file in a folder inside the writing folder, however it is spelled
    // where names ignore case (Linux's do not).
    let plan = if cfg!(target_os = "linux") { folder.join("College").join("Plan.md") } else { PathBuf::from(folder.to_string_lossy().to_uppercase()).join("college").join("plan.md") };
    rig.send(Event::Open(plan));
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("The plan.", "Plan"));
    until(&mut rig, "the text left behind to be written", |_| scratch.read("Here.md") == "Here. More.");
    assert!(!folder.join("plan.md").exists());
    // A file somewhere else is opened where it lies.
    rig.type_text(" Changed.");
    rig.send(Event::Open(away.join("Letter.txt")));
    assert_eq!((rig.text().as_str(), rig.app.session().name()), ("A letter.", "Letter"));
    assert_eq!(scratch.read("College/Plan.md"), "The plan. Changed.");
    assert!(rig.app.session().folder().is_some_and(|folder| folder.ends_with("away")));
    rig.type_text(" Yours.");
    saved(&mut rig);
    assert_eq!(fs::read_to_string(away.join("Letter.txt")).unwrap(), "A letter. Yours.");
    // Something that is not a text is refused in a sentence.
    rig.send(Event::Open(away.join("picture.png")));
    rig.send(Event::Tick);
    assert_eq!(said(&mut rig), "? That file is not a text.");
    leave(&mut rig);
}

/// Copies a folder and everything in it.
fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_folder(&source, &target);
        } else {
            fs::copy(&source, &target).unwrap();
        }
    }
}

/// Frames of the real session for looking at, written when `OK_SHOTS` names
/// the folder they go in. Every sentence in them is said by the session
/// itself, not put there by the test.
#[test]
fn frames_for_looking_at() {
    let Some(shots) = std::env::var_os("OK_SHOTS").map(PathBuf::from) else { return };
    let shot = |rig: &mut Rig, name: &str| rig.shot(&shots.join(format!("w2-folder-{name}.png"))).expect("the frame can be written");

    // The first run, and the first pause after the first line.
    let run = Options { store: true, ..Options::default() };
    let mut rig = start(run);
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    shot(&mut rig, "1-first-run");
    rig.type_text("# The orchard\nThe orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm.");
    rig.wait(2_100);
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    shot(&mut rig, "2-first-pause");
    leave(&mut rig);

    // A return to a folder of texts, two mornings after an evening's work.
    let scratch = Scratch::new("frames");
    copy_folder(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/desk"), &scratch.folder());
    let folder = scratch.folder();
    let mut rig = start(options(&folder, Some("College/The orchard.md")));
    listed(&mut rig, 11);
    rig.host.set_time(2026, 10, 13, 23, 41);
    rig.type_text("\nI think about that tree whenever I am asked to add one more thing to a week that is already full.");
    saved(&mut rig);
    shot(&mut rig, "3-saved");
    leave(&mut rig);
    rig.host.set_time(2026, 10, 15, 9, 0);
    again(&mut rig, options(&folder, None));
    until(&mut rig, "the folder to be read", |rig| rig.app.session().links_here() > 0);
    shot(&mut rig, "4-return");

    // A write that fails: the one alarming thing.
    rig.type_text(" For a long time I believed the answer was to keep all three.");
    let path = folder.join("College/The orchard.md");
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions.clone()).unwrap();
    rig.wait(2_100);
    until(&mut rig, "the failure to be said", |rig| rig.app.session().failing());
    shot(&mut rig, "5-not-saved");
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    until(&mut rig, "the write to succeed", |rig| rig.app.session().on_disk());
    shot(&mut rig, "6-saved-again");

    // Another program changes the file under him.
    let theirs = format!("{}\n\nA paragraph written somewhere else.", rig.text());
    fs::write(&path, theirs).unwrap();
    until(&mut rig, "the newer text", |rig| rig.text().ends_with("somewhere else."));
    until(&mut rig, "ok", |rig| rig.app.session().on_disk());
    shot(&mut rig, "7-changed-on-disk");

    // Words typed after the last save, and then the program is gone.
    rig.type_text(" And these were typed just before it went.");
    std::thread::sleep(Duration::from_millis(100));
    crash(&mut rig);
    again(&mut rig, options(&folder, None));
    shot(&mut rig, "8-brought-back");
    leave(&mut rig);
}
