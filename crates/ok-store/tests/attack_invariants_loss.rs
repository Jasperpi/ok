//! Sequences in which text that `record` returned for ends up in no file at
//! all, although the store reported it safe. Each test states the promise it
//! holds the store to and is expected to fail until the store keeps it.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig, Scratch};
use ok_store::{DocId, News, Setup, Store};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

/// Waits for news matching `want`; panics with everything that did arrive.
fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

/// Collects news for `time` and returns all of it.
fn listen(store: &mut Store, time: Duration) -> Vec<News> {
    let mut seen = Vec::new();
    wait_news(store, &mut seen, time, |_| false);
    seen
}

/// Waits until the journal thread has written every edit handed to it.
fn journaled(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!store.unwritten(), "the journal thread never caught up");
}

/// A new text holding `text`, saved. Returns its handle, the buffer and the edit number.
fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every file of the writing folder and of the data directory (history,
/// recovered copies, journals set aside) that holds `needle`.
fn places_holding(rig: &Rig, needle: &str) -> Vec<PathBuf> {
    let mut all = Vec::new();
    files_under(&rig.folder, &mut all);
    files_under(&rig.data, &mut all);
    all.into_iter().filter(|path| fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).contains(needle)).unwrap_or(false)).collect()
}

/// Promise: an edit `record` returned for survives the process being killed.
///
/// Recovery skips a text's journal when it cannot open it alone, taking that
/// to mean another instance owns the text. Anything else that has the file
/// open at that instant (a backup tool, a scanner, a second ok started on
/// another folder and looking through the same journal directory) has the
/// same effect. The session then knows nothing of that journal: opening the
/// text shows the file as it is, and the first keystroke starts a second
/// journal for the same text beside the first. After a second kill "the
/// highest generation wins" and the other one is removed unread.
#[test]
fn a_journal_that_recovery_could_not_take_is_not_thrown_away_after_a_second_kill() {
    let rig = Rig::new("inv-skipped-journal");
    let path = rig.folder.join("essay.md");

    // First session: one save, then typing that is in the journal only, then a kill.
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
    type_end(&mut store, id, &mut doc, " FIRST-SESSION words typed after the last save.");
    journaled(&mut store);
    store.abandon();
    let journals = rig.journals();
    assert_eq!(journals.len(), 1, "{journals:?}");

    // Second session starts while another program has that journal open.
    let scanner = File::open(rig.data.join("journal").join(&journals[0])).expect("the journal can be read by a scanner");
    let (mut store, recovered) = rig.open();
    println!("second start reported {recovered:?}");
    let opened = store.open_text("essay.md").expect("the text opens");
    println!("second session opened the text as {:?}, restored {:?}", opened.text, opened.restored);
    drop(scanner);
    let mut doc = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc, " SECOND-SESSION words typed after the last save.");
    journaled(&mut store);
    println!("journals during the second session: {:?}", rig.journals());
    store.abandon();

    // Third session: nothing holds anything.
    let (store, recovered) = rig.open();
    println!("third start reported {recovered:?}");
    assert!(store.shutdown(SOON));
    println!("the file reads {:?}; the folder holds {:?}; journals left {:?}", fs::read_to_string(&path).unwrap_or_default(), rig.listing(), rig.journals());
    let first = places_holding(&rig, "FIRST-SESSION");
    let second = places_holding(&rig, "SECOND-SESSION");
    assert!(!second.is_empty(), "the second session's unsaved words are in no file");
    assert!(!first.is_empty(), "the first session's unsaved words are in no file: not in the text, not beside it, not in the history, and their journal is gone");
}

