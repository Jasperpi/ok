//! Tests that need to build journal files byte by byte, or to drive the
//! journal thread message by message, to put the store in states a kill or
//! a power cut can leave and that cannot be reached on purpose from outside.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use ok_text::{Doc, Span};

use crate::format::Format;
use crate::hash;
use crate::history::History;
use crate::journal::{self, Attach, Barrier, JMsg};
use crate::recover::{self, Ctx, Recovery};
use crate::state;
use crate::sys;
use crate::testing::{quick, Scratch};
use crate::wal::{self, Open, Stamp};
use crate::{DocId, Faults, NewsOut, Shared};

/// A writing folder and data directory with no store running on them.
struct Bare {
    _scratch: Scratch,
    root: PathBuf,
    data: PathBuf,
    faults: Faults,
}

impl Bare {
    fn new(name: &str) -> Bare {
        let scratch = Scratch::new(name);
        fs::create_dir_all(scratch.path().join("writing")).unwrap();
        fs::create_dir_all(scratch.path().join("data").join("journal")).unwrap();
        let root = fs::canonicalize(scratch.path().join("writing")).unwrap();
        let data = fs::canonicalize(scratch.path().join("data")).unwrap();
        Bare { _scratch: scratch, root, data, faults: Faults::default() }
    }

    fn recover(&self) -> recover::Outcome {
        let volume = sys::volume(&self.root);
        let history = History::new(self.data.join("history"));
        recover::run(&Ctx { root: &self.root, data: &self.data, volume: &volume, history: &history, faults: &self.faults })
    }

    fn journal_path(&self, name: &str) -> PathBuf {
        self.data.join("journal").join(name)
    }

    fn journals(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.data.join("journal")).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn open_record(&self, folder: &Path, rel: &str, text: &str, on_disk: bool, known: &[&str]) -> Open {
        Open {
            edit: 0,
            wall_ms: sys::unix_ms_now(),
            on_disk,
            format: Format::default(),
            disk: hash::state(text.as_bytes()),
            stamp: Stamp::default(),
            known: known.iter().map(|k| hash::state(k.as_bytes())).collect(),
            folder: folder.to_path_buf(),
            rel: PathBuf::from(rel),
            caret: None,
            text: Arc::new(text.to_string()),
        }
    }
}

/// A journal's bytes: its header, an `Open`, then one edit per appended string.
fn journal_bytes(docid: u64, generation: u32, open: &Open, appended: &[&str]) -> (Vec<u8>, String) {
    let mut bytes = wal::header(docid, generation).to_vec();
    wal::push_open(&mut bytes, 0, open);
    let mut doc = Doc::from_text(&open.text);
    for (n, piece) in appended.iter().enumerate() {
        let change = doc.replace(Span::at(doc.end()), piece);
        wal::push_edit(&mut bytes, n as u64 + 1, &wal::edit_payload(open.edit + n as u64 + 1, &change, doc.len_bytes()));
    }
    (bytes, doc.to_text("\n"))
}

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_kill_at_every_byte_of_a_half_made_generation_leaves_the_whole_one_in_charge() {
    let bare = Bare::new("part-prefix");
    let docid = 0x1111_2222_3333_4444;
    let saved = "Saved before the crash.";
    let (whole, typed) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "essay.md", saved, true, &[]), &[" Typed", " after", " it."]);
    // The generation that was being written when the program died. Were it
    // ever taken for the journal, the three edits above would be gone.
    let (part, _) = journal_bytes(docid, 2, &bare.open_record(&bare.root, "essay.md", saved, true, &[]), &[]);
    for cut in 0..=part.len() {
        fs::write(bare.root.join("essay.md"), saved).unwrap();
        fs::write(bare.journal_path(&wal::file_name(docid, 1)), &whole).unwrap();
        fs::write(bare.journal_path(&format!("{}.part", wal::file_name(docid, 2))), &part[..cut]).unwrap();
        let outcome = bare.recover();
        assert_eq!(outcome.report.len(), 1, "cut {cut}");
        assert_eq!(outcome.report[0].how, Recovery::BroughtBack, "cut {cut}");
        assert_eq!(read(&bare.root.join("essay.md")), typed, "cut {cut}");
        assert!(bare.journals().is_empty(), "cut {cut}: journals left behind: {:?}", bare.journals());
    }
}

