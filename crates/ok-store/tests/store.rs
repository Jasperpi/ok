//! The store as the interface will use it, on real files in temporary
//! folders: saving, closing and coming back, recovery after a simulated
//! kill or power cut, other programs interfering with the file, names that
//! must never be overwritten, versions, renames, discards, and the cost of
//! the keystroke path.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{safe_file_name, Eol, Format, Hold, NameError, News, OpenError, ReadOnly, Recovery, Resolve, Store, StoreError};
use ok_text::{Change, Doc, Pos, Span};

const SOON: Duration = Duration::from_secs(10);
const SHARE_ALL: u32 = 7;

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

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).expect("the file can be read")).expect("the file is UTF-8")
}

/// A new text holding `text`, saved. Returns its handle, the buffer and the edit number.
fn written(store: &mut Store, rel: &str, text: &str) -> (ok_store::DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

fn closed(store: &mut Store, id: ok_store::DocId) -> News {
    expect(store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == id))
}

fn set_read_only(path: &Path, on: bool) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(on);
    fs::set_permissions(path, perms).unwrap();
}

// ------------------------------------------------------------ the plain path

#[test]
fn a_new_text_is_saved_closed_and_comes_back_with_its_caret() {
    let rig = Rig::new("plain");
    let (mut store, recovered) = rig.open();
    assert!(recovered.is_empty());
    let opened = store.create_text("essay.md").unwrap();
    assert_eq!((opened.text.as_str(), opened.edit, opened.read_only, opened.restored), ("", 0, None, None));
    assert!(!rig.folder.join("essay.md").exists(), "no file before the first save");
    let mut doc = Doc::new();
    assert_eq!(type_end(&mut store, opened.doc, &mut doc, "Hello world."), 1);
    assert_eq!(type_end(&mut store, opened.doc, &mut doc, "\nSecond line."), 2);
    // A change that changes nothing is not an edit.
    assert_eq!(store.record(opened.doc, &Change::default(), doc.len_bytes()), 2);
    store.save(opened.doc, doc.to_text("\n"), 2);
    match expect(&mut store, "Saved", |news| matches!(news, News::Saved { .. })) {
        News::Saved { doc: id, edit, bytes, at_ms } => {
            assert_eq!((id, edit, bytes), (opened.doc, 2, 25));
            assert!(at_ms > 1_600_000_000_000);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(read(&rig.folder.join("essay.md")), "Hello world.\nSecond line.");
    assert_eq!(rig.listing(), ["essay.md"], "nothing but the text in the folder");
    assert_eq!(rig.journals().len(), 1, "the journal exists while the text is open");

    store.close(opened.doc, None, Some(Pos { line: 1, col: 3 }));
    assert_eq!(closed(&mut store, opened.doc), News::Closed { doc: opened.doc, beside: None, in_journal: false });
    assert_eq!(store.record(opened.doc, &Change::default(), 0), 0, "a closed handle records nothing");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty(), "a clean close removes the journal");

    let (mut store, recovered) = rig.open();
    assert!(recovered.is_empty());
    let again = store.open_text("essay.md").unwrap();
    assert_eq!(again.text, "Hello world.\nSecond line.");
    assert_eq!(again.caret, Some(Pos { line: 1, col: 3 }));
    assert_eq!((again.format, again.read_only, again.restored, again.mixed_endings), (Format::default(), None, None, false));
    assert_eq!(store.open_text("ESSAY.md").err(), Some(StoreError::AlreadyOpen(again.doc)));
    assert_eq!(store.open_text("nothing.md").err(), Some(StoreError::Missing));
    assert!(store.shutdown(SOON));
}

#[test]
fn line_endings_and_the_byte_order_mark_are_written_back() {
    let rig = Rig::new("format");
    fs::write(rig.folder.join("win.txt"), b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
    fs::write(rig.folder.join("mixed.md"), b"a\r\nb\r\nc\nd").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("win.txt").unwrap();
    assert_eq!(opened.text, "one\ntwo\n");
    assert_eq!(opened.format, Format { eol: Eol::CrLf, bom: true });
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, "three");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(fs::read(rig.folder.join("win.txt")).unwrap(), b"\xEF\xBB\xBFone\r\ntwo\r\nthree");

    let mixed = store.open_text("mixed.md").unwrap();
    assert!(mixed.mixed_endings);
    assert_eq!((mixed.text.as_str(), mixed.format.eol), ("a\nb\nc\nd", Eol::CrLf));
    assert_eq!(fs::read(rig.folder.join("mixed.md")).unwrap(), b"a\r\nb\r\nc\nd", "opening changes nothing");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_file_that_is_not_utf8_is_shown_and_never_rewritten() {
    let rig = Rig::new("notutf8");
    let latin = b"caf\xE9 au lait\r\n".to_vec();
    fs::write(rig.folder.join("latin.txt"), &latin).unwrap();
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "wide\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(rig.folder.join("wide.txt"), &utf16).unwrap();
    let (mut store, _) = rig.open();

    let opened = store.open_text("latin.txt").unwrap();
    assert_eq!(opened.read_only, Some(ReadOnly::NotUtf8));
    assert_eq!(opened.text, "caf\u{FFFD} au lait\n");
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, "typed anyway");
    store.save(opened.doc, doc.to_text("\n"), edit);
    expect(&mut store, "Held NotUtf8", |news| matches!(news, News::Held { why: Hold::NotUtf8, .. }));
    assert_eq!(fs::read(rig.folder.join("latin.txt")).unwrap(), latin);
    // Closing with something typed puts it beside the file, never in it.
    store.close(opened.doc, Some((doc.to_text("\n"), edit)), None);
    let News::Closed { beside: Some(beside), in_journal: false, .. } = closed(&mut store, opened.doc) else { panic!("expected a file beside") };
    assert!(beside.to_string_lossy().starts_with("latin (edited "), "{beside:?}");
    assert_eq!(read(&rig.folder.join(&beside)), "caf\u{FFFD} au lait\ntyped anyway");
    assert_eq!(fs::read(rig.folder.join("latin.txt")).unwrap(), latin);

    let wide = store.open_text("wide.txt").unwrap();
    assert_eq!((wide.read_only, wide.text.as_str()), (Some(ReadOnly::NotUtf8), "wide\n"));
    store.close(wide.doc, None, None);
    assert_eq!(closed(&mut store, wide.doc), News::Closed { doc: wide.doc, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert_eq!(fs::read(rig.folder.join("wide.txt")).unwrap(), utf16);
    assert_eq!(rig.listing().len(), 3);
    assert!(rig.journals().is_empty());
}

#[test]
fn names_that_windows_or_the_folder_cannot_take_are_refused() {
    let rig = Rig::new("names");
    fs::write(rig.folder.join("taken.md"), "here first").unwrap();
    let (mut store, _) = rig.open();
    let refused = |store: &mut Store, name: &str| match store.create_text(name) {
        Err(StoreError::Name(why)) => why,
        other => panic!("{name:?} should be refused, got {other:?}"),
    };
    assert_eq!(refused(&mut store, "taken.md"), NameError::Taken);
    assert_eq!(refused(&mut store, "TAKEN.MD"), NameError::Taken);
    for device in ["CON.md", "prn.txt", "AUX.md", "nul.md", "COM1.md", "com9.txt", "LPT1.md", "lpt9.md"] {
        assert_eq!(refused(&mut store, device), NameError::Reserved, "{device}");
    }
    for (name, c) in [("what?.md", '?'), ("a:b.md", ':'), ("a*b.md", '*'), ("a|b.md", '|'), ("a<b.md", '<'), ("a>b.md", '>'), ("a\"b.md", '"')] {
        assert_eq!(refused(&mut store, name), NameError::Forbidden(c), "{name}");
    }
    assert_eq!(refused(&mut store, "essay.md."), NameError::TrailingDotOrSpace);
    assert_eq!(refused(&mut store, "essay.md "), NameError::TrailingDotOrSpace);
    assert_eq!(refused(&mut store, "sub \\essay.md"), NameError::TrailingDotOrSpace);
    assert_eq!(refused(&mut store, ".hidden.md"), NameError::Hidden);
    assert_eq!(refused(&mut store, "notes.docx"), NameError::NotAText);
    assert_eq!(refused(&mut store, "..\\outside.md"), NameError::Outside);
    assert_eq!(refused(&mut store, "C:\\outside.md"), NameError::Outside);
    assert_eq!(rig.listing(), ["taken.md"], "a refused name makes nothing");

    // A title becomes a name that is accepted, in a folder made on the way.
    let name = format!("College\\{}.md", safe_file_name("What is \"justice\"? A reply: part 1"));
    let opened = store.create_text(&name).unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "x");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(read(&rig.folder.join("College").join("What is justice A reply part 1.md")), "x");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_save_that_a_newer_one_overtakes_is_skipped() {
    let rig = Rig::new("skip");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    for _ in 0..60 {
        let edit = type_end(&mut store, opened.doc, &mut doc, "word ");
        store.save(opened.doc, doc.to_text("\n"), edit);
    }
    let mut seen = Vec::new();
    assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: 60, .. })).is_some(), "{seen:?}");
    let saves = seen.iter().filter(|news| matches!(news, News::Saved { .. })).count();
    assert!(saves < 30, "{saves} saves were made for 60 requests sent in one burst");
    assert_eq!(read(&rig.folder.join("essay.md")), doc.to_text("\n"));
    assert!(store.shutdown(SOON));
}

