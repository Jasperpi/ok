//! Paths the store's own tests do not walk: a text that is a symbolic link,
//! a text further down than 260 characters, and a text whose sub-folder is
//! deleted under it while it is open.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ok_store::testing::{save_wait, type_end, wait_news, Rig};
use ok_store::{News, Recovery, Store};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

fn journaled(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!store.unwritten(), "the journal thread never caught up");
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

/// The base design, section 1: "Document paths are canonicalized at open so
/// a symlink is saved through, not replaced", and its test list: "a
/// symlinked file (the link must survive a save)".
///
/// The store reads through the link and then renames its temp file onto the
/// link's own name. The link is gone, a plain file stands in its place, and
/// the file the link pointed at keeps the old text for good.
#[test]
fn a_text_that_is_a_symbolic_link_is_saved_through_the_link() {
    let rig = Rig::new("inv-symlink");
    let elsewhere = rig.folder.parent().expect("the scratch folder").join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let real = elsewhere.join("the real essay.md");
    fs::write(&real, "The real file.").unwrap();
    let link = rig.folder.join("essay.md");
    if let Err(e) = std::os::windows::fs::symlink_file(&real, &link) {
        // Making a link needs a privilege or Developer Mode; without it there is nothing to test.
        println!("NOT TESTED: a symbolic link cannot be made here ({e})");
        return;
    }
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").expect("the linked text opens");
    println!("opened through the link as {:?}, read_only {:?}", opened.text, opened.read_only);
    assert_eq!(opened.text, "The real file.");
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " And a line typed in ok.");
    store.save(opened.doc, doc.to_text("\n"), edit);
    let answer = expect(&mut store, "Saved or Held", |news| matches!(news, News::Saved { .. } | News::Held { .. }));
    println!("the save was answered with {answer:?}");
    assert!(store.shutdown(SOON));

    let still_a_link = fs::symlink_metadata(&link).map(|meta| meta.file_type().is_symlink()).unwrap_or(false);
    let behind_the_link = fs::read_to_string(&real).unwrap();
    let under_the_name = fs::read_to_string(&link).unwrap_or_default();
    println!("after the save: still a link {still_a_link}; the real file reads {behind_the_link:?}; the name in the folder reads {under_the_name:?}");
    if matches!(answer, News::Saved { .. }) {
        assert!(still_a_link, "the save replaced the symbolic link with a plain file");
        assert_eq!(behind_the_link, doc.to_text("\n"), "the file the link points at did not get the text");
    } else {
        // Held is an honest answer too, as long as nothing was replaced.
        assert!(still_a_link);
        assert_eq!(behind_the_link, "The real file.");
    }
}

/// The builder's list of what was not tested: "paths longer than 260
/// characters through the whole store". Create, save, keep a version, kill,
/// recover, rename and discard a text 400 characters down.
#[test]
fn a_text_further_down_than_260_characters_goes_through_every_operation() {
    let rig = Rig::new("inv-long-path");
    let part = "A folder whose name is long enough to matter once three of them are nested inside one another";
    let rel: PathBuf = [part, part, part, "The essay at the bottom.md"].iter().collect();
    let moved: PathBuf = [part, part, part, "The essay, renamed.md"].iter().collect();
    let full = rig.folder.join(&rel);
    assert!(full.as_os_str().len() > 300, "the path is only {} long", full.as_os_str().len());

    let (mut store, _) = rig.open();
    let opened = store.create_text(&rel).expect("a text can be started far down");
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Saved once.");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(fs::read_to_string(&full).unwrap(), "Saved once.");
    store.keep(opened.doc, doc.to_text("\n"), edit, "kept");
    let kept = expect(&mut store, "Kept", |news| matches!(news, News::Kept { .. }));
    assert!(matches!(kept, News::Kept { bytes: 11, .. }), "{kept:?}");
    assert!(!store.versions(opened.doc).unwrap().is_empty());

    type_end(&mut store, opened.doc, &mut doc, " LONG-PATH words typed after the last save.");
    journaled(&mut store);
    store.abandon();

    let (mut store, recovered) = rig.open();
    println!("recovery far down reported {:?}", recovered.iter().map(|r| (&r.how, r.words)).collect::<Vec<_>>());
    assert_eq!(fs::read_to_string(&full).unwrap(), doc.to_text("\n"), "the journal's text did not reach the file");
    assert!(recovered.iter().any(|r| r.how == Recovery::BroughtBack && r.words == 7), "{recovered:?}");

    let opened = store.open_text(&rel).expect("the text opens again");
    assert_eq!(opened.text, doc.to_text("\n"));
    store.rename(&rel, &moved).expect("it can be renamed");
    assert!(rig.folder.join(&moved).exists() && !full.exists());
    let edit = type_end(&mut store, opened.doc, &mut doc, " And after the rename.");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(fs::read_to_string(rig.folder.join(&moved)).unwrap(), doc.to_text("\n"));
    let gone = store.discard(&moved, None).expect("it can be discarded");
    println!("discarded to {}", gone.display());
    assert_eq!(fs::read_to_string(rig.folder.join(&gone)).unwrap(), doc.to_text("\n"));
    assert!(!rig.folder.join(&moved).exists());
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty(), "{:?}", rig.journals());
}

/// The builder's list of what was not tested: "a text in a sub-folder that
/// is deleted while the text is open". Nothing typed may be lost, and the
/// interface must be told the saves are not arriving.
#[test]
fn a_text_whose_sub_folder_is_deleted_under_it_loses_nothing_and_says_so() {
    let rig = Rig::new("inv-subfolder-gone");
    let (mut store, _) = rig.open();
    let opened = store.create_text(r"College\essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Saved in College.");
    save_wait(&mut store, opened.doc, &doc, edit);

    let edit = type_end(&mut store, opened.doc, &mut doc, " SUBFOLDER-GONE words typed before the folder went.");
    journaled(&mut store);
    fs::remove_dir_all(rig.folder.join("College")).expect("another program can delete the sub-folder");
    store.save(opened.doc, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let told = wait_news(&mut store, &mut seen, Duration::from_secs(4), |news| matches!(news, News::Held { .. } | News::Removed { .. }));
    let told_all: Vec<&News> = seen.iter().filter(|news| !matches!(news, News::Tree(_))).collect();
    println!("after the sub-folder was deleted: {told_all:?}");

    let edit = type_end(&mut store, opened.doc, &mut doc, " And more after.");
    store.close(opened.doc, Some((doc.to_text("\n"), edit)), None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    println!("closing was answered with {closed:?}; the folder holds {:?}", rig.listing());
    assert!(store.shutdown(SOON));
    let (store, recovered) = rig.open();
    println!("the next start reported {:?}; the folder holds {:?}", recovered.iter().map(|r| (&r.rel, &r.how, r.words)).collect::<Vec<_>>(), rig.listing());
    assert!(store.shutdown(SOON));

    let held = places_holding(&rig, "SUBFOLDER-GONE");
    let in_folder: Vec<&PathBuf> = held.iter().filter(|path| path.starts_with(&rig.folder)).collect();
    println!("the words are in {held:?}");
    assert!(told.is_some(), "four seconds after the folder went the interface had been told nothing");
    assert!(!in_folder.is_empty(), "what he typed is not in any file of the writing folder");
    assert!(in_folder.iter().any(|path| fs::read_to_string(path).unwrap_or_default() == doc.to_text("\n")), "no file holds the whole last text");
}