#[test]
fn of_two_generations_the_higher_whole_one_wins_and_a_torn_one_is_set_aside() {
    // Both whole: the kill came after the new generation was renamed into
    // place and before the old one was removed.
    let bare = Bare::new("two-generations");
    let docid = 0xAAAA_0000_0000_0001;
    let (first, after_first) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "essay.md", "A.", true, &[]), &[" One."]);
    let (second, after_second) = journal_bytes(docid, 2, &bare.open_record(&bare.root, "essay.md", &after_first, true, &["A."]), &[" Two."]);
    fs::write(bare.root.join("essay.md"), "A.").unwrap();
    fs::write(bare.journal_path(&wal::file_name(docid, 1)), &first).unwrap();
    fs::write(bare.journal_path(&wal::file_name(docid, 2)), &second).unwrap();
    let outcome = bare.recover();
    assert_eq!(outcome.report.len(), 1);
    assert_eq!(outcome.report[0].how, Recovery::BroughtBack);
    assert_eq!(read(&bare.root.join("essay.md")), after_second);
    assert_eq!(after_second, "A. One. Two.");
    assert!(bare.journals().is_empty(), "{:?}", bare.journals());

    // The higher one has no whole Open record (something other than us cut
    // it short): it is kept in bad\ and the lower one is used.
    let bare = Bare::new("torn-generation");
    let (first, after_first) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "essay.md", "A.", true, &[]), &[" One."]);
    let (second, _) = journal_bytes(docid, 2, &bare.open_record(&bare.root, "essay.md", &after_first, true, &["A."]), &[" Two."]);
    fs::write(bare.root.join("essay.md"), "A.").unwrap();
    fs::write(bare.journal_path(&wal::file_name(docid, 1)), &first).unwrap();
    fs::write(bare.journal_path(&wal::file_name(docid, 2)), &second[..60]).unwrap();
    // A file that is not a journal at all under a journal's name.
    fs::write(bare.journal_path(&wal::file_name(0xBBBB, 1)), b"not a journal").unwrap();
    let outcome = bare.recover();
    assert_eq!(outcome.report.len(), 1);
    assert_eq!(outcome.report[0].how, Recovery::BroughtBack, "{:?}", outcome.report);
    assert_eq!(read(&bare.root.join("essay.md")), after_first);
    assert!(bare.journals().is_empty(), "{:?}", bare.journals());
    let bad: Vec<_> = fs::read_dir(bare.data.join("journal").join("bad")).unwrap().flatten().collect();
    assert_eq!(bad.len(), 2, "what would not parse is kept, not erased");
}

#[test]
fn a_rename_cut_off_before_or_after_the_move_finds_the_file_either_way() {
    for moved in [false, true] {
        let bare = Bare::new("path-record");
        let docid = 0xCCCC_0000_0000_0001;
        let (mut bytes, _) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "old.md", "Text.", true, &[]), &[" More."]);
        wal::push_path(&mut bytes, 2, Path::new("old.md"), Path::new("Sub\\new.md"));
        fs::create_dir_all(bare.root.join("Sub")).unwrap();
        let at = if moved { "Sub\\new.md" } else { "old.md" };
        fs::write(bare.root.join(at), "Text.").unwrap();
        fs::write(bare.journal_path(&wal::file_name(docid, 1)), &bytes).unwrap();
        let outcome = bare.recover();
        assert_eq!(outcome.report.len(), 1, "moved {moved}");
        assert_eq!((outcome.report[0].rel.as_path(), &outcome.report[0].how), (Path::new(at), &Recovery::BroughtBack), "moved {moved}");
        assert_eq!(read(&bare.root.join(at)), "Text. More.");
        let other = if moved { "old.md" } else { "Sub\\new.md" };
        assert!(!bare.root.join(other).exists(), "the text must exist under one name only");
    }
}

