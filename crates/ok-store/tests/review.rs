//! Tests written while the reviewers' findings were being resolved, for what
//! their own tests do not pin down: the answers the store now gives where it
//! used to give none, the cases in which a text is put beside its file
//! because nothing else can vouch for it, and the paths that only run when
//! another program is in the way at an awkward moment.

use std::fs::{self, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, Hold, News, Recovery, Store};
use ok_text::{Doc, Span};

const SOON: Duration = Duration::from_secs(10);

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

fn listen(store: &mut Store, time: Duration) -> Vec<News> {
    let mut seen = Vec::new();
    wait_news(store, &mut seen, time, |_| false);
    seen
}

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).expect("the file can be read")).expect("the file is UTF-8")
}

/// A new text holding `text`, saved.
fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

fn closed(store: &mut Store, id: DocId) -> News {
    expect(store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == id))
}

/// Waits until every recorded edit has been written to a journal file.
fn journaled(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up");
        while store.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Saves, and waits both for the save to be reported and for the journal
/// alarm, which was up, to be reported down. The two come in either order.
fn saved_and_calm(store: &mut Store, id: DocId, doc: &Doc, edit: u64) {
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let saved = wait_news(store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert!(saved.is_some(), "{seen:?}");
    if !seen.contains(&News::JournalAlarm(false)) {
        let calm = wait_news(store, &mut seen, SOON, |news| matches!(news, News::JournalAlarm(_)));
        assert_eq!(calm, Some(News::JournalAlarm(false)), "{seen:?}");
    }
    assert!(!seen.contains(&News::JournalAlarm(true)), "{seen:?}");
}

/// The one history folder there is.
fn history_folder(rig: &Rig) -> PathBuf {
    let folders: Vec<PathBuf> = fs::read_dir(rig.data.join("history")).unwrap().flatten().map(|entry| entry.path()).collect();
    assert_eq!(folders.len(), 1, "{folders:?}");
    folders[0].clone()
}

/// Puts a plain file where the text's history folder was, so that no version can be kept.
fn block_history(rig: &Rig) -> PathBuf {
    let folder = history_folder(rig);
    fs::remove_dir_all(&folder).unwrap();
    fs::write(&folder, b"").unwrap();
    folder
}

// ------------------------------------------------- a journal held at the start

/// The journal a kill left is held by a program that shares nothing when the
/// store starts. The start says the text is waiting. The program lets go,
/// he opens the text, and what he typed is there, unsaved, as after any
/// start that could not put a text back.
#[test]
fn a_journal_held_at_the_start_and_let_go_comes_back_when_the_text_is_opened() {
    let rig = Rig::new("review-unread");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    type_end(&mut store, id, &mut doc, " Two words.");
    journaled(&mut store);
    store.abandon();
    let journals = rig.journals();
    assert_eq!(journals.len(), 1);

    let holder = OpenOptions::new().read(true).share_mode(0).open(rig.data.join("journal").join(&journals[0])).unwrap();
    let started = Instant::now();
    let (mut store, report) = rig.open();
    assert!(started.elapsed() < Duration::from_secs(2), "a held journal kept the start waiting {:?}", started.elapsed());
    assert_eq!(report.iter().map(|told| (told.rel.as_path(), &told.how)).collect::<Vec<_>>(), [(Path::new("essay.md"), &Recovery::Waiting)]);
    assert_eq!(read(&rig.folder.join("essay.md")), "Saved.");
    assert_eq!(store.create_text("essay.md").err(), Some(ok_store::StoreError::Name(ok_store::NameError::Taken)), "a text that is waiting is not started over");
    drop(holder);

    let opened = store.open_text("essay.md").unwrap();
    assert_eq!((opened.text.as_str(), opened.restored), ("Saved. Two words.", Some(2)));
    store.save(opened.doc, opened.text.clone(), opened.edit);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { .. }));
    assert_eq!(read(&rig.folder.join("essay.md")), "Saved. Two words.");
    store.close(opened.doc, None, None);
    assert_eq!(closed(&mut store, opened.doc), News::Closed { doc: opened.doc, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty(), "{:?}", rig.journals());
    let (store, report) = rig.open();
    assert!(report.is_empty(), "{report:?}");
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing(), ["essay.md"]);
}