// ------------------------------------------------------- kill and power loss

#[test]
fn unsaved_edits_come_back_after_a_kill() {
    let rig = Rig::new("kill-sim");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "one two three");
    type_end(&mut store, id, &mut doc, " four");
    type_end(&mut store, id, &mut doc, " five");
    store.caret(id, Pos { line: 0, col: 7 });
    store.abandon();
    assert_eq!(read(&rig.folder.join("essay.md")), "one two three", "the file is the last save");
    assert_eq!(rig.journals().len(), 1);

    let (mut store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!((recovered[0].rel.as_path(), &recovered[0].how, recovered[0].words), (Path::new("essay.md"), &Recovery::BroughtBack, 2));
    assert_eq!(recovered[0].folder, store.folder());
    assert_eq!(read(&rig.folder.join("essay.md")), "one two three four five");
    assert!(rig.journals().is_empty());
    assert_eq!(rig.listing(), ["essay.md"]);
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!((opened.text.as_str(), opened.restored), ("one two three four five", None));
    // What was the file before recovery is in the history.
    let versions = store.versions(opened.doc).unwrap();
    let recovery = versions.iter().find(|v| v.label.as_deref() == Some("recovery")).expect("a recovery version");
    assert_eq!(store.read_version(opened.doc, &recovery.name).unwrap(), "one two three");
    assert!(store.shutdown(SOON));

    let (store, recovered) = rig.open();
    assert!(recovered.is_empty(), "recovery has nothing left to do: {recovered:?}");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_text_never_saved_is_written_out_by_recovery() {
    let rig = Rig::new("never-saved");
    let (mut store, _) = rig.open();
    let opened = store.create_text("Notes\\first.md").unwrap();
    let mut doc = Doc::new();
    type_end(&mut store, opened.doc, &mut doc, "typed and then the power went");
    store.abandon();
    assert!(!rig.folder.join("Notes").join("first.md").exists());
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::WroteBack(PathBuf::from("Notes\\first.md")));
    assert_eq!(recovered[0].words, 6);
    assert_eq!(read(&rig.folder.join("Notes").join("first.md")), "typed and then the power went");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_power_cut_that_undoes_the_last_rename_is_seen_as_our_own_older_state() {
    for rotate_at in [u64::MAX, 1] {
        let rig = Rig::new("power-revert");
        let mut tuning = quick();
        // With rotate_at 1 the journal starts afresh at every save, so only
        // the states carried in each new generation can vouch for the file.
        tuning.rotate_at = rotate_at;
        let (mut store, _) = rig.open_with(tuning.clone());
        let (id, mut doc, _) = written(&mut store, "essay.md", "First paragraph.");
        let first = doc.to_text("\n");
        for more in ["\n\nSecond.", "\n\nThird.", "\n\nFourth."] {
            let edit = type_end(&mut store, id, &mut doc, more);
            save_wait(&mut store, id, &doc, edit);
        }
        type_end(&mut store, id, &mut doc, "\n\nUnsaved fifth.");
        store.abandon();
        if rotate_at == 1 {
            let journals = rig.journals();
            assert!(journals.len() == 1 && !journals[0].contains(".1.wal"), "the journal should have rotated: {journals:?}");
        }
        // The power cut: the folder shows a save from three saves ago.
        fs::write(rig.folder.join("essay.md"), &first).unwrap();

        let (store, recovered) = rig.open_with(tuning);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].how, Recovery::BroughtBack, "rotate_at {rotate_at}");
        assert_eq!(read(&rig.folder.join("essay.md")), doc.to_text("\n"));
        assert_eq!(rig.listing(), ["essay.md"], "its own older state must never be kept as a second file");
        assert!(store.shutdown(SOON));
    }
}

#[test]
fn a_file_changed_while_we_were_down_is_left_alone_and_ours_goes_beside_it() {
    let rig = Rig::new("power-outside");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Mine, saved.");
    type_end(&mut store, id, &mut doc, " And typed after.");
    store.abandon();
    fs::write(rig.folder.join("essay.md"), "Someone else wrote this meanwhile.").unwrap();

    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    let Recovery::KeptBeside(beside) = &recovered[0].how else { panic!("{recovered:?}") };
    assert!(beside.to_string_lossy().starts_with("essay (recovered "), "{beside:?}");
    assert_eq!(read(&rig.folder.join("essay.md")), "Someone else wrote this meanwhile.");
    assert_eq!(read(&rig.folder.join(beside)), "Mine, saved. And typed after.");
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing().len(), 2);
    let (store, recovered) = rig.open();
    assert!(recovered.is_empty());
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing().len(), 2, "recovering twice makes no second copy");
}

