//! Attack, from outside the crate: other programs at the same file. Every
//! combination of access and share mode another program can hold the file
//! with; another program's version arriving while the buffer is clean and
//! while it is not; a crash just after such a version was taken; and a long
//! run with writers of every kind, readers of every kind and a typist all
//! at one file, after which every version either side ever had must still
//! be somewhere.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ok_store::testing::{replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, Hold, News, ReadOnly, Recovery, Resolve, Store, StoreError};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x0001_0000;
const FILE_APPEND_DATA: u32 = 0x4;
const FILE_READ_ATTRIBUTES: u32 = 0x80;

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

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).expect("the file can be read")).expect("the file is UTF-8")
}

/// A new text holding `text`, saved.
fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

/// Every version kept under this rig's data directory, as (file name, text).
fn all_versions(rig: &Rig) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(docs) = fs::read_dir(rig.data.join("history")) else { return out };
    for doc in docs.flatten() {
        let Ok(files) = fs::read_dir(doc.path()) else { continue };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if name != "origin.txt" && name != "caret.txt" {
                out.push((name, String::from_utf8_lossy(&fs::read(file.path()).unwrap_or_default()).into_owned()));
            }
        }
    }
    out
}

/// Opens `path` the way another program might, trying for a moment in case
/// the store itself has the file open just then.
fn hold(path: &Path, access: u32, share: u32) -> File {
    let mut last = None;
    for _ in 0..200 {
        match OpenOptions::new().access_mode(access).share_mode(share).open(path) {
            Ok(file) => return file,
            Err(error) => last = Some(error),
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("could not open {path:?} with access {access:#x} share {share}: {last:?}");
}

// ------------------------------------------- his version, kept or not (fail)

/// `News::ChangedOutside` says what to do when another program's version
/// arrives while there are unsaved edits: "answer with `Store::resolve`, or
/// take theirs the same way after keeping yours with `Store::keep`." That
/// second way is what DESIGN.md's sentence needs: "This text changed on
/// disk. Showing the newer one; yours is in earlier."
///
/// Done exactly so: his text is kept nowhere, and the version labelled as
/// his holds the other program's text.
#[test]
fn keeping_mine_as_a_version_before_taking_theirs_keeps_mine() {
    let rig = Rig::new("ao-keep-then-take");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "The essay as last saved.");
    let path = rig.folder.join("essay.md");
    let edit = type_end(&mut store, id, &mut doc, " A paragraph he typed that is in no file yet.");
    let mine = doc.to_text("\n");
    fs::write(&path, "The version another device put here.").unwrap();
    store.save(id, mine.clone(), edit);
    let News::ChangedOutside { text: theirs, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert!(!clean);

    // Keep mine as a version, then take theirs.
    store.keep(id, mine.clone(), edit, "mine");
    let after_keep = listen(&mut store, Duration::from_millis(300));
    let edit = replace_all(&mut store, id, &mut doc, &theirs);
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&path), "The version another device put here.");

    let versions = store.versions(id).unwrap();
    let texts: Vec<(Option<String>, String)> = versions.iter().map(|v| (v.label.clone(), store.read_version(id, &v.name).unwrap())).collect();
    store.close(id, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty(), "and the journal that held his paragraph is gone too");
    assert!(
        texts.iter().any(|(_, text)| text == &mine),
        "his paragraph is in no file and no version. News after keep(): {after_keep:?}. Versions: {texts:?}. Folder: {:?}",
        rig.listing()
    );
    assert!(!texts.iter().any(|(label, text)| label.as_deref() == Some("mine") && text == &theirs), "the version labelled mine holds their text: {texts:?}");
}

// ------------------------------------ a reload, then a crash (fail)

/// Research test list: recovery "must never classify its own older state as
/// an outside change". A sync client updates the file while the buffer is
/// clean; the store hands the new text over and reports it saved once the
/// buffer reads the same; he types a sentence; the process is killed before
/// the next save. The file is exactly the state the store last reported
/// saved, so recovery must bring it forward, not leave it stale and put a
/// second file beside it.
#[test]
fn a_reload_then_a_crash_is_not_taken_for_an_outside_change() {
    let rig = Rig::new("ao-reload-crash");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Mine.");
    let path = rig.folder.join("essay.md");
    fs::write(&path, "Theirs, from the other device.").unwrap();
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert!(clean);
    let edit = replace_all(&mut store, id, &mut doc, &text);
    let news = save_wait(&mut store, id, &doc, edit);
    assert!(news.iter().any(|n| matches!(n, News::Saved { edit: e, .. } if *e == edit)), "the store called this state saved");
    type_end(&mut store, id, &mut doc, " And a sentence of his own.");
    // Let the journal reach its file, then the kill.
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].how, Recovery::BroughtBack, "folder now holds {:?}", rig.listing());
    assert_eq!(read(&path), "Theirs, from the other device. And a sentence of his own.");
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(store.shutdown(SOON));
}