// ----------------------------------------------------- versions, kept or not

/// Ctrl S while another program's version is waiting to be taken or
/// refused: the version is kept at once, from his text, and he is told so;
/// the save itself is answered as held. A second version asked for when
/// none can be written is answered with `NotKept`, not with silence.
#[test]
fn keep_while_another_programs_version_waits_keeps_his_text_at_once_and_says_so() {
    let rig = Rig::new("review-keep-held");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let edit = type_end(&mut store, id, &mut doc, " Mine, not yet saved.");
    let mine = doc.to_text("\n");
    fs::write(rig.folder.join("essay.md"), "Theirs.").unwrap();
    store.save(id, mine.clone(), edit);
    expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { clean: false, .. }));

    store.keep(id, mine.clone(), edit, "mine");
    let mut seen = Vec::new();
    let held = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Held { .. }));
    assert_eq!(held, Some(News::Held { doc: id, why: Hold::ChangedOutside }), "{seen:?}");
    let kept = seen.iter().find(|news| matches!(news, News::Kept { .. })).unwrap_or_else(|| panic!("no Kept before the Held: {seen:?}"));
    assert!(matches!(kept, News::Kept { doc, edit: e, bytes, .. } if *doc == id && *e == edit && *bytes == mine.len() as u64), "{kept:?}");
    assert!(!seen.iter().any(|news| matches!(news, News::Saved { .. })), "{seen:?}");
    let versions = store.versions(id).unwrap();
    assert_eq!(versions[0].label.as_deref(), Some("mine"));
    assert_eq!(store.read_version(id, &versions[0].name).unwrap(), mine);
    assert_eq!(read(&rig.folder.join("essay.md")), "Theirs.");

    // A plain save asked for meanwhile is answered too, every time.
    for _ in 0..2 {
        store.save(id, mine.clone(), edit);
        assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::ChangedOutside });
    }

    // No version can be written now: asked to keep one, the store says it could not.
    block_history(&rig);
    store.keep(id, mine.clone(), edit, "again");
    let mut seen = Vec::new();
    let held = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Held { .. }));
    assert_eq!(held, Some(News::Held { doc: id, why: Hold::ChangedOutside }));
    assert!(seen.contains(&News::NotKept { doc: id, edit }), "{seen:?}");
    assert!(!seen.iter().any(|news| matches!(news, News::Kept { .. })), "{seen:?}");
    assert!(store.shutdown(SOON));
}

/// A text cut down to almost nothing is not written over the fuller text
/// while that fuller text can be kept nowhere: the save is held and says
/// why, and goes through once a version can be written again.
#[test]
fn a_big_cut_waits_until_the_fuller_text_can_be_kept_as_a_version() {
    let rig = Rig::new("review-cut");
    let long = "A sentence of the essay as it stood. ".repeat(100);
    fs::write(rig.folder.join("essay.md"), &long).unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    let blocked = block_history(&rig);

    let edit = replace_all(&mut store, opened.doc, &mut doc, "x");
    store.save(opened.doc, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. })), News::Held { doc: opened.doc, why: Hold::NoHistory });
    assert_eq!(read(&rig.folder.join("essay.md")), long, "the fuller text is still the file");
    assert_eq!(rig.listing(), ["essay.md"]);

    fs::remove_file(&blocked).unwrap();
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&rig.folder.join("essay.md")), "x");
    let versions = store.versions(opened.doc).unwrap();
    let cut = versions.iter().find(|version| version.label.as_deref() == Some("before-cut")).expect("the fuller text is a version now");
    assert_eq!(store.read_version(opened.doc, &cut.name).unwrap(), long);
    assert!(store.shutdown(SOON));
}

// ------------------------------------------- nothing can vouch for the text