#[test]
fn a_file_that_vanished_comes_back_only_if_something_was_unsaved() {
    // Unsaved text and the name gone (as a cut during a save on exFAT can leave it).
    let rig = Rig::new("power-missing");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    type_end(&mut store, id, &mut doc, " Unsaved.");
    store.abandon();
    fs::remove_file(rig.folder.join("essay.md")).unwrap();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::WroteBack(PathBuf::from("essay.md")));
    assert_eq!(read(&rig.folder.join("essay.md")), "Saved. Unsaved.");
    assert!(store.shutdown(SOON));

    // Everything saved, then removed by hand while we were down: his choice.
    let rig = Rig::new("power-removed");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let edit = type_end(&mut store, id, &mut doc, " All of it.");
    save_wait(&mut store, id, &doc, edit);
    store.abandon();
    fs::remove_file(rig.folder.join("essay.md")).unwrap();
    let (store, recovered) = rig.open();
    assert!(recovered.is_empty(), "{recovered:?}");
    assert!(rig.listing().is_empty());
    assert!(rig.journals().is_empty());
    assert!(store.shutdown(SOON));
}

#[test]
fn a_journal_cut_short_or_zero_filled_gives_a_whole_earlier_state() {
    for cut in [0usize, 1, 7, 20, 33, 64, 200] {
        let rig = Rig::new("power-tail");
        let (mut store, _) = rig.open();
        let (id, mut doc, _) = written(&mut store, "essay.md", "Base.");
        let mut states = vec![doc.to_text("\n")];
        for n in 0..30 {
            type_end(&mut store, id, &mut doc, &format!(" w{n}"));
            states.push(doc.to_text("\n"));
        }
        store.abandon();
        // A power cut loses an unflushed tail: shorter, and what is left may read as zeros.
        let journal = rig.data.join("journal").join(&rig.journals()[0]);
        let mut bytes = fs::read(&journal).unwrap();
        let keep = bytes.len().saturating_sub(cut);
        bytes.truncate(keep);
        bytes.extend_from_slice(&[0u8; 512]);
        fs::write(&journal, &bytes).unwrap();

        let (store, _) = rig.open();
        let text = read(&rig.folder.join("essay.md"));
        let at = states.iter().position(|s| *s == text);
        assert!(at.is_some(), "cut {cut}: the recovered text is not a state the text was in: {text:?}");
        if cut == 0 {
            assert_eq!(at, Some(30));
        }
        assert!(store.shutdown(SOON));
        assert_eq!(rig.listing(), ["essay.md"]);
    }
}

#[test]
fn a_save_whose_last_flush_failed_never_lets_the_journal_go() {
    let rig = Rig::new("power-flush2");
    let tuning = quick();
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "Durable.");
    let durable = doc.to_text("\n");
    tuning.faults.second_flush.store(1117, Ordering::Relaxed);
    let edit = type_end(&mut store, id, &mut doc, " Renamed but not flushed.");
    store.save(id, doc.to_text("\n"), edit);
    expect(&mut store, "Held NotDurable", |news| matches!(news, News::Held { why: Hold::NotDurable, .. }));
    let seen = listen(&mut store, Duration::from_millis(50));
    assert!(!seen.iter().any(|news| matches!(news, News::Saved { .. })), "a save that is not durable must not be reported: {seen:?}");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: true });
    assert!(store.shutdown(SOON));
    assert_eq!(rig.journals().len(), 1, "the journal must outlive a save that was not flushed after its rename");

    // The power goes and the rename is undone.
    tuning.faults.second_flush.store(0, Ordering::Relaxed);
    fs::write(rig.folder.join("essay.md"), &durable).unwrap();
    let (store, recovered) = rig.open_with(tuning);
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::BroughtBack);
    assert_eq!(read(&rig.folder.join("essay.md")), "Durable. Renamed but not flushed.");
    assert!(store.shutdown(SOON));
}

#[test]
fn edits_typed_between_a_save_request_and_its_barrier_survive_rotation_and_a_kill() {
    let rig = Rig::new("rotation");
    let mut tuning = quick();
    tuning.rotate_at = 1;
    for round in 0..6 {
        let (mut store, recovered) = rig.open_with(tuning.clone());
        assert!(recovered.is_empty(), "round {round}: {recovered:?}");
        let opened = match store.open_text("essay.md") {
            Ok(opened) => opened,
            Err(_) => store.create_text("essay.md").unwrap(),
        };
        let mut doc = Doc::from_text(&opened.text);
        let mut edit = 0;
        for n in 0..40 {
            edit = type_end(&mut store, opened.doc, &mut doc, &format!("r{round}e{n} "));
        }
        // The save is asked for, and typing goes straight on: these two edits
        // reach the journal before the save's own record does.
        store.save(opened.doc, doc.to_text("\n"), edit);
        type_end(&mut store, opened.doc, &mut doc, "between-one ");
        type_end(&mut store, opened.doc, &mut doc, "between-two ");
        expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
        type_end(&mut store, opened.doc, &mut doc, "after ");
        store.abandon();

        let (store, recovered) = rig.open_with(tuning.clone());
        assert_eq!(recovered.len(), 1, "round {round}");
        assert_eq!(recovered[0].how, Recovery::BroughtBack);
        assert_eq!(read(&rig.folder.join("essay.md")), doc.to_text("\n"), "round {round}");
        assert!(store.shutdown(SOON));
    }
}

// ------------------------------------------------------ outside interference

#[test]
fn a_writer_holding_the_file_means_busy_nothing_written_and_everything_journaled() {
    let rig = Rig::new("busy-writer");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Before.");
    let path = rig.folder.join("essay.md");
    // Another program has the file open for writing, sharing everything.
    let writer = OpenOptions::new().write(true).share_mode(SHARE_ALL).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed while it was held.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held Busy", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::Busy });
    assert_eq!(read(&path), "Before.", "nothing was written");
    assert_eq!(rig.listing(), ["essay.md"], "no temp file left behind");
    // Killed now, with the other program still at it: the text is in the journal.
    store.abandon();
    let (mut store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::Waiting, "the file cannot be read safely while it is being written");
    assert_eq!(read(&path), "Before.");
    assert_eq!(rig.journals().len(), 1);
    // Opening the text brings back what was typed, unsaved.
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.text, "Before. Typed while it was held.");
    assert!(opened.restored.is_some());
    store.save(opened.doc, opened.text.clone(), opened.edit);
    expect(&mut store, "Held Busy", |news| matches!(news, News::Held { why: Hold::Busy, .. }));
    // The other program lets go: the save goes through by itself.
    drop(writer);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: 0, .. }));
    assert_eq!(read(&path), "Before. Typed while it was held.");
    store.close(opened.doc, None, None);
    closed(&mut store, opened.doc);
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

