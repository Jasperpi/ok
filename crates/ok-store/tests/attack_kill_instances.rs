//! What happens to a crashed session's journals when something else has a
//! hand on them at the next start: a second instance of the program opening
//! another writing folder at the same moment, or any program that happens to
//! have a journal file open.
//!
//! Every journal lives in the one data directory, whichever folder its text
//! belongs to, and every starting instance walks all of them. These tests
//! start two stores at once on two folders that share a data directory, and
//! start one store while a journal is held, and then ask the only question
//! that matters: is everything that was typed still where he will find it?

use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, save_wait, type_end, Rig};
use ok_store::{OpenError, Recovered, Setup, Store};
use ok_text::{Doc, Pos, Span};

const SOON: Duration = Duration::from_secs(10);

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).unwrap_or_default()).unwrap_or_default()
}

/// Waits until every recorded edit has been written to a journal file: what
/// a kill at this instant would leave behind.
fn settle(store: &mut Store) {
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up");
        while store.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Every file under `dir`, folders walked, as (path inside it, bytes).
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned(), bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Puts `dir` back to exactly the files in `files`.
fn restore(dir: &Path, files: &[(String, Vec<u8>)]) {
    for _ in 0..200 {
        match fs::remove_dir_all(dir) {
            Ok(()) => break,
            Err(_) if !dir.exists() => break,
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(!dir.exists(), "{} could not be cleared", dir.display());
    fs::create_dir_all(dir).unwrap();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
    }
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

fn open_on(folder: &Path, data: &Path) -> Result<(Store, Vec<Recovered>), OpenError> {
    let mut setup = Setup::new(folder.to_path_buf(), data.to_path_buf());
    setup.tuning = quick();
    setup.note = "window 1".to_string();
    Store::open(setup)
}

/// The journal files under `data`, by name.
fn journals(data: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(data.join("journal")).map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".wal")).collect()).unwrap_or_default();
    names.sort();
    names
}

/// He had two windows open, each on its own writing folder, when the machine
/// went down; only one of the folders had anything unsaved. Afterwards both
/// are started again within a few milliseconds of each other (by hand, or by
/// whatever restarts programs at sign-in). This restores that crashed session
/// `rounds` times and starts two stores on it, the other folder's first or
/// second. Returns the rounds in which the folder with the unsaved text was
/// refused, and the texts that were neither brought back nor spoken of.
fn start_both(name: &str, rounds: usize, other_first: bool) -> (Vec<String>, Vec<String>) {
    const TEXTS: usize = 8;
    let rig = Rig::new(name);
    let here = rig.folder.clone();
    let other = rig.folder.with_file_name("other writing");
    fs::create_dir_all(&other).unwrap();

    // The crashed session: eight short texts, each saved once and then worked
    // on for a long while without a save (words typed and taken back), so
    // that each has a long journal to read.
    let mut typed: Vec<(String, String)> = Vec::new();
    {
        let (mut store, _) = open_on(&here, &rig.data).expect("the store opens");
        for n in 0..TEXTS {
            let rel = format!("text-{n}.md");
            let opened = store.create_text(&rel).unwrap();
            let mut doc = Doc::new();
            let edit = type_end(&mut store, opened.doc, &mut doc, &format!("Text {n}, saved once.\n"));
            save_wait(&mut store, opened.doc, &doc, edit);
            let piece = " and a word he then took back";
            for _ in 0..3000 {
                type_end(&mut store, opened.doc, &mut doc, piece);
                let end = doc.end();
                let change = doc.replace(Span::new(Pos { line: end.line, col: end.col - piece.len() }, end), "");
                store.record(opened.doc, &change, doc.len_bytes());
            }
            type_end(&mut store, opened.doc, &mut doc, &format!("Typed into text {n} after its last save."));
            typed.push((rel, doc.to_text("\n")));
        }
        settle(&mut store);
        store.abandon();
    }
    let start = [tree(&here), tree(&rig.data)];
    assert_eq!(journals(&rig.data).len(), TEXTS);

    let mut rng = Rng(0x1257_A9CE_5EED ^ (std::process::id() as u64) << 17 | 1);
    let mut refused: Vec<String> = Vec::new();
    let mut unsaid: Vec<String> = Vec::new();
    for round in 0..rounds {
        restore(&here, &start[0]);
        restore(&rig.data, &start[1]);
        // One of the two starts up to 25 ms after the other.
        let lag = Duration::from_micros(rng.below(25_000));
        let gate = Arc::new(Barrier::new(2));
        let (their_gate, their_folder, their_data) = (gate.clone(), other.clone(), rig.data.clone());
        let their_lag = if other_first { Duration::ZERO } else { lag };
        let theirs = std::thread::spawn(move || {
            their_gate.wait();
            std::thread::sleep(their_lag);
            open_on(&their_folder, &their_data)
        });
        gate.wait();
        std::thread::sleep(if other_first { lag } else { Duration::ZERO });
        let mine = open_on(&here, &rig.data);
        let theirs = theirs.join().expect("the other instance's thread ends");
        let (their_store, their_report) = theirs.unwrap_or_else(|error| panic!("round {round}: the other folder, which nobody else touches, did not open: {error:?}"));
        let order = if other_first { format!("the other folder's instance started {lag:?} earlier") } else { format!("the other folder's instance started {lag:?} later") };
        match mine {
            Err(error) => {
                refused.push(format!("round {round} ({order}): {error:?}"));
                assert!(their_store.shutdown(SOON));
            }
            Ok((my_store, my_report)) => {
                for (rel, text) in &typed {
                    let in_file = read(&here.join(rel)) == *text;
                    let spoken_of = my_report.iter().chain(&their_report).any(|told| told.rel == Path::new(rel));
                    if !in_file && !spoken_of {
                        unsaid.push(format!("round {round} ({order}): {rel}"));
                    }
                }
                assert!(my_store.shutdown(SOON));
                assert!(their_store.shutdown(SOON));
            }
        }
    }
    (refused, unsaid)
}

