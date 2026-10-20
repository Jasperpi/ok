//! Attack, from outside the crate: what happens to the bytes of a file that
//! came from another program. Line endings of every kind, byte-order marks,
//! UTF-16 with and without a mark, and files that stop being UTF-8 while
//! they are open. The design says a file's line endings and byte-order mark
//! are kept as they were and that a file that is not UTF-8 is never
//! rewritten; each test here holds the store to one sentence of that.

use std::fs;
use std::path::Path;
use std::time::Duration;

use ok_store::testing::{replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{Eol, Format, News, ReadOnly, Recovery, Store};
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

/// The bytes of a file, shown as text with the line breaks spelled out.
fn shown(path: &Path) -> String {
    String::from_utf8_lossy(&fs::read(path).expect("the file can be read")).replace('\r', "\\r").replace('\n', "\\n")
}

/// Every version kept for any text under this rig's data directory, as text.
fn all_versions(rig: &Rig) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(docs) = fs::read_dir(rig.data.join("history")) else { return out };
    for doc in docs.flatten() {
        let Ok(files) = fs::read_dir(doc.path()) else { continue };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if name != "origin.txt" && name != "caret.txt" {
                out.push(String::from_utf8_lossy(&fs::read(file.path()).unwrap_or_default()).into_owned());
            }
        }
    }
    out
}

// ---------------------------------------------------------- what must hold

/// The plain cases, which hold: CRLF with a byte-order mark, through edits,
/// a close, a reopen and a crash, comes back byte for byte.
#[test]
fn crlf_and_a_byte_order_mark_survive_edits_a_close_a_reopen_and_a_crash() {
    let rig = Rig::new("ao-crlf");
    let path = rig.folder.join("win.txt");
    fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\n\r\nthree\r\n").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("win.txt").unwrap();
    assert_eq!((opened.format, opened.mixed_endings), (Format { eol: Eol::CrLf, bom: true }, false));
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, "four\n");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(shown(&path), "\u{FEFF}one\\r\\ntwo\\r\\n\\r\\nthree\\r\\nfour\\r\\n");
    store.close(opened.doc, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));

    let again = store.open_text("win.txt").unwrap();
    assert_eq!((again.format, again.mixed_endings), (Format { eol: Eol::CrLf, bom: true }, false));
    let mut doc = Doc::from_text(&again.text);
    type_end(&mut store, again.doc, &mut doc, "five");
    store.abandon();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].how, Recovery::BroughtBack);
    assert_eq!(shown(&path), "\u{FEFF}one\\r\\ntwo\\r\\n\\r\\nthree\\r\\nfour\\r\\nfive");
    assert!(store.shutdown(SOON));
}

/// A file with a byte-order mark and nothing else, an empty file, and a file
/// that is one line break: each is still itself after being opened, typed
/// in, and having the typing taken out again.
#[test]
fn the_smallest_files_come_back_as_they_were() {
    let rig = Rig::new("ao-small");
    let cases: [(&str, &[u8]); 4] = [("mark.md", b"\xEF\xBB\xBF"), ("empty.md", b""), ("crlf.md", b"\r\n"), ("lf.md", b"\n")];
    for (name, bytes) in cases {
        fs::write(rig.folder.join(name), bytes).unwrap();
    }
    let (mut store, _) = rig.open();
    for (name, bytes) in cases {
        let opened = store.open_text(name).unwrap();
        let mut doc = Doc::from_text(&opened.text);
        let edit = type_end(&mut store, opened.doc, &mut doc, "x");
        save_wait(&mut store, opened.doc, &doc, edit);
        let edit = replace_all(&mut store, opened.doc, &mut doc, &opened.text);
        save_wait(&mut store, opened.doc, &doc, edit);
        assert_eq!(fs::read(rig.folder.join(name)).unwrap(), bytes, "{name}");
    }
    assert!(store.shutdown(SOON));
}

// ------------------------------------------------- mixed line endings (fail)

/// DESIGN.md: "A file's line endings and byte-order mark are kept as they
/// were." A file with both kinds of line break has one word typed at its end
/// and is saved. The breaks nobody touched must still be what they were.
#[test]
fn mixed_line_endings_survive_a_save_untouched() {
    let rig = Rig::new("ao-mixed");
    let path = rig.folder.join("mixed.md");
    fs::write(&path, b"one\r\ntwo\r\nthree\nfour\r\n").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("mixed.md").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, "five");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(shown(&path), "one\\r\\ntwo\\r\\nthree\\nfour\\r\\nfive", "the line break after 'three' was not his to change");
    assert!(store.shutdown(SOON));
}