#[test]
fn readers_do_not_stop_a_save_unless_they_refuse_to_share_delete() {
    let rig = Rig::new("readers");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Old text.");
    let path = rig.folder.join("essay.md");

    // A reader that shares everything: the save goes through under it, and
    // the reader goes on seeing the text it opened.
    let mut sharing = OpenOptions::new().read(true).share_mode(SHARE_ALL).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " New text.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&path), "Old text. New text.");
    let mut through_handle = String::new();
    std::io::Read::read_to_string(&mut sharing, &mut through_handle).unwrap();
    assert_eq!(through_handle, "Old text.");
    drop(sharing);

    // A reader that does not share delete: the file cannot be replaced.
    let stubborn = OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " More.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::Busy });
    assert_eq!(read(&path), "Old text. New text.");
    assert_eq!(rig.listing(), ["essay.md"]);
    drop(stubborn);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&path), "Old text. New text. More.");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_file_replaced_outside_is_shown_when_clean_and_kept_apart_when_not() {
    let rig = Rig::new("outside");
    let (mut store, _) = rig.open();
    let (id, mut doc, first_edit) = written(&mut store, "essay.md", "My first version.");
    let path = rig.folder.join("essay.md");

    // Clean buffer: the new text is handed over.
    fs::write(&path, "Their version.\r\n").unwrap();
    let News::ChangedOutside { text, clean, edit, format, read_only, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((text.as_str(), clean, edit, read_only), ("Their version.\n", true, first_edit, None));
    assert_eq!(format.eol, Eol::CrLf);
    // The interface takes it as an edit; the store sees the buffer now reads as the file does.
    let edit = replace_all(&mut store, id, &mut doc, &text);
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(fs::read(&path).unwrap(), b"Their version.\r\n", "taking theirs rewrites nothing");
    let versions = store.versions(id).unwrap();
    let before = versions.iter().find(|v| v.label.as_deref() == Some("before-reload")).expect("the text we had is kept");
    assert_eq!(store.read_version(id, &before.name).unwrap(), "My first version.");

    // Unsaved edits: both versions exist and the file is not touched.
    let edit = type_end(&mut store, id, &mut doc, "Mine again, unsaved.");
    fs::write(&path, "Theirs, a second time.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    let News::ChangedOutside { text: theirs, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((theirs.as_str(), clean), ("Theirs, a second time.", false));
    let seen = listen(&mut store, Duration::from_millis(100));
    assert!(!seen.iter().any(|news| matches!(news, News::Saved { .. })), "{seen:?}");
    assert_eq!(read(&path), "Theirs, a second time.");
    let versions = store.versions(id).unwrap();
    assert!(versions.iter().any(|v| v.label.as_deref() == Some("outside")));

    // Keep mine: the next save goes over theirs, which stays in the history.
    store.resolve(id, Resolve::KeepMine);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(fs::read(&path).unwrap(), b"Their version.\r\nMine again, unsaved.");
    let outside: Vec<String> = store.versions(id).unwrap().iter().filter(|v| v.label.as_deref() == Some("outside")).map(|v| store.read_version(id, &v.name).unwrap()).collect();
    assert_eq!(outside, ["Theirs, a second time."]);
    assert!(store.shutdown(SOON));
}

#[test]
fn two_keep_both_conflicts_in_one_second_leave_three_files() {
    let rig = Rig::new("keep-both");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Original.");
    let path = rig.folder.join("essay.md");
    let mut mine = Vec::new();
    for round in 0..2 {
        let edit = type_end(&mut store, id, &mut doc, &format!(" Mine {round}."));
        fs::write(&path, format!("Theirs {round}.")).unwrap();
        store.save(id, doc.to_text("\n"), edit);
        expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { clean: false, .. }));
        store.resolve(id, Resolve::KeepBoth { text: doc.to_text("\n"), edit });
        let News::KeptBeside { rel, .. } = expect(&mut store, "KeptBeside", |news| matches!(news, News::KeptBeside { .. })) else { unreachable!() };
        assert!(rel.to_string_lossy().starts_with("essay (mine "), "{rel:?}");
        assert_eq!(read(&rig.folder.join(&rel)), doc.to_text("\n"));
        mine.push((rel, doc.to_text("\n")));
        // The buffer takes theirs, as the interface does after keeping both.
        let edit = replace_all(&mut store, id, &mut doc, &format!("Theirs {round}."));
        save_wait(&mut store, id, &doc, edit);
    }
    assert_ne!(mine[0].0, mine[1].0, "the second kept copy must not land on the first");
    assert_eq!(rig.listing().len(), 3, "{:?}", rig.listing());
    for (rel, text) in &mine {
        assert_eq!(&read(&rig.folder.join(rel)), text);
    }
    assert_eq!(read(&path), "Theirs 1.");
    // Closing without choosing keeps both by itself.
    let edit = type_end(&mut store, id, &mut doc, " Mine at close.");
    fs::write(&path, "Theirs at close.").unwrap();
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let News::Closed { beside: Some(beside), in_journal: false, .. } = closed(&mut store, id) else { panic!("expected a kept copy") };
    assert_eq!(read(&rig.folder.join(beside)), "Theirs 1. Mine at close.");
    assert_eq!(read(&path), "Theirs at close.");
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing().len(), 4);
    assert!(rig.journals().is_empty());
}

#[test]
fn a_file_caught_between_truncate_and_write_is_never_loaded_as_empty() {
    let rig = Rig::new("truncated");
    let (mut store, _) = rig.open();
    let (id, _doc, _) = written(&mut store, "essay.md", "Five paragraphs of essay.");
    let path = rig.folder.join("essay.md");
    // Another program truncates the file and has not yet written its new version.
    let mut other = OpenOptions::new().write(true).share_mode(SHARE_ALL).open(&path).unwrap();
    other.set_len(0).unwrap();
    store.rescan();
    let seen = listen(&mut store, Duration::from_millis(400));
    assert!(!seen.iter().any(|news| matches!(news, News::ChangedOutside { .. })), "an empty half-written file was taken for the text: {seen:?}");
    other.write_all(b"The outsider's whole new version.").unwrap();
    drop(other);
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((text.as_str(), clean), ("The outsider's whole new version.", true));
    let _ = id;
    assert!(store.shutdown(SOON));
}