// ------------------------------------------- every way of holding the file

/// Another program holds the saved file open in every combination of access
/// and share mode while a save is asked for. Each save must end as `Saved`
/// with the whole new text in the file, or as `Held` with the whole old text
/// in the file, no temp file left, and `Saved` once the other program lets
/// go. A holder that could write must always keep the save out, and what it
/// then writes must be seen as an outside change, never replaced unseen.
#[test]
fn every_way_another_program_can_hold_the_file_ends_in_saved_or_held_and_nothing_between() {
    let rig = Rig::new("ao-matrix-save");
    let (mut store, _) = rig.open();
    let accesses = [(GENERIC_READ, "read"), (GENERIC_WRITE, "write"), (GENERIC_READ | GENERIC_WRITE, "read+write"), (FILE_APPEND_DATA, "append"), (DELETE, "delete"), (FILE_READ_ATTRIBUTES, "attributes")];
    let mut table = String::new();
    let mut n = 0;
    for (access, access_name) in accesses {
        for share in 0..8u32 {
            n += 1;
            let rel = format!("text {n}.md");
            let path = rig.folder.join(&rel);
            let old = format!("Old text {n}.");
            let (id, mut doc, _) = written(&mut store, &rel, &old);
            let mut holder = hold(&path, access, share);
            let edit = type_end(&mut store, id, &mut doc, " New words.");
            let new = doc.to_text("\n");
            store.save(id, new.clone(), edit);
            let outcome = expect(&mut store, "Saved or Held", |news| matches!(news, News::Saved { doc, .. } | News::Held { doc, .. } if *doc == id));
            let can_write = access & (GENERIC_WRITE | FILE_APPEND_DATA) != 0;
            match outcome {
                News::Saved { edit: saved, .. } => {
                    assert_eq!(saved, edit);
                    assert_eq!(read(&path), new, "{access_name} share {share}");
                    assert!(!can_write, "{access_name} share {share}: the file was replaced under a program that had it open for writing");
                    table.push_str(&format!("{access_name:>10} share {share}: saved\n"));
                    drop(holder);
                }
                News::Held { why, .. } => {
                    assert_eq!(why, Hold::Busy, "{access_name} share {share}");
                    // The other program can still read the whole old text by a fresh open, if anyone can.
                    if let Ok(bytes) = OpenOptions::new().read(true).share_mode(7).open(&path).and_then(|mut f| {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut f, &mut bytes).map(|_| bytes)
                    }) {
                        assert_eq!(String::from_utf8_lossy(&bytes), old, "{access_name} share {share}");
                    }
                    let names = rig.listing();
                    assert!(!names.iter().any(|name| name.ends_with(".tmp")), "{access_name} share {share}: a temp file was left: {names:?}");
                    if access & GENERIC_WRITE != 0 {
                        // It writes its own version through the handle it had all along.
                        holder.set_len(0).unwrap();
                        holder.write_all(b"What the other program wrote.").unwrap();
                        drop(holder);
                        let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { doc, .. } if *doc == id)) else { unreachable!() };
                        assert_eq!((text.as_str(), clean), ("What the other program wrote.", false), "{access_name} share {share}");
                        assert_eq!(read(&path), "What the other program wrote.");
                        store.resolve(id, Resolve::KeepMine);
                        table.push_str(&format!("{access_name:>10} share {share}: held, its write seen\n"));
                    } else {
                        drop(holder);
                        table.push_str(&format!("{access_name:>10} share {share}: held\n"));
                    }
                    expect(&mut store, "Saved after release", |news| matches!(news, News::Saved { doc, edit: e, .. } if *doc == id && *e == edit));
                    assert_eq!(read(&path), new, "{access_name} share {share}");
                }
                _ => unreachable!(),
            }
            store.close(id, None, None);
            expect(&mut store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == id));
        }
    }
    println!("{table}");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
    assert!(!rig.listing().iter().any(|name| name.ends_with(".tmp")));
}