/// Promise (the text of `News::ChangedOutside`): when both versions exist,
/// "take theirs the same way after keeping yours with `Store::keep`". And the
/// design's sentence for it: "Showing the newer one; yours is in earlier."
///
/// While the text is held for the outside change `keep` does nothing and says
/// nothing; its label then rides on the next save, which is the save of
/// their text. His own version is never written anywhere.
#[test]
fn keeping_mine_and_then_taking_theirs_leaves_my_version_among_the_earlier_ones() {
    let rig = Rig::new("inv-take-theirs");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
    let path = rig.folder.join("essay.md");

    let mine_edit = type_end(&mut store, id, &mut doc, " MY-PARAGRAPH, typed and not yet saved.");
    let mine = doc.to_text("\n");
    fs::write(&path, "Their version, written by another program.").unwrap();
    store.save(id, mine.clone(), mine_edit);
    let News::ChangedOutside { text: theirs, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert!(!clean);

    // Keep mine, as the news text says to.
    store.keep(id, mine.clone(), mine_edit, "mine");
    let answer = listen(&mut store, Duration::from_millis(400));
    println!("keep() while held for an outside change was answered with {answer:?}");

    // Then take theirs: an ordinary edit, recorded and saved.
    let edit = replace_all(&mut store, id, &mut doc, &theirs);
    let after = save_wait(&mut store, id, &doc, edit);
    println!("taking theirs was answered with {after:?}");

    let versions = store.versions(id).unwrap();
    let kept: Vec<(Option<String>, String)> = versions.iter().map(|v| (v.label.clone(), store.read_version(id, &v.name).unwrap())).collect();
    println!("versions now: {kept:?}");
    store.close(id, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    assert!(store.shutdown(SOON));
    assert!(kept.iter().any(|(_, text)| text == &mine), "his own version is not among the earlier ones: {kept:?}");
    assert!(!places_holding(&rig, "MY-PARAGRAPH").is_empty(), "his version is in no file at all");
}

/// Promise: `News::Closed { in_journal: true }` means the text "waits in the
/// journal", and opening the text again in the same session brings it back
/// (the store's own test `text_left_in_the_journal_at_close_...` shows that
/// for a healthy journal).
///
/// When the journal cannot be written at that moment (the alarm is up) and
/// the file cannot take the text either (one full disk does both, since by
/// default the journal and the writing folder are on C:), closing the text
/// drops the only copy, which was in the journal thread's memory, and still
/// reports `in_journal: true`. The disks coming back does not bring it back.
#[test]
fn text_closed_while_neither_the_journal_nor_the_file_could_be_written_is_not_dropped() {
    let rig = Rig::new("inv-double-fault");
    let tuning = quick();
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");

    // The disk fills: journal writes fail and so does the save's temp file.
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    tuning.faults.temp_write.store(112, Ordering::Relaxed);
    let edit = type_end(&mut store, id, &mut doc, " DISK-FULL words, typed while nothing could be written.");
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));

    // He switches to another text: the interface closes this one with its last text.
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    println!("closing was answered with {closed:?}");
    let News::Closed { in_journal, .. } = closed else { unreachable!() };
    // The disk stays full a little longer than the close takes.
    let while_full = listen(&mut store, Duration::from_millis(400));
    println!("with the disk still full, after the close: {while_full:?}");

    // Space is freed. The journal thread retries once a second.
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    tuning.faults.temp_write.store(0, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1500));
    let seen = listen(&mut store, Duration::from_millis(100));
    println!("after the disk came back: {seen:?}");

    let opened = store.open_text("essay.md").expect("the text opens again");
    println!("opened again as {:?}, restored {:?}", opened.text, opened.restored);
    let finished = store.shutdown(SOON);
    println!("shutdown returned {finished}; journals {:?}", rig.journals());
    assert!(in_journal, "the text was not saved, so it must be reported as waiting");
    assert!(opened.text.contains("DISK-FULL"), "Closed said in_journal, but opening the text again gave {:?}", opened.text);
}

/// The same double failure at exit: the store answers that the text is in
/// the journal and that shutdown finished, and the text is nowhere.
#[test]
fn shutdown_does_not_report_success_when_the_last_text_is_in_no_file() {
    let rig = Rig::new("inv-double-fault-exit");
    let tuning = quick();
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    tuning.faults.temp_write.store(112, Ordering::Relaxed);
    let edit = type_end(&mut store, id, &mut doc, " DISK-FULL words, typed while nothing could be written.");
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    let finished = store.shutdown(SOON);
    println!("closing was answered with {closed:?}; shutdown returned {finished}");

    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    tuning.faults.temp_write.store(0, Ordering::Relaxed);
    let (store, recovered) = rig.open_with(tuning);
    println!("the next start reported {recovered:?}");
    assert!(store.shutdown(SOON));
    let held = places_holding(&rig, "DISK-FULL");
    let claimed_safe = finished && matches!(closed, News::Closed { in_journal: true, .. });
    assert!(!(held.is_empty() && claimed_safe), "Closed said in_journal and shutdown returned true, yet the text is in no file after the next start");
}