/// Two starts at the same instant, on the journal's own folder (`own`) or
/// on two other folders that share its data directory. What decides who
/// takes a journal up is the lock of the folder the journal names: held for
/// the session by an instance opened on that folder, and for the length of
/// one recovery by an instance opened elsewhere. The journal files
/// themselves are opened so that readers never keep each other out.
fn race_two_recoveries(own: bool) {
    for round in 0..20 {
        let bare = Bare::new("recover-race");
        let docid = 0xDDDD_0000_0000_0001 + round;
        // The file is something else, so a recovery that runs makes a file
        // beside it: two recoveries would show as a second file, or as none
        // if each thought the other had it.
        let (bytes, typed) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "essay.md", "Saved.", true, &[]), &[" Unsaved."]);
        fs::write(bare.root.join("essay.md"), "Changed by someone while we were down.").unwrap();
        fs::write(bare.journal_path(&wal::file_name(docid, 1)), &bytes).unwrap();
        let elsewhere: Vec<PathBuf> = (0..2)
            .map(|n| {
                let folder = bare.root.with_file_name(format!("another folder {n}"));
                fs::create_dir_all(&folder).unwrap();
                fs::canonicalize(&folder).unwrap()
            })
            .collect();
        let barrier = std::sync::Barrier::new(2);
        let reports: Vec<Vec<recover::Recovered>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|n| {
                    let (bare, barrier, elsewhere) = (&bare, &barrier, &elsewhere);
                    scope.spawn(move || {
                        barrier.wait();
                        let root = if own { &bare.root } else { &elsewhere[n] };
                        // As Store::open does: the folder's lock first, then recovery.
                        let Ok(_lock) = state::take_lock(&bare.data, root, "test", false) else { return Vec::new() };
                        let volume = sys::volume(root);
                        let history = History::new(bare.data.join("history"));
                        recover::run(&Ctx { root, data: &bare.data, volume: &volume, history: &history, faults: &bare.faults }).report
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        let acted: usize = reports.iter().map(Vec::len).sum();
        assert_eq!(acted, 1, "round {round}: {reports:?}");
        let files: Vec<String> = fs::read_dir(&bare.root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(files.len(), 2, "round {round}: {files:?}");
        let beside = files.iter().find(|name| name.contains("(recovered ")).expect("a recovered copy");
        assert_eq!(read(&bare.root.join(beside)), typed);
        assert_eq!(read(&bare.root.join("essay.md")), "Changed by someone while we were down.");
        assert!(bare.journals().is_empty());
    }
}

#[test]
fn two_starts_at_once_on_a_journals_own_folder_take_it_up_exactly_once() {
    race_two_recoveries(true);
}

#[test]
fn two_starts_at_once_on_other_folders_take_a_journal_up_exactly_once() {
    race_two_recoveries(false);
}

#[cfg(windows)]
#[test]
fn a_journal_someone_is_reading_is_recovered_all_the_same_and_one_nobody_may_read_is_said_to_wait() {
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt;
    let bare = Bare::new("journal-held");
    let docid = 0xEEEE_0000_0000_0001;
    let (bytes, typed) = journal_bytes(docid, 1, &bare.open_record(&bare.root, "essay.md", "Saved.", true, &[]), &[" Unsaved."]);
    let journal = bare.journal_path(&wal::file_name(docid, 1));
    let history = History::new(bare.data.join("history"));
    assert!(history.set_origin(docid, &bare.root, Path::new("essay.md")));

    // Held with no sharing at all: it cannot be read. It is this folder's
    // text by the note kept with its history, and is said to be waiting.
    fs::write(bare.root.join("essay.md"), "Saved.").unwrap();
    fs::write(&journal, &bytes).unwrap();
    let alone = fs::OpenOptions::new().read(true).share_mode(0).open(&journal).unwrap();
    let outcome = bare.recover();
    assert_eq!(outcome.report.iter().map(|told| (told.rel.as_path(), &told.how)).collect::<Vec<_>>(), [(Path::new("essay.md"), &Recovery::Waiting)]);
    assert_eq!(outcome.unread.get("essay.md"), Some(&docid));
    assert!(outcome.pending.is_empty());
    assert_eq!(read(&bare.root.join("essay.md")), "Saved.");
    drop(alone);
    assert_eq!(bare.journals(), [wal::file_name(docid, 1)], "and it is still there");

    // Readers of every kind that share reading: none of them is in the way.
    for share in [1u32, 1 | 2, 1 | 4, 7] {
        fs::write(bare.root.join("essay.md"), "Saved.").unwrap();
        fs::write(&journal, &bytes).unwrap();
        let reader = fs::OpenOptions::new().read(true).share_mode(share).open(&journal).unwrap();
        let outcome = bare.recover();
        drop(reader);
        if share & 4 == 0 {
            // It will not let the file be removed, so the journal cannot be
            // taken up for good: waiting, said so, nothing touched.
            assert_eq!(outcome.report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::Waiting], "share {share}");
            assert_eq!(read(&bare.root.join("essay.md")), "Saved.", "share {share}");
            assert_eq!(bare.journals().len(), 1, "share {share}");
        } else {
            assert_eq!(outcome.report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::BroughtBack], "share {share}");
            assert_eq!(read(&bare.root.join("essay.md")), typed, "share {share}");
            assert!(bare.journals().is_empty(), "share {share}: {:?}", bare.journals());
        }
    }
}