/// The same holders while a text is being opened. Opening must give the
/// whole text or say it is busy; a text a writer has is never shown as
/// writable; and when the other program lets go the text becomes writable
/// without being opened again.
#[test]
fn opening_a_text_another_program_holds_is_whole_or_busy_and_never_writable_under_a_writer() {
    let rig = Rig::new("ao-matrix-open");
    let (mut store, _) = rig.open();
    let accesses = [(GENERIC_READ, "read"), (GENERIC_WRITE, "write"), (GENERIC_READ | GENERIC_WRITE, "read+write"), (FILE_APPEND_DATA, "append"), (DELETE, "delete"), (FILE_READ_ATTRIBUTES, "attributes")];
    let mut table = String::new();
    let mut n = 0;
    for (access, access_name) in accesses {
        for share in 0..8u32 {
            n += 1;
            let rel = format!("text {n}.md");
            let path = rig.folder.join(&rel);
            let content = format!("Whole text {n}.");
            fs::write(&path, &content).unwrap();
            let holder = hold(&path, access, share);
            let can_write = access & (GENERIC_WRITE | FILE_APPEND_DATA) != 0;
            let started = Instant::now();
            let result = store.open_text(&rel);
            let took = started.elapsed();
            assert!(took < Duration::from_secs(2), "{access_name} share {share}: opening took {took:?}");
            match result {
                Ok(opened) => {
                    assert_eq!(opened.text, content, "{access_name} share {share}");
                    if can_write {
                        assert_eq!(opened.read_only, Some(ReadOnly::Busy), "{access_name} share {share}: shown writable while another program had it open for writing");
                    }
                    table.push_str(&format!("{access_name:>10} share {share}: opened, read_only {:?}, {} ms\n", opened.read_only, took.as_millis()));
                    drop(holder);
                    if opened.read_only == Some(ReadOnly::Busy) {
                        expect(&mut store, "Released", |news| matches!(news, News::Released { doc } if *doc == opened.doc));
                    }
                    let mut doc = Doc::from_text(&opened.text);
                    let edit = type_end(&mut store, opened.doc, &mut doc, " More.");
                    save_wait(&mut store, opened.doc, &doc, edit);
                    assert_eq!(read(&path), format!("{content} More."));
                    store.close(opened.doc, None, None);
                    expect(&mut store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == opened.doc));
                }
                Err(error) => {
                    assert_eq!(error, StoreError::Busy, "{access_name} share {share}");
                    assert_eq!(share & 1, 0, "{access_name} share {share}: a holder that shares reading must not keep the text from being shown");
                    table.push_str(&format!("{access_name:>10} share {share}: busy, {} ms\n", took.as_millis()));
                    drop(holder);
                    let opened = store.open_text(&rel).expect("opens once the holder is gone");
                    assert_eq!((opened.text.as_str(), opened.read_only), (content.as_str(), None));
                    store.close(opened.doc, None, None);
                    expect(&mut store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == opened.doc));
                }
            }
        }
    }
    println!("{table}");
    assert!(store.shutdown(SOON));
}

// --------------------------------------------------------- the long run

/// A small generator of its own, so that each thread's choices repeat.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

#[repr(C)]
#[derive(Default)]
struct ByHandle {
    attrs: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetFileInformationByHandle(handle: isize, info: *mut ByHandle) -> i32;
}

/// The file index and size behind an open handle: which file it is.
fn identity(file: &File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    let mut info = ByHandle::default();
    // SAFETY: `info` is a live BY_HANDLE_FILE_INFORMATION and the handle is open.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as isize, &mut info) } == 0 {
        return None;
    }
    Some((((info.index_high as u64) << 32) | info.index_low as u64, ((info.size_high as u64) << 32) | info.size_low as u64))
}

