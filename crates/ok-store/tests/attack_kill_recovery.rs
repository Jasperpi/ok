//! Attacks on recovery, made from outside the crate: journals left by a
//! simulated kill and then cut short at every byte, corrupted, or forged
//! outright; the file in the folder put into every state a power cut could
//! leave it in; a journal another program is holding when the store starts.
//! Every test works in temporary folders of its own.
//!
//! The journal's byte format is written out again here, by hand, because the
//! crate keeps its own writer private: a forged journal has to be made by
//! something other than the code under test anyway.

// Holds files open the way Windows programs do; there is no such thing elsewhere.
#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, Hold, News, Recovered, Recovery, Store};
use ok_text::{Doc, Span};

const SOON: Duration = Duration::from_secs(10);
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;
const SHARE_ALL: u32 = 7;

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).expect("the file can be read")).expect("the file is UTF-8")
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

/// A new text holding `text`, saved.
fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
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
                let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned();
                out.push((rel, fs::read(&path).unwrap_or_default()));
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

/// Opens the store, which runs recovery, and shuts it down again.
fn recover(rig: &Rig) -> Vec<Recovered> {
    let (store, report) = rig.open();
    assert!(store.shutdown(SOON), "shutdown after recovery timed out");
    report
}

// ------------------------------------------------- the journal format, by hand

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn state(bytes: &[u8]) -> (u64, u64) {
    (bytes.len() as u64, fnv1a(bytes))
}

const KIND_OPEN: u8 = 1;
const KIND_EDIT: u8 = 2;
const KIND_BEGIN: u8 = 3;
const KIND_DONE: u8 = 4;
const HEADER: usize = 24;

fn journal_name(docid: u64, generation: u32) -> String {
    format!("{docid:016x}.{generation}.wal")
}

/// What a forged `Open` record says.
struct OpenSpec {
    on_disk: bool,
    disk: (u64, u64),
    index: u64,
    known: Vec<(u64, u64)>,
    folder: Vec<u16>,
    rel: Vec<u16>,
    text: String,
}

impl OpenSpec {
    fn new(folder: &Path, rel: &str, text: &str, on_disk: bool) -> OpenSpec {
        OpenSpec { on_disk, disk: state(text.as_bytes()), index: 0, known: Vec::new(), folder: folder.as_os_str().encode_wide().collect(), rel: rel.encode_utf16().collect(), text: text.to_string() }
    }

    fn payload(&self) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&1_791_729_125_000u64.to_le_bytes());
        p.extend_from_slice(&[self.on_disk as u8, 0, 0]);
        p.extend_from_slice(&self.disk.0.to_le_bytes());
        p.extend_from_slice(&self.disk.1.to_le_bytes());
        p.extend_from_slice(&(self.text.len() as u64).to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&self.index.to_le_bytes());
        p.extend_from_slice(&(self.known.len() as u16).to_le_bytes());
        for (len, hash) in &self.known {
            p.extend_from_slice(&len.to_le_bytes());
            p.extend_from_slice(&hash.to_le_bytes());
        }
        for path in [&self.folder, &self.rel] {
            p.extend_from_slice(&(path.len() as u32).to_le_bytes());
            for unit in path {
                p.extend_from_slice(&unit.to_le_bytes());
            }
        }
        p.push(0);
        p.extend_from_slice(self.text.as_bytes());
        p
    }

    /// How many bytes of the payload come before the text.
    fn text_at(&self) -> usize {
        self.payload().len() - self.text.len()
    }
}

/// A journal file built record by record.
struct Forged {
    bytes: Vec<u8>,
    seq: u64,
}

impl Forged {
    fn new(docid: u64, generation: u32) -> Forged {
        let mut bytes = b"OKJ1\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&docid.to_le_bytes());
        bytes.extend_from_slice(&generation.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        Forged { bytes, seq: 0 }
    }

    fn record(&mut self, kind: u8, payload: &[u8]) -> &mut Forged {
        let mut body = vec![kind];
        body.extend_from_slice(&self.seq.to_le_bytes());
        body.extend_from_slice(payload);
        self.bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(&crc32(&body).to_le_bytes());
        self.bytes.extend_from_slice(&body);
        self.seq += 1;
        self
    }

    fn open(&mut self, spec: &OpenSpec) -> &mut Forged {
        self.record(KIND_OPEN, &spec.payload())
    }

    /// An edit record with every field given as is, fitting or not.
    fn edit(&mut self, edit: u64, start: (u32, u32), old_end: (u32, u32), removed: u32, len_after: u64, inserted: &str) -> &mut Forged {
        let mut p = Vec::new();
        p.extend_from_slice(&edit.to_le_bytes());
        for n in [start.0, start.1, old_end.0, old_end.1, removed] {
            p.extend_from_slice(&n.to_le_bytes());
        }
        p.extend_from_slice(&len_after.to_le_bytes());
        p.extend_from_slice(inserted.as_bytes());
        self.record(KIND_EDIT, &p)
    }

    /// An honest edit: `piece` typed at the end of `doc`.
    fn typed(&mut self, doc: &mut Doc, edit: u64, piece: &str) -> &mut Forged {
        let change = doc.replace(Span::at(doc.end()), piece);
        let at = |p: ok_text::Pos| (p.line as u32, p.col as u32);
        self.edit(edit, at(change.start), at(change.old_end), change.removed.len() as u32, doc.len_bytes() as u64, &change.inserted)
    }

    fn put(&self, rig: &Rig, name: &str) {
        fs::create_dir_all(rig.data.join("journal")).unwrap();
        fs::write(rig.data.join("journal").join(name), &self.bytes).unwrap();
    }
}

/// One record of a journal file as found by walking its length fields.
struct Rec {
    kind: u8,
    end: usize,
    /// The edit number an Edit, Begin or Done record carries.
    edit: u64,
}

fn records(bytes: &[u8]) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut at = HEADER;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if len < 9 || at + 8 + len > bytes.len() {
            break;
        }
        let kind = bytes[at + 8];
        let payload = &bytes[at + 17..at + 8 + len];
        let edit = if matches!(kind, KIND_EDIT | KIND_BEGIN | KIND_DONE) && payload.len() >= 8 { u64::from_le_bytes(payload[..8].try_into().unwrap()) } else { 0 };
        at += 8 + len;
        out.push(Rec { kind, end: at, edit });
    }
    out
}

