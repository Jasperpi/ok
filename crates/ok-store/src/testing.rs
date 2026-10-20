//! What this crate's tests and its headless helper share: temporary folders
//! (made under the build's own target directory and nowhere else, and removed
//! again when dropped), a store opened on them with short waits, and a
//! stream of random edits that can be produced twice.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_text::{Change, Doc, Span};

use crate::{DocId, News, Recovered, Setup, Store, Tuning};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The folder all scratch folders are made in: `<target>\ok-store-scratch`,
/// worked out from where the running test or example binary lies.
pub fn scratch_root() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if matches!(dir.file_name().and_then(|name| name.to_str()), Some("deps" | "examples")) {
        dir.pop();
    }
    dir.pop();
    dir.join("ok-store-scratch")
}

/// The headless helper built from `examples\headless.rs`, as found beside
/// the running test binary.
pub fn headless_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if matches!(dir.file_name().and_then(|name| name.to_str()), Some("deps" | "examples")) {
        dir.pop();
    }
    dir.join("examples").join("headless.exe")
}

/// A folder of its own for one test, gone again when the value is dropped.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Makes an empty folder whose name starts with `name`.
    pub fn new(name: &str) -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = scratch_root().join(format!("{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("the scratch folder can be made");
        Scratch { path }
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A scanner may hold a file for a moment after a test lets go of it.
        for _ in 0..50 {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(_) if !self.path.exists() => return,
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

/// The default tuning with every wait shortened, so that tests of holds,
/// retries and removals take milliseconds instead of seconds.
pub fn quick() -> Tuning {
    Tuning {
        flush_after: Duration::from_millis(40),
        flush_pause: Duration::from_millis(15),
        bound: Duration::from_secs(20),
        watch_settle: Duration::from_millis(30),
        missing_after: Duration::from_millis(200),
        ladder: vec![5, 10, 20],
        retry_first: Duration::from_millis(150),
        retry_every: Duration::from_millis(300),
        scan_texts: false,
        ..Tuning::default()
    }
}

/// A writing folder and a data directory of their own for one test.
pub struct Rig {
    _scratch: Scratch,
    /// The writing folder.
    pub folder: PathBuf,
    /// The data directory.
    pub data: PathBuf,
}

impl Rig {
    /// Makes both folders, empty, under a scratch folder named after `name`.
    pub fn new(name: &str) -> Rig {
        let scratch = Scratch::new(name);
        let folder = scratch.path().join("writing");
        let data = scratch.path().join("data");
        fs::create_dir_all(&folder).expect("the writing folder can be made");
        Rig { _scratch: scratch, folder, data }
    }

    /// What to open the store with: these folders, the note `test`, this tuning.
    pub fn setup(&self, tuning: Tuning) -> Setup {
        Setup { folder: self.folder.clone(), data: self.data.clone(), wake: Arc::new(|| {}), note: "test".to_string(), tuning }
    }

    /// Opens the store with [`quick`] tuning. Panics if it cannot be opened.
    pub fn open(&self) -> (Store, Vec<Recovered>) {
        self.open_with(quick())
    }

    /// Opens the store with the tuning given. Panics if it cannot be opened.
    pub fn open_with(&self, tuning: Tuning) -> (Store, Vec<Recovered>) {
        Store::open(self.setup(tuning)).expect("the store opens")
    }

    /// The journal files present, by name.
    pub fn journals(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.data.join("journal")).map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".wal")).collect()).unwrap_or_default();
        names.sort();
        names
    }

    /// The names of everything directly in the writing folder.
    pub fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.folder).map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        names.sort();
        names
    }
}

/// Polls the store until a piece of news satisfies `want`, and returns it.
/// Everything polled on the way, the wanted one included, is added to
/// `seen`. `None` when `within` runs out.
pub fn wait_news(store: &mut Store, seen: &mut Vec<News>, within: Duration, want: impl Fn(&News) -> bool) -> Option<News> {
    let end = Instant::now() + within;
    loop {
        while let Some(news) = store.poll() {
            seen.push(news.clone());
            if want(&news) {
                return Some(news);
            }
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Types `text` at the end of `doc` and records the edit. Returns its number.
pub fn type_end(store: &mut Store, id: DocId, doc: &mut Doc, text: &str) -> u64 {
    let change = doc.replace(Span::at(doc.end()), text);
    store.record(id, &change, doc.len_bytes())
}

/// Replaces the whole of `doc` with `text` and records the edit.
pub fn replace_all(store: &mut Store, id: DocId, doc: &mut Doc, text: &str) -> u64 {
    let change = doc.replace(Span { start: ok_text::Pos::default(), end: doc.end() }, text);
    store.record(id, &change, doc.len_bytes())
}

/// Asks for a save of `doc` at `edit` and waits for it to be reported.
/// Panics, listing the news that did arrive, if it is not within ten seconds.
pub fn save_wait(store: &mut Store, id: DocId, doc: &Doc, edit: u64) -> Vec<News> {
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let saved = wait_news(store, &mut seen, Duration::from_secs(10), |news| matches!(news, News::Saved { doc, edit: e, .. } if *doc == id && *e == edit));
    assert!(saved.is_some(), "no Saved for edit {edit}; news was {seen:?}");
    seen
}

const PIECES: [&str; 14] = ["a", "e", " ", "the ", "word ", "\n", "\n\n", ". ", "\u{00E9}", "n\u{0313}", "\u{4E00}", "x\ny", "[[link]] ", "TK "];

/// A stream of random edits that is the same every time for the same seed,
/// so that the process applying them and the test checking them agree on
/// what the text was after each one.
pub struct Script {
    state: u64,
}

impl Script {
    /// The stream of edits that belongs to `seed`.
    pub fn new(seed: u64) -> Script {
        Script { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1 }
    }

    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// Makes one edit to `doc` and returns it. Never a no-op. Mostly a few
    /// letters typed or deleted; now and then a paste of a few kilobytes or
    /// a large cut, so that the text stays a few kilobytes long.
    pub fn step(&mut self, doc: &mut Doc) -> Change {
        let len = doc.len_bytes();
        let at = self.below(len + 1);
        let roll = self.below(1000);
        let start = doc.pos_at(at);
        let change = if len > 24_000 && roll < 500 {
            let n = 2000 + self.below(4000);
            doc.replace(Span::new(start, doc.pos_at(at + n)), "")
        } else if roll < 220 {
            let n = 1 + self.below(12);
            doc.replace(Span::new(start, doc.pos_at(at + n)), "")
        } else if roll < 226 {
            let words = 100 + self.below(900);
            let mut paste = String::new();
            for i in 0..words {
                paste.push_str(PIECES[(i + words) % PIECES.len()]);
                paste.push_str("pasted ");
            }
            doc.replace(Span::at(start), &paste)
        } else {
            doc.replace(Span::at(start), PIECES[self.below(PIECES.len())])
        };
        if change.is_noop() {
            doc.replace(Span::at(doc.end()), "x")
        } else {
            change
        }
    }
}
