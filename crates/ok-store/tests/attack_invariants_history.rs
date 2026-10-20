//! What the store says and does when a version cannot be written to the
//! history (the data directory's disk is full, or the folder cannot be
//! written). The tests stand that in by putting a plain file where the
//! text's history folder was, so every attempt to keep a version fails.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ok_store::testing::{save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, News, Resolve, Store};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

/// Replaces the one history folder there is with a file of the same name.
/// The first version is written just after `Saved` is sent, so the folder
/// is waited for, and then left alone for a moment so that write is over.
fn block_history(rig: &Rig) {
    let history = rig.data.join("history");
    let list = || -> Vec<PathBuf> { fs::read_dir(&history).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect() };
    let end = std::time::Instant::now() + SOON;
    while list().is_empty() {
        assert!(std::time::Instant::now() < end, "no history folder was ever made");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(150));
    let folders = list();
    assert_eq!(folders.len(), 1, "{folders:?}");
    fs::remove_dir_all(&folders[0]).unwrap();
    fs::write(&folders[0], b"").unwrap();
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

/// `Store::keep`: "saves ... and also keeps the text as a labelled version",
/// answered with `News::Kept`, which the interface prints as
/// `Kept 14:02. 2,481 bytes on disk.`
///
/// `Kept` is sent whether or not the version was written.
#[test]
fn kept_is_reported_only_when_a_version_was_written() {
    let rig = Rig::new("inv-kept-nothing");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "First save.");
    block_history(&rig);

    let edit = type_end(&mut store, id, &mut doc, " And the part he presses Ctrl S for.");
    store.keep(id, doc.to_text("\n"), edit, "kept");
    let mut seen = Vec::new();
    let kept = wait_news(&mut store, &mut seen, Duration::from_secs(3), |news| matches!(news, News::Kept { .. }));
    let versions = store.versions(id).unwrap();
    println!("keep() was answered with {:?}; the versions that exist: {versions:?}", seen.iter().filter(|news| !matches!(news, News::Tree(_))).collect::<Vec<_>>());
    assert!(store.shutdown(SOON));
    let there = versions.iter().any(|version| version.label.as_deref() == Some("kept"));
    assert!(kept.is_none() || there, "Kept was reported and no version was written");
}

/// Invariant 4: "If they differ, we stop and keep both", and the design's
/// "keep mine (save over; theirs is in history)".
///
/// `Resolve::KeepMine` tries to put their version in the history and saves
/// over it whether or not that worked. (`Store::rewrite` does this the right
/// way round: no snapshot, no write.)
#[test]
fn keeping_mine_does_not_destroy_their_version_when_it_cannot_be_kept() {
    let rig = Rig::new("inv-keep-mine");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
    let path = rig.folder.join("essay.md");
    block_history(&rig);

    let edit = type_end(&mut store, id, &mut doc, " Mine, typed and not yet saved.");
    fs::write(&path, "THEIR-VERSION, written by another program while he typed.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    let changed = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. }));
    assert!(matches!(changed, News::ChangedOutside { clean: false, .. }), "{changed:?}");

    store.resolve(id, Resolve::KeepMine);
    let mut seen = Vec::new();
    let answer = wait_news(&mut store, &mut seen, Duration::from_secs(3), |news| matches!(news, News::Saved { .. } | News::Held { .. }));
    println!("KeepMine with no history to keep theirs in was answered with {answer:?}");
    assert!(store.shutdown(SOON));
    let on_disk = fs::read_to_string(&path).unwrap();
    let theirs = places_holding(&rig, "THEIR-VERSION");
    println!("the file reads {on_disk:?}; their version is in {theirs:?}");
    assert!(!theirs.is_empty(), "their version was saved over and is in no file: not in the folder, not in the history");
}