/// A rig whose writing folder already holds `essay.md`, and that folder as
/// the store will spell it.
fn rig_with(name: &str, essay: Option<&str>) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    if let Some(text) = essay {
        fs::write(rig.folder.join("essay.md"), text).unwrap();
    }
    let root = fs::canonicalize(&rig.folder).unwrap();
    (rig, root)
}

// --------------------------------------------------------------- real sessions

/// The first character of a text can be U+FEFF (it comes along when text is
/// copied out of some web pages and files). The journal must replay such a
/// text like any other.
#[test]
fn a_text_that_begins_with_a_zero_width_no_break_space_keeps_what_is_typed_after_a_rotation() {
    let rig = Rig::new("attack-feff");
    let mut tuning = quick();
    tuning.rotate_at = 1;
    let (mut store, _) = rig.open_with(tuning);
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "\u{FEFF}Pasted from a web page.");
    // This save starts a new journal file whose first record holds the text.
    save_wait(&mut store, opened.doc, &doc, edit);
    type_end(&mut store, opened.doc, &mut doc, " Then a paragraph he typed himself");
    type_end(&mut store, opened.doc, &mut doc, " and never got to save.");
    settle(&mut store);
    store.abandon();

    let typed = doc.to_text("\n");
    let report = recover(&rig);
    let after = read(&rig.folder.join("essay.md"));
    assert_eq!(after, typed, "the journal held every edit, yet recovery ({report:?}) left the file without them");
}

/// A scanner or backup tool that happens to have the journal open at the
/// moment the store starts must not make the store forget the journal.
#[test]
fn a_journal_another_program_holds_at_start_is_still_brought_back_or_spoken_of() {
    let rig = Rig::new("attack-busy-journal-1");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved before the crash.");
    type_end(&mut store, id, &mut doc, " Typed after the last save.");
    settle(&mut store);
    store.abandon();

    let journals = rig.journals();
    assert_eq!(journals.len(), 1, "{journals:?}");
    let scanner = OpenOptions::new().read(true).share_mode(SHARE_ALL).open(rig.data.join("journal").join(&journals[0])).unwrap();
    let (mut store, report) = rig.open();
    drop(scanner);
    let opened = store.open_text("essay.md").unwrap();
    let told = !report.is_empty() || opened.restored.is_some();
    let shown = opened.text.contains("Typed after the last save.");
    store.abandon();
    assert!(told || shown, "after a crash the text opens as {:?} with no word that anything is waiting (report {report:?}, restored {:?})", opened.text, opened.restored);
}

/// The same start, followed by ordinary work and a second crash: the first
/// session's unsaved text must still exist somewhere.
#[test]
fn a_journal_another_program_holds_at_start_survives_the_next_crash() {
    let rig = Rig::new("attack-busy-journal-2");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved before the crash.");
    type_end(&mut store, id, &mut doc, " First session, never saved.");
    settle(&mut store);
    store.abandon();

    let journals = rig.journals();
    assert_eq!(journals.len(), 1, "{journals:?}");
    let scanner = OpenOptions::new().read(true).share_mode(SHARE_ALL).open(rig.data.join("journal").join(&journals[0])).unwrap();
    let (mut store, _) = rig.open();
    drop(scanner);
    let opened = store.open_text("essay.md").unwrap();
    let mut doc = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc, " Second session.");
    settle(&mut store);
    let between = rig.journals();
    store.abandon();

    let report = recover(&rig);
    let texts: Vec<(String, String)> = files_under(&rig.folder).into_iter().map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned())).collect();
    assert!(
        texts.iter().any(|(_, text)| text.contains("First session, never saved.")),
        "the first session's journal ({journals:?}, then {between:?}) was thrown away: recovery said {report:?} and the folder holds {texts:?}"
    );
}