#[test]
fn a_removed_file_is_reported_after_a_moment_and_noticed_when_it_returns() {
    let rig = Rig::new("removed");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Here.");
    let path = rig.folder.join("essay.md");

    // Gone and straight back with the same text (a program saving by delete then rename): nothing to report.
    fs::remove_file(&path).unwrap();
    fs::write(&path, "Here.").unwrap();
    let seen = listen(&mut store, Duration::from_millis(500));
    assert!(!seen.iter().any(|news| matches!(news, News::Removed { .. } | News::ChangedOutside { .. })), "{seen:?}");

    // Gone and staying gone.
    fs::remove_file(&path).unwrap();
    expect(&mut store, "Removed", |news| matches!(news, News::Removed { .. }));
    let edit = type_end(&mut store, id, &mut doc, " Typed after it vanished.");
    store.save(id, doc.to_text("\n"), edit);
    let seen = listen(&mut store, Duration::from_millis(150));
    assert!(!seen.iter().any(|news| matches!(news, News::Saved { .. })), "{seen:?}");
    assert!(!path.exists(), "a removed file is not recreated behind his back while the text is open");

    // Back, with other contents: it reappears as an outside change, ours untouched in the journal.
    fs::write(&path, "Put back by someone.").unwrap();
    expect(&mut store, "Reappeared", |news| matches!(news, News::Reappeared { .. }));
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((text.as_str(), clean), ("Put back by someone.", false));
    assert!(store.shutdown(SOON));

    // Removed, with unsaved text, and then closed: the text is written back under its name.
    let rig = Rig::new("removed-close");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Here.");
    let path = rig.folder.join("essay.md");
    fs::remove_file(&path).unwrap();
    expect(&mut store, "Removed", |news| matches!(news, News::Removed { .. }));
    let edit = type_end(&mut store, id, &mut doc, " And more.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: false });
    assert_eq!(read(&path), "Here. And more.");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

#[test]
fn a_file_renamed_outside_is_followed_where_the_volume_allows() {
    let rig = Rig::new("renamed-outside");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Named one thing.");
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Renamed in Explorer.md")).unwrap();
    assert_eq!(expect(&mut store, "Moved", |news| matches!(news, News::Moved { .. })), News::Moved { doc: id, rel: PathBuf::from("Renamed in Explorer.md") });
    let edit = type_end(&mut store, id, &mut doc, " Then another.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("Renamed in Explorer.md")), "Named one thing. Then another.");
    assert_eq!(rig.listing(), ["Renamed in Explorer.md"]);
    assert!(store.shutdown(SOON));
}

#[test]
fn a_file_made_read_only_is_held_and_never_touched() {
    let rig = Rig::new("read-only");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Writable.");
    let path = rig.folder.join("essay.md");
    set_read_only(&path, true);
    let edit = type_end(&mut store, id, &mut doc, " Typed after.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held ReadOnly", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::ReadOnly });
    assert_eq!(read(&path), "Writable.");
    assert!(fs::metadata(&path).unwrap().permissions().readonly(), "the attribute is never cleared by us");
    assert_eq!(rig.listing(), ["essay.md"]);

    // The attribute is taken off again: the held save goes through.
    set_read_only(&path, false);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&path), "Writable. Typed after.");

    // Read-only at close with something unsaved: it goes beside the file.
    set_read_only(&path, true);
    let edit = type_end(&mut store, id, &mut doc, " Last words.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let News::Closed { beside: Some(beside), in_journal: false, .. } = closed(&mut store, id) else { panic!("expected a file beside") };
    assert!(beside.to_string_lossy().starts_with("essay (edited "), "{beside:?}");
    assert_eq!(read(&rig.folder.join(&beside)), "Writable. Typed after. Last words.");
    assert_eq!(read(&path), "Writable. Typed after.");
    assert!(fs::metadata(&path).unwrap().permissions().readonly());

    // And it opens saying so.
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.read_only, Some(ReadOnly::Attribute));
    assert!(store.shutdown(SOON));
    set_read_only(&path, false);
}

#[test]
fn a_hard_linked_file_is_not_replaced() {
    let rig = Rig::new("linked");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "One file.");
    let path = rig.folder.join("essay.md");
    let link = rig.folder.join("second name.md");
    if fs::hard_link(&path, &link).is_err() {
        return;
    }
    let edit = type_end(&mut store, id, &mut doc, " Two names.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held Linked", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::Linked });
    assert_eq!((read(&path), read(&link)), ("One file.".to_string(), "One file.".to_string()));
    let other = store.open_text("second name.md").unwrap();
    assert_eq!(other.read_only, Some(ReadOnly::Linked));
    // With the other name gone it is an ordinary file again.
    store.close(other.doc, None, None);
    closed(&mut store, other.doc);
    fs::remove_file(&link).unwrap();
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&path), "One file. Two names.");
    assert!(store.shutdown(SOON));
}

#[test]
fn the_folder_renamed_away_under_an_open_text_loses_nothing() {
    let rig = Rig::new("folder-gone");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "In the old folder.");
    let by_id = store.volume().open_by_id;
    let moved = rig.folder.with_file_name("writing renamed");
    // The store holds no file open between saves, so the folder can be renamed.
    let mut renamed = false;
    for _ in 0..100 {
        if fs::rename(&rig.folder, &moved).is_ok() {
            renamed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(renamed, "the folder could not be renamed while the store was idle");
    let edit = type_end(&mut store, id, &mut doc, " Typed after it moved.");
    store.save(id, doc.to_text("\n"), edit);
    if by_id {
        // Found again by its file index: saving carries on in the new place.
        let mut seen = Vec::new();
        assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit)).is_some(), "{seen:?}");
        assert!(seen.iter().any(|news| matches!(news, News::FolderMoved { .. })), "{seen:?}");
        assert_eq!(read(&moved.join("essay.md")), "In the old folder. Typed after it moved.");
        assert!(!rig.folder.exists());
        assert!(store.shutdown(SOON));
    } else {
        // Not findable: held, journaled, and written back at the old path at the next start.
        expect(&mut store, "Held FolderGone", |news| matches!(news, News::Held { why: Hold::FolderGone, .. }));
        store.abandon();
        let (store, recovered) = rig.open();
        assert_eq!(recovered.len(), 1);
        assert_eq!(read(&rig.folder.join("essay.md")), "In the old folder. Typed after it moved.");
        assert!(store.shutdown(SOON));
    }
}

#[test]
fn a_first_save_never_lands_on_a_name_that_appeared_meanwhile() {
    let rig = Rig::new("name-taken");
    let (mut store, _) = rig.open();
    let opened = store.create_text("new.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Mine.");
    // Another device or program makes a file of that name first.
    fs::write(rig.folder.join("new.md"), "Someone else's.").unwrap();
    store.save(opened.doc, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held NameTaken", |news| matches!(news, News::Held { .. })), News::Held { doc: opened.doc, why: Hold::NameTaken });
    assert_eq!(read(&rig.folder.join("new.md")), "Someone else's.");
    assert_eq!(rig.listing(), ["new.md"]);
    // Given another name, the text is saved there.
    store.rename("new.md", "new of mine.md").unwrap();
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&rig.folder.join("new of mine.md")), "Mine.");
    assert_eq!(read(&rig.folder.join("new.md")), "Someone else's.");

    // Left unresolved until close, it is written under a numbered name instead.
    let second = store.create_text("late.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, second.doc, &mut doc, "Also mine.");
    fs::write(rig.folder.join("late.md"), "Theirs.").unwrap();
    store.close(second.doc, Some((doc.to_text("\n"), edit)), None);
    assert_eq!(closed(&mut store, second.doc), News::Closed { doc: second.doc, beside: Some(PathBuf::from("late-2.md")), in_journal: false });
    assert_eq!((read(&rig.folder.join("late.md")), read(&rig.folder.join("late-2.md"))), ("Theirs.".to_string(), "Also mine.".to_string()));
    assert!(store.shutdown(SOON));
}