#[test]
fn an_earlier_sessions_journal_is_never_outranked_into_the_bin() {
    // Two journals of one text, as a start that could not read the first
    // one, a session on the stale file and a second kill leave them. The
    // higher one does not carry on from the lower: it starts from the file
    // as it was, not from what the lower one had typed into it.
    for (older, newer) in [(15u32, 16u32), (1, 2), (3, 40)] {
        let bare = Bare::new("two-sessions");
        let docid = 0xABCD_0000_0000_0001;
        fs::write(bare.root.join("essay.md"), "Saved.").unwrap();
        let (first, first_text) = journal_bytes(docid, older, &bare.open_record(&bare.root, "essay.md", "Saved.", true, &[]), &[" FIRST SESSION, never saved."]);
        let (second, second_text) = journal_bytes(docid, newer, &bare.open_record(&bare.root, "essay.md", "Saved.", true, &[]), &[" Second session, never saved either."]);
        fs::write(bare.journal_path(&wal::file_name(docid, older)), &first).unwrap();
        fs::write(bare.journal_path(&wal::file_name(docid, newer)), &second).unwrap();
        let outcome = bare.recover();
        let hows: Vec<&Recovery> = outcome.report.iter().map(|told| &told.how).collect();
        assert!(matches!(hows.as_slice(), [Recovery::BroughtBack, Recovery::KeptBeside(_)]), "{:?}", outcome.report);
        // The file carries on from the later session; the earlier one's words are beside it.
        assert_eq!(read(&bare.root.join("essay.md")), second_text);
        let Recovery::KeptBeside(beside) = &outcome.report[1].how else { unreachable!() };
        assert_eq!(read(&bare.root.join(beside)), first_text);
        assert!(bare.journals().is_empty(), "{:?}", bare.journals());
        // And a further start finds nothing more to do.
        assert!(bare.recover().report.is_empty());
        assert_eq!(fs::read_dir(&bare.root).unwrap().count(), 2);
    }

    // The same two files where the higher one does carry on from the lower
    // (a kill between making a journal afresh and removing the old one, told
    // by the session number both carry): the lower is spent and says nothing.
    let bare = Bare::new("one-session");
    let docid = 0xABCD_0000_0000_0002;
    fs::write(bare.root.join("essay.md"), "Saved.").unwrap();
    let with_session = |generation: u32, open: &Open, appended: &[&str], first_edit: u64| {
        let mut bytes = wal::header(docid, generation).to_vec();
        wal::push_open(&mut bytes, 0, open);
        wal::push_lineage(&mut bytes, 1, 0x5E55_1011);
        let mut doc = Doc::from_text(&open.text);
        for (n, piece) in appended.iter().enumerate() {
            let change = doc.replace(Span::at(doc.end()), piece);
            wal::push_edit(&mut bytes, n as u64 + 2, &wal::edit_payload(first_edit + n as u64, &change, doc.len_bytes()));
        }
        (bytes, doc.to_text("\n"))
    };
    // The lower one was cut short (its last edit never reached the disk), so
    // its last text is not a text the higher one passes through.
    let (lower, _) = with_session(1, &bare.open_record(&bare.root, "essay.md", "Saved.", true, &[]), &[" One."], 1);
    let mut later = bare.open_record(&bare.root, "essay.md", "Saved. One. Two.", false, &["Saved."]);
    later.edit = 2;
    let (higher, text) = with_session(2, &later, &[" Three."], 3);
    fs::write(bare.journal_path(&wal::file_name(docid, 1)), &lower).unwrap();
    fs::write(bare.journal_path(&wal::file_name(docid, 2)), &higher).unwrap();
    let outcome = bare.recover();
    assert_eq!(outcome.report.iter().map(|told| &told.how).collect::<Vec<_>>(), [&Recovery::BroughtBack]);
    assert_eq!(read(&bare.root.join("essay.md")), text);
    assert_eq!(fs::read_dir(&bare.root).unwrap().count(), 1, "its own session's older journal is never put beside the text");
    assert!(bare.journals().is_empty());
}

