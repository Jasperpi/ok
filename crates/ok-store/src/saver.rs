//! The saver thread: the only code that touches the writing folder, the
//! history and the state files once the store is open.
//!
//! It owns what is known about each open text's file (the bytes last read or
//! written and the file's stamp) so that the comparison made before every
//! save is against the truth and not against what another thread believed.
//! It never sleeps on a busy file: a save that cannot go through is put on a
//! timer and the thread goes back to its queue, so opening a text or listing
//! versions is answered within one file operation.
//!
//! Every request for a save is answered: with `Saved`, with `Held` and the
//! reason, or with the news that says what happened to the file instead
//! (`ChangedOutside`, `Removed`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_text::doc::normalize_newlines;
use ok_text::Pos;

use crate::format::{self, Decoded, Format};
use crate::fsio::{self, Fail};
use crate::hash::{self, State};
use crate::history::{History, Version};
use crate::journal::{Attach, Barrier, JMsg};
use crate::names::{self, NameError};
use crate::recover::{self, Pending, Take};
use crate::state;
use crate::sys::{self, Event, Info, Volume, Watch, Woke};
use crate::tree::{self, Tree};
use crate::wal::{Open, Stamp};
use crate::{DocId, DocShared, Hold, News, NewsOut, Opened, ReadOnly, Resolve, Shared, StoreError, Tuning};

/// Pauses, in milliseconds, while another program is writing a file we want to read.
/// How long a file changed by another program is left to settle before it
/// is read as their version, where nothing stops us reading a file that is
/// still being written.
#[cfg(unix)]
const SETTLING_MS: u64 = 150;

const READ_PAUSES: [u64; 3] = [10, 30, 100];
/// How long a file another program has open for writing must hold still
/// before what it holds is shown.
const STILL: Duration = Duration::from_millis(40);
/// How long closing one text may go on retrying a save another program is in
/// the way of. Short, because the next thing asked for (usually opening
/// another text) waits behind it; what is not saved stays in the journal.
const CLOSE_TRIES: Duration = Duration::from_millis(250);
/// The same at shutdown, when nothing else is waiting.
const EXIT_TRIES: Duration = Duration::from_millis(1500);
/// A leftover save temp is cleared away once it is this old.
const STALE_TEMP_MS: u64 = 10 * 60 * 1000;

const WAITING: u8 = 0;
const STARTED: u8 = 1;
const WITHDRAWN: u8 = 2;

/// Where the answer to a request goes, and the word between the two threads
/// on whether the request is still wanted. The caller waits only so long; if
/// it gives up first it withdraws the request, and the saver, which must
/// claim a request before starting on it, then leaves it undone. So a request
/// answered with "it took too long" is never carried out afterwards behind
/// the caller's back.
pub(crate) struct Reply<T> {
    tx: Sender<Result<T, StoreError>>,
    claim: Arc<AtomicU8>,
}

/// The caller's end of a [`Reply`].
pub(crate) struct Asked<T> {
    pub rx: Receiver<Result<T, StoreError>>,
    claim: Arc<AtomicU8>,
}

impl<T> Reply<T> {
    pub(crate) fn new() -> (Reply<T>, Asked<T>) {
        let (tx, rx) = mpsc::channel();
        let claim = Arc::new(AtomicU8::new(WAITING));
        (Reply { tx, claim: claim.clone() }, Asked { rx, claim })
    }