#[test]
fn a_file_renamed_while_we_were_down_is_found_again_by_recovery() {
    let rig = Rig::new("renamed-down");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved under the old name.");
    type_end(&mut store, id, &mut doc, " Typed, then the crash.");
    store.abandon();
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Renamed while down.md")).unwrap();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!((recovered[0].rel.as_path(), &recovered[0].how), (Path::new("Renamed while down.md"), &Recovery::BroughtBack));
    assert_eq!(read(&rig.folder.join("Renamed while down.md")), "Saved under the old name. Typed, then the crash.");
    assert_eq!(rig.listing(), ["Renamed while down.md"], "the old name must not be made again beside the renamed file");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_volume_without_the_one_step_rename_still_saves_and_still_refuses_to_overwrite() {
    let rig = Rig::new("old-rename");
    let tuning = quick();
    // Stands in for exFAT and FAT, where only the old rename exists.
    tuning.faults.no_one_step.store(1, Ordering::Relaxed);
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "First.");
    let path = rig.folder.join("essay.md");
    let made = fs::metadata(&path).unwrap().created().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    for n in 0..5 {
        let edit = type_end(&mut store, id, &mut doc, &format!(" Save {n}."));
        save_wait(&mut store, id, &doc, edit);
    }
    assert_eq!(read(&path), doc.to_text("\n"));
    assert_eq!(fs::metadata(&path).unwrap().created().unwrap(), made, "the file keeps its creation time across saves");
    assert_eq!(rig.listing(), ["essay.md"]);

    // Here any open handle on the file keeps the rename out, even one that shares everything.
    let reader = OpenOptions::new().read(true).share_mode(SHARE_ALL).open(&path).unwrap();
    let before = read(&path);
    let edit = type_end(&mut store, id, &mut doc, " While a reader had it.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::Busy });
    assert_eq!(read(&path), before);
    assert_eq!(rig.listing(), ["essay.md"]);
    drop(reader);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));

    // The comparison before the rename still catches another program's version.
    let edit = type_end(&mut store, id, &mut doc, " Mine.");
    fs::write(&path, "Theirs.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { clean: false, .. }));
    assert_eq!(read(&path), "Theirs.");
    store.resolve(id, Resolve::KeepMine);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&path), doc.to_text("\n"));

    // And recovery brings the file forward the same way.
    type_end(&mut store, id, &mut doc, " Unsaved at the crash.");
    store.abandon();
    let (store, recovered) = rig.open_with(tuning);
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::BroughtBack);
    assert_eq!(read(&path), doc.to_text("\n"));
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(store.shutdown(SOON));
}

// -------------------------------------------------------------- one instance

#[test]
fn two_stores_on_one_folder_exactly_one_gets_it() {
    let rig = Rig::new("two-stores");
    let (first, _) = rig.open();
    let mut spelled = rig.setup(quick());
    spelled.folder = PathBuf::from(format!("{}\\", rig.folder.to_string_lossy().to_uppercase()));
    for setup in [rig.setup(quick()), spelled] {
        match Store::open(setup) {
            Err(OpenError::Locked(owner)) => {
                assert_eq!(owner.pid, std::process::id());
                assert_eq!(owner.note, "test");
                assert_eq!(owner.folder, first.folder());
            }
            Ok(_) => panic!("a second store opened the same folder"),
            Err(other) => panic!("{other:?}"),
        }
    }
    assert!(first.shutdown(SOON));
    let (second, _) = rig.open();
    assert!(second.shutdown(SOON));
}

#[test]
fn two_instances_started_together_on_a_crashed_session_exactly_one_recovers() {
    for round in 0..8 {
        let rig = Rig::new("two-recover");
        let (mut store, _) = rig.open();
        let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
        type_end(&mut store, id, &mut doc, " Unsaved when it crashed.");
        store.abandon();

        let barrier = Arc::new(Barrier::new(2));
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = barrier.clone();
                    let setup = rig.setup(quick());
                    scope.spawn(move || {
                        barrier.wait();
                        let result = Store::open(setup);
                        // Hold the folder long enough for the other to be refused rather than queue up behind us.
                        std::thread::sleep(Duration::from_millis(150));
                        result.map(|(store, recovered)| {
                            store.shutdown(SOON);
                            recovered
                        })
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        let winners: Vec<_> = results.iter().filter_map(|result| result.as_ref().ok()).collect();
        assert_eq!(winners.len(), 1, "round {round}: {results:?}");
        assert_eq!(winners[0].len(), 1);
        assert_eq!(winners[0][0].how, Recovery::BroughtBack);
        assert!(results.iter().any(|result| matches!(result, Err(OpenError::Locked(_)))));
        assert_eq!(read(&rig.folder.join("essay.md")), "Saved. Unsaved when it crashed.");
        assert_eq!(rig.listing(), ["essay.md"], "recovered once, not twice");
    }
}

// ------------------------------------------------------------------- history

#[test]
fn versions_are_kept_listed_read_and_brought_back() {
    let rig = Rig::new("versions");
    let long = "A sentence of the essay as it stood. ".repeat(140);
    fs::write(rig.folder.join("essay.md"), &long).unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let id = opened.doc;
    let mut doc = Doc::from_text(&opened.text);
    assert!(store.versions(id).unwrap().is_empty());

    // Select all, type one letter, pause: the save that follows cuts the file to a letter.
    let edit = replace_all(&mut store, id, &mut doc, "x");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), "x");
    let versions = store.versions(id).unwrap();
    let cut = versions.iter().find(|v| v.label.as_deref() == Some("before-cut")).expect("a before-cut version");
    assert_eq!(cut.bytes, long.len() as u64);
    assert_eq!(store.read_version(id, &cut.name).unwrap(), long);
    assert!(versions.windows(2).all(|pair| pair[0].at_ms >= pair[1].at_ms), "newest first");

    // Ctrl S: kept with a label, and the answer carries the size read back from the disk.
    let edit = type_end(&mut store, id, &mut doc, " and a little more");
    store.keep(id, doc.to_text("\n"), edit, "kept");
    let News::Kept { bytes, edit: kept_edit, at_ms, .. } = expect(&mut store, "Kept", |news| matches!(news, News::Kept { .. })) else { unreachable!() };
    assert_eq!((bytes, kept_edit), (fs::metadata(rig.folder.join("essay.md")).unwrap().len(), edit));
    assert!(at_ms > 1_600_000_000_000);
    let versions = store.versions(id).unwrap();
    assert_eq!(versions[0].label.as_deref(), Some("kept"));
    assert_eq!(versions[0].bytes, bytes);
    assert_eq!(store.read_version(id, &versions[0].name).unwrap(), "x and a little more");

    // Bringing the long one back keeps the present text too.
    let old = store.bring_back(id, &cut.name, &doc.to_text("\n")).unwrap();
    assert_eq!(old, long);
    let edit = replace_all(&mut store, id, &mut doc, &old);
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), long);
    let versions = store.versions(id).unwrap();
    let replaced = versions.iter().find(|v| v.label.as_deref() == Some("replaced")).expect("the present text was kept");
    assert_eq!(store.read_version(id, &replaced.name).unwrap(), "x and a little more");

    assert_eq!(store.read_version(id, "20200101-000000.md").err(), Some(StoreError::NoSuchVersion));
    assert_eq!(store.read_version(id, "..\\origin.txt").err(), Some(StoreError::NoSuchVersion));
    assert!(store.shutdown(SOON));

    // The history is found again in a later session.
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let labels: Vec<Option<String>> = store.versions(opened.doc).unwrap().into_iter().map(|v| v.label).collect();
    for label in ["before-cut", "kept", "replaced"] {
        assert!(labels.contains(&Some(label.to_string())), "{labels:?}");
    }
    assert!(store.shutdown(SOON));
}

// ------------------------------------------------- rename, rewrite, discard