#[test]
#[cfg(windows)]
fn a_journal_that_names_a_place_outside_the_folder_is_never_followed_there() {
    let bare = Bare::new("journal-outside");
    let outside = bare.root.with_file_name("not his.md");
    fs::write(&outside, "Someone else's file.").unwrap();
    let cases = [outside.to_string_lossy().into_owned(), "..\\not his.md".to_string(), "..\\..\\climbing.md".to_string(), "sub\\..\\..\\not his.md".to_string(), "C:not his.md".to_string(), String::new()];
    for (n, rel) in cases.iter().enumerate() {
        // The journal claims the outside file's very bytes as a save of its own.
        let (bytes, typed) = journal_bytes(0xF000 + n as u64, 1, &bare.open_record(&bare.root, rel, "Someone else's file.", true, &[]), &[" Overwritten."]);
        fs::write(bare.journal_path(&wal::file_name(0xF000 + n as u64, 1)), &bytes).unwrap();
        let outcome = bare.recover();
        assert_eq!(read(&outside), "Someone else's file.", "{rel:?}");
        assert_eq!(fs::read_dir(&bare.root).unwrap().count(), 0, "{rel:?}");
        assert_eq!(fs::read_dir(bare.root.parent().unwrap()).unwrap().count(), 3, "{rel:?}: nothing new beside the writing folder");
        let [told] = outcome.report.as_slice() else { panic!("{rel:?}: {:?}", outcome.report) };
        let Recovery::Elsewhere(landed) = &told.how else { panic!("{rel:?}: {told:?}") };
        assert!(landed.starts_with(bare.data.join("recovered")), "{rel:?}: {landed:?}");
        assert_eq!(read(landed), typed, "{rel:?}");
        assert!(bare.journals().is_empty(), "{rel:?}");
    }
}