/// The journal's disk is failing and another program has the file, so the
/// text can go to neither. Closing must not answer "it is in the journal":
/// the text is written into a file of its own beside the text, and the
/// answer says where.
#[test]
fn a_text_the_journal_cannot_vouch_for_is_put_beside_its_file_at_close() {
    let rig = Rig::new("review-unsaved");
    let tuning = quick();
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let path = rig.folder.join("essay.md");
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    let edit = type_end(&mut store, id, &mut doc, " Typed while the journal could not be written.");
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    let holder = OpenOptions::new().write(true).share_mode(7).open(&path).unwrap();

    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let News::Closed { beside: Some(beside), in_journal: false, .. } = closed(&mut store, id) else { panic!("expected the text beside its file") };
    assert!(beside.to_string_lossy().starts_with("essay (unsaved "), "{beside:?}");
    assert_eq!(read(&rig.folder.join(&beside)), doc.to_text("\n"));
    assert_eq!(read(&path), "Saved.");
    drop(holder);
    // With the text safe in a file the journal has nothing left to fail at.
    assert_eq!(expect(&mut store, "the alarm going down", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(false));
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    assert!(store.shutdown(SOON));
    let (store, report) = rig.open_with(tuning);
    assert!(report.is_empty(), "{report:?}");
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing().len(), 2, "{:?}", rig.listing());
}

// ------------------------------------------------- the interface's own slips

/// A text handed to `save` with an edit number it is not the text of is not
/// written and not reported saved: the answer is `Held` with the reason, and
/// the right pair saves as usual.
#[test]
fn a_text_that_is_not_the_text_at_its_edit_number_is_refused_before_it_is_written() {
    let rig = Rig::new("review-wrong-edit");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "one ");
    let second = type_end(&mut store, id, &mut doc, "two ");
    let stale = doc.to_text("\n");
    let latest = type_end(&mut store, id, &mut doc, "three");
    store.save(id, stale.clone(), latest);
    assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. } | News::Saved { .. })), News::Held { doc: id, why: Hold::WrongEdit });
    assert_eq!(read(&rig.folder.join("essay.md")), "one ", "nothing was written");
    // The same older text with its own number is a true pair, and is saved.
    store.save(id, stale.clone(), second);
    let answer = expect(&mut store, "Saved", |news| matches!(news, News::Held { .. } | News::Saved { .. }));
    assert!(matches!(answer, News::Saved { doc, edit, bytes: 8, .. } if doc == id && edit == second), "{answer:?}");
    assert_eq!(read(&rig.folder.join("essay.md")), stale);
    save_wait(&mut store, id, &doc, latest);
    assert_eq!(read(&rig.folder.join("essay.md")), "one two three");
    store.close(id, None, None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

/// Lengths handed to `record` that do not add up to the journal's own count
/// raise the alarm, and the next save puts it right: either the count was
/// right all along, or the journal starts over from the text it is handed.
/// After that a kill brings back exactly the buffer.
#[test]
fn lengths_that_do_not_add_up_raise_the_alarm_until_a_save_puts_it_right() {
    let rig = Rig::new("review-doubt");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");

    // Only the numbers are wrong: every length is five too many.
    let mut edit = 0;
    for piece in [" a", " b", " c"] {
        let change = doc.replace(Span::at(doc.end()), piece);
        edit = store.record(id, &change, doc.len_bytes() + 5);
    }
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    saved_and_calm(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), "Start. a b c");

    // The buffer itself has parted from what was recorded: one edit never
    // reached the store, so every length after it is one more than the
    // journal counts.
    doc.replace(Span::at(doc.end()), "!");
    let edit = type_end(&mut store, id, &mut doc, " d");
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    saved_and_calm(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), "Start. a b c! d");

    // From there the journal rebuilds the buffer again.
    type_end(&mut store, id, &mut doc, " and on");
    journaled(&mut store);
    store.abandon();
    let (store, report) = rig.open();
    assert_eq!(report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::BroughtBack]);
    assert_eq!(read(&rig.folder.join("essay.md")), "Start. a b c! d and on");
    assert!(store.shutdown(SOON));
}

// ------------------------------------------------------ line breaks and names