/// A session whose journal is then cut at every byte, as a power cut that
/// lost the unflushed tail would cut it, against every state the file may
/// have been left in. Whatever the cut, the file never goes backwards, never
/// holds anything but a whole state, and holds everything the journal still
/// vouches for whenever the journal can tell the file is ours.
#[test]
fn a_journal_cut_at_every_byte_against_every_state_of_the_file() {
    let rig = Rig::new("attack-cut");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    // states[n] is the text after n edits.
    let mut states = vec![String::new()];
    let mut saved = Vec::new();
    for round in 0..4 {
        let mut edit = 0;
        for n in 0..3 {
            edit = type_end(&mut store, opened.doc, &mut doc, &format!(" r{round}w{n}\u{00E9}"));
            states.push(doc.to_text("\n"));
        }
        save_wait(&mut store, opened.doc, &doc, edit);
        saved.push(edit as usize);
    }
    for n in 0..3 {
        type_end(&mut store, opened.doc, &mut doc, &format!(" tail{n}\n"));
        states.push(doc.to_text("\n"));
    }
    settle(&mut store);
    store.abandon();

    let names = rig.journals();
    assert_eq!(names.len(), 1, "{names:?}");
    let journal_path = rig.data.join("journal").join(&names[0]);
    let whole = fs::read(&journal_path).unwrap();
    let recs = records(&whole);
    assert_eq!(recs.iter().filter(|r| r.kind == KIND_EDIT).count(), states.len() - 1, "one edit record per edit");
    assert_eq!(recs.iter().filter(|r| r.kind == KIND_BEGIN).count(), saved.len());
    let open_end = recs[0].end;
    let state_of = |text: &str| states.iter().position(|s| s == text);

    let mut tried = 0;
    let mut planted_states: Vec<Option<usize>> = saved.iter().map(|&s| Some(s)).collect();
    planted_states.push(None);
    for (which, planted) in planted_states.iter().enumerate() {
        // Every byte for the file as the kill left it; around every record
        // boundary for the others.
        let every_byte = which + 2 == planted_states.len();
        let cuts: Vec<usize> = if every_byte { (0..=whole.len()).collect() } else { recs.iter().flat_map(|r| [r.end - 1, r.end, r.end + 1]).filter(|&c| c <= whole.len()).chain([0, HEADER - 1, HEADER]).collect() };
        for cut in cuts {
            empty_dir(&rig.folder);
            empty_dir(&rig.data.join("journal"));
            if let Some(p) = planted {
                fs::write(rig.folder.join("essay.md"), &states[*p]).unwrap();
            }
            fs::write(&journal_path, &whole[..cut]).unwrap();
            let report = recover(&rig);
            tried += 1;

            let here = format!("cut {cut} of {}, file planted at {planted:?}, report {report:?}", whole.len());
            let files = files_under(&rig.folder);
            let essay = files.iter().find(|(name, _)| name == "essay.md").map(|(_, bytes)| String::from_utf8(bytes.clone()).expect("whole UTF-8"));
            let beside: Vec<&(String, Vec<u8>)> = files.iter().filter(|(name, _)| name != "essay.md").collect();
            for (name, bytes) in &beside {
                assert!(name.contains("(recovered "), "{here}: a stray file {name}");
                let text = String::from_utf8(bytes.clone()).expect("whole UTF-8");
                assert!(state_of(&text).is_some(), "{here}: {name} holds no whole state");
            }
            let got = essay.as_deref().map(|text| state_of(text).unwrap_or_else(|| panic!("{here}: essay.md holds no whole state: {text:?}")));

            if cut < open_end {
                // No whole Open record: the journal says nothing and nothing may change.
                assert_eq!(got, *planted, "{here}");
                assert!(beside.is_empty(), "{here}");
                continue;
            }
            let inside: Vec<&Rec> = recs.iter().filter(|r| r.end <= cut).collect();
            let edits = inside.iter().filter(|r| r.kind == KIND_EDIT).count();
            let done = inside.iter().filter(|r| r.kind == KIND_DONE).map(|r| r.edit as usize).next_back();
            let unsaved = done.is_none_or(|d| edits > d);
            match planted {
                None => {
                    // Never saved, or typed in since: written back. Saved and gone: left gone.
                    assert_eq!(got, if unsaved { Some(edits) } else { None }, "{here}");
                    assert!(beside.is_empty(), "{here}");
                }
                Some(p) => {
                    let begun = inside.iter().any(|r| r.kind == KIND_BEGIN && r.edit as usize == *p);
                    if *p == edits || begun {
                        // The journal vouches for the file: it is ours, merely behind.
                        assert_eq!(got, Some(edits), "{here}: the file is one of our own saves and must be brought forward");
                        assert!(beside.is_empty(), "{here}: our own older save was taken for someone else's");
                    } else {
                        // The journal lost the record of this save: the file is left alone.
                        assert_eq!(got, Some(*p), "{here}: a file the journal cannot vouch for was touched");
                        assert!(*p >= edits, "{here}: a save can only be unknown to a journal that ends before it");
                    }
                }
            }
            assert!(rig.journals().is_empty() || !report.is_empty(), "{here}: a journal was kept without a word: {:?}", rig.journals());
        }
    }
    println!("journal cut test: {tried} recoveries over a journal of {} bytes and {} records", whole.len(), recs.len());
}

/// One byte of the journal damaged somewhere, anywhere: recovery must still
/// start, and the file must not go backwards or stop being a whole state.
#[test]
fn a_journal_with_a_damaged_byte_anywhere_never_harms_the_file() {
    let rig = Rig::new("attack-damage");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let mut states = vec![String::new()];
    let mut last_saved = 0;
    for round in 0..3 {
        let mut edit = 0;
        for n in 0..4 {
            edit = type_end(&mut store, opened.doc, &mut doc, &format!(" r{round}w{n}"));
            states.push(doc.to_text("\n"));
        }
        save_wait(&mut store, opened.doc, &doc, edit);
        last_saved = edit as usize;
    }
    for n in 0..4 {
        type_end(&mut store, opened.doc, &mut doc, &format!(" tail{n}"));
        states.push(doc.to_text("\n"));
    }
    settle(&mut store);
    store.abandon();
    let names = rig.journals();
    let journal_path = rig.data.join("journal").join(&names[0]);
    let whole = fs::read(&journal_path).unwrap();
    let state_of = |text: &str| states.iter().position(|s| s == text);

    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    // Every byte of the header and of the first record's frame, then random places.
    let mut places: Vec<usize> = (0..HEADER + 17).collect();
    places.extend((0..260).map(|_| next(whole.len())));
    let mut brought_forward = 0;
    let mut beside = 0;
    let tried = places.len();
    for at in places {
        let mut damaged = whole.clone();
        damaged[at] ^= 1 << next(8);
        empty_dir(&rig.folder);
        empty_dir(&rig.data.join("journal"));
        fs::write(rig.folder.join("essay.md"), &states[last_saved]).unwrap();
        fs::write(&journal_path, &damaged).unwrap();
        let report = recover(&rig);
        let here = format!("byte {at} of {} damaged, report {report:?}", whole.len());
        for (name, bytes) in files_under(&rig.folder) {
            let text = String::from_utf8(bytes).unwrap_or_else(|_| panic!("{here}: {name} is not whole UTF-8"));
            let n = state_of(&text).unwrap_or_else(|| panic!("{here}: {name} holds no whole state: {text:?}"));
            if name == "essay.md" {
                assert!(n >= last_saved, "{here}: the file went back from state {last_saved} to {n}");
                brought_forward += (n > last_saved) as usize;
            } else {
                assert!(name.contains("(recovered "), "{here}: a stray file {name}");
                beside += 1;
            }
        }
        assert!(rig.folder.join("essay.md").exists(), "{here}: the file is gone");
    }
    // Damage after the last save's record leaves that record readable, so the
    // file is still known as ours and what was typed before the damage comes back.
    assert!(brought_forward > 0, "no damaged journal brought anything forward");
    println!("damaged journal test: {tried} journals with one damaged byte; the file was brought forward in {brought_forward}, an older text was put beside it in {beside}");
}