#[test]
fn a_journal_for_another_folder_is_recovered_there_or_set_where_it_can_be_found() {
    let bare = Bare::new("other-folder");
    // Another writing folder that exists and that no instance has open.
    let other = bare.root.with_file_name("other writing");
    fs::create_dir_all(&other).unwrap();
    let other = fs::canonicalize(&other).unwrap();
    fs::write(other.join("there.md"), "Saved there.").unwrap();
    let (bytes, typed_there) = journal_bytes(1, 1, &bare.open_record(&other, "there.md", "Saved there.", true, &[]), &[" And typed."]);
    fs::write(bare.journal_path(&wal::file_name(1, 1)), &bytes).unwrap();
    // A folder that has since been deleted.
    let vanished = bare.root.with_file_name("vanished");
    let (bytes, typed_gone) = journal_bytes(2, 1, &bare.open_record(&vanished, "Notes\\lost.md", "Saved once.", true, &[]), &[" Never saved."]);
    fs::write(bare.journal_path(&wal::file_name(2, 1)), &bytes).unwrap();

    let outcome = bare.recover();
    assert!(outcome.pending.is_empty());
    assert_eq!(outcome.report.len(), 2, "{:?}", outcome.report);
    let there = outcome.report.iter().find(|r| r.folder == other).expect("the other folder's text");
    assert_eq!((there.rel.as_path(), &there.how), (Path::new("there.md"), &Recovery::BroughtBack));
    assert_eq!(read(&other.join("there.md")), typed_there);
    let gone = outcome.report.iter().find(|r| r.folder == vanished).expect("the vanished folder's text");
    let Recovery::Elsewhere(landed) = &gone.how else { panic!("{gone:?}") };
    assert!(landed.starts_with(bare.data.join("recovered")));
    assert_eq!(read(landed), typed_gone);
    assert!(!vanished.exists(), "a deleted folder is not made again");
    assert!(bare.journals().is_empty(), "{:?}", bare.journals());
    assert!(fs::read_dir(&bare.root).unwrap().next().is_none(), "nothing lands in the folder being opened");
}