/// The same for a file whose line breaks are lone carriage returns (classic
/// Mac files, and what some exporters still write).
#[test]
fn lone_carriage_returns_survive_a_save_untouched() {
    let rig = Rig::new("ao-cr");
    let path = rig.folder.join("oldmac.txt");
    fs::write(&path, b"one\rtwo\rthree").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("oldmac.txt").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " four");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(shown(&path), "one\\rtwo\\rthree four");
    assert!(store.shutdown(SOON));
}

/// Ctrl S on a text nobody has typed in keeps a version and answers with
/// the byte count. It must not change the file: nothing was edited.
#[test]
fn keeping_a_version_of_an_untouched_text_does_not_rewrite_its_file() {
    let rig = Rig::new("ao-keep-untouched");
    let path = rig.folder.join("mixed.md");
    let original = b"one\r\ntwo\r\nthree\nfour\r\n";
    fs::write(&path, original).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("mixed.md").unwrap();
    store.keep(opened.doc, opened.text.clone(), opened.edit, "kept");
    let News::Kept { bytes, .. } = expect(&mut store, "Kept", |news| matches!(news, News::Kept { .. })) else { unreachable!() };
    assert_eq!(fs::read(&path).unwrap(), original, "nothing was typed, yet the file now reads {}", shown(&path));
    assert_eq!(bytes, original.len() as u64);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    assert!(store.shutdown(SOON));
}

/// Another program puts a version with mixed endings in the file while the
/// buffer is clean. The interface takes it the way `News::ChangedOutside`
/// says to. Taking the other program's version must not rewrite it.
#[test]
fn taking_an_outside_version_with_mixed_endings_rewrites_nothing() {
    let rig = Rig::new("ao-take-mixed");
    let path = rig.folder.join("essay.md");
    fs::write(&path, b"mine\n").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    let theirs = b"theirs\r\nline two\r\nline three\nline four\r\n";
    fs::write(&path, theirs).unwrap();
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert!(clean);
    let edit = replace_all(&mut store, opened.doc, &mut doc, &text);
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(fs::read(&path).unwrap(), theirs, "the outside version was rewritten as {}", shown(&path));
    assert!(store.shutdown(SOON));
}

// ----------------------------------------------------------- UTF-16 (fail)

/// A UTF-16 file with no byte-order mark (what `cmd /u /c echo` writes).
/// Every second byte is zero, which happens to be valid UTF-8, so it is
/// opened as an ordinary text. Its line breaks are CR 00 LF 00; the store
/// reads the CR as a lone line break and writes LF 00 LF 00, so every line
/// break in the file is doubled for any program that reads it as UTF-16.
/// The file must either open for reading only or be written back unharmed.
#[test]
fn utf16_without_a_byte_order_mark_is_never_rewritten_wrongly() {
    let rig = Rig::new("ao-utf16");
    let path = rig.folder.join("wide.txt");
    let mut original = Vec::new();
    for unit in "first line\r\nsecond line\r\n".encode_utf16() {
        original.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(&path, &original).unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("wide.txt").unwrap();
    if opened.read_only == Some(ReadOnly::NotUtf8) {
        assert!(store.shutdown(SOON));
        return;
    }
    // Ctrl S, with nothing typed.
    store.keep(opened.doc, opened.text.clone(), opened.edit, "kept");
    listen(&mut store, Duration::from_millis(400));
    let now = fs::read(&path).unwrap();
    let breaks = |bytes: &[u8]| bytes.chunks_exact(2).filter(|unit| unit == &[0x0A, 0x00]).count();
    assert_eq!(breaks(&now), breaks(&original), "the file had {} line feeds as UTF-16 and now has {}", breaks(&original), breaks(&now));
    assert_eq!(now, original);
    assert!(store.shutdown(SOON));
}

/// UTF-16 with a mark, both byte orders, and Latin-1: opened for reading
/// only, and no call that writes ever touches the file. This holds.
#[test]
fn files_that_are_not_utf8_are_untouched_by_save_keep_close_and_rewrite() {
    let rig = Rig::new("ao-notutf8");
    let mut le = vec![0xFF, 0xFE];
    let mut be = vec![0xFE, 0xFF];
    for unit in "wide \u{00E9}\r\ntext".encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
        be.extend_from_slice(&unit.to_be_bytes());
    }
    let latin = b"caf\xE9\r\nau lait\n".to_vec();
    let cases = [("le.txt", le), ("be.txt", be), ("latin.md", latin)];
    for (name, bytes) in &cases {
        fs::write(rig.folder.join(name), bytes).unwrap();
    }
    let (mut store, _) = rig.open();
    for (name, bytes) in &cases {
        assert_eq!(store.rewrite(name, "anything", "else").err(), Some(ok_store::StoreError::ReadOnly), "{name}");
        let opened = store.open_text(name).unwrap();
        assert_eq!(opened.read_only, Some(ReadOnly::NotUtf8), "{name}");
        let mut doc = Doc::from_text(&opened.text);
        let edit = type_end(&mut store, opened.doc, &mut doc, " typed anyway");
        store.save(opened.doc, doc.to_text("\n"), edit);
        store.keep(opened.doc, doc.to_text("\n"), edit, "kept");
        listen(&mut store, Duration::from_millis(150));
        assert_eq!(&fs::read(rig.folder.join(name)).unwrap(), bytes, "{name} after save and keep");
        store.close(opened.doc, Some((doc.to_text("\n"), edit)), None);
        let News::Closed { beside, .. } = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. })) else { unreachable!() };
        assert!(beside.is_some(), "what was typed goes beside the file");
        assert_eq!(&fs::read(rig.folder.join(name)).unwrap(), bytes, "{name} after close");
    }
    assert!(store.shutdown(SOON));
}