/// A drive that did not keep its promise can hand back a file several saves
/// old. The journal remembers its own recent saves across rotations so that
/// such a file is still known as ours.
#[test]
fn a_file_set_back_several_saves_is_still_known_as_ours_after_rotations() {
    let rig = Rig::new("attack-known");
    let mut tuning = quick();
    tuning.rotate_at = 1;
    let (mut store, _) = rig.open_with(tuning);
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let mut saves = Vec::new();
    for k in 0..22 {
        let edit = type_end(&mut store, opened.doc, &mut doc, &format!(" save{k}"));
        save_wait(&mut store, opened.doc, &doc, edit);
        saves.push(doc.to_text("\n"));
    }
    type_end(&mut store, opened.doc, &mut doc, " and an unsaved tail.");
    settle(&mut store);
    // The last rotation is finished once a flush has been answered behind it.
    store.flush();
    std::thread::sleep(Duration::from_millis(150));
    store.abandon();
    let last = doc.to_text("\n");
    let journals: Vec<(String, Vec<u8>)> = files_under(&rig.data.join("journal")).into_iter().filter(|(name, _)| name.contains(".wal")).collect();
    assert_eq!(journals.len(), 1, "{:?}", journals.iter().map(|(n, _)| n).collect::<Vec<_>>());

    let mut window = 0;
    for back in 0..saves.len() {
        empty_dir(&rig.folder);
        empty_dir(&rig.data.join("journal"));
        fs::write(rig.folder.join("essay.md"), &saves[saves.len() - 1 - back]).unwrap();
        for (name, bytes) in &journals {
            fs::write(rig.data.join("journal").join(name), bytes).unwrap();
        }
        let report = recover(&rig);
        let files = files_under(&rig.folder);
        let texts: Vec<String> = files.iter().map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned()).collect();
        assert!(texts.contains(&last), "file set back {back} saves: what was typed is in no file ({report:?}, {:?})", files.iter().map(|(n, _)| n).collect::<Vec<_>>());
        let ours = files.len() == 1 && report.iter().all(|r| r.how == Recovery::BroughtBack);
        if ours && window == back {
            window = back + 1;
        }
        if back < 15 {
            assert!(ours, "file set back {back} saves was not recognised as our own: {report:?}");
        }
    }
    println!("known-states test: a file set back by up to {} saves is recognised as ours", window - 1);
}

/// Another program keeps the file from being replaced, across a kill, a
/// second kill before he opens the text, a third just after he has, and a
/// fourth after more typing. Nothing typed may go missing on the way.
#[test]
fn a_text_held_by_another_program_through_four_kills_comes_back_whole() {
    let rig = Rig::new("attack-held");
    let path = rig.folder.join("essay.md");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    // Open for reading, refusing to share delete: nothing can be renamed over it.
    let holder = OpenOptions::new().read(true).share_mode(SHARE_READ | SHARE_WRITE).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed while it was held.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let held = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Held { why: Hold::Busy, .. }));
    assert!(held.is_some(), "{seen:?}");
    settle(&mut store);
    store.abandon();
    let typed = doc.to_text("\n");

    // First restart, the holder still there: nothing can be written yet.
    let (store, report) = rig.open();
    assert_eq!(report.iter().map(|r| &r.how).collect::<Vec<_>>(), [&Recovery::Waiting], "{report:?}");
    assert_eq!(read(&path), "Saved.");
    store.abandon();

    // Second restart: the journal is still there and still says the same.
    let (mut store, report) = rig.open();
    assert_eq!(report.iter().map(|r| &r.how).collect::<Vec<_>>(), [&Recovery::Waiting], "{report:?}");
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.text, typed);
    assert!(opened.restored.is_some_and(|words| words > 0), "{:?}", opened.restored);
    // Killed the moment the text has come back.
    store.abandon();

    let (mut store, report) = rig.open();
    assert_eq!(report.iter().map(|r| &r.how).collect::<Vec<_>>(), [&Recovery::Waiting], "{report:?}");
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.text, typed);
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " And more after the restart.");
    store.save(opened.doc, doc.to_text("\n"), edit);
    let held = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Held { why: Hold::Busy, .. }));
    assert!(held.is_some(), "{seen:?}");
    settle(&mut store);
    store.abandon();
    let typed = doc.to_text("\n");
    assert_eq!(read(&path), "Saved.", "the held file was never touched");

    drop(holder);
    let report = recover(&rig);
    assert_eq!(report.iter().map(|r| &r.how).collect::<Vec<_>>(), [&Recovery::BroughtBack], "{report:?}");
    assert_eq!(read(&path), typed);
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(rig.journals().is_empty(), "{:?}", rig.journals());
}