fn rounds() -> usize {
    std::env::var("OK_ATTACK_INSTANCES").ok().and_then(|v| v.parse().ok()).unwrap_or(40)
}

/// The other folder's instance starts a moment earlier. While it is looking
/// after this folder's journals it holds this folder's lock, and this
/// folder's own window, started in that moment, must still open.
#[test]
fn a_folder_is_not_refused_while_another_folders_instance_recovers_its_journals() {
    let rounds = rounds();
    let (refused, unsaid) = start_both("instances-refused", rounds, true);
    println!("the other folder's instance first, {rounds} rounds: the folder with unsaved text was refused {} times; a text was neither brought back nor spoken of {} times", refused.len(), unsaid.len());
    assert!(refused.is_empty(), "in {rounds} rounds, opening the folder was refused {} times although no instance had it open as its writing folder; the first: {:?}", refused.len(), refused.first());
}

/// The other folder's instance starts a moment later and walks the same
/// journals while this folder's instance is recovering them. Every text must
/// still be brought back, or at least spoken of in one of the two reports.
#[test]
fn no_text_goes_unsaid_when_another_folders_instance_walks_the_journals_too() {
    let rounds = rounds();
    let (refused, unsaid) = start_both("instances-unsaid", rounds, false);
    println!("the other folder's instance second, {rounds} rounds: the folder with unsaved text was refused {} times; a text was neither brought back nor spoken of {} times", refused.len(), unsaid.len());
    assert!(refused.is_empty(), "{refused:?}");
    assert!(unsaid.is_empty(), "in {rounds} rounds, {} texts were left in the journal with the file stale and no word in either instance's report; the first: {:?}", unsaid.len(), unsaid.first());
}
/// A journal recovery could not take up at start is still lying in the data
/// directory, under a high generation number if the crashed session had
/// rotated it. He opens the text (it shows the stale file), types, and the
/// program is killed again. What he typed in this session was acknowledged
/// and journaled; it must not be thrown away in favour of the older journal,
/// nor the older journal in favour of it.
///
/// Corrected after the fix: a program that merely reads the journal (sharing
/// everything, as this test first had it) no longer keeps recovery out, so
/// such a start brings the text back and there is nothing left to test. The
/// holder here shares nothing, which does keep the journal from being read,
/// and it holds on until after the text has been opened, which is the worst
/// case. The start must then say the text is waiting instead of saying
/// nothing; that replaces the old precondition that the report be empty.
#[test]
fn text_typed_after_a_start_that_skipped_a_journal_survives_the_next_kill() {
    let rig = Rig::new("instances-lineage");
    let mut tuning = quick();
    tuning.rotate_at = 1;
    let (mut store, _) = rig.open_with(tuning.clone());
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    // A small text saved many times: the journal starts afresh at most saves.
    for n in 0..14 {
        let edit = type_end(&mut store, opened.doc, &mut doc, &format!(" w{n}"));
        save_wait(&mut store, opened.doc, &doc, edit);
    }
    type_end(&mut store, opened.doc, &mut doc, " FIRST-SESSION-UNSAVED");
    settle(&mut store);
    store.flush();
    std::thread::sleep(Duration::from_millis(150));
    store.abandon();
    let first = rig.journals();
    assert_eq!(first.len(), 1, "{first:?}");
    let generation: u32 = first[0].split('.').nth(1).and_then(|g| g.parse().ok()).unwrap_or(0);
    assert!(generation >= 2, "the crashed session's journal was never rotated: {first:?}");

    // The second start, with some other program holding the journal and sharing nothing.
    let scanner = OpenOptions::new().read(true).share_mode(0).open(rig.data.join("journal").join(&first[0])).unwrap();
    let (mut store, report) = rig.open_with(tuning.clone());
    assert_eq!(report.iter().map(|told| (told.rel.as_path(), &told.how)).collect::<Vec<_>>(), [(Path::new("essay.md"), &ok_store::Recovery::Waiting)], "a journal that cannot be read is said to be waiting");
    let opened = store.open_text("essay.md").unwrap();
    drop(scanner);
    assert!(opened.restored.is_none() && !opened.text.contains("FIRST-SESSION-UNSAVED"), "this test is about a text that opened without what the held journal has: {:?}", opened.text);
    let mut doc = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc, " SECOND-SESSION-UNSAVED");
    settle(&mut store);
    store.flush();
    std::thread::sleep(Duration::from_millis(150));
    let between = rig.journals();
    store.abandon();

    let (store, report) = rig.open_with(tuning);
    assert!(store.shutdown(SOON));
    let files: Vec<(String, String)> = tree(&rig.folder).into_iter().map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect();
    let anywhere = |words: &str| files.iter().any(|(_, text)| text.contains(words));
    assert!(
        anywhere("SECOND-SESSION-UNSAVED"),
        "what was typed in the second session is in no file: the journals were {first:?} after the first kill and {between:?} at the second, recovery said {report:?}, and the folder holds {:?}; journals left: {:?}",
        files.iter().map(|(name, text)| format!("{name}: ...{}", &text[text.len().saturating_sub(48)..])).collect::<Vec<_>>(),
        rig.journals()
    );
    assert!(anywhere("FIRST-SESSION-UNSAVED"), "what was typed in the first session is in no file: {report:?}");
}