#[test]
fn a_text_is_renamed_with_its_journal_and_its_history() {
    let rig = Rig::new("rename");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Before the rename.");
    fs::write(rig.folder.join("other.md"), "another text").unwrap();
    assert_eq!(store.rename("essay.md", "other.md").err(), Some(StoreError::Name(NameError::Taken)));
    assert_eq!(store.rename("essay.md", "CON.md").err(), Some(StoreError::Name(NameError::Reserved)));
    assert_eq!(store.rename("missing.md", "x.md").err(), Some(StoreError::Missing));

    // Unsaved text at the moment of the rename travels in the journal.
    type_end(&mut store, id, &mut doc, " Typed just before.");
    store.rename("essay.md", "College\\The orchard.md").unwrap();
    assert!(!rig.folder.join("essay.md").exists());
    assert_eq!(read(&rig.folder.join("College").join("The orchard.md")), "Before the rename.");
    type_end(&mut store, id, &mut doc, " And after.");
    store.abandon();
    let (mut store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!((recovered[0].rel.as_path(), &recovered[0].how), (Path::new("College\\The orchard.md"), &Recovery::BroughtBack));
    assert_eq!(read(&rig.folder.join("College").join("The orchard.md")), "Before the rename. Typed just before. And after.");
    assert!(!rig.folder.join("essay.md").exists(), "the old name must not be recreated");

    // The history went with it.
    let opened = store.open_text("College\\The orchard.md").unwrap();
    assert!(!store.versions(opened.doc).unwrap().is_empty());
    // A text that is not open is simply moved.
    store.rename("other.md", "College\\Other.md").unwrap();
    assert_eq!(read(&rig.folder.join("College").join("Other.md")), "another text");
    assert!(store.shutdown(SOON));
}

#[test]
fn another_text_is_rewritten_only_if_it_still_reads_as_expected() {
    let rig = Rig::new("rewrite");
    fs::write(rig.folder.join("plan.md"), b"See [[essay]] first.\r\n").unwrap();
    let (mut store, _) = rig.open();
    assert_eq!(store.rewrite("plan.md", "something else", "x").err(), Some(StoreError::Changed));
    assert_eq!(fs::read(rig.folder.join("plan.md")).unwrap(), b"See [[essay]] first.\r\n");
    store.rewrite("plan.md", "See [[essay]] first.\n", "See [[The orchard]] first.\n").unwrap();
    assert_eq!(fs::read(rig.folder.join("plan.md")).unwrap(), b"See [[The orchard]] first.\r\n", "its line endings are kept");
    assert_eq!(rig.listing(), ["plan.md"]);
    assert_eq!(store.rewrite("missing.md", "", "x").err(), Some(StoreError::Missing));

    // The text as it was is a labelled version.
    let opened = store.open_text("plan.md").unwrap();
    let versions = store.versions(opened.doc).unwrap();
    let mend = versions.iter().find(|v| v.label.as_deref() == Some("before-mend")).expect("a before-mend version");
    assert_eq!(store.read_version(opened.doc, &mend.name).unwrap(), "See [[essay]] first.\n");
    // An open text is the interface's to change, through its buffer.
    assert_eq!(store.rewrite("plan.md", "See [[The orchard]] first.\n", "y").err(), Some(StoreError::AlreadyOpen(opened.doc)));
    assert!(store.shutdown(SOON));
}

#[test]
fn a_discarded_text_is_moved_never_erased_and_never_onto_another() {
    let rig = Rig::new("discard");
    let (mut store, _) = rig.open();
    for round in 0..3 {
        let (id, mut doc, _) = written(&mut store, "Drafts\\essay.md", &format!("Draft {round}."));
        let edit = type_end(&mut store, id, &mut doc, " Its last words.");
        let went = store.discard("Drafts\\essay.md", Some((doc.to_text("\n"), edit))).unwrap();
        let expected = if round == 0 { "essay.md".to_string() } else { format!("essay-{}.md", round + 1) };
        assert_eq!(went, PathBuf::from(".discarded").join("Drafts").join(&expected));
        assert_eq!(read(&rig.folder.join(&went)), format!("Draft {round}. Its last words."), "its last text went with it");
        assert!(!rig.folder.join("Drafts").join("essay.md").exists());
        closed(&mut store, id);
    }
    // A text that is not open goes the same way.
    fs::write(rig.folder.join("loose.txt"), "loose").unwrap();
    assert_eq!(store.discard("loose.txt", None).unwrap(), PathBuf::from(".discarded\\loose.txt"));
    assert_eq!(store.discard("loose.txt", None).err(), Some(StoreError::Missing));
    let kept: Vec<String> = fs::read_dir(rig.folder.join(".discarded").join("Drafts")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(kept.len(), 3, "{kept:?}");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

// ------------------------------------------------------ the folder as a tree

#[test]
fn the_folder_is_listed_its_texts_are_read_and_changes_outside_are_seen() {
    let rig = Rig::new("tree");
    fs::create_dir_all(rig.folder.join("College")).unwrap();
    fs::create_dir_all(rig.folder.join(".discarded")).unwrap();
    fs::write(rig.folder.join("b.md"), "bee [[a]]").unwrap();
    fs::write(rig.folder.join("A.txt"), "ay").unwrap();
    fs::write(rig.folder.join("College").join("The orchard.md"), "orchard\r\ntrees").unwrap();
    fs::write(rig.folder.join(".discarded").join("gone.md"), "gone").unwrap();
    fs::write(rig.folder.join(".hidden.md"), "hidden").unwrap();
    fs::write(rig.folder.join("picture.png"), "not a text").unwrap();
    let mut tuning = quick();
    tuning.scan_texts = true;
    let (mut store, _) = rig.open_with(tuning);
    let mut seen = Vec::new();
    let enough = |seen: &Vec<News>| seen.iter().filter(|news| matches!(news, News::FileText { .. })).count();
    let end = Instant::now() + SOON;
    while enough(&seen) < 3 && Instant::now() < end {
        wait_news(&mut store, &mut seen, Duration::from_millis(20), |_| false);
    }
    let News::Tree(tree) = seen.iter().find(|news| matches!(news, News::Tree(_))).expect("a tree") else { unreachable!() };
    let names: Vec<&str> = tree.root.texts.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["A.txt", "b.md"]);
    assert_eq!(tree.root.folders.len(), 1);
    assert_eq!(tree.root.folders[0].texts[0].rel, PathBuf::from("College\\The orchard.md"));
    assert_eq!(tree.find(Path::new("b.md")).map(|t| t.bytes), Some(9));
    let mut texts: Vec<(String, String)> = seen.iter().filter_map(|news| if let News::FileText { rel, text } = news { Some((rel.to_string_lossy().into_owned(), text.clone())) } else { None }).collect();
    texts.sort();
    assert_eq!(texts, [("A.txt".to_string(), "ay".to_string()), ("College\\The orchard.md".to_string(), "orchard\ntrees".to_string()), ("b.md".to_string(), "bee [[a]]".to_string())]);

    // A text appears from outside: the listing and its contents follow.
    fs::write(rig.folder.join("College").join("new.md"), "fresh").unwrap();
    assert_eq!(expect(&mut store, "FileText for the new text", |news| matches!(news, News::FileText { .. })), News::FileText { rel: PathBuf::from("College\\new.md"), text: "fresh".to_string() });
    // One changes: its contents are sent again.
    fs::write(rig.folder.join("b.md"), "bee, rewritten").unwrap();
    assert_eq!(expect(&mut store, "FileText for the changed text", |news| matches!(news, News::FileText { .. })), News::FileText { rel: PathBuf::from("b.md"), text: "bee, rewritten".to_string() });
    // One goes: the listing says so.
    fs::remove_file(rig.folder.join("A.txt")).unwrap();
    expect(&mut store, "a Tree without A.txt", |news| matches!(news, News::Tree(tree) if tree.find(Path::new("A.txt")).is_none() && tree.find(Path::new("College\\new.md")).is_some()));
    assert!(store.shutdown(SOON));
}

// --------------------------------------------------------------- state files

#[test]
fn state_files_are_remembered_across_sessions() {
    let rig = Rig::new("state");
    let (mut store, _) = rig.open();
    assert_eq!(store.read_state("window"), None);
    store.write_state("window", b"0 0 1920 1080 full");
    store.write_state("window", b"10 10 800 600 framed");
    store.write_state("last", b"College\\The orchard.md");
    store.write_state("..\\escape", b"no");
    assert_eq!(store.read_state("window").unwrap(), b"10 10 800 600 framed", "read back at once, before it is on disk");
    assert!(store.shutdown(SOON));
    let (store, _) = rig.open();
    assert_eq!(store.read_state("window").unwrap(), b"10 10 800 600 framed");
    assert_eq!(store.read_state("last").unwrap(), b"College\\The orchard.md");
    assert_eq!(store.read_state("..\\escape"), None);
    assert!(store.shutdown(SOON));
    assert!(!rig.data.join("escape").exists());

    let settings = rig.data.join("settings.txt");
    ok_store::write_atomic(&settings, b"measure 66\n").unwrap();
    ok_store::write_atomic(&settings, b"measure 72\n").unwrap();
    assert_eq!(fs::read(&settings).unwrap(), b"measure 72\n");
    assert_eq!(ok_store::write_atomic(&rig.data.join("no such folder").join("x"), b"x"), Err(3));
}

// ------------------------------------------------------- the journal's alarm

#[test]
fn a_failing_journal_raises_the_alarm_and_typing_and_saving_go_on() {
    let rig = Rig::new("alarm");
    let tuning = quick();
    let (mut store, _) = rig.open_with(tuning.clone());
    let (id, mut doc, _) = written(&mut store, "essay.md", "Journal working.");
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    let edit = type_end(&mut store, id, &mut doc, " Journal failing.");
    assert_eq!(expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    assert!(store.unwritten());
    // The real save is still made: it is then the best protection there is.
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), "Journal working. Journal failing.");
    let more = type_end(&mut store, id, &mut doc, " Typed during the failure.");
    assert_eq!(more, edit + 1);
    // The disk answers again: the journal catches up from memory and the alarm goes down.
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    assert_eq!(expect(&mut store, "the alarm going down", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(false));
    assert!(!store.unwritten());
    store.abandon();
    let (store, recovered) = rig.open_with(tuning);
    assert_eq!(recovered.len(), 1);
    assert_eq!(read(&rig.folder.join("essay.md")), "Journal working. Journal failing. Typed during the failure.");
    assert!(store.shutdown(SOON));
}

// ----------------------------------------------------------------- shutdown

#[test]
fn shutdown_returns_within_its_bound_and_what_could_not_be_saved_comes_back() {
    let rig = Rig::new("shutdown");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let path = rig.folder.join("essay.md");
    let holder = OpenOptions::new().write(true).share_mode(SHARE_ALL).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed, and the file is held by another program at exit.");
    store.close(id, Some((doc.to_text("\n"), edit)), Some(Pos { line: 0, col: 4 }));
    let started = Instant::now();
    let finished = store.shutdown(Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_millis(3500), "shutdown took {:?}", started.elapsed());
    assert!(finished);
    assert_eq!(read(&path), "Saved.");
    assert_eq!(rig.journals().len(), 1, "what could not be saved stays in the journal");
    drop(holder);
    let (mut store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::BroughtBack);
    assert_eq!(read(&path), "Saved. Typed, and the file is held by another program at exit.");
    assert_eq!(store.open_text("essay.md").unwrap().caret, Some(Pos { line: 0, col: 4 }));
    assert!(store.shutdown(SOON));

    // Dropped without a word: the same care is taken.
    let rig = Rig::new("shutdown-drop");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    type_end(&mut store, id, &mut doc, " Never asked to be saved.");
    drop(store);
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(read(&rig.folder.join("essay.md")), "Saved. Never asked to be saved.");
    assert!(store.shutdown(SOON));
}

#[test]
fn text_left_in_the_journal_at_close_is_the_text_when_it_is_opened_again() {
    let rig = Rig::new("left-in-journal");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let path = rig.folder.join("essay.md");
    let holder = OpenOptions::new().write(true).share_mode(SHARE_ALL).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed, and it could not be saved at close.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: true });
    drop(holder);
    // The same session, the same text: the buffer is what was typed, not what the file says.
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.text, "Saved. Typed, and it could not be saved at close.");
    assert_eq!(opened.restored, Some(9));
    assert_eq!(read(&path), "Saved.");
    store.save(opened.doc, opened.text.clone(), opened.edit);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: 0, .. }));
    assert_eq!(read(&path), "Saved. Typed, and it could not be saved at close.");
    store.close(opened.doc, None, None);
    assert_eq!(closed(&mut store, opened.doc), News::Closed { doc: opened.doc, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty(), "{:?}", rig.journals());
    let (store, recovered) = rig.open();
    assert!(recovered.is_empty(), "{recovered:?}");
    assert!(store.shutdown(SOON));
}