fn read_shared(path: &Path) -> Option<String> {
    for _ in 0..20 {
        if let Ok(mut file) = OpenOptions::new().read(true).share_mode(7).open(path) {
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut file, &mut bytes).is_ok() {
                return Some(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

/// A shared log of who did what when, printed around a failure.
#[derive(Clone)]
struct Trace {
    start: Instant,
    lines: Arc<Mutex<Vec<(u128, String)>>>,
}

impl Trace {
    fn new() -> Trace {
        Trace { start: Instant::now(), lines: Arc::new(Mutex::new(Vec::new())) }
    }

    fn note(&self, what: String) {
        let at = self.start.elapsed().as_micros();
        self.lines.lock().unwrap().push((at, what));
    }

    /// Everything noted from a little before the first line that mentions
    /// `needle` to a little after the last.
    fn around(&self, needle: &str) -> String {
        let mut lines = self.lines.lock().unwrap().clone();
        lines.sort();
        let hits: Vec<u128> = lines.iter().filter(|(_, what)| what.contains(needle)).map(|(at, _)| *at).collect();
        let (Some(first), Some(last)) = (hits.first(), hits.last()) else { return String::new() };
        let mut out = String::new();
        for (at, what) in lines.iter().filter(|(at, _)| *at + 150_000 >= *first && *at <= *last + 400_000) {
            out.push_str(&format!("{:>9.3} ms  {what}\n", *at as f64 / 1000.0));
        }
        out
    }
}

/// The two ends of a text and its length: enough to tell versions apart in a log.
fn head(text: &str) -> String {
    let cut: String = text.chars().take(14).collect();
    let tail: String = text.chars().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{cut}..{tail} ({} bytes)", text.len())
}

/// What the interface would be doing through the long run: one buffer, the
/// documented answer to every `ChangedOutside`.
struct Typist {
    trace: Trace,
    id: DocId,
    doc: Doc,
    latest: u64,
    saved: u64,
    /// Their text, waiting to be taken once ours has been kept beside.
    awaiting: Option<String>,
    asked_at: Instant,
    known: Arc<Mutex<HashSet<String>>>,
    kept_beside: Vec<PathBuf>,
    conflicts: usize,
    reloads: usize,
}

impl Typist {
    fn ask_save(&mut self, store: &mut Store) {
        let text = self.doc.to_text("\n");
        self.known.lock().unwrap().insert(text.clone());
        self.trace.note(format!("typist asks save of edit {} [{}]", self.latest, head(&text)));
        store.save(self.id, text, self.latest);
    }

    fn take(&mut self, store: &mut Store, theirs: &str) {
        self.latest = replace_all(store, self.id, &mut self.doc, theirs);
        self.trace.note(format!("typist takes theirs as edit {} [{}]", self.latest, head(theirs)));
        self.ask_save(store);
    }

    fn keep_both(&mut self, store: &mut Store) {
        let mine = self.doc.to_text("\n");
        self.known.lock().unwrap().insert(mine.clone());
        self.trace.note(format!("typist asks keep-both at edit {} [{}]", self.latest, head(&mine)));
        store.resolve(self.id, Resolve::KeepBoth { text: mine, edit: self.latest });
        self.asked_at = Instant::now();
    }

    fn hear(&mut self, store: &mut Store, news: News) {
        match &news {
            News::ChangedOutside { text, clean, edit, .. } => self.trace.note(format!("news ChangedOutside clean {clean} edit {edit} (typist at {}) [{}]", self.latest, head(text))),
            News::Tree(_) | News::FileText { .. } => {}
            other => self.trace.note(format!("news {other:?}")),
        }
        match news {
            News::ChangedOutside { text, clean, edit, read_only, .. } => {
                assert_eq!(read_only, None);
                if clean && edit == self.latest && self.awaiting.is_none() {
                    self.reloads += 1;
                    self.take(store, &text);
                } else {
                    self.conflicts += 1;
                    self.awaiting = Some(text);
                    self.keep_both(store);
                }
            }
            News::KeptBeside { rel, .. } => {
                self.kept_beside.push(rel);
                if let Some(theirs) = self.awaiting.take() {
                    self.take(store, &theirs);
                }
            }
            News::Saved { edit, .. } => self.saved = self.saved.max(edit),
            _ => {}
        }
    }

    fn poll(&mut self, store: &mut Store) {
        while let Some(news) = store.poll() {
            self.hear(store, news);
        }
        // A keep-both that could not be written is asked for again.
        if self.awaiting.is_some() && self.asked_at.elapsed() > Duration::from_millis(500) {
            self.keep_both(store);
        }
    }
}

const TRUNCATE: usize = 0;
const RENAME: usize = 1;
const DELETE_RECREATE: usize = 2;

/// The longest the store's own rename may come after its last look at the
/// file, as this test allows it: the one exposure the corrected design
/// accepts (its section 10: another program's version is lost "only if it
/// replaces the file by a POSIX rename ... between step g and step h").
/// The look is a query on a handle the store already holds and the rename
/// follows it at once, so the gap is the time the rename call takes to reach
/// the file system, about half a millisecond here; the allowance is generous
/// because the saver thread can lose the processor in between.
const GAP: Duration = Duration::from_millis(40);

/// When the witness saw which file under the name: microseconds since the
/// start of the run, and the file's index (or nothing there).
type Sightings = Arc<Mutex<Vec<(u128, Option<u64>)>>>;

/// One version the other program put in the file.
#[derive(Clone)]
struct Wrote {
    version: String,
    /// What it read in the file just before.
    before: Option<String>,
    kind: usize,
    /// For a version put under the name as a file of its own (renamed into
    /// place, or made anew after a delete): which file it is.
    index: Option<u64>,
}

/// Some seconds of everything at once on one file: a typist saving every
/// few words; another program replacing the file in the given `ways`
/// (truncate and write, temp file and rename, delete and recreate) every
/// `pause.0` to `pause.0 + pause.1` milliseconds; with `holders`, two more
/// programs opening it in random access and share modes; and one reading it
/// the careful way and checking that what it reads is always one whole
/// version.
///
/// Afterwards: every word the typist typed is in the file, in a file beside
/// it, or in a kept version; every version the other program wrote was
/// either replaced by that program itself or is still somewhere; the file
/// was never seen holding anything but one whole version; no temp file is
/// left; and the store shuts down clean.
///
/// Corrected after the fix, in the one place where the test asked for more
/// than the design promises. A save replaces the file by one rename, which
/// is what keeps the file whole through a kill; Windows has no rename that
/// replaces only if the name still leads to a given file, so between the
/// store's last look and its rename there is a moment (see [`GAP`]) in which
/// another program's rename or delete of the same name goes unseen. The
/// store now looks through the handle it holds, immediately before the
/// rename, and looks again after it, which catches a text renamed away in
/// that moment; a version put under the name in that moment (renamed in, or
/// made anew after a delete), or a delete alone, cannot be told from the
/// store's own replacement afterwards. So: a version the other program put
/// under the name as a file of its own counts as lost only if a witness saw
/// it there for longer than the gap, and a delete answered by the store's
/// file appearing under the name is counted and shown, not failed on. Every
/// other kind of loss fails the test as before; in particular a version
/// written into the file in place, which the store's guard keeps out for
/// the whole of a save, is never excused.
fn long_run(name: &str, ways: &'static [usize], holders: bool, seconds: u64, pause: (u64, u64)) {
    let rig = Rig::new(name);
    let path = rig.folder.join("essay.md");
    let (mut store, _) = rig.open();
    let known = Arc::new(Mutex::new(HashSet::new()));
    let (id, doc, latest) = written(&mut store, "essay.md", "start");
    known.lock().unwrap().insert("start".to_string());
    let trace = Trace::new();
    let mut typist = Typist { trace: trace.clone(), id, doc, latest, saved: latest, awaiting: None, asked_at: Instant::now(), known: known.clone(), kept_beside: Vec::new(), conflicts: 0, reloads: 0 };

    let stop = Arc::new(AtomicBool::new(false));
    // Each entry: a version the other program wrote, and what it found in the file just before writing it.
    let outside_log: Arc<Mutex<Vec<Wrote>>> = Arc::new(Mutex::new(Vec::new()));
    let broken: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Which file the witness saw under the name, each time that changed, in
    // microseconds since the start of the run.
    let sightings: Sightings = Arc::new(Mutex::new(Vec::new()));

    let writer = {
        let (path, stop, known, log, trace) = (path.clone(), stop.clone(), known.clone(), outside_log.clone(), trace.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(0x1234_5678_9ABC_DEF1);
            let mut k = 0u32;
            let mut kinds = [0u32; 3];
            // Deletes after which the name was found taken again before the
            // file could be made anew: how long after the delete, and by what.
            let mut overtaken: Vec<(u32, Duration, Option<String>)> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(pause.0 + rng.below(pause.1)));
                k += 1;
                let version = format!("OUT{k} {}.", "x".repeat(rng.below(120) as usize));
                known.lock().unwrap().insert(version.clone());
                let before = read_shared(&path);
                let kind = ways[rng.below(ways.len() as u64) as usize];
                trace.note(format!("other program starts OUT{k}. by {}, having read [{}]", ["truncate and write", "temp and rename", "delete and recreate"][kind], before.as_deref().map(head).unwrap_or_default()));
                let mut done = false;
                let mut index = None;
                for _ in 0..400 {
                    done = match kind {
                        // Truncate and write through one handle, as most editors save.
                        0 => fs::write(&path, &version).is_ok(),
                        // Write a temp file and rename it over, as careful editors and sync clients do.
                        1 => {
                            let temp = path.with_file_name(format!("essay.md.other-{k}"));
                            let written = fs::write(&temp, &version).is_ok();
                            index = OpenOptions::new().access_mode(0).share_mode(7).open(&temp).ok().and_then(|file| identity(&file)).map(|(index, _)| index);
                            let renamed = written && fs::rename(&temp, &path).is_ok();
                            if !renamed {
                                let _ = fs::remove_file(&temp);
                            }
                            renamed
                        }
                        // Delete, pause, make it again.
                        _ => {
                            let was_there = path.exists();
                            let removed = if was_there { fs::remove_file(&path) } else { Ok(()) };
                            if let Err(error) = &removed {
                                trace.note(format!("other program could not delete for OUT{k}. yet: {error:?}"));
                            }
                            if removed.is_err() {
                                false
                            } else {
                                let deleted = Instant::now();
                                let mut first_refusal = None;
                                trace.note(format!("other program deleted the file for OUT{k}. (it was there: {was_there}); the name can still be opened: {:?}", OpenOptions::new().access_mode(0).share_mode(7).open(&path).map(|_| ()).map_err(|e| e.raw_os_error())));
                                std::thread::sleep(Duration::from_millis(rng.below(4)));
                                let mut made = false;
                                let mut refusal = None;
                                for _ in 0..400 {
                                    let attempt = OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut f| {
                                        index = identity(&f).map(|(index, _)| index);
                                        f.write_all(version.as_bytes())
                                    });
                                    match attempt {
                                        Ok(()) => {
                                            made = true;
                                            break;
                                        }
                                        Err(error) => {
                                            first_refusal.get_or_insert(deleted.elapsed());
                                            refusal = Some(error);
                                        }
                                    }
                                    std::thread::sleep(Duration::from_millis(2));
                                }
                                if !made {
                                    // It deleted the file and something put a file back under the name before it
                                    // could: a save of the store's that was past its last look when the delete
                                    // came. The other program does what such a program would: it tries again.
                                    let there = read_shared(&path);
                                    trace.note(format!("other program deleted the file for OUT{k}. and could not make it again ({refusal:?}); the name now holds [{}]", there.as_deref().map(head).unwrap_or_default()));
                                    overtaken.push((k, first_refusal.unwrap_or_default(), there));
                                }
                                made
                            }
                        }
                    };
                    if done {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                trace.note(format!("other program finished OUT{k}. written: {done}"));
                if done {
                    kinds[kind] += 1;
                    log.lock().unwrap().push(Wrote { version, before, kind, index });
                }
            }
            (kinds, overtaken)
        })
    };

    let holders: Vec<_> = (0..if holders { 2u64 } else { 0 })
        .map(|which| {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut rng = Rng(0xA5A5_0000_1111_2222 + which * 7919);
                let accesses = [GENERIC_READ, GENERIC_READ, GENERIC_READ | GENERIC_WRITE, DELETE, FILE_READ_ATTRIBUTES, FILE_APPEND_DATA];
                let mut held = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let access = accesses[rng.below(accesses.len() as u64) as usize];
                    let share = rng.below(8) as u32;
                    if let Ok(file) = OpenOptions::new().access_mode(access).share_mode(share).open(&path) {
                        held += 1;
                        std::thread::sleep(Duration::from_millis(rng.below(4)));
                        drop(file);
                    }
                    std::thread::sleep(Duration::from_millis(1 + rng.below(12)));
                }
                held
            })
        })
        .collect();

    // A witness that takes no part: it looks at the name every tenth of a
    // millisecond through a handle that asks for no access (so it is in
    // nobody's way) and notes each time a different file is there.
    let witness = {
        let (path, stop, trace, sightings) = (path.clone(), stop.clone(), trace.clone(), sightings.clone());
        std::thread::spawn(move || {
            let mut last = None;
            while !stop.load(Ordering::Relaxed) {
                let now = OpenOptions::new().access_mode(0).share_mode(7).open(&path).ok().and_then(|file| identity(&file));
                if now != last {
                    sightings.lock().unwrap().push((trace.start.elapsed().as_micros(), now.map(|(index, _)| index)));
                    match now {
                        Some((index, size)) => trace.note(format!("witness: the name holds file {index:#x}, {size} bytes")),
                        None => trace.note("witness: the name holds nothing".to_string()),
                    }
                    last = now;
                }
                let pause = Instant::now();
                while pause.elapsed() < Duration::from_micros(100) {
                    std::thread::yield_now();
                }
            }
        })
    };

    let checker = {
        let (path, stop, known, broken) = (path.clone(), stop.clone(), known.clone(), broken.clone());
        std::thread::spawn(move || {
            let mut reads = 0u32;
            while !stop.load(Ordering::Relaxed) {
                // Deny writers, as the store does: a read that succeeds was not made in the middle of a write.
                if let Ok(mut file) = OpenOptions::new().read(true).share_mode(1 | 4).open(&path) {
                    let mut bytes = Vec::new();
                    if std::io::Read::read_to_end(&mut file, &mut bytes).is_ok() {
                        reads += 1;
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        if !known.lock().unwrap().contains(&text) {
                            broken.lock().unwrap().push(text);
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(3));
            }
            reads
        })
    };

    // The typist.
    let mut tokens = Vec::new();
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut n = 0u32;
    while Instant::now() < end && !writer.is_finished() {
        typist.poll(&mut store);
        if typist.awaiting.is_none() {
            n += 1;
            let token = format!(" <m{n}>");
            typist.latest = type_end(&mut store, id, &mut typist.doc, &token);
            trace.note(format!("typist types{token} as edit {}", typist.latest));
            tokens.push(token);
            if n % 4 == 0 {
                typist.ask_save(&mut store);
            }
        }
        std::thread::sleep(Duration::from_millis(3));
    }

    stop.store(true, Ordering::Relaxed);
    let (kinds, overtaken) = writer.join().expect("the writer thread");
    witness.join().expect("the witness thread");
    for (k, after, there) in &overtaken {
        // The store's file can only appear under a name just deleted if its
        // save was already past its last look: that is, at once, before the
        // other program's first try at making the file again (which comes
        // within four milliseconds of its delete). The time is shown; the
        // bound is loose because this thread may have lost the processor.
        println!("the other program deleted the file for its version {k} and {after:?} later found the name taken again, by [{}]", there.as_deref().map(head).unwrap_or_default());
        assert!(*after < Duration::from_secs(1), "a file was put back under a name the other program had deleted {after:?} before:\n{}", trace.around(&format!("OUT{k}.")));
    }
    let held: u32 = holders.into_iter().map(|h| h.join().expect("a holder thread")).sum();
    let reads = checker.join().expect("the checker thread");

    // Everything settles: any conflict is answered, the last words are saved.
    let settle = Instant::now() + Duration::from_secs(30);
    let mut asked = Instant::now() - Duration::from_secs(1);
    loop {
        typist.poll(&mut store);
        if typist.awaiting.is_none() {
            if typist.saved == typist.latest {
                // Nothing more may arrive that would change the buffer.
                let quiet = listen(&mut store, Duration::from_millis(500));
                if quiet.iter().all(|news| !matches!(news, News::ChangedOutside { .. } | News::KeptBeside { .. })) {
                    break;
                }
                for news in quiet {
                    typist.hear(&mut store, news);
                }
                continue;
            }
            if asked.elapsed() > Duration::from_millis(300) {
                typist.ask_save(&mut store);
                asked = Instant::now();
            }
        }
        assert!(Instant::now() < settle, "never settled: saved {} latest {} awaiting {:?}; folder {:?}", typist.saved, typist.latest, typist.awaiting, rig.listing());
        std::thread::sleep(Duration::from_millis(2));
    }
    let last = typist.doc.to_text("\n");
    assert_eq!(read(&path), last, "the file is the buffer once everything has settled");
    store.close(id, None, None);
    assert_eq!(expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. })), News::Closed { doc: id, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());

    // Where a version can still be: the file, the files beside it, the kept versions.
    let mut corpus = vec![last];
    let names = rig.listing();
    for name in &names {
        assert!(!name.ends_with(".tmp"), "a temp file was left: {names:?}");
        if name != "essay.md" && !name.starts_with("essay.md.other-") {
            corpus.push(read(&rig.folder.join(name)));
        }
    }
    corpus.extend(all_versions(&rig).into_iter().map(|(_, text)| text));

    let log = outside_log.lock().unwrap().clone();
    println!(
        "typed {} words; {} reloads, {} conflicts, {} kept beside; the other program wrote {} versions (in place {}, by rename {}, by delete {}; {} of its deletes were overtaken by a save); holders opened it {} times; {} careful reads; {} files in the folder",
        tokens.len(),
        typist.reloads,
        typist.conflicts,
        typist.kept_beside.len(),
        log.len(),
        kinds[0],
        kinds[1],
        kinds[2],
        overtaken.len(),
        held,
        reads,
        names.len()
    );

    let broken = broken.lock().unwrap().clone();
    assert!(broken.is_empty(), "the file was read holding something that is no whole version of either side: {:?}", &broken[..broken.len().min(3)]);

    let lost: Vec<&String> = tokens.iter().filter(|token| !corpus.iter().any(|text| text.contains(token.as_str()))).collect();
    if let Some(first) = lost.first() {
        println!("what happened around{first}:\n{}", trace.around(first.trim()));
    }
    assert!(lost.is_empty(), "{} of {} typed words are in no file and no version, the first {:?}", lost.len(), tokens.len(), &lost[..lost.len().min(5)]);

    // How long the witness saw a given file under the name: from its first
    // sighting to the first sighting of anything else after it. Zero if it
    // was never seen there at all; an hour if it was never seen to go.
    let sightings = sightings.lock().unwrap().clone();
    let in_place = |index: u64| -> Duration {
        let Some(at) = sightings.iter().position(|(_, seen)| *seen == Some(index)) else { return Duration::ZERO };
        match sightings[at..].iter().find(|(_, seen)| *seen != Some(index)) {
            Some((gone, _)) => Duration::from_micros((gone - sightings[at].0) as u64),
            None => Duration::from_secs(3600),
        }
    };
    let mut gone = Vec::new();
    let mut in_the_gap = Vec::new();
    let mut unsure = 0;
    for (i, wrote) in log.iter().enumerate() {
        let version = &wrote.version;
        let still = corpus.iter().any(|text| text.contains(version.as_str()));
        match log.get(i + 1).map(|next| &next.before) {
            // The other program replaced its own version: its to lose.
            Some(Some(before)) if before.contains(version.as_str()) => {}
            _ if still => {}
            Some(None) => unsure += 1,
            _ => match wrote.index.map(in_place) {
                // Put under the name, as a file of its own, in the moment
                // between the store's last look and its own rename: the
                // exposure the design accepts. A version written into the
                // file in place can never be among these, because the store
                // keeps writers out of the file for the whole of a save.
                Some(there) if wrote.kind != TRUNCATE && there < GAP => in_the_gap.push((version.split(' ').next().unwrap_or_default().to_string(), ["in place", "by rename", "by delete"][wrote.kind], there)),
                _ => gone.push(version.clone()),
            },
        }
    }
    if !in_the_gap.is_empty() {
        println!("versions the other program put under the name between the store's last look and its rename, and how long the witness saw each there: {in_the_gap:?}");
        if std::env::var("OK_ATTACK_SHOW_GAP").is_ok() {
            for (name, ..) in &in_the_gap {
                println!("what happened around {name}:\n{}", trace.around(&format!("{name}.")));
            }
        }
    }
    if let Some(first) = gone.first() {
        let name = first.split(' ').next().unwrap_or_default();
        println!("what happened around {name}:\n{}", trace.around(&format!("{name}.")));
    }
    assert!(gone.is_empty(), "{} of {} versions the other program wrote were replaced by the store unseen and are nowhere (unsure about {unsure} more), the first {:?}", gone.len(), log.len(), &gone[..gone.len().min(3)]);
}