/// Not a promise of the specification: a way the interface can lose text by
/// a small slip that the store has what it needs to catch.
///
/// `save` takes a text and the edit number it corresponds to and believes
/// the pairing. If the interface hands over a text captured two edits ago
/// with the newest edit number (a snapshot taken on a timer, the number read
/// at send time), the store reports that number saved, the journal counts
/// every edit as covered, and a clean close removes it. `record` was given
/// the length after every edit, so a text of the wrong length for its number
/// could be refused, or at least never counted as covering the journal.
#[test]
fn a_save_whose_text_is_older_than_its_edit_number_does_not_let_the_journal_go() {
    let rig = Rig::new("inv-mismatch");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    type_end(&mut store, opened.doc, &mut doc, "one ");
    type_end(&mut store, opened.doc, &mut doc, "two ");
    let stale = doc.to_text("\n");
    let latest = type_end(&mut store, opened.doc, &mut doc, "THREE");
    journaled(&mut store);
    store.save(opened.doc, stale, latest);
    let answer = expect(&mut store, "an answer to the save", |news| matches!(news, News::Saved { .. } | News::Held { .. }));
    println!("the mismatched save was answered with {answer:?}");
    store.close(opened.doc, None, None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    println!("closing was answered with {closed:?}");
    assert!(store.shutdown(SOON));
    let (store, recovered) = rig.open();
    println!("the next start reported {recovered:?}; the file reads {:?}", fs::read_to_string(rig.folder.join("essay.md")).unwrap_or_default());
    assert!(store.shutdown(SOON));
    assert!(!places_holding(&rig, "THREE").is_empty(), "the last edit was reported saved, its journal was removed, and it is in no file");
}

/// The same skip without any outside program: two instances started at the
/// same moment on two different writing folders share one journal directory,
/// and each looks through all of it. While the one that does not own the
/// folder has the journal open (alone, to read where it belongs), the one
/// that does own it is refused, takes that for another instance's text, and
/// starts without recovering it.
#[test]
fn an_instance_on_another_folder_never_keeps_the_owner_from_recovering_its_journal() {
    const ROUNDS: usize = 40;
    let mut skipped = Vec::new();
    let mut by_the_other = Vec::new();
    for round in 0..ROUNDS {
        let scratch = Scratch::new("inv-two-folders");
        let (here, there, data) = (scratch.path().join("here"), scratch.path().join("there"), scratch.path().join("data"));
        let setup = |folder: &Path| {
            let mut setup = Setup::new(folder.to_path_buf(), data.clone());
            setup.tuning = quick();
            setup
        };
        let (mut store, _) = Store::open(setup(&here)).unwrap();
        let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
        type_end(&mut store, id, &mut doc, " Typed after the last save.");
        journaled(&mut store);
        store.abandon();

        let barrier = Arc::new(Barrier::new(2));
        let other = {
            let (barrier, setup) = (barrier.clone(), setup(&there));
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(setup).map(|(store, recovered)| (store.shutdown(SOON), recovered.len())).ok()
            })
        };
        barrier.wait();
        let (store, recovered) = Store::open(setup(&here)).unwrap();
        let other = other.join().unwrap().expect("the other instance opens its own folder");
        let text = fs::read_to_string(here.join("essay.md")).unwrap();
        if text != doc.to_text("
") {
            // Nobody brought the file forward: the journal is still lying there.
            skipped.push(round);
        } else if recovered.is_empty() {
            assert_eq!(other.1, 1, "round {round}");
            by_the_other.push(round);
        }
        assert!(store.shutdown(SOON));
    }
    println!("of {ROUNDS} starts: the owner skipped its journal and nobody recovered it in {skipped:?}; the other instance recovered it (so the owner has nothing to say) in {by_the_other:?}");
    assert!(skipped.is_empty(), "in {} of {ROUNDS} starts the folder's owner skipped its own journal because another instance was reading it", skipped.len());
}