// -------------------------------------------------------- the keystroke path

#[test]
fn recording_an_edit_costs_microseconds() {
    const EDITS: usize = 100_000;
    let rig = Rig::new("keystroke");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    // The edits are made first, so that only `record` is timed.
    let mut changes = Vec::with_capacity(EDITS);
    let mut len = 0;
    for n in 0..EDITS {
        let piece = if n % 60 == 59 { "\n" } else if n % 6 == 5 { " " } else { "e" };
        let change = doc.replace(Span::at(doc.end()), piece);
        len += piece.len();
        changes.push((change, len));
    }
    assert_eq!(len, doc.len_bytes());
    let mut worst = Duration::ZERO;
    let started = Instant::now();
    let mut last = 0;
    for (n, (change, len)) in changes.iter().enumerate() {
        if n % 1000 == 0 {
            // One in a thousand is timed alone, to see the worst as well as the mean.
            let one = Instant::now();
            last = store.record(opened.doc, change, *len);
            worst = worst.max(one.elapsed());
        } else {
            last = store.record(opened.doc, change, *len);
        }
    }
    let total = started.elapsed();
    let mean_us = total.as_secs_f64() * 1e6 / EDITS as f64;
    println!("record(): {EDITS} calls in {total:?}, mean {mean_us:.3} microseconds, worst of 100 sampled singly {worst:?}");
    assert_eq!(last, EDITS as u64);
    assert!(mean_us < 50.0, "record() averaged {mean_us:.3} microseconds");
    // And every one of them reached the journal: kill it now and look.
    let end = Instant::now() + SOON;
    while store.unwritten() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!store.unwritten(), "the journal thread never caught up");
    store.abandon();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(read(&rig.folder.join("essay.md")), doc.to_text("\n"));
    assert!(store.shutdown(SOON));
}