/// After a crash he renames the file in Explorer before starting the program
/// again, and at that start another program has the file open so that it
/// cannot be replaced. Recovery finds the file under its new name and says
/// the text is waiting there: "opening the text brings back what was typed".
/// Opening it must do that.
#[test]
fn text_waiting_for_a_file_found_under_another_name_comes_back_when_that_file_is_opened() {
    let rig = Rig::new("attack-waiting-moved");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved before the crash.");
    type_end(&mut store, id, &mut doc, " Typed after the last save.");
    settle(&mut store);
    store.abandon();
    let typed = doc.to_text("\n");

    let moved = rig.folder.join("draft.md");
    fs::rename(rig.folder.join("essay.md"), &moved).unwrap();
    // Open for reading, refusing to share delete: nothing can be renamed over it.
    let holder = OpenOptions::new().read(true).share_mode(SHARE_READ | SHARE_WRITE).open(&moved).unwrap();
    let (mut store, report) = rig.open();
    let told: Vec<(&Path, &Recovery)> = report.iter().map(|r| (r.rel.as_path(), &r.how)).collect();
    assert_eq!(told, [(Path::new("draft.md"), &Recovery::Waiting)], "this test is about a text reported as waiting under the name the file has now");
    drop(holder);
    let opened = store.open_text("draft.md").unwrap();
    let journals = rig.journals();
    store.abandon();
    assert!(
        opened.restored.is_some() && opened.text == typed,
        "recovery said the text was waiting at draft.md, but opening draft.md gave {:?} with restored {:?}; the journal ({journals:?}) is still filed under the name the file used to have",
        opened.text,
        opened.restored
    );
}

/// He renames the writing folder while the program runs. The store finds it
/// again and carries on saving there. A kill after that must still bring
/// what was typed back into the file, in the folder under its new name.
#[test]
fn text_typed_after_the_folder_was_renamed_and_followed_comes_back_into_the_file() {
    let rig = Rig::new("attack-folder-moved");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved under the folder's first name.");
    let moved = rig.folder.with_file_name("writing renamed");
    let mut renamed = false;
    for _ in 0..100 {
        if fs::rename(&rig.folder, &moved).is_ok() {
            renamed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(renamed, "the folder could not be renamed while the store was idle");
    let edit = type_end(&mut store, id, &mut doc, " Saved after it got its new name.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit)).is_some(), "{seen:?}");
    let followed = seen.iter().find_map(|news| match news {
        News::FolderMoved { folder } => Some(folder.clone()),
        _ => None,
    });
    let followed = followed.unwrap_or_else(|| panic!("the folder was not followed: {seen:?}"));
    type_end(&mut store, id, &mut doc, " Typed there and never saved.");
    settle(&mut store);
    store.abandon();
    let typed = doc.to_text("\n");

    // The interface was told where the folder went, and starts there.
    let mut setup = rig.setup(quick());
    setup.folder = followed.clone();
    let (store, report) = Store::open(setup).expect("the store opens on the renamed folder");
    assert!(store.shutdown(SOON));
    let now = read(&followed.join("essay.md"));
    let _ = fs::remove_dir_all(&moved);
    assert_eq!(now, typed, "the journal still named the folder's old path, so recovery did not bring the text back into the file: {report:?}");
}

/// Another program rewrites the file while the buffer is clean; the
/// interface shows the newer text the way the store asks it to (one edit,
/// one save). From then on that text is the file as far as he is concerned,
/// and what he types next belongs in it after a kill, not in a second file.
#[test]
fn text_typed_after_taking_an_outside_version_comes_back_into_the_file() {
    let rig = Rig::new("attack-took-theirs");
    let path = rig.folder.join("essay.md");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "His first version.");
    fs::write(&path, "Rewritten on another device.").unwrap();
    store.rescan();
    let mut seen = Vec::new();
    let news = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::ChangedOutside { .. }));
    let Some(News::ChangedOutside { text, clean: true, .. }) = news else { panic!("{seen:?}") };
    let edit = replace_all(&mut store, id, &mut doc, &text);
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&path), "Rewritten on another device.");
    type_end(&mut store, id, &mut doc, " And he carried on typing.");
    settle(&mut store);
    store.abandon();

    let report = recover(&rig);
    assert_eq!(rig.listing(), ["essay.md"], "recovery took the version he had accepted for a stranger's: {report:?}");
    assert_eq!(read(&path), doc.to_text("\n"), "{report:?}");
}

// ------------------------------------------------------------ forged journals

