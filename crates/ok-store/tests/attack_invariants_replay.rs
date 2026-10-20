//! Three ways for text that reached a journal file to be missing after the
//! next start: a journal that was busy at the moment the text was opened
//! again, a buffer whose first character the replay drops, and a length the
//! journal accepts although no replay can ever pass it.

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, News, Pos, Store};
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

fn journaled(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!store.unwritten(), "the journal thread never caught up");
}

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

fn places_holding(rig: &Rig, needle: &str) -> Vec<PathBuf> {
    let mut all = Vec::new();
    files_under(&rig.folder, &mut all);
    files_under(&rig.data, &mut all);
    all.into_iter().filter(|path| fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).contains(needle)).unwrap_or(false)).collect()
}

/// The builder's promise for a text closed while its file could not take it:
/// "Such a journal is brought back if the text is reopened in the same
/// session." `News::Closed { in_journal: true }` is what the interface is told.
///
/// If the journal file cannot be opened alone at the moment of reopening
/// (anything reading it will do: a backup, a scanner), the store forgets the
/// journal, shows the file as it stands, and starts a second journal for the
/// same text. After a kill the higher generation wins and the first is
/// removed unread.
#[test]
fn text_left_in_the_journal_at_close_comes_back_at_reopen_even_if_the_journal_is_being_read() {
    let rig = Rig::new("inv-reopen-busy");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
    let path = rig.folder.join("essay.md");

    // Another program keeps the file from being replaced while he switches texts.
    let holder = OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " LEFT-IN-JOURNAL words the file could not take.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    println!("closing was answered with {closed:?}");
    assert!(matches!(closed, News::Closed { in_journal: true, .. }));
    drop(holder);
    let journals = rig.journals();
    assert_eq!(journals.len(), 1, "{journals:?}");

    // He comes back to the text while something is reading its journal.
    let reader = File::open(rig.data.join("journal").join(&journals[0])).expect("the journal can be opened for reading");
    let opened = store.open_text("essay.md").expect("the text opens");
    drop(reader);
    println!("reopened as {:?}, restored {:?}", opened.text, opened.restored);
    let shown = opened.text.clone();

    // He types on in what he was shown, and the process is killed.
    let mut again = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut again, " Typed after coming back.");
    journaled(&mut store);
    println!("journals now: {:?}", rig.journals());
    store.abandon();
    let (store, recovered) = rig.open();
    println!("the next start reported {:?}", recovered.iter().map(|r| (&r.how, r.words)).collect::<Vec<_>>());
    assert!(store.shutdown(SOON));
    let held = places_holding(&rig, "LEFT-IN-JOURNAL");
    println!("the file reads {:?}; the words that were left in the journal are in {held:?}", fs::read_to_string(&path).unwrap_or_default());
    assert!(!held.is_empty(), "Closed said the text waits in the journal; after a reopen and a kill it is in no file at all");
    assert!(shown.contains("LEFT-IN-JOURNAL"), "Closed said the text waits in the journal; reopening it in the same session showed {shown:?}");
}

/// Promise: "An edit that `record` has returned for is in the journal file
/// within milliseconds, and survives the process being killed."
///
/// The replay rebuilds the text with `Doc::from_text`, which drops a leading
/// U+FEFF. The buffer in the interface is never re-read and keeps it. So
/// once a journal has been started afresh from a saved text that begins with
/// that character (a paste from a program that puts one on the clipboard is
/// enough), every later edit fails the replay's position and length checks.
/// Recovery then takes the text as it was at the rotation for the newest,
/// and writes it over the file.
#[test]
fn edits_typed_after_a_rotation_survive_a_kill_when_the_text_begins_with_a_byte_order_mark() {
    let rig = Rig::new("inv-feff");
    let mut tuning = quick();
    tuning.rotate_at = 1;
    let (mut store, _) = rig.open_with(tuning.clone());
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    // Pasted at the very top, the way the clipboard handed it over.
    let paste = doc.replace(Span::at(Pos::default()), "\u{FEFF}Pasted from somewhere else.");
    store.record(opened.doc, &paste, doc.len_bytes());
    let mut edit = 0;
    for letter in " and then typed on, letter by letter, for a while longer.".chars() {
        edit = type_end(&mut store, opened.doc, &mut doc, &letter.to_string());
    }
    save_wait(&mut store, opened.doc, &doc, edit);
    let path = rig.folder.join("essay.md");
    assert_eq!(fs::read(&path).unwrap(), doc.to_text("\n").into_bytes());

    // The journal is long for so short a text: the save starts it afresh.
    let end = Instant::now() + SOON;
    while !rig.journals().iter().any(|name| name.ends_with(".2.wal")) {
        assert!(Instant::now() < end, "the journal was never rotated: {:?}", rig.journals());
        std::thread::sleep(Duration::from_millis(5));
    }
    for word in [" TAIL-AFTER-ROTATION", " words", " typed", " and", " journaled."] {
        type_end(&mut store, opened.doc, &mut doc, word);
    }
    journaled(&mut store);
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    let (store, recovered) = rig.open_with(tuning);
    println!("the next start reported {:?}", recovered.iter().map(|r| (&r.how, r.words)).collect::<Vec<_>>());
    assert!(store.shutdown(SOON));
    let on_disk = fs::read(&path).unwrap();
    println!("the file reads {:?}", String::from_utf8_lossy(&on_disk));
    let held = places_holding(&rig, "TAIL-AFTER-ROTATION");
    assert!(!held.is_empty(), "five edits that were written and flushed to the journal are in no file after the kill");
    assert_eq!(on_disk, doc.to_text("\n").into_bytes());
}

/// Not a promise of the specification: a slip in the interface that the
/// store is placed to catch and does not.
///
/// `record` takes the document's length after the edit and writes it into
/// the journal unseen. The replay checks it, and ends at the first edit
/// whose length does not fit: everything typed after it is dropped at the
/// next start, however long ago the slip was. The journal thread has the
/// opening text and every edit, so it can keep the running length itself and
/// raise the alarm the moment a number does not fit.
#[test]
fn a_wrong_length_handed_to_record_does_not_end_recovery_in_silence() {
    let rig = Rig::new("inv-wrong-length");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    type_end(&mut store, opened.doc, &mut doc, "one ");
    // The slip: the length in characters, not in bytes, for one edit.
    let change = doc.replace(Span::at(doc.end()), "caf\u{00E9} ");
    let in_characters = doc.to_text("\n").chars().count();
    assert_ne!(in_characters, doc.len_bytes());
    store.record(opened.doc, &change, in_characters);
    type_end(&mut store, opened.doc, &mut doc, "two ");
    type_end(&mut store, opened.doc, &mut doc, "THREE-AFTER-THE-SLIP");
    journaled(&mut store);
    let during = listen(&mut store, Duration::from_millis(500));
    let warned = during.iter().any(|news| matches!(news, News::JournalAlarm(true)));
    println!("news while typing after the slip: {during:?}; unwritten() {}", store.unwritten());
    store.abandon();

    let (store, recovered) = rig.open();
    println!("the next start reported {:?}", recovered.iter().map(|r| (&r.how, r.words)).collect::<Vec<_>>());
    assert!(store.shutdown(SOON));
    let text = fs::read_to_string(rig.folder.join("essay.md")).unwrap_or_default();
    println!("the file reads {text:?}");
    assert!(warned || text.contains("THREE-AFTER-THE-SLIP"), "the journal took an edit no replay can pass, said nothing, and the next start came back with {text:?}");
}