/// Everything at once: all three ways of replacing the file, and other
/// programs opening it in every access and share mode.
#[test]
fn a_long_run_with_writers_and_readers_of_every_kind_costs_neither_side_a_version() {
    long_run("ao-long-run", &[TRUNCATE, RENAME, DELETE_RECREATE], true, 8, (25, 60));
}

/// Only an editor that saves by truncating the file and writing it again,
/// and nobody else at the file. The store's deny-write guard keeps such a
/// writer out for the length of a save, so nothing is ever replaced unseen.
#[test]
fn a_program_that_truncates_and_writes_never_has_a_version_replaced_unseen() {
    long_run("ao-run-truncate", &[TRUNCATE], false, 5, (12, 25));
}

/// Only a program that saves the careful way, by writing a temp file and
/// renaming it over the text, and nobody else at the file. The corrected
/// design calls the gap between the store's last check and its own rename
/// "microseconds". A version that lands in it is replaced by the store
/// without ever having been read: it is in no file, no version, and no
/// `ChangedOutside` was sent for it.
///
/// As corrected (see [`long_run`]): a version that a witness saw under the
/// name for longer than the gap must never be replaced unseen; one that was
/// there for less is the exposure the design accepts, and is counted.
#[test]
fn a_version_renamed_into_place_during_a_save_is_never_replaced_unseen() {
    long_run("ao-run-rename", &[RENAME], false, 6, (12, 25));
}

/// Only a program that deletes the text and, a few milliseconds later,
/// makes it again. If the delete lands in the same gap, the store's rename
/// puts a file back under the name the other program has just removed, and
/// that program's `create_new` then fails for good.
///
/// As corrected (see [`long_run`]): a delete that lands in the gap is
/// counted and shown, and the other program then tries again as such a
/// program would; a version it gets under the name and that stays there
/// longer than the gap must never be replaced unseen.
#[test]
fn a_file_deleted_during_a_save_is_not_put_back_by_the_store() {
    long_run("ao-run-delete", &[DELETE_RECREATE], false, 6, (12, 25));
}
