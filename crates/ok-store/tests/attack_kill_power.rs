//! Power cuts in which the two disks lose different amounts. The journal is
//! on one volume and the writing folder may be on another; each comes back
//! holding everything that was flushed to it and some part, or none, of what
//! was not. This test builds every such pair of states for one session that
//! saved, renamed its text into a sub-folder while it had unsaved words,
//! saved again and typed on:
//!
//! * the journal cut at each record boundary from the first finished save on
//!   (and one byte past it, which leaves a torn record behind);
//! * the folder with the text under its old name or its new one, holding the
//!   first save or the second.
//!
//! Only pairs a power cut can really produce are tried. The rules: the
//! journal is flushed before the folder is touched by a save or a rename, so
//! the folder never shows something the journal has no record of beginning;
//! a rename is not flushed by itself, so the folder may still show the old
//! name until a later flush on that volume, which the second save's is.
//!
//! Whatever the pair, afterwards the text must be in exactly one file, under
//! the name it had or the name it was given, holding every edit the cut
//! journal still has, with nothing beside it.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use ok_store::testing::{save_wait, type_end, Rig};
use ok_store::Store;
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);
const HEADER: usize = 24;
const KIND_EDIT: u8 = 2;
const KIND_BEGIN: u8 = 3;
const KIND_DONE: u8 = 4;
const KIND_PATH: u8 = 5;

fn settle(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up");
        while store.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Every file under `dir`, as (path inside it, bytes), folders walked.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push((path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned(), fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn empty_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let gone = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        gone.unwrap_or_else(|e| panic!("{} could not be cleared: {e}", path.display()));
    }
}

/// One record of a journal file as found by walking its length fields: its
/// kind and the offset just past it.
fn records(bytes: &[u8]) -> Vec<(u8, usize)> {
    let mut out = Vec::new();
    let mut at = HEADER;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if len < 9 || at + 8 + len > bytes.len() {
            break;
        }
        at += 8 + len;
        out.push((bytes[at - len], at));
    }
    out
}

#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn a_power_cut_that_costs_the_journal_and_the_folder_different_amounts_around_a_rename() {
    let rig = Rig::new("power-rename");
    let (mut store, _) = rig.open();
    let opened = store.create_text("a.md").unwrap();
    let mut doc = Doc::new();
    // states[n] is the text after n edits.
    let mut states = vec![String::new()];
    let mut typed = |store: &mut Store, doc: &mut Doc, words: &str| {
        let edit = type_end(store, opened.doc, doc, words);
        states.push(doc.to_text("\n"));
        edit
    };
    typed(&mut store, &mut doc, "One.");
    typed(&mut store, &mut doc, " Two.");
    let edit = typed(&mut store, &mut doc, " Three.\n");
    save_wait(&mut store, opened.doc, &doc, edit);
    typed(&mut store, &mut doc, "Four, unsaved when the text was renamed.");
    typed(&mut store, &mut doc, " Five.\n");
    settle(&mut store);
    store.rename("a.md", "Sub\\b.md").expect("the text can be renamed while it is open");
    let edit = typed(&mut store, &mut doc, "Six, typed under the new name.");
    save_wait(&mut store, opened.doc, &doc, edit);
    typed(&mut store, &mut doc, " Seven.");
    typed(&mut store, &mut doc, " Eight, never saved.");
    settle(&mut store);
    store.abandon();
    assert_eq!(fs::read_to_string(rig.folder.join("Sub").join("b.md")).unwrap(), states[6]);

    let names = rig.journals();
    assert_eq!(names.len(), 1, "{names:?}");
    let journal_path = rig.data.join("journal").join(&names[0]);
    let whole = fs::read(&journal_path).unwrap();
    let recs = records(&whole);
    let kinds: Vec<u8> = recs.iter().map(|r| r.0).collect();
    let end_of = |kind: u8, nth: usize| recs.iter().filter(|r| r.0 == kind).nth(nth).map(|r| r.1).unwrap_or_else(|| panic!("the journal has no record {nth} of kind {kind}: {kinds:?}"));
    assert_eq!(kinds.iter().filter(|&&k| k == KIND_EDIT).count(), 8, "{kinds:?}");
    let first_done = end_of(KIND_DONE, 0);
    let path_end = end_of(KIND_PATH, 0);
    let second_begin = end_of(KIND_BEGIN, 1);
    let second_done = end_of(KIND_DONE, 1);
    assert!(first_done < path_end && path_end < second_begin && second_begin < second_done, "{kinds:?}");

    // (file name, which save it holds)
    let old_name_first_save = ("a.md", 3usize);
    let new_name_first_save = ("Sub\\b.md", 3usize);
    let new_name_second_save = ("Sub\\b.md", 6usize);
    let mut tried = 0;
    let mut cuts: Vec<usize> = recs.iter().map(|r| r.1).filter(|&end| end >= first_done).flat_map(|end| [end, end + 1]).filter(|&cut| cut <= whole.len()).collect();
    cuts.dedup();
    for cut in cuts {
        let mut folders = Vec::new();
        if cut < second_done {
            // The rename is not on the folder's disk until something there is flushed after it.
            folders.push(old_name_first_save);
        }
        if cut >= path_end {
            folders.push(new_name_first_save);
        }
        if cut >= second_begin {
            folders.push(new_name_second_save);
        }
        let edits = recs.iter().filter(|r| r.0 == KIND_EDIT && r.1 <= cut).count();
        for (name, save) in folders {
            empty_dir(&rig.folder);
            empty_dir(&rig.data.join("journal"));
            let planted = rig.folder.join(name);
            fs::create_dir_all(planted.parent().unwrap()).unwrap();
            fs::write(&planted, &states[save]).unwrap();
            fs::write(&journal_path, &whole[..cut]).unwrap();
            let (store, report) = rig.open();
            assert!(store.shutdown(SOON), "shutdown after recovery timed out");
            tried += 1;

            let here = format!("journal cut at {cut} of {} ({edits} edits in it), folder holding {name} at save {save}; report {report:?}", whole.len());
            let files = files_under(&rig.folder);
            assert_eq!(files.len(), 1, "{here}: the folder holds {:?}", files.iter().map(|(name, _)| name).collect::<Vec<_>>());
            let (found, bytes) = &files[0];
            assert!(found == "a.md" || found == "Sub\\b.md", "{here}: the text is under {found}, a name it never had");
            let text = String::from_utf8(bytes.clone()).unwrap_or_else(|_| panic!("{here}: {found} is not whole UTF-8"));
            assert_eq!(text, states[edits.max(save)], "{here}: {found} does not hold every edit the journal still had");
            assert!(rig.journals().is_empty() || !report.is_empty(), "{here}: a journal was kept without a word: {:?}", rig.journals());
        }
    }
    println!("power cut around a rename: {tried} pairs of journal and folder states recovered");
}