/// The same skipped journal, and this time no second kill: he works on,
/// saves, and closes everything properly. The first session's words are
/// still only in that journal. The next start must find them.
///
/// Corrected the same way as the test above: the holder shares nothing and
/// holds on through the opening of the text, and the start must report the
/// text as waiting.
#[test]
fn a_skipped_journal_is_not_forgotten_after_a_clean_session_on_the_same_text() {
    let rig = Rig::new("instances-clean-after");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Saved before the crash.");
    save_wait(&mut store, opened.doc, &doc, edit);
    type_end(&mut store, opened.doc, &mut doc, " FIRST-SESSION-UNSAVED");
    settle(&mut store);
    store.abandon();
    let first = rig.journals();
    assert_eq!(first.len(), 1, "{first:?}");

    let scanner = OpenOptions::new().read(true).share_mode(0).open(rig.data.join("journal").join(&first[0])).unwrap();
    let (mut store, report) = rig.open();
    assert_eq!(report.iter().map(|told| (told.rel.as_path(), &told.how)).collect::<Vec<_>>(), [(Path::new("essay.md"), &ok_store::Recovery::Waiting)], "a journal that cannot be read is said to be waiting");
    let opened = store.open_text("essay.md").unwrap();
    drop(scanner);
    assert!(opened.restored.is_none() && !opened.text.contains("FIRST-SESSION-UNSAVED"), "this test is about a text that opened without what the held journal has: {:?}", opened.text);
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " Second session, saved and closed.");
    store.close(opened.doc, Some((doc.to_text("\n"), edit)), None);
    assert!(store.shutdown(SOON));
    let between = rig.journals();

    let (store, report) = rig.open();
    assert!(store.shutdown(SOON));
    let files: Vec<(String, String)> = tree(&rig.folder).into_iter().map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect();
    assert!(
        files.iter().any(|(_, text)| text.contains("FIRST-SESSION-UNSAVED")),
        "the first session's unsaved words are in no file after a clean second session: journals {first:?}, then {between:?}; recovery said {report:?}; the folder holds {files:?}"
    );
    assert!(files.iter().any(|(_, text)| text.contains("Second session, saved and closed.")), "{files:?}");
}