/// Mending a text that is not open, and recovery after a kill, both leave
/// the line breaks they do not touch as the file has them.
#[test]
fn mending_and_recovery_keep_the_line_breaks_they_do_not_touch() {
    let rig = Rig::new("review-mixed");
    let plan = rig.folder.join("plan.md");
    fs::write(&plan, b"See [[essay]].\r\nline two\nline three\r\n").unwrap();
    let essay = rig.folder.join("essay.md");
    fs::write(&essay, b"one\r\ntwo\rthree\nfour").unwrap();
    let (mut store, _) = rig.open();
    store.rewrite("plan.md", "See [[essay]].\nline two\nline three\n", "See [[The orchard]].\nline two\nline three\n").unwrap();
    assert_eq!(fs::read(&plan).unwrap(), b"See [[The orchard]].\r\nline two\nline three\r\n");

    let opened = store.open_text("essay.md").unwrap();
    assert!(opened.mixed_endings);
    let mut doc = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc, " five");
    journaled(&mut store);
    store.abandon();
    assert_eq!(fs::read(&essay).unwrap(), b"one\r\ntwo\rthree\nfour");
    let (store, report) = rig.open();
    assert_eq!(report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::BroughtBack]);
    assert_eq!(fs::read(&essay).unwrap(), b"one\r\ntwo\rthree\nfour five");
    assert!(store.shutdown(SOON));
}

/// A rename in Explorer that changes only the capitals of an open text's
/// name is told to the interface like any other rename.
#[test]
fn capitals_changed_in_explorer_are_told_as_a_move() {
    let rig = Rig::new("review-capitals");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Text.");
    listen(&mut store, Duration::from_millis(100));
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Essay.md")).unwrap();
    assert_eq!(expect(&mut store, "Moved", |news| matches!(news, News::Moved { .. })), News::Moved { doc: id, rel: PathBuf::from("Essay.md") });
    let edit = type_end(&mut store, id, &mut doc, " More.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(rig.listing(), ["Essay.md"]);
    // After a kill the text comes back under the name it has now.
    type_end(&mut store, id, &mut doc, " Unsaved.");
    journaled(&mut store);
    store.abandon();
    let (store, report) = rig.open();
    assert_eq!(report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::BroughtBack]);
    assert_eq!(rig.listing(), ["Essay.md"]);
    assert_eq!(read(&rig.folder.join("Essay.md")), "Text. More. Unsaved.");
    assert!(store.shutdown(SOON));
}

/// A text reached through a link is saved where it really is, and the link
/// stays a link. A link to a file cannot be made on this machine without a
/// privilege, so this is run with a linked folder (a junction), which needs
/// none and takes the same road through the store: the file's real path, as
/// its open handle gives it, is where the new file is put.
#[test]
fn a_text_reached_through_a_linked_folder_is_saved_where_it_really_is() {
    let rig = Rig::new("review-junction");
    let elsewhere = rig.data.with_file_name("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("essay.md"), "Kept somewhere else.").unwrap();
    let link = rig.folder.join("Linked");
    let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&elsewhere).output();
    if !made.is_ok_and(|out| out.status.success()) {
        println!("NOT RUN: a linked folder could not be made here");
        return;
    }
    let (mut store, _) = rig.open();
    let opened = store.open_text("Linked\\essay.md").unwrap();
    assert_eq!((opened.text.as_str(), opened.read_only), ("Kept somewhere else.", None));
    let mut doc = Doc::from_text(&opened.text);
    for piece in [" And typed here.", " Twice."] {
        let edit = type_end(&mut store, opened.doc, &mut doc, piece);
        save_wait(&mut store, opened.doc, &doc, edit);
        assert_eq!(read(&elsewhere.join("essay.md")), doc.to_text("\n"));
    }
    assert!(fs::symlink_metadata(&link).unwrap().file_attributes() & 0x400 != 0, "the link is no longer a link");
    let names: Vec<String> = fs::read_dir(&elsewhere).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["essay.md"], "no temp file left where the file really is");
    store.close(opened.doc, None, None);
    assert_eq!(closed(&mut store, opened.doc), News::Closed { doc: opened.doc, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    fs::remove_dir(&link).expect("the link can be removed");
    assert!(elsewhere.join("essay.md").exists(), "removing the link leaves what it led to");
}