/// The journal thread on its own, with a handle to feed it.
struct Thread {
    tx: mpsc::Sender<JMsg>,
    shared: Arc<Shared>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Thread {
    fn start(bare: &Bare, rotate_at: u64) -> Thread {
        let mut tuning = quick();
        tuning.rotate_at = rotate_at;
        let shared = Arc::new(Shared::default());
        let (news, _unread) = mpsc::channel();
        let out = NewsOut { tx: news, wake: Arc::new(|| {}), woken: Arc::new(AtomicBool::new(false)) };
        let (tx, rx) = mpsc::channel();
        let handle = journal::spawn(bare.data.join("journal"), tuning, shared.clone(), out, rx).unwrap();
        Thread { tx, shared, handle: Some(handle) }
    }

    fn send(&self, msg: JMsg) {
        self.tx.send(msg).unwrap();
    }

    /// Flushes everything. True if all of it is on the disk.
    fn flush(&self) -> bool {
        let (tx, rx) = mpsc::channel();
        self.send(JMsg::Flush { reply: Some(tx) });
        rx.recv_timeout(Duration::from_secs(10)).expect("the journal thread answers")
    }

    /// The barrier of a save of `text`, said to be the text at `edit`.
    fn begin(&self, id: DocId, edit: u64, text: &str) -> Barrier {
        let (tx, rx) = mpsc::channel();
        self.send(JMsg::Begin { id, edit, state: hash::state(text.as_bytes()), format: Format::default(), text: Arc::new(text.to_string()), temp: "~essay.md.0a0a0a0a.tmp".to_string(), reply: tx });
        rx.recv_timeout(Duration::from_secs(10)).expect("the journal thread answers")
    }

    /// Closes a text; `covered` is the edit a file of his holds and that
    /// text's length. True if the journal is whole on the disk or gone
    /// because nothing more was needed of it.
    fn close(&self, id: DocId, covered: Option<(u64, u64)>) -> bool {
        let (tx, rx) = mpsc::channel();
        self.send(JMsg::Close { id, covered, reply: Some(tx) });
        rx.recv_timeout(Duration::from_secs(10)).expect("the journal thread answers")
    }

    /// Ends the thread as a kill would.
    fn kill(mut self) {
        self.send(JMsg::Abandon);
        self.handle.take().unwrap().join().unwrap();
    }
}

/// Types `piece` at the end of `doc` and sends the edit to the journal thread.
fn typed(thread: &Thread, id: DocId, doc: &mut Doc, edit: u64, piece: &str) {
    let change = doc.replace(Span::at(doc.end()), piece);
    thread.send(JMsg::Edit { id, edit, payload: wal::edit_payload(edit, &change, doc.len_bytes()) });
}

fn replayed(bare: &Bare) -> (String, wal::Replay) {
    let journals: Vec<String> = bare.journals().into_iter().filter(|name| name.ends_with(".wal")).collect();
    assert_eq!(journals.len(), 1, "{journals:?}");
    let parsed = wal::parse(&fs::read(bare.journal_path(&journals[0])).unwrap()).expect("a journal");
    (journals[0].clone(), wal::replay(&parsed).expect("a journal that starts with Open"))
}

#[test]
fn rotation_carries_every_edit_above_the_saved_one_whenever_it_arrived() {
    let bare = Bare::new("journal-rotation");
    let thread = Thread::start(&bare, 1);
    let id = DocId(0);
    let base = bare.open_record(&bare.root, "essay.md", "Base.", true, &[]);
    thread.send(JMsg::Attach(Box::new(Attach { id, docid: 7, base, floor: 0, eager: false, spent: Vec::new(), replaces: None })));
    thread.flush();
    assert!(bare.journals().iter().all(|name| !name.ends_with(".wal")), "no journal before the first edit");

    let mut doc = Doc::from_text("Base.");
    for edit in 1..=3 {
        typed(&thread, id, &mut doc, edit, &format!(" e{edit}"));
    }
    // The interface asks for a save of edit 3 and goes on typing; the saver
    // is still comparing the file when edits 4 and 5 reach the journal.
    let saved = doc.to_text("\n");
    typed(&thread, id, &mut doc, 4, " e4");
    typed(&thread, id, &mut doc, 5, " e5");
    assert_eq!(thread.begin(id, 3, &saved), Barrier::Ready);
    let (name, at_begin) = replayed(&bare);
    assert!(name.ends_with(".1.wal"));
    assert_eq!((at_begin.last_edit, at_begin.unsaved()), (5, true));
    assert_eq!(at_begin.temps, ["~essay.md.0a0a0a0a.tmp"]);
    thread.send(JMsg::Done { id, edit: 3, state: hash::state(saved.as_bytes()), stamp: Stamp { size: saved.len() as u64, modified: 1, index: 2 }, format: Format::default(), text: Arc::new(saved.clone()) });
    typed(&thread, id, &mut doc, 6, " e6");
    thread.flush();

    let (name, after) = replayed(&bare);
    assert!(name.ends_with(".2.wal"), "the journal rotated at the save: {name}");
    assert_eq!(after.open.text.as_str(), saved);
    assert_eq!(after.text, doc.to_text("\n"), "edits 4 and 5 were written before the save's own record and must be carried");
    assert_eq!(after.last_edit, 6);
    assert!(after.open.on_disk);
    assert!(after.known.contains(&hash::state(b"Base.")), "the state before the save is still vouched for");
    assert!(after.known.contains(&hash::state(saved.as_bytes())));
    assert_eq!(thread.shared.written.load(std::sync::atomic::Ordering::Acquire), 6);
    thread.kill();
    let (_, killed) = replayed(&bare);
    assert_eq!(killed.text, doc.to_text("\n"));
}

#[test]
fn a_journal_is_removed_only_when_a_durable_save_covers_its_last_edit() {
    let bare = Bare::new("journal-close");
    let thread = Thread::start(&bare, u64::MAX);
    let attach = |id: u32, docid: u64| {
        let base = bare.open_record(&bare.root, &format!("text{id}.md"), "Base.", true, &[]);
        thread.send(JMsg::Attach(Box::new(Attach { id: DocId(id), docid, base, floor: 0, eager: false, spent: Vec::new(), replaces: None })));
    };
    let done = |id: u32, edit: u64, text: &str| {
        thread.send(JMsg::Done { id: DocId(id), edit, state: hash::state(text.as_bytes()), stamp: Stamp::default(), format: Format::default(), text: Arc::new(text.to_string()) });
    };
    // Saved up to its last edit: the journal goes.
    attach(1, 1);
    let mut doc = Doc::from_text("Base.");
    typed(&thread, DocId(1), &mut doc, 1, " One.");
    done(1, 1, &doc.to_text("\n"));
    assert!(thread.close(DocId(1), Some((1, doc.len_bytes() as u64))));
    // An edit after the last save: it stays, whatever the saver believes.
    attach(2, 2);
    let mut doc = Doc::from_text("Base.");
    typed(&thread, DocId(2), &mut doc, 1, " One.");
    done(2, 1, &doc.to_text("\n"));
    let saved_len = doc.len_bytes() as u64;
    typed(&thread, DocId(2), &mut doc, 2, " Two.");
    assert!(thread.close(DocId(2), Some((1, saved_len))), "kept, and whole on the disk");
    // Never durably saved at all (the flush after the rename failed): it stays.
    attach(3, 3);
    let mut doc = Doc::from_text("Base.");
    typed(&thread, DocId(3), &mut doc, 1, " One.");
    assert!(thread.close(DocId(3), None));
    // Said to be in a file up to its last edit, but with a text of another
    // length than that edit gives: it is not the text, and the journal stays.
    attach(4, 4);
    let mut doc = Doc::from_text("Base.");
    typed(&thread, DocId(4), &mut doc, 1, " One.");
    assert!(thread.close(DocId(4), Some((1, 5))));
    thread.flush();
    let left: Vec<String> = bare.journals().into_iter().filter(|name| name.ends_with(".wal")).collect();
    assert_eq!(left, [wal::file_name(2, 1), wal::file_name(3, 1), wal::file_name(4, 1)]);
    let kept = wal::replay(&wal::parse(&fs::read(bare.journal_path(&left[0])).unwrap()).unwrap()).unwrap();
    assert_eq!((kept.text.as_str(), kept.unsaved(), kept.closed), ("Base. One. Two.", true, false));
    thread.kill();
}

#[test]
fn a_failed_journal_write_is_made_good_from_memory_in_a_new_generation() {
    let bare = Bare::new("journal-repair");
    let mut tuning = quick();
    tuning.rotate_at = u64::MAX;
    let faults = tuning.faults.clone();
    let shared = Arc::new(Shared::default());
    let (news, _unread) = mpsc::channel();
    let out = NewsOut { tx: news, wake: Arc::new(|| {}), woken: Arc::new(AtomicBool::new(false)) };
    let (tx, rx) = mpsc::channel();
    let handle = journal::spawn(bare.data.join("journal"), tuning, shared.clone(), out, rx).unwrap();
    let thread = Thread { tx, shared, handle: Some(handle) };
    let id = DocId(0);
    let base = bare.open_record(&bare.root, "essay.md", "Base.", true, &[]);
    thread.send(JMsg::Attach(Box::new(Attach { id, docid: 9, base, floor: 0, eager: false, spent: Vec::new(), replaces: None })));
    let mut doc = Doc::from_text("Base.");
    typed(&thread, id, &mut doc, 1, " One.");
    assert!(thread.flush());
    assert!(bare.data.join("journal").join("reserve.bin").exists());

    // The disk fills: writes fail, the reserve is given up, the alarm is up.
    faults.journal_write.store(112, std::sync::atomic::Ordering::Relaxed);
    typed(&thread, id, &mut doc, 2, " Two.");
    typed(&thread, id, &mut doc, 3, " Three.");
    assert!(!thread.flush(), "a flush that could not be made says so");
    assert!(thread.shared.journal_failed.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(thread.shared.written.load(std::sync::atomic::Ordering::Acquire), 1);
    assert!(!bare.data.join("journal").join("reserve.bin").exists(), "the megabyte held back is released when the disk fills");
    // A save's barrier is answered honestly: the journal cannot vouch for it.
    assert_eq!(thread.begin(id, 3, &doc.to_text("\n")), Barrier::Unconfirmed);

    // Space again: everything typed meanwhile is written from memory.
    faults.journal_write.store(0, std::sync::atomic::Ordering::Relaxed);
    typed(&thread, id, &mut doc, 4, " Four.");
    thread.flush();
    assert!(!thread.shared.journal_failed.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(thread.shared.written.load(std::sync::atomic::Ordering::Acquire), 4);
    let (name, after) = replayed(&bare);
    assert!(!name.ends_with(".1.wal"), "a journal that may end in half a record is replaced, not appended to");
    assert_eq!((after.text.as_str(), after.last_edit), ("Base. One. Two. Three. Four.", 4));
    thread.kill();
}