    /// Claims the request for the saver. False if the caller has withdrawn it.
    fn begin(&self) -> bool {
        self.claim.compare_exchange(WAITING, STARTED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    fn send(self, result: Result<T, StoreError>) {
        let _ = self.tx.send(result);
    }
}

impl<T> Asked<T> {
    /// Withdraws the request. False if the saver has already started on it.
    pub(crate) fn withdraw(&self) -> bool {
        self.claim.compare_exchange(WAITING, WITHDRAWN, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

pub(crate) enum SMsg {
    Open { id: DocId, rel: PathBuf, create: bool, reply: Reply<(Opened, Arc<DocShared>)> },
    Save { id: DocId, text: String, edit: u64, keep: Option<String> },
    Resolve { id: DocId, how: Resolve },
    Close { id: DocId, last: Option<(String, u64)>, caret: Option<Pos> },
    Versions { id: DocId, reply: Reply<Vec<Version>> },
    ReadVersion { id: DocId, name: String, reply: Reply<String> },
    BringBack { id: DocId, name: String, present: String, reply: Reply<String> },
    Rename { from: PathBuf, to: PathBuf, reply: Reply<()> },
    Rewrite { rel: PathBuf, was: String, now: String, reply: Reply<()> },
    Discard { rel: PathBuf, last: Option<(String, u64)>, reply: Reply<PathBuf> },
    State { name: String, bytes: Vec<u8> },
    Active(bool),
    Rescan,
    /// Finish what can be finished, answer, and end the thread. The answer
    /// is false if some text is, at the end, neither in a file nor known to
    /// be whole in its journal.
    Shutdown { reply: Sender<bool> },
    /// End the thread at once, as a kill would.
    Abandon,
}

/// A save that has been asked for and not yet done.
#[derive(Clone)]
struct Want {
    text: Arc<String>,
    edit: u64,
    /// Also keep the saved text as a labelled version and report its size.
    keep: Option<String>,
}

/// What is known about the file behind an open text.
enum Disk {
    /// No file yet: a new text. Its first save must not replace anything.
    Absent,
    /// These bytes were the file when it was last read or written.
    Known(Arc<Vec<u8>>),
    /// The file could not be read; only `own` can vouch for what is there.
    Unknown,
}

/// A version another program put in the file, waiting to be taken or refused.
struct Theirs {
    bytes: Arc<Vec<u8>>,
    text: String,
    format: Format,
    mixed: bool,
    stamp: Stamp,
    created: u64,
}

struct Doc {
    id: DocId,
    docid: u64,
    key: String,
    rel: PathBuf,
    path: PathBuf,
    format: Format,
    /// The file's line breaks are not all of one kind: it is written back
    /// with every break nobody touched left as it is.
    mixed: bool,
    shared: Arc<DocShared>,
    disk: Disk,
    /// `disk` was read past a writer, without the guard: good for showing,
    /// never for keeping as a version.
    shown: bool,
    /// The edit number the file's bytes correspond to, when they are a state of the buffer.
    disk_edit: Option<u64>,
    /// The length of the text at `disk_edit`, with `\n` line breaks.
    disk_len: u64,
    stamp: Stamp,
    created: u64,
    /// Fingerprints of states a recovered journal knows to be ours.
    own: Vec<State>,
    /// The edit number and bytes the journal last confirmed at a save's barrier.
    vouched: Option<(u64, State)>,
    /// Not valid UTF-8: shown, never written.
    never: bool,
    hold: Option<Hold>,
    want: Option<Want>,
    /// A request for a save has come in and has had no answer yet.
    asked: bool,
    theirs: Option<Theirs>,
    /// How far down the quick retry ladder the present trouble has gone.
    step: usize,
    /// How many slow retries have been made since.
    slow: u32,
    retry_at: Option<Instant>,
    recheck_at: Option<Instant>,
    missing_since: Option<Instant>,
    removed: bool,
    /// The last save was renamed into place but its flush failed: write it again.
    undurable: bool,
    origin_noted: bool,
    /// When the last timed version was kept in this session.
    snapped: Option<Instant>,
    /// A save has happened since the last version was kept.
    unsnapped: bool,
    /// Size of the newest kept version, once looked up.
    newest_len: Option<Option<u64>>,
}

/// A text that was closed with something no file of his holds yet. The
/// journal thread still has it; so does this, when the interface handed its
/// last text over at close.
struct Left {
    docid: u64,
    /// The handle it had, by which the journal thread still knows it.
    id: DocId,
    /// The number of its last edit.
    latest: u64,
    /// The whole last text, if it was given.
    text: Option<Arc<String>>,
    /// The text its file held when it was closed, for the count of words.
    before: String,
    format: Format,
    own: Vec<State>,
    /// Its path inside the writing folder.
    rel: PathBuf,
    caret: Option<Pos>,
    /// The journal said it holds the text whole, on the disk.
    durable: bool,
}

/// A text coming back from a journal, or from memory, instead of from its file.
struct Back {
    docid: u64,
    /// New journal files are numbered above this generation.
    floor: u32,
    text: String,
    /// The text of the last finished save, for the count of words.
    before: String,
    format: Format,
    known: Vec<State>,
    caret: Option<Pos>,
    unsaved: bool,
    /// Journal files this text's new journal replaces.
    files: Vec<File>,
    /// The handle under which the journal thread still holds it.
    replaces: Option<DocId>,
}

/// What one attempt at a save came to.
enum Tried {
    Saved(fsio::Written),
    /// The file already holds exactly this text.
    Same(Info),
    /// The file holds something that is not ours.
    Outside(Vec<u8>, Info),
    Missing,
    /// The text is not the text at the edit number it was handed over with.
    WrongEdit,
    Fail(Fail),
}

/// What became of keeping a version.
enum Snap {
    Kept(Version),
    /// The newest version already holds exactly this.
    Same,
    Failed(Fail),
}

fn stamp_of(info: &Info) -> Stamp {
    Stamp { size: info.size, modified: info.modified, index: info.index }
}

/// The length the interface counts for a text: every line break one byte.
fn text_len(text: &str) -> u64 {
    normalize_newlines(text).len() as u64
}

/// A text as the journal counts it, with `\n` for every line break. The
/// interface is asked to hand texts over that way; one that comes otherwise
/// is put right here, so that its length is the length the edits give.
fn plain(text: String) -> String {
    if text.contains('\r') {
        normalize_newlines(&text).into_owned()
    } else {
        text
    }
}

impl From<Fail> for StoreError {
    fn from(fail: Fail) -> StoreError {
        match fail {
            Fail::Missing => StoreError::Missing,
            Fail::FolderGone => StoreError::FolderGone,
            Fail::Busy => StoreError::Busy,
            Fail::Denied | Fail::CannotWrite | Fail::NoHistory => StoreError::CannotWrite,
            Fail::ReadOnly => StoreError::ReadOnly,
            Fail::Linked => StoreError::Linked,
            Fail::NameTaken => StoreError::Name(NameError::Taken),
            Fail::DiskFull => StoreError::DiskFull,
            Fail::Changed => StoreError::Changed,
            Fail::NoOneStep => StoreError::Busy,
            Fail::TooLarge => StoreError::TooLarge,
            Fail::BadPath => StoreError::Io(123),
            Fail::NotDurable(code) | Fail::Io(code) => StoreError::Io(code),
        }
    }
}

/// What the saver is started with.
pub(crate) struct Start {
    pub root: PathBuf,
    pub data: PathBuf,
    pub volume: Volume,
    pub tuning: Tuning,
    pub history: History,
    pub rx: Receiver<SMsg>,
    pub event: Arc<Event>,
    pub j: Sender<JMsg>,
    pub out: NewsOut,
    pub shared: Arc<Shared>,
    /// Texts with a history, by lower-case path inside the folder.
    pub ids: HashMap<String, u64>,
    /// Journals recovery could not finish, by the same key.
    pub pending: HashMap<String, Pending>,
    /// Journals recovery could not read at all, by the same key.
    pub unread: HashMap<String, u64>,
    /// What this instance wrote into the folder's lock.
    pub note: String,
}

pub(crate) struct Saver {
    root: PathBuf,
    data: PathBuf,
    volume: Volume,
    tuning: Tuning,
    history: History,
    rx: Receiver<SMsg>,
    event: Arc<Event>,
    j: Sender<JMsg>,
    out: NewsOut,
    shared: Arc<Shared>,
    ids: HashMap<String, u64>,
    pending: HashMap<String, Pending>,
    unread: HashMap<String, u64>,
    /// Texts closed in this session with something still in no file of his.
    left: HashMap<String, Left>,
    docs: Vec<Doc>,
    /// The one-step replacing rename is in use on this volume.
    posix: bool,
    /// The writing folder's own file index, to find it again if it is renamed.
    root_index: u64,
    note: String,
    /// Locks on the names the writing folder has been followed to.
    locks: Vec<state::Lock>,
    watch: Option<Watch>,
    active: bool,
    rescan_at: Option<Instant>,
    first_scan: bool,
    tree: Option<Arc<Tree>>,
    /// Size and last-write time of every text whose contents have been sent.
    seen: HashMap<String, (u64, u64)>,
    /// Texts waiting to be read, and their keys, so that asking whether one
    /// is already waiting does not mean walking the queue.
    queue: VecDeque<PathBuf>,
    queued: HashSet<String>,
    states: HashMap<String, Vec<u8>>,
    states_at: Option<Instant>,
}

impl Saver {
    pub(crate) fn new(start: Start) -> Saver {
        let Start { root, data, volume, tuning, history, rx, event, j, out, shared, ids, pending, unread, note } = start;
        let posix = volume.atomic_replace && tuning.faults.no_one_step.load(Ordering::Relaxed) == 0;
        let root_index = sys::stat(&root).map(|info| info.index).unwrap_or(0);
        Saver {
            root,
            data,
            volume,
            tuning,
            history,
            rx,
            event,
            j,
            out,
            shared,
            ids,
            pending,
            unread,
            left: HashMap::new(),
            docs: Vec::new(),
            posix,
            root_index,
            note,
            locks: Vec::new(),
            watch: None,
            active: true,
            rescan_at: None,
            first_scan: true,
            tree: None,
            seen: HashMap::new(),
            queue: VecDeque::new(),
            queued: HashSet::new(),
            states: HashMap::new(),
            states_at: None,
        }
    }

    fn index(&self, id: DocId) -> Option<usize> {
        self.docs.iter().position(|doc| doc.id == id)
    }

    fn index_of_key(&self, key: &str) -> Option<usize> {
        self.docs.iter().position(|doc| doc.key == key)
    }

    /// Changes why a text is held. The interface is told when that is news,
    /// and also when a request for a save is waiting for its answer; `quiet`
    /// is for the cases in which other news says it instead.
    fn set_hold(&mut self, i: usize, hold: Option<Hold>, quiet: bool) {
        let doc = &mut self.docs[i];
        let changed = doc.hold != hold;
        doc.hold = hold;
        let id = doc.id;
        match hold {
            Some(why) => {
                let asked = std::mem::take(&mut doc.asked);
                if !quiet && (changed || asked) {
                    self.out.send(News::Held { doc: id, why });
                }
            }
            None if changed && !quiet => self.out.send(News::Released { doc: id }),
            None => {}
        }
    }

    fn note_origin(&mut self, i: usize) {
        let doc = &mut self.docs[i];
        if !doc.origin_noted {
            doc.origin_noted = self.history.set_origin(doc.docid, &self.root, &doc.rel);
        }
    }

    /// Keeps `bytes` as a version of text `i`.
    fn snap(&mut self, i: usize, bytes: &[u8], label: Option<&str>, force: bool) -> Snap {
        self.note_origin(i);
        let doc = &mut self.docs[i];
        let ending = names::ending_of(&doc.rel);
        match self.history.snapshot(doc.docid, &ending, bytes, label, sys::unix_ms_now(), force) {
            Ok(Some(version)) => {
                doc.newest_len = Some(Some(version.bytes));
                Snap::Kept(version)
            }
            Ok(None) => Snap::Same,
            Err(fail) => Snap::Failed(fail),
        }
    }

    /// The bytes text `i` is written as: in its file's own line-ending style,
    /// and, where the file's breaks are not all of one kind, with every break
    /// nobody touched copied from the file as it stands.
    fn bytes_for(&self, i: usize, text: &str) -> Vec<u8> {
        let doc = &self.docs[i];
        match (&doc.disk, doc.mixed) {
            (Disk::Known(disk), true) => format::encode_like(disk, text, doc.format),
            _ => format::encode(text, doc.format),
        }
    }

    /// A version he asked to be kept, and the save it was to follow cannot
    /// be made for a reason that waiting does not cure: it is kept now, from
    /// the text it was asked for with, and the label is spent. So the label
    /// can never ride on to a later request and land on another text.
    fn keep_now(&mut self, i: usize) {
        let Some(want) = self.docs[i].want.as_mut() else { return };
        let Some(label) = want.keep.take() else { return };
        let (text, edit) = (want.text.clone(), want.edit);
        let bytes = format::encode(&text, self.docs[i].format);
        let id = self.docs[i].id;
        match self.snap(i, &bytes, Some(label.as_str()), true) {
            Snap::Kept(version) => self.out.send(News::Kept { doc: id, edit, bytes: version.bytes, at_ms: sys::unix_ms_now() }),
            _ => self.out.send(News::NotKept { doc: id, edit }),
        }
    }

    // ---------------------------------------------------------------- opening

    /// The text for `key` that waits in a journal instead of in its file: one
    /// recovery could not put back at start, one whose journal could not be
    /// read then, or one closed earlier in this session with something unsaved.
    fn coming_back(&mut self, key: &str) -> Result<Option<Back>, StoreError> {
        if let Some(Pending { docid, generation, replay, files }) = self.pending.remove(key) {
            let unsaved = replay.unsaved();
            return Ok(Some(Back { docid, floor: generation, before: replay.open.text.as_str().to_string(), format: replay.format, known: replay.known, caret: replay.caret, text: replay.text, unsaved, files, replaces: None }));
        }
        if let Some(docid) = self.unread.remove(key) {
            // Whatever had the journal at start may have let go of it by now.
            // If it has not, the text is opened as its file stands and this
            // session goes on from there; the journal stays where it is, a
            // new one is numbered above it, and the next start, which knows
            // an earlier session's journal from a later one's, puts its text
            // beside the file.
            let dir = self.data.join("journal");
            if let Take::Taken(taken) = recover::take(&dir, docid, recover::generations(&dir, docid), &recover::TAKE_PAUSES) {
                let replay = taken.replay;
                let unsaved = replay.unsaved();
                return Ok(Some(Back { docid, floor: taken.generation, before: replay.open.text.as_str().to_string(), format: replay.format, known: replay.known, caret: replay.caret, text: replay.text, unsaved, files: taken.files, replaces: None }));
            }
            return Ok(None);
        }
        self.take_left(key)
    }

    /// Takes up a text that was closed earlier in this session with
    /// something unsaved. Its last text is in memory here if the interface
    /// gave it at close, and with the journal thread, which is asked for
    /// what it holds; the journal file itself is read only if that thread is
    /// gone. No program that happens to be reading the journal file can be
    /// in the way of this.
    fn take_left(&mut self, key: &str) -> Result<Option<Back>, StoreError> {
        let Some(left) = self.left.get(key) else { return Ok(None) };
        let (tx, rx) = mpsc::channel();
        let held = match self.j.send(JMsg::Held { id: left.id, reply: tx }) {
            Ok(()) => match rx.recv_timeout(self.tuning.barrier) {
                Ok(held) => held,
                // The journal thread is waiting on a disk. With no text in
                // hand there is nothing true to show yet: the text stays
                // where it is and the caller asks again.
                Err(RecvTimeoutError::Timeout) if left.text.is_none() => return Err(StoreError::Timeout),
                Err(_) => None,
            },
            Err(_) => None,
        };
        let Some(left) = self.left.remove(key) else { return Ok(None) };
        let mut back = Back { docid: left.docid, floor: 0, text: String::new(), before: left.before, format: left.format, known: left.own, caret: left.caret, unsaved: true, files: Vec::new(), replaces: None };
        let journaled = match held {
            Some(held) => {
                back.floor = held.generation;
                back.known.extend(held.known);
                back.caret = held.caret.or(back.caret);
                back.replaces = Some(left.id);
                Some(held.text)
            }
            None => {
                // The journal thread no longer has it: what it wrote is in its file.
                let dir = self.data.join("journal");
                match recover::take(&dir, left.docid, recover::generations(&dir, left.docid), &recover::TAKE_PAUSES) {
                    Take::Taken(taken) => {
                        back.floor = taken.generation;
                        back.known.extend(taken.replay.known.iter().copied());
                        back.caret = taken.replay.caret.or(back.caret);
                        back.files = taken.files;
                        Some(taken.replay.text)
                    }
                    _ => None,
                }
            }
        };
        // What the interface handed over at close is the text itself; the
        // journal's rebuilding of it is the next best thing.
        match left.text.map(|text| text.as_str().to_string()).or(journaled) {
            Some(text) => {
                back.text = text;
                Ok(Some(back))
            }
            None => Ok(None),
        }
    }

    fn open(&mut self, id: DocId, rel: &Path, create: bool) -> Result<(Opened, Arc<DocShared>), StoreError> {
        let rel = names::check_rel(rel, create).map_err(StoreError::Name)?;
        let key = names::key(&rel);
        if let Some(i) = self.index_of_key(&key) {
            return Err(StoreError::AlreadyOpen(self.docs[i].id));
        }
        let path = self.root.join(&rel);
        let known_id = self.ids.get(&key).copied();
        let limit = self.tuning.open_limit;
        let mut doc = Doc {
            id,
            docid: known_id.unwrap_or(0),
            key: key.clone(),
            rel: rel.clone(),
            path: path.clone(),
            format: Format::default(),
            mixed: false,
            shared: Arc::new(DocShared::default()),
            disk: Disk::Absent,
            shown: false,
            disk_edit: Some(0),
            disk_len: 0,
            stamp: Stamp::default(),
            created: 0,
            own: Vec::new(),
            vouched: None,
            never: false,
            hold: None,
            want: None,
            asked: false,
            theirs: None,
            step: 0,
            slow: 0,
            retry_at: None,
            recheck_at: None,
            missing_since: None,
            removed: false,
            undurable: false,
            origin_noted: known_id.is_some(),
            snapped: None,
            unsnapped: false,
            newest_len: None,
        };
        let mut opened = Opened { doc: id, text: String::new(), format: Format::default(), mixed_endings: false, read_only: None, caret: None, restored: None, edit: 0 };
        let mut attach = Attach {
            id,
            docid: 0,
            base: Open { edit: 0, wall_ms: sys::unix_ms_now(), on_disk: false, format: Format::default(), disk: (0, 0), stamp: Stamp::default(), known: Vec::new(), folder: self.root.clone(), rel: rel.clone(), caret: None, text: Arc::new(String::new()) },
            floor: 0,
            eager: false,
            spent: Vec::new(),
            replaces: None,
        };

        if create {
            if names::taken(&path) || self.pending.contains_key(&key) || self.unread.contains_key(&key) || self.left.contains_key(&key) {
                return Err(StoreError::Name(NameError::Taken));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| StoreError::from(fsio::open_fail(sys::code(&e))))?;
            }
        } else if let Some(back) = self.coming_back(&key)? {
            // A journal that could not be finished, at start or when the text
            // was last closed: what was typed then is the text now, unsaved.
            let Back { docid, floor, text, before, format, known, caret, unsaved, files, replaces } = back;
            doc.docid = docid;
            doc.format = format;
            doc.own = known;
            doc.disk = Disk::Unknown;
            doc.disk_edit = None;
            let mut before = Some(before);
            let mut mine = format::encode(&text, format);
            match fsio::read_patiently(&path, limit, &READ_PAUSES) {
                Ok(found) => {
                    doc.stamp = stamp_of(&found.info);
                    doc.created = found.info.created;
                    doc.mixed = matches!(format::decode(&found.bytes), Decoded::Text { mixed: true, .. });
                    mine = format::encode_like(&found.bytes, &text, format);
                    if found.bytes == mine {
                        let info = found.info;
                        doc.disk = Disk::Known(Arc::new(found.bytes));
                        drop(found.file);
                        // The file says everything the journal does. If no
                        // finished save vouches for it, flush it now; failing
                        // that the journal stays and the next save rewrites it.
                        let read_only = info.attrs & sys::ATTR_READONLY != 0;
                        if !unsaved || read_only || fsio::flush_in_place(&path) {
                            before = None;
                            doc.disk_edit = Some(0);
                            doc.disk_len = text.len() as u64;
                        } else {
                            before = Some(text.clone());
                            doc.undurable = true;
                        }
                    } else if doc.own.contains(&hash::state(&found.bytes)) {
                        if let Decoded::Text { text, .. } = format::decode(&found.bytes) {
                            before = Some(text);
                        }
                        doc.disk = Disk::Known(Arc::new(found.bytes));
                    }
                }
                Err(Fail::Missing) => doc.disk = Disk::Absent,
                Err(_) => {}
            }
            opened.restored = before.map(|before| format::words_added(&before, &text));
            opened.caret = caret;
            opened.format = format;
            opened.mixed_endings = doc.mixed;
            opened.text = text;
            attach.base.format = format;
            attach.base.known = doc.own.clone();
            attach.floor = floor;
            attach.replaces = replaces;
            if opened.restored.is_some() {
                attach.eager = true;
                attach.spent = files;
            } else {
                // The file already says everything the journal does.
                attach.base.on_disk = true;
                attach.base.disk = hash::state(&mine);
                attach.base.stamp = doc.stamp;
                for file in &files {
                    sys::delete_on_close(file);
                }
            }
        } else {
            let (bytes, info, busy) = match fsio::read_patiently(&path, limit, &READ_PAUSES) {
                Ok(found) => {
                    // The name as it is spelled on the disk is the one to
                    // save under: opening `the orchard.md` must not cost
                    // `The Orchard.md` its capitals at the first save.
                    if let Some(spelled) = sys::final_path(&found.file).and_then(|real| self.respelled(&key, &real)) {
                        if spelled != rel {
                            doc.path = self.root.join(&spelled);
                            attach.base.rel = spelled.clone();
                            doc.rel = spelled;
                            doc.origin_noted = false;
                        }
                    }
                    (found.bytes, found.info, false)
                }
                Err(Fail::Busy) => {
                    // Another program has it open for writing. What is there
                    // is shown, for reading only, if it is something and
                    // holds still: a file caught between a truncate and the
                    // write that follows is empty or changing, and is never
                    // handed over as the text.
                    let (first, _) = fsio::read_shared(&path, limit)?;
                    std::thread::sleep(STILL);
                    let (bytes, info) = fsio::read_shared(&path, limit)?;
                    if bytes.is_empty() || bytes != first {
                        return Err(StoreError::Busy);
                    }
                    (bytes, info, true)
                }
                Err(fail) => return Err(fail.into()),
            };
            match format::decode(&bytes) {
                Decoded::Text { text, format, mixed } => {
                    opened.text = text;
                    opened.format = format;
                    opened.mixed_endings = mixed;
                    doc.format = format;
                    doc.mixed = mixed;
                    attach.base.on_disk = !busy;
                }
                Decoded::Other { lossy } => {
                    opened.text = lossy;
                    opened.read_only = Some(ReadOnly::NotUtf8);
                    doc.never = true;
                    doc.hold = Some(Hold::NotUtf8);
                }
            }
            if opened.read_only.is_none() {
                if busy {
                    opened.read_only = Some(ReadOnly::Busy);
                    doc.hold = Some(Hold::Busy);
                    doc.recheck_at = Some(Instant::now() + Duration::from_millis(500));
                } else if info.attrs & sys::ATTR_READONLY != 0 {
                    opened.read_only = Some(ReadOnly::Attribute);
                    doc.hold = Some(Hold::ReadOnly);
                } else if info.links > 1 {
                    opened.read_only = Some(ReadOnly::Linked);
                    doc.hold = Some(Hold::Linked);
                }
            }
            doc.shown = busy;
            doc.stamp = stamp_of(&info);
            doc.created = info.created;
            doc.disk_len = opened.text.len() as u64;
            attach.base.format = opened.format;
            attach.base.disk = hash::state(&bytes);
            attach.base.stamp = doc.stamp;
            doc.disk = Disk::Known(Arc::new(bytes));
        }

        if doc.docid == 0 {
            doc.docid = *self.ids.entry(key).or_insert_with(|| hash::random() | 1);
            // Where this text lives is noted from the first moment it has an
            // id: a journal that cannot be read at some later start is then
            // still known to be this folder's, and is spoken of.
            doc.origin_noted = self.history.set_origin(doc.docid, &self.root, &doc.rel);
        } else {
            self.ids.insert(key, doc.docid);
        }
        if opened.caret.is_none() {
            opened.caret = self.history.caret(doc.docid);
        }
        if known_id.is_some() {
            self.history.thin(doc.docid, sys::unix_ms_now());
        }
        attach.docid = doc.docid;
        attach.base.caret = opened.caret;
        attach.base.text = Arc::new(opened.text.clone());
        let shared = doc.shared.clone();
        let _ = self.j.send(JMsg::Attach(Box::new(attach)));
        self.docs.push(doc);
        Ok((opened, shared))
    }

    /// The path inside the folder that `real` is, if it is the text `key`
    /// names: the same name as the disk spells it.
    fn respelled(&self, key: &str, real: &Path) -> Option<PathBuf> {
        let rel = real.strip_prefix(&self.root).ok()?;
        (names::key(rel) == key).then(|| rel.to_path_buf())
    }

    /// The file of text `i` is spelled `rel` on the disk now (he changed its
    /// capitals in Explorer): the text goes by that spelling from here on,
    /// so that a save cannot put the old one back.
    fn adopt_spelling(&mut self, i: usize, rel: PathBuf) {
        let doc = &mut self.docs[i];
        let old = std::mem::replace(&mut doc.rel, rel.clone());
        doc.path = self.root.join(&rel);
        doc.origin_noted = false;
        let id = doc.id;
        let (tx, _unheard) = mpsc::channel();
        let _ = self.j.send(JMsg::Path { id, old, new: rel.clone(), reply: tx });
        self.note_origin(i);
        self.out.send(News::Moved { doc: id, rel });
    }

    // ----------------------------------------------------------------- saving

    /// One whole try at putting text `i`'s wanted save into its file.
    fn attempt(&mut self, i: usize) {
        let Some(want) = self.docs[i].want.clone() else { return };
        let doc = &mut self.docs[i];
        doc.retry_at = None;
        if doc.never {
            // Never written to its own file. The wanted text is kept so that
            // closing can put it in a file beside it; every request is answered.
            self.keep_now(i);
            self.set_hold(i, Some(Hold::NotUtf8), false);
            return;
        }
        if doc.removed {
            self.keep_now(i);
            self.set_hold(i, Some(Hold::Removed), false);
            return;
        }
        match doc.theirs.as_ref().map(|theirs| theirs.text == *want.text) {
            // Still two versions; the journal holds ours until he chooses.
            Some(false) => {
                self.keep_now(i);
                self.set_hold(i, Some(Hold::ChangedOutside), false);
                return;
            }
            // The buffer now reads as the outside version does: theirs is taken.
            Some(true) => {
                if let Some(theirs) = doc.theirs.take() {
                    doc.format = theirs.format;
                    doc.mixed = theirs.mixed;
                    doc.stamp = theirs.stamp;
                    doc.created = theirs.created;
                    doc.disk = Disk::Known(theirs.bytes);
                    doc.shown = false;
                }
            }
            None => {}
        }
        let bytes = Arc::new(self.bytes_for(i, &want.text));
        let tried = self.write_through(i, &want, &bytes);
        match tried {
            Tried::Saved(written) => {
                if written.posix_refused {
                    self.no_one_step();
                }
                let doc = &mut self.docs[i];
                doc.created = written.created;
                doc.unsnapped = true;
                self.saved(i, want, bytes, written.stamp, true);
            }
            Tried::Same(info) => self.saved(i, want, bytes, stamp_of(&info), false),
            Tried::Outside(found, info) => self.outside(i, found, info),
            Tried::Missing => self.missing(i),
            Tried::WrongEdit => {
                // Nothing was written, and nothing can be: asking again with
                // the same pair would get the same answer.
                self.keep_now(i);
                self.docs[i].want = None;
                self.set_hold(i, Some(Hold::WrongEdit), false);
            }
            Tried::Fail(Fail::NoOneStep) => {
                // Not a volume for the one-step rename after all: use the old one from now on.
                self.no_one_step();
                self.docs[i].retry_at = Some(Instant::now());
            }
            Tried::Fail(fail) => self.failed(i, fail, bytes),
        }
    }

    /// The volume refused the one-step replacing rename it claimed to have.
    fn no_one_step(&mut self) {
        if self.posix {
            self.posix = false;
            self.out.send(News::NotAtomic);
        }
    }

    /// Steps b to j of a save: guard and compare, journal barrier, then the
    /// temp, its two flushes and the rename between them.
    fn write_through(&mut self, i: usize, want: &Want, bytes: &Arc<Vec<u8>>) -> Tried {
        let limit = self.tuning.open_limit;
        // Where the new file goes: the text's own path, unless the file
        // turns out to live elsewhere (see below).
        let mut target = self.docs[i].path.clone();
        let mut guard: Option<File> = None;
        let mut before: Option<Info> = None;
        let mut same = None;
        if !matches!(self.docs[i].disk, Disk::Absent) {
            let (file, info) = match fsio::open_guard(&target) {
                Ok(opened) => opened,
                Err(Fail::Missing) => return Tried::Missing,
                Err(fail) => return Tried::Fail(fail),
            };
            // The handle knows where the file really is. If that is the same
            // name in other capitals, he has renamed it and the new spelling
            // is the text's name now. If it is another place altogether, the
            // name in the folder is a link: the save goes through to the
            // file it leads to and the link stays a link.
            if let Some(real) = sys::final_path(&file) {
                if real != target {
                    if let Some(spelled) = self.respelled(&self.docs[i].key, &real) {
                        self.adopt_spelling(i, spelled);
                    }
                    target = real;
                }
            }
            let doc = &self.docs[i];
            let away = info.attrs & sys::ATTR_PLACEHOLDER != 0 && info.size == doc.stamp.size && info.modified == doc.stamp.modified;
            if !away {
                let found = match fsio::read_all(&file, info.size, limit) {
                    Ok(found) => found,
                    Err(fail) => return Tried::Fail(fail),
                };
                let ours = matches!(&doc.disk, Disk::Known(disk) if **disk == found) || doc.own.contains(&hash::state(&found));
                if !ours {
                    return Tried::Outside(found, info);
                }
                if found == **bytes && !doc.undurable {
                    same = Some(info);
                } else if let Err(fail) = self.before_write(i, &found, bytes.len() as u64) {
                    return Tried::Fail(fail);
                }
            }
            if same.is_none() {
                if info.attrs & sys::ATTR_READONLY != 0 {
                    return Tried::Fail(Fail::ReadOnly);
                }
                if info.links > 1 {
                    return Tried::Fail(Fail::Linked);
                }
            }
            guard = Some(file);
            before = Some(info);
        }

        // Every edit in this text must be on the journal disk before the
        // folder changes, so that recovery can tell our states from others';
        // and the journal, which has counted every edit, says whether this
        // text is the text at the edit number it came with. A file that
        // already holds the text goes through the same door, once: that is
        // how the journal learns that a version taken from another program
        // is ours from here on.
        let temp = fsio::temp_name(&target);
        let state = hash::state(bytes);
        let doc = &self.docs[i];
        if same.is_none() || doc.vouched != Some((want.edit, state)) {
            let (tx, rx) = mpsc::channel();
            let begun = JMsg::Begin { id: doc.id, edit: want.edit, state, format: doc.format, text: want.text.clone(), temp: temp.clone(), reply: tx };
            // If the journal cannot confirm (its disk is failing or slow) the
            // save goes ahead all the same: getting the text into the file is
            // then the best protection there is. Not so when the journal
            // thread has ended because the store is shutting down: everything
            // typed is already flushed there, and a save it can no longer
            // record would leave a file the next start could not recognise
            // as ours.
            let answer = match self.j.send(begun) {
                Ok(()) => rx.recv_timeout(self.tuning.barrier),
                Err(_) => Err(RecvTimeoutError::Disconnected),
            };
            match answer {
                Ok(Barrier::Mismatch) => return Tried::WrongEdit,
                Ok(Barrier::Ready) => self.docs[i].vouched = Some((want.edit, state)),
                Ok(Barrier::Unconfirmed) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    if self.shared.stopping.load(Ordering::Acquire) {
                        return Tried::Fail(Fail::Busy);
                    }
                }
            }
        }
        if let Some(info) = same {
            return Tried::Same(info);
        }

        let posix = self.posix;
        let write = fsio::Write { target: &target, first_temp: Some(&temp), bytes, created: before.map(|info| info.created), replace: before.is_some(), posix, faults: &self.tuning.faults };
        let result = fsio::write_file(&write, &mut |look| match &before {
            Some(info) => fsio::verify_unchanged(&target, info, posix, &mut guard, look),
            None => Ok(()),
        });
        drop(guard);
        match result {
            Ok(written) => Tried::Saved(written),
            Err(fail) => Tried::Fail(fail),
        }
    }

    /// Versions kept just before a file is replaced: the text as it was when
    /// this session first touched it, and the text as it stands whenever a
    /// save would cut it down by more than a fifth (and more than 1 KiB).
    /// The second is not optional: if the fuller text cannot be kept, the
    /// cut is not written over it.
    fn before_write(&mut self, i: usize, on_disk: &[u8], new_len: u64) -> Result<(), Fail> {
        if self.docs[i].snapped.is_none() && !self.docs[i].unsnapped {
            self.snap(i, on_disk, None, false);
        }
        let doc = &mut self.docs[i];
        let newest = match doc.newest_len {
            Some(len) => len,
            None => {
                let len = self.history.list(doc.docid).first().map(|version| version.bytes);
                doc.newest_len = Some(len);
                len
            }
        };
        let reference = newest.unwrap_or(on_disk.len() as u64).max(on_disk.len() as u64);
        if reference > new_len && reference - new_len > (reference / 5).max(1024) {
            if let Snap::Failed(_) = self.snap(i, on_disk, Some("before-cut"), false) {
                return Err(Fail::NoHistory);
            }
        }
        Ok(())
    }

    /// The file holds the wanted text and is known to be durable.
    fn saved(&mut self, i: usize, want: Want, bytes: Arc<Vec<u8>>, stamp: Stamp, wrote: bool) {
        let doc = &mut self.docs[i];
        if doc.mixed {
            doc.mixed = matches!(format::decode(&bytes), Decoded::Text { mixed: true, .. });
        }
        doc.disk = Disk::Known(bytes.clone());
        doc.shown = false;
        doc.disk_edit = Some(want.edit);
        doc.disk_len = text_len(&want.text);
        doc.stamp = stamp;
        doc.own.clear();
        doc.undurable = false;
        doc.step = 0;
        doc.slow = 0;
        doc.retry_at = None;
        doc.missing_since = None;
        doc.want = None;
        doc.asked = false;
        let (id, format) = (doc.id, doc.format);
        self.seen.insert(self.docs[i].key.clone(), (stamp.size, stamp.modified));
        let _ = self.j.send(JMsg::Done { id, edit: want.edit, state: hash::state(&bytes), stamp, format, text: want.text.clone() });
        self.set_hold(i, None, true);
        self.out.send(News::Saved { doc: id, edit: want.edit, bytes: stamp.size, at_ms: sys::filetime_to_unix_ms(stamp.modified) });
        if wrote {
            let due = self.docs[i].snapped.is_none_or(|at| at.elapsed() >= self.tuning.snapshot_every);
            if due {
                self.snap(i, &bytes, None, false);
                let doc = &mut self.docs[i];
                doc.snapped = Some(Instant::now());
                doc.unsnapped = false;
                self.history.thin(doc.docid, sys::unix_ms_now());
            }
        }
        if let Some(label) = &want.keep {
            // He is told a version was kept only if one was, and the size he
            // is told is read back from the file system, not assumed.
            match self.snap(i, &bytes, Some(label.as_str()), true) {
                Snap::Kept(_) => {
                    let on_disk = sys::stat(&self.docs[i].path).map(|info| info.size).unwrap_or(stamp.size);
                    self.out.send(News::Kept { doc: id, edit: want.edit, bytes: on_disk, at_ms: sys::unix_ms_now() });
                }
                _ => self.out.send(News::NotKept { doc: id, edit: want.edit }),
            }
        }
    }

    /// A save did not go through. Quick retries first, saying nothing; then
    /// the text is held, the interface is told why, and slow retries go on.
    fn failed(&mut self, i: usize, fail: Fail, bytes: Arc<Vec<u8>>) {
        let now = Instant::now();
        let (hold, quick) = match fail {
            Fail::Busy | Fail::Denied | Fail::Changed | Fail::Missing | Fail::NoOneStep => (Hold::Busy, true),
            Fail::ReadOnly => (Hold::ReadOnly, false),
            Fail::Linked => (Hold::Linked, false),
            Fail::NameTaken => (Hold::NameTaken, false),
            Fail::DiskFull => (Hold::DiskFull, false),
            Fail::CannotWrite => (Hold::CannotWrite, false),
            Fail::FolderGone => (Hold::FolderGone, false),
            Fail::NoHistory => (Hold::NoHistory, false),
            Fail::NotDurable(_) => (Hold::NotDurable, true),
            Fail::TooLarge | Fail::BadPath => (Hold::Io(123), false),
            Fail::Io(code) => (Hold::Io(code), true),
        };
        let doc = &mut self.docs[i];
        if let Fail::NotDurable(_) = fail {
            // The rename happened, so the file is ours; only its durability is in doubt.
            if let Ok(info) = sys::stat(&doc.path) {
                doc.stamp = stamp_of(&info);
            }
            doc.disk = Disk::Known(bytes);
            doc.disk_edit = None;
            doc.undurable = true;
        }
        if quick && doc.step < self.tuning.ladder.len() {
            doc.retry_at = Some(now + Duration::from_millis(self.tuning.ladder[doc.step]));
            doc.step += 1;
            return;
        }
        doc.retry_at = match hold {
            Hold::NameTaken => None,
            _ if doc.slow == 0 => Some(now + self.tuning.retry_first),
            _ => Some(now + self.tuning.retry_every),
        };
        doc.slow += 1;
        if matches!(hold, Hold::ReadOnly | Hold::Linked | Hold::NameTaken) {
            self.keep_now(i);
        }
        self.set_hold(i, Some(hold), false);
        if hold == Hold::FolderGone && self.follow_folder() {
            self.docs[i].retry_at = Some(now);
        }
    }

    // ------------------------------------------------------- outside changes

    /// The file holds bytes that are not ours.
    fn outside(&mut self, i: usize, found: Vec<u8>, info: Info) {
        let doc = &mut self.docs[i];
        let latest = doc.shared.latest.load(Ordering::Acquire);
        let mut clean = doc.disk_edit == Some(latest) && doc.want.is_none();
        let id = doc.id;
        // What was read past a writer when the text was opened is not a
        // version of anything: it is not kept as one.
        let old = match &doc.disk {
            Disk::Known(bytes) if !doc.shown => Some(bytes.clone()),
            _ => None,
        };
        doc.missing_since = None;
        doc.step = 0;
        let decoded = format::decode(&found);
        if clean {
            // The interface is about to put their version in the buffer.
            // What we had saved goes into the history first; if it cannot,
            // the buffer is the only copy and both versions are kept apart.
            if let Some(old) = old {
                if let Snap::Failed(_) = self.snap(i, &old, Some("before-reload"), false) {
                    clean = false;
                }
            }
        } else if let Decoded::Text { .. } = decoded {
            self.snap(i, &found, Some("outside"), false);
        }
        let doc = &mut self.docs[i];
        match decoded {
            Decoded::Text { text, format, mixed } => {
                doc.theirs = Some(Theirs { bytes: Arc::new(found), text: text.clone(), format, mixed, stamp: stamp_of(&info), created: info.created });
                doc.never = false;
                self.set_hold(i, Some(Hold::ChangedOutside), true);
                self.out.send(News::ChangedOutside { doc: id, text, format, clean, edit: latest, read_only: None });
            }
            Decoded::Other { lossy } => {
                // It is no longer UTF-8: from here on it is shown and never written.
                doc.never = true;
                doc.theirs = None;
                doc.stamp = stamp_of(&info);
                doc.created = info.created;
                doc.disk = Disk::Known(Arc::new(found));
                doc.shown = false;
                let format = doc.format;
                self.set_hold(i, Some(Hold::NotUtf8), false);
                self.out.send(News::ChangedOutside { doc: id, text: lossy, format, clean, edit: latest, read_only: Some(ReadOnly::NotUtf8) });
            }
        }
    }

    /// The file is not where it was. Programs that save by deleting and then
    /// renaming make it vanish for a moment, so nothing is declared yet.
    fn missing(&mut self, i: usize) {
        let doc = &mut self.docs[i];
        if matches!(doc.disk, Disk::Absent) {
            return;
        }
        doc.missing_since.get_or_insert_with(Instant::now);
    }

    /// The file has stayed away: follow it if it was only renamed inside the
    /// folder, otherwise say it was removed.
    fn declare_missing(&mut self, i: usize) {
        if sys::stat(&self.docs[i].path).is_ok() {
            self.docs[i].missing_since = None;
            self.check(i);
            // A save that was waiting to learn where the file went is made now.
            if self.docs[i].want.is_some() && self.docs[i].theirs.is_none() {
                self.docs[i].retry_at = Some(Instant::now());
            }
            return;
        }
        if sys::stat(&self.root).is_err() {
            // The folder itself is away; that is a different trouble.
            self.docs[i].missing_since = None;
            if !self.follow_folder() {
                self.set_hold(i, Some(Hold::FolderGone), false);
            }
            return;
        }
        if let Some(rel) = self.find_renamed(i) {
            let (tx, rx) = mpsc::channel();
            let doc = &mut self.docs[i];
            let old = std::mem::replace(&mut doc.rel, rel.clone());
            if self.j.send(JMsg::Path { id: doc.id, old, new: rel.clone(), reply: tx }).is_ok() {
                let _ = rx.recv_timeout(self.tuning.barrier);
            }
            doc.path = self.root.join(&rel);
            let old_key = std::mem::replace(&mut doc.key, names::key(&rel));
            doc.missing_since = None;
            doc.origin_noted = false;
            let (id, docid, key) = (doc.id, doc.docid, doc.key.clone());
            self.ids.remove(&old_key);
            self.ids.insert(key, docid);
            self.note_origin(i);
            self.out.send(News::Moved { doc: id, rel });
            self.check(i);
            // The save that found the name gone and waited to learn why is
            // made now, under the name the file has.
            let doc = &mut self.docs[i];
            if doc.want.is_some() && doc.theirs.is_none() {
                doc.retry_at = Some(Instant::now());
            }
            return;
        }
        let doc = &mut self.docs[i];
        doc.removed = true;
        doc.missing_since = None;
        let id = doc.id;
        self.set_hold(i, Some(Hold::Removed), true);
        self.out.send(News::Removed { doc: id });
    }

    /// Looks for an open text's file under another name, by its file index.
    fn find_renamed(&self, i: usize) -> Option<PathBuf> {
        let doc = &self.docs[i];
        if !self.volume.open_by_id || doc.stamp.index == 0 {
            return None;
        }
        let hint = sys::open_dir(&self.root).ok()?;
        let full = sys::final_path(&sys::open_by_id(&hint, doc.stamp.index)?)?;
        let rel = names::check_rel(full.strip_prefix(&self.root).ok()?, false).ok()?;
        let hidden = rel.components().any(|part| part.as_os_str().to_string_lossy().starts_with('.'));
        if hidden || self.index_of_key(&names::key(&rel)).is_some() {
            return None;
        }
        Some(rel)
    }

    /// Finds the writing folder again after it was renamed or moved on its
    /// volume, and carries on there.
    fn follow_folder(&mut self) -> bool {
        if sys::stat(&self.root).is_ok() || !self.volume.open_by_id || self.root_index == 0 {
            return false;
        }
        let Ok(hint) = sys::open_dir(&sys::volume_root(&self.root)) else { return false };
        let Some(found) = sys::open_by_id(&hint, self.root_index) else { return false };
        let Some(path) = sys::final_path(&found) else { return false };
        drop(found);
        let lower = path.to_string_lossy().to_lowercase();
        if lower.contains("$recycle.bin") || lower.contains("$extend") || names::same_folder(&path, &self.root) {
            return false;
        }
        if !sys::stat(&path).is_ok_and(|info| info.attrs & sys::ATTR_DIRECTORY != 0) {
            return false;
        }
        // The folder is ours under its new name too: a second launch that
        // finds it there must be told who has it, not let in.
        if let Ok(lock) = state::take_lock(&self.data, &path, &self.note, false) {
            self.locks.push(lock);
        }
        self.history.move_folder(&self.root, &path);
        self.root = path.clone();
        let now = Instant::now();
        for doc in &mut self.docs {
            doc.path = self.root.join(&doc.rel);
            if doc.want.is_some() {
                doc.retry_at = Some(now);
            }
        }
        // Every journal learns the new place, durably, before anything more
        // is saved there: after a kill it is the journal that says which
        // folder a text is brought back into.
        let (tx, rx) = mpsc::channel();
        if self.j.send(JMsg::Folder { folder: path.clone(), reply: tx }).is_ok() {
            let _ = rx.recv_timeout(self.tuning.barrier);
        }
        if self.active {
            self.watch = Watch::new(&self.root);
        }
        self.rescan_at = Some(now);
        self.out.send(News::FolderMoved { folder: path });
        for i in 0..self.docs.len() {
            if self.docs[i].hold == Some(Hold::FolderGone) && self.docs[i].want.is_none() {
                self.set_hold(i, None, false);
            }
        }
        true
    }

    /// Looks at an open text's file without being asked to save: on a folder
    /// notification, when the window comes back, and on a timer while held.
    fn check(&mut self, i: usize) {
        let doc = &mut self.docs[i];
        if matches!(doc.disk, Disk::Absent) {
            return;
        }
        let info = match sys::stat(&doc.path) {
            Ok(info) => info,
            Err(_) => {
                if !doc.removed {
                    self.missing(i);
                }
                return;
            }
        };
        // Said to be removed, or only out of sight for a moment: either way
        // the file is looked at again, but only a file that was said to be
        // removed is said to be back.
        let was_removed = doc.removed;
        let was_away = was_removed || doc.missing_since.is_some();
        let reference = doc.theirs.as_ref().map(|theirs| theirs.stamp).unwrap_or(doc.stamp);
        let read_only = info.attrs & sys::ATTR_READONLY != 0;
        let touched = stamp_of(&info) != reference || was_away;
        // Shown for reading only because a writer had it when it was opened:
        // that is looked at again every time, until the writer is gone.
        let shown_busy = doc.hold == Some(Hold::Busy) && doc.want.is_none();
        if !touched && !shown_busy && read_only == (doc.hold == Some(Hold::ReadOnly)) {
            return;
        }
        let found = match fsio::read_guarded(&doc.path, self.tuning.open_limit) {
            Ok(found) => found,
            Err(Fail::Missing) => {
                if !doc.removed {
                    self.missing(i);
                }
                return;
            }
            Err(_) => {
                // A writer is at it: look again shortly.
                doc.recheck_at = Some(Instant::now() + Duration::from_millis(250));
                return;
            }
        };
        let info = found.info;
        let same = match (&doc.theirs, &doc.disk) {
            (Some(theirs), _) => *theirs.bytes == found.bytes,
            (None, Disk::Known(bytes)) => **bytes == found.bytes,
            (None, _) => doc.own.contains(&hash::state(&found.bytes)),
        };
        drop(found.file);
        // Windows keeps a file that is being written from being read; these
        // systems do not, so a file written a moment ago may be half of
        // what its writer is putting there. It is looked at again shortly.
        #[cfg(unix)]
        if !same && sys::unix_ms_now().saturating_sub(sys::filetime_to_unix_ms(info.modified)) < SETTLING_MS {
            doc.recheck_at = Some(Instant::now() + Duration::from_millis(SETTLING_MS));
            return;
        }
        let id = doc.id;
        doc.missing_since = None;
        if was_removed {
            doc.removed = false;
            if doc.hold == Some(Hold::Removed) {
                self.set_hold(i, None, true);
            }
            self.out.send(News::Reappeared { doc: id });
        }
        let doc = &mut self.docs[i];
        if !same {
            self.outside(i, found.bytes, info);
            return;
        }
        // Read under the guard now, and the same: what was shown is the text.
        doc.shown = false;
        if doc.theirs.is_none() {
            doc.stamp = stamp_of(&info);
            doc.created = info.created;
        }
        let waiting = doc.want.is_some();
        let mut released = false;
        match doc.hold {
            Some(Hold::ReadOnly) if !read_only => {
                self.set_hold(i, None, waiting);
                released = true;
            }
            // Busy with a save waiting is for the next attempt to judge: a
            // reader can keep a rename out without keeping our read out.
            Some(Hold::Busy) if !waiting => self.set_hold(i, None, false),
            None if read_only => self.set_hold(i, Some(Hold::ReadOnly), false),
            _ => {}
        }
        // A waiting save is tried at once only if something really changed.
        // Our own failed attempts stir the folder too (a temp file comes and
        // goes); answering those would turn slow retries into a fast loop.
        let doc = &mut self.docs[i];
        if waiting && doc.theirs.is_none() && (touched || released) {
            doc.retry_at = Some(Instant::now());
        }
    }

    /// Writes `text` into a new file beside `path`, tagged and dated, never
    /// over anything. Returns where it went, inside the writing folder.
    fn write_beside(&mut self, path: &Path, format: Format, text: &str, tag: &str) -> Option<PathBuf> {
        let bytes = format::encode(text, format);
        let beside = names::sibling(path, tag, sys::unix_ms_now(), 1);
        let written = fsio::write_numbered(&beside, &bytes, &self.tuning.faults).ok()?;
        self.rescan_at = Some(Instant::now());
        Some(written.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or(written))
    }

    fn resolve(&mut self, i: usize, how: Resolve) {
        match how {
            Resolve::KeepMine => {
                let Some(theirs) = self.docs[i].theirs.take() else { return };
                // Their version stays in the history before ours goes over
                // it. If the history cannot take it, it is put in a file
                // beside the text instead; if that cannot be done either,
                // nothing goes over it.
                if let Snap::Failed(_) = self.snap(i, &theirs.bytes, Some("outside"), false) {
                    let beside = names::sibling(&self.docs[i].path, "outside", sys::unix_ms_now(), 1);
                    if fsio::write_numbered(&beside, &theirs.bytes, &self.tuning.faults).is_err() {
                        // Still two versions, and still held for that.
                        let doc = &mut self.docs[i];
                        doc.theirs = Some(theirs);
                        let id = doc.id;
                        self.out.send(News::Held { doc: id, why: Hold::NoHistory });
                        return;
                    }
                    self.rescan_at = Some(Instant::now());
                }
                let doc = &mut self.docs[i];
                // Ours goes over theirs whole, in the style ours had: nothing
                // of their bytes is ours to copy line breaks from.
                doc.mixed = false;
                doc.disk = Disk::Known(theirs.bytes);
                doc.shown = false;
                doc.disk_edit = None;
                doc.stamp = theirs.stamp;
                doc.created = theirs.created;
                if doc.want.is_some() {
                    doc.retry_at = Some(Instant::now());
                }
                self.set_hold(i, None, false);
            }
            Resolve::KeepBoth { text, edit } => {
                let doc = &self.docs[i];
                if doc.theirs.is_none() {
                    return;
                }
                let (id, path, format) = (doc.id, doc.path.clone(), doc.format);
                match self.write_beside(&path, format, &text, "mine") {
                    Some(rel) => {
                        let doc = &mut self.docs[i];
                        // Ours is safe in its own file; the buffer may now take theirs.
                        if doc.want.as_ref().is_some_and(|want| want.edit <= edit) {
                            doc.want = None;
                        }
                        self.out.send(News::KeptBeside { doc: id, rel });
                    }
                    None => self.out.send(News::Held { doc: id, why: Hold::CannotWrite }),
                }
            }
        }
    }

    // ---------------------------------------------------------------- closing

    /// Tries a wanted save for the last time. When a text is being closed
    /// there is no later, so the quick retries that would otherwise be left
    /// to a timer are made here, one after another, until the save is made,
    /// the trouble turns out to be one that waiting does not cure, or
    /// `within` has passed.
    fn last_tries(&mut self, i: usize, within: Duration) {
        if self.docs[i].want.is_none() {
            return;
        }
        let started = Instant::now();
        self.attempt(i);
        while self.docs[i].want.is_some() && self.docs[i].hold.is_none() {
            let Some(at) = self.docs[i].retry_at else { break };
            if at.saturating_duration_since(started) > within {
                break;
            }
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
            self.attempt(i);
        }
    }

    /// Asks the journal thread whether it holds text `id` whole, on the disk.
    fn journal_holds(&self, id: DocId, covered: Option<(u64, u64)>) -> bool {
        let (tx, rx) = mpsc::channel();
        self.j.send(JMsg::Close { id, covered, reply: Some(tx) }).is_ok() && rx.recv_timeout(self.tuning.barrier) == Ok(true)
    }

    /// Closes text `i`. The last text is saved if it can be; if its own file
    /// cannot take it, it goes into a file beside it where that is safe, and
    /// otherwise stays in the journal for the next start, and in memory for
    /// the rest of this session.
    fn close(&mut self, i: usize, last: Option<(String, u64)>, caret: Option<Pos>, within: Duration) {
        if let Some((text, edit)) = last {
            let keep = self.docs[i].want.take().and_then(|want| want.keep);
            self.docs[i].want = Some(Want { text: Arc::new(plain(text)), edit, keep });
        }
        self.last_tries(i, within);
        // A version asked for and not yet kept is kept now, whatever became of the save.
        self.keep_now(i);
        let doc = &self.docs[i];
        let latest = doc.shared.latest.load(Ordering::Acquire);
        // The newest edit whose text is in a file of his, and that text's length.
        let mut covered = doc.disk_edit.map(|edit| (edit, doc.disk_len));
        let mut beside = None;
        // The whole text as last typed, if the interface gave it.
        let mut whole = None;
        if let Some(want) = doc.want.clone() {
            if want.edit >= latest {
                whole = Some(want.text.clone());
            }
            if doc.disk_edit != Some(want.edit) {
                let bytes = format::encode(&want.text, doc.format);
                let now = sys::unix_ms_now();
                let target = match doc.hold {
                    Some(Hold::ChangedOutside) => Some(names::sibling(&doc.path, "mine", now, 1)),
                    Some(Hold::ReadOnly | Hold::NotUtf8 | Hold::Linked) => Some(names::sibling(&doc.path, "edited", now, 1)),
                    Some(Hold::Removed | Hold::NameTaken) => Some(doc.path.clone()),
                    _ => None,
                };
                if let Some(written) = target.and_then(|target| fsio::write_numbered(&target, &bytes, &self.tuning.faults).ok()) {
                    covered = Some((want.edit, text_len(&want.text)));
                    if written != doc.path {
                        beside = Some(written.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or(written));
                    }
                    self.rescan_at = Some(Instant::now());
                }
            }
        }
        if self.docs[i].unsnapped || self.docs[i].removed {
            if let Disk::Known(bytes) = &self.docs[i].disk {
                let bytes = bytes.clone();
                self.snap(i, &bytes, None, false);
                self.history.thin(self.docs[i].docid, sys::unix_ms_now());
            }
        }
        if let Some(caret) = caret {
            self.note_origin(i);
            self.history.set_caret(self.docs[i].docid, caret);
        }
        let doc = self.docs.remove(i);
        if covered.is_some_and(|(edit, _)| edit >= latest) {
            let _ = self.j.send(JMsg::Close { id: doc.id, covered, reply: None });
            self.out.send(News::Closed { doc: doc.id, beside, in_journal: false });
            return;
        }
        // Something typed is in no file of his. The journal is asked whether
        // it holds it whole; "it is in the journal" is said on its word, not
        // on a count of edit numbers.
        let durable = self.journal_holds(doc.id, covered);
        if !durable {
            if let Some(text) = &whole {
                // The journal cannot vouch for it (its disk is failing, or
                // the thread is gone). A file beside the text can hold it.
                if let Some(rel) = self.write_beside(&doc.path, doc.format, text, "unsaved") {
                    let _ = self.j.send(JMsg::Close { id: doc.id, covered: Some((latest, text_len(text))), reply: None });
                    self.out.send(News::Closed { doc: doc.id, beside: Some(rel), in_journal: false });
                    return;
                }
            }
        }
        // It waits: in the journal, and here in memory, so that opening the
        // text again brings it back whatever becomes of the journal's disk.
        let mut own = doc.own.clone();
        let before = match &doc.disk {
            Disk::Known(bytes) => {
                own.push(hash::state(bytes));
                match format::decode(bytes) {
                    Decoded::Text { text, .. } => text,
                    Decoded::Other { lossy } => lossy,
                }
            }
            _ => String::new(),
        };
        self.left.insert(doc.key.clone(), Left { docid: doc.docid, id: doc.id, latest, text: whole, before, format: doc.format, own, rel: doc.rel.clone(), caret, durable });
        self.out.send(News::Closed { doc: doc.id, beside, in_journal: true });
    }

    // ------------------------------------------------- versions, names, moves

    fn version_text(&self, i: usize, name: &str) -> Result<String, StoreError> {
        let bytes = self.history.read(self.docs[i].docid, name).ok_or(StoreError::NoSuchVersion)?;
        Ok(match format::decode(&bytes) {
            Decoded::Text { text, .. } => text,
            Decoded::Other { lossy } => lossy,
        })
    }

    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), StoreError> {
        let from = names::check_rel(from, false).map_err(StoreError::Name)?;
        let to = names::check_rel(to, true).map_err(StoreError::Name)?;
        let (from_key, to_key) = (names::key(&from), names::key(&to));
        let (source, target) = (self.root.join(&from), self.root.join(&to));
        if from_key != to_key && (names::taken(&target) || self.index_of_key(&to_key).is_some()) {
            return Err(StoreError::Name(NameError::Taken));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| StoreError::from(fsio::open_fail(sys::code(&e))))?;
        }
        let open = self.index_of_key(&from_key);
        let on_disk = open.is_none_or(|i| !matches!(self.docs[i].disk, Disk::Absent));
        if let Some(i) = open {
            // The journal learns the new name, durably, before the folder does:
            // a crash between the two must not strand the text under either.
            let (tx, rx) = mpsc::channel();
            if self.j.send(JMsg::Path { id: self.docs[i].id, old: from.clone(), new: to.clone(), reply: tx }).is_ok() {
                let _ = rx.recv_timeout(self.tuning.barrier);
            }
        }
        if on_disk {
            if let Err(fail) = fsio::move_no_replace(&source, &target) {
                if let Some(i) = open {
                    let (tx, _rx) = mpsc::channel();
                    let _ = self.j.send(JMsg::Path { id: self.docs[i].id, old: to, new: from, reply: tx });
                }
                return Err(fail.into());
            }
        }
        if let Some(i) = open {
            let doc = &mut self.docs[i];
            doc.rel = to.clone();
            doc.path = target;
            doc.key = to_key.clone();
            doc.origin_noted = false;
            if doc.hold == Some(Hold::NameTaken) {
                doc.retry_at = Some(Instant::now());
            }
        }
        if let Some(docid) = self.ids.remove(&from_key) {
            self.ids.insert(to_key.clone(), docid);
            self.history.set_origin(docid, &self.root, &to);
            if let Some(i) = open {
                self.docs[i].origin_noted = true;
            }
        }
        if let Some(pending) = self.pending.remove(&from_key) {
            self.pending.insert(to_key.clone(), pending);
        }
        if let Some(docid) = self.unread.remove(&from_key) {
            self.unread.insert(to_key.clone(), docid);
        }
        if let Some(mut left) = self.left.remove(&from_key) {
            // Its journal, which the journal thread still holds, learns the new name too.
            let (tx, rx) = mpsc::channel();
            if self.j.send(JMsg::Path { id: left.id, old: from.clone(), new: to.clone(), reply: tx }).is_ok() {
                let _ = rx.recv_timeout(self.tuning.barrier);
            }
            left.rel = to.clone();
            self.left.insert(to_key.clone(), left);
        }
        if let Some(seen) = self.seen.remove(&from_key) {
            self.seen.insert(to_key, seen);
        }
        self.rescan_at = Some(Instant::now());
        Ok(())
    }

    /// Replaces the contents of a text that is not open, provided it still
    /// reads `was`, keeping the old contents as a version first.
    fn rewrite(&mut self, rel: &Path, was: &str, now: &str) -> Result<(), StoreError> {
        let rel = names::check_rel(rel, false).map_err(StoreError::Name)?;
        let key = names::key(&rel);
        if let Some(i) = self.index_of_key(&key) {
            return Err(StoreError::AlreadyOpen(self.docs[i].id));
        }
        let path = self.root.join(&rel);
        let found = fsio::read_patiently(&path, self.tuning.open_limit, &READ_PAUSES)?;
        let Decoded::Text { text, format, .. } = format::decode(&found.bytes) else { return Err(StoreError::ReadOnly) };
        if text != was {
            return Err(StoreError::Changed);
        }
        let info = found.info;
        if info.attrs & sys::ATTR_READONLY != 0 {
            return Err(StoreError::ReadOnly);
        }
        if info.links > 1 {
            return Err(StoreError::Linked);
        }
        let docid = *self.ids.entry(key).or_insert_with(|| hash::random() | 1);
        self.history.set_origin(docid, &self.root, &rel);
        self.history.snapshot(docid, &names::ending_of(&rel), &found.bytes, Some("before-mend"), sys::unix_ms_now(), false)?;
        // Every line break the mending does not touch stays as the file has it.
        let bytes = format::encode_like(&found.bytes, now, format);
        let posix = self.posix;
        // Written where the file really is, under the name the disk gives it.
        let target = sys::final_path(&found.file).unwrap_or_else(|| path.clone());
        let mut guard = Some(found.file);
        let write = fsio::Write { target: &target, first_temp: None, bytes: &bytes, created: Some(info.created), replace: true, posix, faults: &self.tuning.faults };
        let result = fsio::write_file(&write, &mut |look| fsio::verify_unchanged(&target, &info, posix, &mut guard, look));
        drop(guard);
        if let (Err(Fail::NoOneStep), true) = (&result, posix) {
            // Once more, from the start, with the old rename.
            self.no_one_step();
            return self.rewrite(&rel, was, now);
        }
        result?;
        self.rescan_at = Some(Instant::now());
        Ok(())
    }

    /// Moves a text into `.discarded` inside the writing folder. Nothing is
    /// erased and nothing already there is replaced.
    fn discard(&mut self, rel: &Path, last: Option<(String, u64)>) -> Result<PathBuf, StoreError> {
        let rel = names::check_rel(rel, false).map_err(StoreError::Name)?;
        let key = names::key(&rel);
        let path = self.root.join(&rel);
        if let Some(i) = self.index_of_key(&key) {
            if let Some((text, edit)) = last {
                self.docs[i].want = Some(Want { text: Arc::new(plain(text)), edit, keep: None });
            }
            self.last_tries(i, CLOSE_TRIES);
            let doc = &self.docs[i];
            if doc.want.is_some() {
                // What he last typed is not in the file: moving it away now would hide that.
                return Err(match doc.hold {
                    Some(Hold::ChangedOutside) => StoreError::Changed,
                    Some(Hold::ReadOnly | Hold::NotUtf8) => StoreError::ReadOnly,
                    Some(Hold::Linked) => StoreError::Linked,
                    Some(Hold::DiskFull) => StoreError::DiskFull,
                    Some(Hold::FolderGone) => StoreError::FolderGone,
                    Some(Hold::Removed) => StoreError::Missing,
                    _ => StoreError::Busy,
                });
            }
        }
        let moved_to = match fsio::read_patiently(&path, self.tuning.open_limit, &READ_PAUSES) {
            Ok(found) => {
                let docid = *self.ids.entry(key.clone()).or_insert_with(|| hash::random() | 1);
                self.history.set_origin(docid, &self.root, &rel);
                let _ = self.history.snapshot(docid, &names::ending_of(&rel), &found.bytes, Some("before-discard"), sys::unix_ms_now(), false);
                drop(found);
                let base = self.root.join(".discarded").join(&rel);
                if let Some(parent) = base.parent() {
                    fs::create_dir_all(parent).map_err(|e| StoreError::from(fsio::open_fail(sys::code(&e))))?;
                }
                let mut landed = Err(StoreError::Name(NameError::Taken));
                for n in 1..=999 {
                    let target = names::numbered(&base, n);
                    match fsio::move_no_replace(&path, &target) {
                        Ok(()) => {
                            landed = Ok(target.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or(target));
                            break;
                        }
                        Err(Fail::NameTaken) => continue,
                        Err(fail) => {
                            landed = Err(fail.into());
                            break;
                        }
                    }
                }
                landed?
            }
            Err(Fail::Missing) if self.index_of_key(&key).is_some() => PathBuf::new(),
            Err(fail) => return Err(fail.into()),
        };
        if let Some(i) = self.index_of_key(&key) {
            // The journal goes only if the file that was moved holds every edit.
            let doc = self.docs.remove(i);
            let latest = doc.shared.latest.load(Ordering::Acquire);
            let _ = self.j.send(JMsg::Close { id: doc.id, covered: doc.disk_edit.map(|edit| (edit, doc.disk_len)), reply: None });
            self.out.send(News::Closed { doc: doc.id, beside: None, in_journal: doc.disk_edit != Some(latest) });
        }
        self.seen.remove(&key);
        self.rescan_at = Some(Instant::now());
        Ok(moved_to)
    }

    // ------------------------------------------------------------ the folder

    /// Lists the folder, says so if the listing changed, and queues every
    /// text whose size or time is new for reading.
    fn rescan(&mut self) {
        let walk = match tree::walk(&self.root) {
            Ok(walk) => walk,
            Err(_) => {
                self.follow_folder();
                return;
            }
        };
        if self.active && self.watch.is_none() {
            self.watch = Watch::new(&self.root);
        }
        if self.first_scan {
            self.first_scan = false;
            let now = sys::unix_ms_now();
            for (temp, modified) in &walk.temps {
                if now.saturating_sub(*modified) > STALE_TEMP_MS {
                    fsio::to_trash(&self.data, temp, now);
                }
            }
        }
        let mut present = HashSet::new();
        let mut respelled = Vec::new();
        for text in walk.tree.texts() {
            let key = names::key(&text.rel);
            let known = self.seen.get(&key).is_some_and(|&(size, modified)| size == text.bytes && sys::filetime_to_unix_ms(modified) == text.modified_ms);
            if !known && self.tuning.scan_texts && !text.placeholder && text.bytes <= self.tuning.scan_limit && self.queued.insert(key.clone()) {
                self.queue.push_back(text.rel.clone());
            }
            // An open text whose name is spelled otherwise on the disk now.
            if let Some(i) = self.docs.iter().position(|doc| doc.key == key) {
                if self.docs[i].rel != text.rel && !matches!(self.docs[i].disk, Disk::Absent) {
                    respelled.push((i, text.rel.clone()));
                }
            }
            present.insert(key);
        }
        self.seen.retain(|key, _| present.contains(key));
        for (i, rel) in respelled {
            self.adopt_spelling(i, rel);
        }
        let tree = Arc::new(walk.tree);
        if self.tree.as_ref() != Some(&tree) {
            self.tree = Some(tree.clone());
            self.out.send(News::Tree(tree));
        }
    }

    /// Reads one queued text and hands its contents to the interface.
    fn scan_one(&mut self, rel: PathBuf) {
        let key = names::key(&rel);
        self.queued.remove(&key);
        match fsio::read_guarded(&self.root.join(&rel), self.tuning.scan_limit) {
            Ok(found) => {
                self.seen.insert(key, (found.info.size, found.info.modified));
                let text = match format::decode(&found.bytes) {
                    Decoded::Text { text, .. } => text,
                    Decoded::Other { lossy } => lossy,
                };
                drop(found.file);
                self.out.send(News::FileText { rel, text });
            }
            Err(Fail::Busy) => {
                // Being written right now: the notification that follows brings us back.
                self.rescan_at.get_or_insert(Instant::now() + Duration::from_secs(1));
            }
            Err(_) => {}
        }
    }

    fn write_states(&mut self) {
        self.states_at = None;
        for (name, bytes) in self.states.drain() {
            let _ = state::write_state(&self.data, &name, &bytes);
        }
    }

    // --------------------------------------------------------------- the loop

    /// Handles one message. False ends the thread.
    fn handle(&mut self, msg: SMsg) -> bool {
        match msg {
            SMsg::Open { id, rel, create, reply } => {
                if reply.begin() {
                    let result = self.open(id, &rel, create);
                    reply.send(result);
                }
            }
            SMsg::Save { id, text, edit, keep } => {
                if let Some(i) = self.index(id) {
                    // A version asked for earlier and not yet kept goes with
                    // the newer text, as long as nothing stands between them
                    // and the file. Where something does (another program's
                    // version waiting, a hold), it is kept at once from the
                    // text it was asked for with: the newer text may be
                    // theirs, and his label must not land on it.
                    let doc = &self.docs[i];
                    if doc.want.as_ref().is_some_and(|want| want.keep.is_some()) && (doc.theirs.is_some() || doc.hold.is_some()) {
                        self.keep_now(i);
                    }
                    let doc = &mut self.docs[i];
                    let keep = keep.or_else(|| doc.want.take().and_then(|want| want.keep));
                    doc.want = Some(Want { text: Arc::new(plain(text)), edit, keep });
                    doc.asked = true;
                    doc.retry_at = Some(Instant::now());
                }
            }
            SMsg::Resolve { id, how } => {
                if let Some(i) = self.index(id) {
                    self.resolve(i, how);
                }
            }
            SMsg::Close { id, last, caret } => {
                if let Some(i) = self.index(id) {
                    self.close(i, last, caret, CLOSE_TRIES);
                }
            }
            SMsg::Versions { id, reply } => {
                if reply.begin() {
                    reply.send(self.index(id).map(|i| self.history.list(self.docs[i].docid)).ok_or(StoreError::NotOpen));
                }
            }
            SMsg::ReadVersion { id, name, reply } => {
                if reply.begin() {
                    reply.send(self.index(id).ok_or(StoreError::NotOpen).and_then(|i| self.version_text(i, &name)));
                }
            }
            SMsg::BringBack { id, name, present, reply } => {
                if reply.begin() {
                    let result = self.index(id).ok_or(StoreError::NotOpen).and_then(|i| {
                        let text = self.version_text(i, &name)?;
                        // The present text is handed back only into a buffer
                        // whose own text has been kept.
                        let bytes = format::encode(&present, self.docs[i].format);
                        match self.snap(i, &bytes, Some("replaced"), false) {
                            Snap::Failed(fail) => Err(fail.into()),
                            _ => Ok(text),
                        }
                    });
                    reply.send(result);
                }
            }
            SMsg::Rename { from, to, reply } => {
                if reply.begin() {
                    let result = self.rename(&from, &to);
                    reply.send(result);
                }
            }
            SMsg::Rewrite { rel, was, now, reply } => {
                if reply.begin() {
                    let result = self.rewrite(&rel, &was, &now);
                    reply.send(result);
                }
            }
            SMsg::Discard { rel, last, reply } => {
                if reply.begin() {
                    let result = self.discard(&rel, last);
                    reply.send(result);
                }
            }
            SMsg::State { name, bytes } => {
                self.states.insert(name, bytes);
                self.states_at.get_or_insert(Instant::now() + Duration::from_millis(50));
            }
            SMsg::Active(active) => {
                self.active = active;
                if active {
                    // Coming back: look at everything once, then watch again,
                    // and try every save that is waiting, at once: whatever
                    // was in its way may have gone while he was elsewhere.
                    self.watch = Watch::new(&self.root);
                    let now = Instant::now();
                    self.rescan_at = Some(now);
                    for doc in &mut self.docs {
                        if doc.want.is_some() && !doc.never && !doc.removed && doc.theirs.is_none() {
                            doc.retry_at = Some(now);
                        }
                    }
                } else {
                    // A watch held on the folder stops its parents being renamed.
                    self.watch = None;
                }
            }
            SMsg::Rescan => self.rescan_at = Some(Instant::now()),
            SMsg::Shutdown { reply } => {
                let all = self.finish();
                let _ = reply.send(all);
                return false;
            }
            SMsg::Abandon => return false,
        }
        true
    }

    /// The orderly end: every open text gets its last try, state files are
    /// written, and a text closed earlier that the journal could not take is
    /// offered to a file once more. False if some text is still in memory
    /// alone at the end.
    fn finish(&mut self) -> bool {
        while !self.docs.is_empty() {
            self.close(0, None, None, EXIT_TRIES);
        }
        self.write_states();
        let mut all = true;
        let doubtful: Vec<String> = self.left.iter().filter(|(_, left)| !left.durable).map(|(key, _)| key.clone()).collect();
        for key in doubtful {
            let Some(left) = self.left.get(&key) else { continue };
            let (id, latest, path, format, text) = (left.id, left.latest, self.root.join(&left.rel), left.format, left.text.clone());
            if self.journal_holds(id, None) {
                continue;
            }
            match text.and_then(|text| self.write_beside(&path, format, &text, "unsaved").map(|_| text)) {
                Some(text) => {
                    let _ = self.j.send(JMsg::Close { id, covered: Some((latest, text_len(&text))), reply: None });
                    self.left.remove(&key);
                }
                None => all = false,
            }
        }
        all
    }

    /// Does everything whose time has come.
    fn due(&mut self) {
        let now = Instant::now();
        if self.rescan_at.is_some_and(|at| now >= at) {
            self.rescan_at = None;
            self.rescan();
            for i in 0..self.docs.len() {
                self.check(i);
            }
        }
        for i in 0..self.docs.len() {
            if self.docs[i].missing_since.is_some_and(|since| now >= since + self.tuning.missing_after) && !self.docs[i].removed {
                self.declare_missing(i);
            }
            if self.docs[i].recheck_at.is_some_and(|at| now >= at) {
                self.docs[i].recheck_at = None;
                self.check(i);
                let doc = &mut self.docs[i];
                if doc.hold == Some(Hold::Busy) && doc.want.is_none() {
                    doc.recheck_at = Some(now + Duration::from_millis(500));
                }
            }
            if self.docs[i].retry_at.is_some_and(|at| now >= at) {
                self.docs[i].retry_at = None;
                self.attempt(i);
            }
        }
        if self.states_at.is_some_and(|at| now >= at) {
            self.write_states();
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut consider = |at: Option<Instant>| {
            if let Some(at) = at {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        };
        consider(self.rescan_at);
        consider(self.states_at);
        for doc in &self.docs {
            consider(doc.retry_at);
            consider(doc.recheck_at);
            if !doc.removed {
                consider(doc.missing_since.map(|since| since + self.tuning.missing_after));
            }
        }
        next
    }

    pub(crate) fn run(mut self) {
        self.watch = Watch::new(&self.root);
        self.rescan_at = Some(Instant::now());
        loop {
            loop {
                match self.rx.try_recv() {
                    Ok(msg) => {
                        if !self.handle(msg) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.finish();
                        return;
                    }
                }
            }
            self.due();
            if let Some(rel) = self.queue.pop_front() {
                // One file, then back to the queue: a long scan never keeps a request waiting.
                self.scan_one(rel);
                continue;
            }
            let wait = self.next_deadline().map(|at| at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as u32);
            if wait == Some(0) {
                continue;
            }
            match sys::wait(&self.event, self.watch.as_ref(), wait) {
                Woke::Watch => {
                    if self.watch.as_ref().is_some_and(|watch| !watch.again()) {
                        self.watch = None;
                    }
                    self.rescan_at.get_or_insert(Instant::now() + self.tuning.watch_settle);
                }
                Woke::Failed => std::thread::sleep(Duration::from_millis(50)),
                Woke::Event | Woke::Timeout => {}
            }
        }
    }
}