/// A length field can claim anything. Whatever follows the last whole record
/// is the end of the log and costs nothing that came before it.
#[test]
fn lengths_that_lie_and_garbage_after_a_whole_record_end_the_log_there() {
    let tails: Vec<(&str, Vec<u8>)> = vec![
        ("len 0xFFFFFFFF", [0xFFu8; 4].iter().copied().chain([0u8; 40]).collect()),
        ("len 0x80000000", 0x8000_0000u32.to_le_bytes().iter().copied().chain([7u8; 40]).collect()),
        ("len 64 MiB, nothing behind it", (64u32 << 20).to_le_bytes().iter().copied().chain([1u8; 12]).collect()),
        ("len 64 MiB + 1", ((64u32 << 20) + 1).to_le_bytes().iter().copied().chain([1u8; 12]).collect()),
        ("len 8", 8u32.to_le_bytes().iter().copied().chain([0u8; 20]).collect()),
        ("len 0", vec![0u8; 64]),
        ("len 9, wrong checksum", 9u32.to_le_bytes().iter().copied().chain([0xAAu8; 13]).collect()),
        ("half a frame", vec![0x10, 0x00, 0x00]),
        ("noise", (0..997u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()),
    ];
    for (what, tail) in tails {
        let (rig, root) = rig_with("attack-lengths", Some("Saved."));
        let mut doc = Doc::from_text("Saved.");
        let mut forged = Forged::new(0x51, 1);
        forged.open(&OpenSpec::new(&root, "essay.md", "Saved.", true)).typed(&mut doc, 1, " One.").typed(&mut doc, 2, " Two.");
        forged.bytes.extend_from_slice(&tail);
        forged.put(&rig, &journal_name(0x51, 1));
        let report = recover(&rig);
        assert_eq!(read(&rig.folder.join("essay.md")), "Saved. One. Two.", "{what}: {report:?}");
        assert_eq!(rig.listing(), ["essay.md"], "{what}");
        assert!(rig.journals().is_empty(), "{what}: {:?}", rig.journals());
    }
}

/// Two generations of one text's journal lying there at start, as a kill in
/// the middle of starting a journal afresh leaves them, and as damage could.
/// The lower one holds two unsaved edits. A higher one that is whole speaks
/// for the text; a higher one that is not costs nothing of what the lower holds.
#[test]
fn two_generations_at_start_the_highest_whole_one_speaks_and_a_broken_one_costs_nothing() {
    type Higher = fn(&Path) -> (String, Vec<u8>);
    let cases: Vec<(&str, Higher, &str)> = vec![
        ("an empty file", |_| (journal_name(0x61, 2), Vec::new()), "Saved. One. Two."),
        ("a header and nothing more", |_| (journal_name(0x61, 2), Forged::new(0x61, 2).bytes), "Saved. One. Two."),
        (
            "a first record cut in half",
            |root| {
                let mut forged = Forged::new(0x61, 2);
                forged.open(&OpenSpec::new(root, "essay.md", "Saved. One. Two. And more that is cut off.", false));
                let half = HEADER + (forged.bytes.len() - HEADER) / 2;
                (journal_name(0x61, 2), forged.bytes[..half].to_vec())
            },
            "Saved. One. Two.",
        ),
        (
            "a whole journal whose header names another generation",
            |root| {
                let mut forged = Forged::new(0x61, 3);
                forged.open(&OpenSpec::new(root, "essay.md", "Not this text.", false));
                (journal_name(0x61, 2), forged.bytes)
            },
            "Saved. One. Two.",
        ),
        (
            "a whole journal whose header names another text",
            |root| {
                let mut forged = Forged::new(0x62, 2);
                forged.open(&OpenSpec::new(root, "essay.md", "Not this text.", false));
                (journal_name(0x61, 2), forged.bytes)
            },
            "Saved. One. Two.",
        ),
        (
            "a whole journal that carries the text on",
            |root| {
                let mut spec = OpenSpec::new(root, "essay.md", "Saved. One. Two.", false);
                spec.known = vec![state(b"Saved.")];
                let mut doc = Doc::from_text("Saved. One. Two.");
                let mut forged = Forged::new(0x61, 2);
                forged.open(&spec).typed(&mut doc, 1, " Three.");
                (journal_name(0x61, 2), forged.bytes)
            },
            "Saved. One. Two. Three.",
        ),
        (
            "a whole journal still under its half-made name",
            |root| {
                let mut spec = OpenSpec::new(root, "essay.md", "Saved. One. Two.", false);
                spec.known = vec![state(b"Saved.")];
                let mut doc = Doc::from_text("Saved. One. Two.");
                let mut forged = Forged::new(0x61, 2);
                forged.open(&spec).typed(&mut doc, 1, " Never renamed into place.");
                (format!("{}.part", journal_name(0x61, 2)), forged.bytes)
            },
            "Saved. One. Two.",
        ),
    ];
    for (what, higher, want) in cases {
        let (rig, root) = rig_with("attack-generations", Some("Saved."));
        let mut doc = Doc::from_text("Saved.");
        let mut lower = Forged::new(0x61, 1);
        lower.open(&OpenSpec::new(&root, "essay.md", "Saved.", true)).typed(&mut doc, 1, " One.").typed(&mut doc, 2, " Two.");
        lower.put(&rig, &journal_name(0x61, 1));
        let (name, bytes) = higher(&root);
        fs::write(rig.data.join("journal").join(name), bytes).unwrap();
        let report = recover(&rig);
        assert_eq!(read(&rig.folder.join("essay.md")), want, "the higher generation is {what}: {report:?}");
        assert_eq!(rig.listing(), ["essay.md"], "the higher generation is {what}");
        assert!(rig.journals().is_empty(), "the higher generation is {what}: {:?}", rig.journals());
        // And a second start finds nothing more to do.
        assert!(recover(&rig).is_empty(), "the higher generation is {what}");
    }
}

/// A record with a good checksum whose payload is too short for its kind, or
/// not UTF-8 where text belongs, is not a record: the log ends before it.
#[test]
fn a_first_record_that_is_not_a_whole_open_makes_the_journal_say_nothing() {
    let (rig, root) = rig_with("attack-open", Some("Saved."));
    let spec = OpenSpec::new(&root, "essay.md", "Saved.", true);
    let payload = spec.payload();
    let mut variants: Vec<Vec<u8>> = (0..spec.text_at()).map(|n| payload[..n].to_vec()).collect();
    // Text that is not UTF-8.
    let mut bad_text = payload.clone();
    bad_text.push(0xC3);
    variants.push(bad_text);
    // Sixty-five thousand remembered states claimed, none there.
    let mut many = payload.clone();
    // Edit number, time, three flags, the two halves of the disk state and
    // the three of the stamp come before the count.
    let count_at = 8 + 8 + 3 + 16 + 24;
    assert_eq!(many[count_at..count_at + 2], [0, 0]);
    many[count_at..count_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    variants.push(many);
    for (n, variant) in variants.iter().enumerate() {
        empty_dir(&rig.data.join("journal"));
        let mut doc = Doc::from_text("Saved.");
        let mut forged = Forged::new(0x52, 1);
        forged.record(KIND_OPEN, variant).typed(&mut doc, 1, " Must not appear.");
        forged.put(&rig, &journal_name(0x52, 1));
        let report = recover(&rig);
        assert!(report.is_empty(), "variant {n}: {report:?}");
        assert_eq!(read(&rig.folder.join("essay.md")), "Saved.", "variant {n}");
        assert_eq!(rig.listing(), ["essay.md"], "variant {n}");
        assert!(rig.journals().is_empty(), "variant {n}: a journal that says nothing is set aside: {:?}", rig.journals());
    }
    let kept = fs::read_dir(rig.data.join("journal").join("bad")).map(|dir| dir.count()).unwrap_or(0);
    assert!(kept > 0, "what would not parse is kept under bad, not erased");
}

/// Edits that do not land the way they were recorded end the replay at the
/// last one that did; nothing after them is applied on top of a wrong text.
#[test]
fn edits_that_do_not_fit_the_open_text_end_the_replay_before_them() {
    type Bad = fn(&mut Forged, &mut Doc);
    let bad: Vec<(&str, Bad)> = vec![
        ("a position past the end", |f, _| {
            f.edit(2, (9, 0), (9, 0), 0, 99, "x");
        }),
        ("a position of four thousand million", |f, _| {
            f.edit(2, (u32::MAX, u32::MAX), (u32::MAX, u32::MAX), 0, 99, "x");
        }),
        ("a start after its end", |f, _| {
            f.edit(2, (0, 5), (0, 2), 3, 9, "");
        }),
        ("a column inside a character", |f, _| {
            f.edit(2, (0, 1), (0, 1), 0, 14, "x");
        }),
        ("a removed length that is wrong", |f, _| {
            f.edit(2, (0, 0), (0, 2), 7, 10, "");
        }),
        ("a removed length of four thousand million", |f, _| {
            f.edit(2, (0, 0), (0, 2), u32::MAX, 0, "");
        }),
        ("a length afterwards that is wrong", |f, d| {
            let end = (0, d.len_bytes() as u32);
            f.edit(2, end, end, 0, 5, "x");
        }),
        ("a length afterwards of zero after removing everything and more", |f, d| {
            let end = (0, d.len_bytes() as u32);
            f.edit(2, (0, 0), end, d.len_bytes() as u32, u64::MAX, "");
        }),
        ("an edit number that skips", |f, d| {
            f.typed(d, 4, "x");
        }),
        ("an edit number that repeats", |f, d| {
            f.typed(d, 1, "x");
        }),
    ];
    for (what, break_it) in bad {
        let (rig, root) = rig_with("attack-edits", Some("\u{00E9}t\u{00E9}."));
        let mut doc = Doc::from_text("\u{00E9}t\u{00E9}.");
        let mut forged = Forged::new(0x53, 1);
        forged.open(&OpenSpec::new(&root, "essay.md", "\u{00E9}t\u{00E9}.", true)).typed(&mut doc, 1, " One.");
        let good = doc.to_text("\n");
        break_it(&mut forged, &mut doc);
        forged.typed(&mut doc, 3, " After the bad one.");
        forged.put(&rig, &journal_name(0x53, 1));
        let report = recover(&rig);
        assert_eq!(read(&rig.folder.join("essay.md")), good, "{what}: {report:?}");
        assert_eq!(rig.listing(), ["essay.md"], "{what}");
    }
}

/// A journal names the file it belongs to. A name that is not a path inside
/// the writing folder must never make recovery write somewhere else.
/// An absolute path where nothing is yet: recovery must not make a file there.
#[test]
fn a_journal_naming_a_new_file_outside_the_folder_makes_nothing_there() {
    let (rig, root) = rig_with("attack-outside-new", None);
    let scratch = root.parent().unwrap().to_path_buf();
    let target = scratch.join("made outside.md");
    let mut doc = Doc::new();
    let mut forged = Forged::new(0x54, 1);
    forged.open(&OpenSpec::new(&root, &target.to_string_lossy(), "", false)).typed(&mut doc, 1, "Written by a forged journal.");
    forged.put(&rig, &journal_name(0x54, 1));
    let report = recover(&rig);
    let made = target.exists();
    let _ = fs::remove_file(&target);
    assert!(!made, "recovery made {} outside the writing folder: {report:?}", target.display());
}

/// An absolute path to a file that is there, with the journal claiming that
/// file's present bytes as one of its own saves: it must not be replaced.
#[test]
fn a_journal_naming_an_existing_file_outside_the_folder_does_not_replace_it() {
    let (rig, root) = rig_with("attack-outside-old", None);
    let scratch = root.parent().unwrap().to_path_buf();
    let target = scratch.join("someone else's.md");
    fs::write(&target, "Not a text of this folder.").unwrap();
    let mut doc = Doc::from_text("Not a text of this folder.");
    let mut forged = Forged::new(0x55, 1);
    forged.open(&OpenSpec::new(&root, &target.to_string_lossy(), "Not a text of this folder.", true)).typed(&mut doc, 1, " Overwritten.");
    forged.put(&rig, &journal_name(0x55, 1));
    let report = recover(&rig);
    let now = read(&target);
    let _ = fs::remove_file(&target);
    assert_eq!(now, "Not a text of this folder.", "recovery replaced a file outside the writing folder: {report:?}");
}

/// A path that climbs out of the writing folder with `..`.
#[test]
fn a_journal_naming_a_file_through_dot_dot_does_not_climb_out_of_the_folder() {
    let (rig, root) = rig_with("attack-outside-dots", None);
    let scratch = root.parent().unwrap().to_path_buf();
    let mut doc = Doc::new();
    let mut forged = Forged::new(0x56, 1);
    forged.open(&OpenSpec::new(&root, "..\\climbed out.md", "", false)).typed(&mut doc, 1, "Written by a forged journal.");
    forged.put(&rig, &journal_name(0x56, 1));
    let report = recover(&rig);
    let made = scratch.join("climbed out.md").exists();
    let _ = fs::remove_file(scratch.join("climbed out.md"));
    assert!(!made, "recovery climbed out of the writing folder: {report:?}");
}

/// The writing folder was on a stick. The program was killed with words
/// unsaved, and the stick comes back under another drive letter (or not at
/// all). The journal names a drive that is not there, so it is kept. Kept
/// without a word, the words in it are stranded: nothing ever tells him they
/// exist, and they come back only if that exact drive letter does.
#[test]
fn unsaved_text_for_a_drive_that_is_not_there_is_at_least_spoken_of() {
    let (rig, _) = rig_with("attack-absent-drive", None);
    // D: is never touched: only a letter nothing is mounted on is used.
    let Some(drive) = "ZYXWVUTSRQ".chars().map(|letter| PathBuf::from(format!("\\\\?\\{letter}:\\"))).find(|drive| !drive.exists()) else { return };
    let mut doc = Doc::from_text("Saved on the stick.");
    let mut forged = Forged::new(0x5C, 1);
    forged.open(&OpenSpec::new(&drive.join("Writing"), "stick.md", "Saved on the stick.", true)).typed(&mut doc, 1, " Forty words typed after the last save.");
    forged.put(&rig, &journal_name(0x5C, 1));
    let report = recover(&rig);
    assert!(rig.journals().contains(&journal_name(0x5C, 1)), "the journal must not be thrown away: {:?}", rig.journals());
    assert!(
        report.iter().any(|told| told.rel == Path::new("stick.md")),
        "a journal holding unsaved words for {} was kept, and recovery's report ({report:?}) does not mention it: the interface has nothing to say and he has no way to learn the words exist",
        drive.join("Writing").display()
    );
}

/// Journals whose text has nowhere to go back to: a sub-folder that was
/// removed, a writing folder that was removed, a drive that is not there.
#[test]
fn a_journal_for_a_place_that_is_gone_loses_nothing_and_breaks_nothing() {
    let (rig, root) = rig_with("attack-gone", None);
    // The sub-folder the text was in is gone: it is made again for unsaved text.
    let mut doc = Doc::from_text("Saved.");
    let mut forged = Forged::new(0x57, 1);
    forged.open(&OpenSpec::new(&root, "College\\Drafts\\essay.md", "Saved.", true)).typed(&mut doc, 1, " Unsaved.");
    forged.put(&rig, &journal_name(0x57, 1));
    // A whole writing folder that was deleted.
    let vanished = root.parent().unwrap().join("no longer here");
    let mut doc = Doc::from_text("Saved.");
    let mut forged = Forged::new(0x58, 1);
    forged.open(&OpenSpec::new(&vanished, "lost.md", "Saved.", true)).typed(&mut doc, 1, " Unsaved elsewhere.");
    forged.put(&rig, &journal_name(0x58, 1));
    // A drive that is not plugged in. D: is never touched.
    let absent = "ZYXWVUTSRQ".chars().map(|letter| PathBuf::from(format!("\\\\?\\{letter}:\\"))).find(|drive| !drive.exists());
    if let Some(drive) = &absent {
        let mut doc = Doc::from_text("Saved.");
        let mut forged = Forged::new(0x59, 1);
        forged.open(&OpenSpec::new(&drive.join("Writing"), "stick.md", "Saved.", true)).typed(&mut doc, 1, " On the stick.");
        forged.put(&rig, &journal_name(0x59, 1));
    }
    // A journal whose folder field is empty and one whose file name is empty.
    let mut forged = Forged::new(0x5A, 1);
    forged.open(&OpenSpec::new(Path::new(""), "nowhere.md", "x", false));
    forged.put(&rig, &journal_name(0x5A, 1));
    let mut forged = Forged::new(0x5B, 1);
    forged.open(&OpenSpec::new(&root, "", "x", false));
    forged.put(&rig, &journal_name(0x5B, 1));

    let report = recover(&rig);
    assert_eq!(read(&rig.folder.join("College\\Drafts\\essay.md")), "Saved. Unsaved.", "{report:?}");
    let elsewhere = report.iter().find_map(|r| match &r.how {
        Recovery::Elsewhere(path) => Some(path.clone()),
        _ => None,
    });
    let elsewhere = elsewhere.unwrap_or_else(|| panic!("the vanished folder's text went nowhere: {report:?}"));
    assert_eq!(read(&elsewhere), "Saved. Unsaved elsewhere.");
    assert!(!vanished.exists(), "a deleted folder is not made again");
    if absent.is_some() {
        assert!(rig.journals().contains(&journal_name(0x59, 1)), "a journal for a drive that is away waits for it: {:?}", rig.journals());
    }
    // And all of that again changes nothing.
    let before = files_under(&rig.folder);
    let again = recover(&rig);
    assert_eq!(files_under(&rig.folder), before, "{again:?}");
    assert!(!again.iter().any(|r| matches!(r.how, Recovery::Elsewhere(_) | Recovery::WroteBack(_) | Recovery::BroughtBack)), "{again:?}");
}