// ------------------------------------ the file stops being UTF-8 (fail)

/// Research design, section 8: when the bytes on disk differ and the buffer
/// has no unsaved edits, "snapshot the old text (before-reload), load the
/// new bytes". The store does that when the new bytes are UTF-8. When they
/// are not, it reports the change (clean, read-only) and keeps nothing, so
/// once the interface shows the newer file the text he had saved is in no
/// file, no version and no journal.
#[test]
fn a_file_replaced_by_one_that_is_not_utf8_keeps_what_we_had_as_a_version() {
    let rig = Rig::new("ao-notutf8-reload");
    let path = rig.folder.join("essay.md");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "The first save of the sitting.");
    save_wait(&mut store, opened.doc, &doc, edit);
    let edit = type_end(&mut store, opened.doc, &mut doc, " Twenty minutes of writing after it, saved too.");
    save_wait(&mut store, opened.doc, &doc, edit);
    let mine = doc.to_text("\n");

    // Another program writes the file in Latin-1.
    fs::write(&path, b"r\xE9sum\xE9 from somewhere else").unwrap();
    let News::ChangedOutside { clean, read_only, text, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((clean, read_only), (true, Some(ReadOnly::NotUtf8)));
    // The interface shows the newer file, as it is told to.
    replace_all(&mut store, opened.doc, &mut doc, &text);
    store.close(opened.doc, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    assert!(store.shutdown(SOON));

    let kept = all_versions(&rig);
    assert!(kept.iter().any(|version| version == &mine), "the text he had saved is gone from the file and was kept nowhere; versions are {kept:?}");
}

// ----------------------------- the format after an outside change (fail)

/// The file had LF endings. Another program rewrites it with CRLF; the
/// buffer is clean, the interface takes the new version, he types, a save
/// goes through (in CRLF, correctly), he types a little more and the
/// process is killed. Recovery must bring the file forward in the line
/// endings it has, not in the ones it had when the text was first opened.
#[test]
fn recovery_writes_the_line_endings_the_file_last_had() {
    let rig = Rig::new("ao-format-crash");
    let path = rig.folder.join("essay.md");
    fs::write(&path, b"one\ntwo\n").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    // Something typed and saved first, so that the journal file exists.
    let edit = type_end(&mut store, opened.doc, &mut doc, "three\n");
    save_wait(&mut store, opened.doc, &doc, edit);

    fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\nthree\r\nfour\r\n").unwrap();
    let News::ChangedOutside { text, clean, format, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((clean, format), (true, Format { eol: Eol::CrLf, bom: true }));
    let edit = replace_all(&mut store, opened.doc, &mut doc, &text);
    save_wait(&mut store, opened.doc, &doc, edit);
    let edit = type_end(&mut store, opened.doc, &mut doc, "five\n");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(shown(&path), "\u{FEFF}one\\r\\ntwo\\r\\nthree\\r\\nfour\\r\\nfive\\r\\n", "a live save keeps the new format");

    type_end(&mut store, opened.doc, &mut doc, "six");
    store.abandon();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(shown(&path), "\u{FEFF}one\\r\\ntwo\\r\\nthree\\r\\nfour\\r\\nfive\\r\\nsix", "recovery did {:?}", recovered[0].how);
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(store.shutdown(SOON));
}
