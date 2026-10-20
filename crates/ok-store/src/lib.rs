//! The save layer of ok: the writing folder, the journal, crash-safe saving,
//! earlier versions and recovery.
//!
//! One [`Store`] is opened on one writing folder. It runs two threads of its
//! own. The **journal thread** appends every edit to a file under the data
//! directory the moment it is typed, and that file alone can rebuild the
//! text. The **saver thread** is the only code that touches the writing
//! folder: it replaces a text's file whole, by one rename of a flushed temp
//! file, after reading what is there and making sure it is still what this
//! program last put there.
//!
//! The thread that owns the `Store` (the one that receives keystrokes) never
//! waits on a disk in [`Store::record`], [`Store::save`], [`Store::caret`] or
//! [`Store::poll`]. The calls that return something read from disk
//! ([`Store::open_text`], [`Store::versions`] and the like) wait for the
//! saver thread for at most [`Tuning::bound`].
//!
//! What it promises:
//!
//! * An edit that [`Store::record`] has returned for is in the journal file
//!   within milliseconds, and survives the process being killed.
//! * The journal is flushed to the disk a second after the first unflushed
//!   edit, or a third of a second into a pause, so a power cut costs at most
//!   about the last second of typing.
//! * A file in the writing folder is never truncated or written in place; a
//!   name believed to be new is never written with a replacing rename; and
//!   nothing of his is ever deleted, only moved.
//! * [`News::Saved`] is sent only after the new file has been flushed a
//!   second time, after its rename, and a journal is removed only after that.
//! * A journal is never passed over because some other program has it open,
//!   and never outranked by a later one that does not carry on from it: what
//!   it holds is brought back, put beside the file, or said to be waiting.
//! * "It is in the journal" is said on the journal thread's word that the
//!   text is whole on the disk, and a text is counted as saved only when its
//!   length is the one the recorded edits give.
//!
//! What it does not promise: a save replaces the file by one rename, and
//! Windows has no rename that replaces only if the name still leads to the
//! file that was compared. Between the store's last look at the file (a
//! query on a handle it holds, made immediately before the rename) and the
//! rename taking effect, about half a millisecond, another program's rename
//! of its own version onto the same name, or its delete of the name, goes
//! unseen: the store's text then stands under the name. A file renamed away
//! in that moment is noticed afterwards and followed. Closing this for good
//! would mean taking the file out of its name before putting the new one
//! in, and then a kill between the two would leave no file at all.
//!
//! Every path is given to the crate: the writing folder, and a data
//! directory that stands in for `%LOCALAPPDATA%\ok`. Nothing is hardcoded.

mod calendar;
mod format;
mod fsio;
mod hash;
mod history;
mod journal;
mod names;
mod recover;
mod saver;
#[cfg(test)]
mod scenario;
mod state;
#[cfg(windows)]
#[path = "sys_windows.rs"]
mod sys;
#[cfg(unix)]
#[path = "sys_unix.rs"]
mod sys;
#[doc(hidden)]
pub mod testing;
mod tree;
mod wal;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use format::{words_added, Eol, Format};
pub use history::Version;
pub use names::{check_name, is_text_name, safe_file_name, NameError, TEXT_ENDINGS};
pub use ok_text::{Change, Pos};
pub use recover::{Recovered, Recovery};
pub use state::Owner;
pub use sys::Volume;
pub use tree::{Folder, Text, Tree};

use journal::JMsg;
use saver::SMsg;

/// An edit that has sat unwritten this long raises the journal alarm.
const JOURNAL_LAG: Duration = Duration::from_millis(250);
/// How long [`Store::open`] waits for a folder that another instance holds
/// only in passing, while it brings back a crashed session's text there.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The handle of one open text. It is never reused within a [`Store`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DocId(u32);

/// What [`Store::open`] is given.
pub struct Setup {
    /// The writing folder. It is made if it does not exist.
    pub folder: PathBuf,
    /// Where the journal, the history, the lock and the state files go: the
    /// stand-in for `%LOCALAPPDATA%\ok`. It should be on a local NTFS disk.
    pub data: PathBuf,
    /// Called by a worker thread when [`Store::poll`] has something new. It
    /// must be quick and must not call back into the store: post a message
    /// to the window and return.
    pub wake: Arc<dyn Fn() + Send + Sync>,
    /// Written into the folder's lock for a second launch to read (see
    /// [`Owner::note`]): the interface puts its window handle here.
    pub note: String,
    /// The waits and sizes the store runs on; [`Tuning::default`] in the program.
    pub tuning: Tuning,
}

impl Setup {
    /// A setup with the default tuning, no note and a wake that does nothing.
    pub fn new(folder: impl Into<PathBuf>, data: impl Into<PathBuf>) -> Setup {
        Setup { folder: folder.into(), data: data.into(), wake: Arc::new(|| {}), note: String::new(), tuning: Tuning::default() }
    }
}

/// The numbers the store runs on. The defaults are the measured, recommended
/// ones; tests shorten them.
#[derive(Clone, Debug)]
pub struct Tuning {
    /// The journal is flushed this long after the first unflushed edit.
    pub flush_after: Duration,
    /// ...or this long after the last edit, whichever comes first.
    pub flush_pause: Duration,
    /// A journal file longer than this (and than four times the text) is
    /// started afresh at the next finished save.
    pub rotate_at: u64,
    /// A timed version is kept at most this often.
    pub snapshot_every: Duration,
    /// The longest a call that returns something waits for the saver thread.
    pub bound: Duration,
    /// The longest a save waits for the journal to confirm its barrier.
    pub barrier: Duration,
    /// How long after a folder notification the folder is looked at.
    pub watch_settle: Duration,
    /// How long a file must stay away before it is called removed.
    pub missing_after: Duration,
    /// Pauses, in milliseconds, between quick retries of a save another
    /// program is in the way of. After the last, the text is reported held.
    pub ladder: Vec<u64>,
    /// The first slow retry of a held save.
    pub retry_first: Duration,
    /// Every slow retry after that.
    pub retry_every: Duration,
    /// Texts larger than this are listed but their contents are not scanned.
    pub scan_limit: u64,
    /// Texts larger than this are not opened.
    pub open_limit: u64,
    /// Read every listed text and send it as [`News::FileText`].
    pub scan_texts: bool,
    /// Failures to inject. All zero outside tests.
    pub faults: Arc<Faults>,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            flush_after: Duration::from_millis(1000),
            flush_pause: Duration::from_millis(300),
            rotate_at: 256 << 10,
            snapshot_every: Duration::from_secs(300),
            bound: Duration::from_secs(2),
            barrier: Duration::from_secs(2),
            watch_settle: Duration::from_millis(150),
            missing_after: Duration::from_secs(1),
            ladder: vec![15, 30, 60, 120, 250, 500, 1000],
            retry_first: Duration::from_secs(5),
            retry_every: Duration::from_secs(15),
            scan_limit: 2 << 20,
            open_limit: 32 << 20,
            scan_texts: true,
            faults: Arc::new(Faults::default()),
        }
    }
}

/// Failures a test can switch on while a store runs. Each field is a Win32
/// error number to fail with, or 0 for no failure.
#[derive(Debug, Default)]
pub struct Faults {
    /// Writing a save's temp file fails (112 is a full disk).
    pub temp_write: AtomicU32,
    /// The flush after a save's rename fails: the file is in place but not
    /// known to be durable.
    pub second_flush: AtomicU32,
    /// Every write to a journal file fails.
    pub journal_write: AtomicU32,
    /// Not a failure: any value but 0 makes the store treat the writing
    /// folder's volume as one without the one-step replacing rename, as
    /// exFAT and FAT are, so that the other save path can be run on NTFS.
    pub no_one_step: AtomicU32,
}

/// Why [`Store::open`] did not open the folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OpenError {
    /// Another running instance has this folder. Hand over to it.
    Locked(Owner),
    /// The writing folder could not be made or reached (Win32 error number).
    Folder(u32),
    /// The data directory could not be made or used (Win32 error number).
    Data(u32),
}

/// Why a call on an open [`Store`] did not do what was asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StoreError {
    /// The name was refused; see [`NameError`]. `Taken` means something by
    /// that name is already there.
    Name(NameError),
    /// No file by that name.
    Missing,
    /// Another program has the file open in a way that keeps us out.
    Busy,
    /// The file is larger than [`Tuning::open_limit`].
    TooLarge,
    /// The writing folder, or the folder the text is in, is not there.
    FolderGone,
    /// The file is read-only, or is not UTF-8 and so is never written.
    ReadOnly,
    /// The file has more than one name (hard links) and is not replaced.
    Linked,
    /// The folder does not take new files.
    CannotWrite,
    /// The disk the writing folder is on is full.
    DiskFull,
    /// The file no longer reads the way the caller believed.
    Changed,
    /// The text is already open, under this handle.
    AlreadyOpen(DocId),
    /// No open text has this handle.
    NotOpen,
    /// No version by that name.
    NoSuchVersion,
    /// The saver thread did not get to the request within [`Tuning::bound`].
    /// The request has been withdrawn: nothing was done, and nothing will be
    /// done later behind the caller's back. Nothing is lost; ask again.
    Timeout,
    /// The saver thread had begun on the request and was still at it when
    /// twice [`Tuning::bound`] had passed (a folder on a network that hangs,
    /// say). It may yet be carried out: look at the next [`News::Tree`]
    /// before asking again.
    Unfinished,
    /// The store has been shut down.
    Stopped,
    /// Anything else, with the Win32 error number.
    Io(u32),
}

/// Why an open text cannot be written to its file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadOnly {
    /// The file is not valid UTF-8 text (which includes UTF-16, with or
    /// without its byte-order mark, and anything holding a zero byte). It is
    /// shown as best it can be and is never rewritten.
    NotUtf8,
    /// The file carries the read-only attribute.
    Attribute,
    /// Another program has the file open for writing. What is shown was
    /// read past it, twice a moment apart, and was the same both times and
    /// not empty; it is still not vouched for until the other program lets
    /// go, when [`News::Released`] follows, or [`News::ChangedOutside`] if
    /// the file then reads otherwise.
    Busy,
    /// The file has more than one name (hard links).
    Linked,
}

/// Why a text's saves are not reaching its file. While a text is held every
/// edit still goes to the journal, so nothing typed is at risk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hold {
    /// Another program changed the file; see [`News::ChangedOutside`].
    ChangedOutside,
    /// The file carries the read-only attribute.
    ReadOnly,
    /// The file is not valid UTF-8.
    NotUtf8,
    /// The file has more than one name (hard links).
    Linked,
    /// Another program has the file open and will not let it be replaced.
    Busy,
    /// The file was removed by another program.
    Removed,
    /// The folder is not there.
    FolderGone,
    /// The disk the writing folder is on is full. The file is untouched.
    DiskFull,
    /// The folder does not take new files.
    CannotWrite,
    /// A new text's name was taken by something else before its first save.
    NameTaken,
    /// The file was replaced but the flush after it failed; it is being
    /// written again.
    NotDurable,
    /// The text has been cut down by a lot and the fuller text on disk
    /// cannot be kept as a version first (the data directory cannot be
    /// written), so the cut is not written over it yet; or another
    /// program's version could be kept nowhere, so [`Resolve::KeepMine`]
    /// was not carried out.
    NoHistory,
    /// The text handed to [`Store::save`] is not the text at the edit number
    /// it came with: it has another length than the edits recorded up to
    /// that number give. Nothing was written and nothing is counted as
    /// saved. This is a mistake in the caller; ask again with the buffer as
    /// it stands and its own edit number.
    WrongEdit,
    /// The saver thread has ended, so nothing more reaches a file in this
    /// session. Every edit still goes to the journal and comes back at the
    /// next start.
    Stopped,
    /// Anything else, with the Win32 error number.
    Io(u32),
}

/// How to settle a text another program changed while it had unsaved edits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resolve {
    /// Keep the buffer: the next save goes over the outside version, which
    /// is kept in the history first.
    KeepMine,
    /// Write the buffer (this text, at this edit number) to a file beside
    /// the original, named `<name> (mine <date time>)`. The original keeps
    /// the outside version, which the interface then loads into the buffer.
    KeepBoth { text: String, edit: u64 },
}

/// What [`Store::open_text`] and [`Store::create_text`] return.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Opened {
    /// The handle every later call about this text takes.
    pub doc: DocId,
    /// The text, with `\n` line breaks and no byte-order mark.
    pub text: String,
    /// The file's line-ending style and byte-order mark. They are remembered
    /// by the store and written back with every save.
    pub format: Format,
    /// The file uses more than one kind of line break (or carries more than
    /// one byte-order mark). Saving keeps every break nobody touched as it
    /// is; a break that is typed is written in the commoner style.
    pub mixed_endings: bool,
    /// Set when the text cannot be written to its file, with the reason.
    pub read_only: Option<ReadOnly>,
    /// Where the caret was left, if it is known.
    pub caret: Option<Pos>,
    /// Set when `text` comes from a journal an earlier session left and is
    /// not yet in the file: the number of words it holds that the file does
    /// not. The buffer is unsaved; ask for a save.
    pub restored: Option<usize>,
    /// The edit number `text` corresponds to. [`Store::record`] counts on from it.
    pub edit: u64,
}

/// Something the store has to tell. Read with [`Store::poll`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum News {
    /// The text as it was at edit number `edit` is in its file, flushed
    /// after the rename. `bytes` is the file's size and `at_ms` its
    /// last-write time in milliseconds since 1970. The interface prints `ok`
    /// when `edit` is the latest number [`Store::record`] returned.
    Saved { doc: DocId, edit: u64, bytes: u64, at_ms: u64 },
    /// The answer to [`Store::keep`]: the text at `edit` is kept as a
    /// version. After [`News::Saved`] for the same edit, `bytes` is the size
    /// of the text's file, read back from the file system. When the save
    /// itself could not be made for a reason that waiting does not cure (see
    /// [`Store::keep`]) there is no `Saved`, [`News::Held`] follows, and
    /// `bytes` is the size of the kept version.
    Kept { doc: DocId, edit: u64, bytes: u64, at_ms: u64 },
    /// [`Store::keep`] was asked for the text at `edit` and no version could
    /// be written (the data directory is full or cannot be written). The
    /// save itself is answered separately.
    NotKept { doc: DocId, edit: u64 },
    /// Saves of this text are not reaching its file, and why. It is retried
    /// by the store; the journal holds everything meanwhile.
    Held { doc: DocId, why: Hold },
    /// A hold has ended without a save to announce it.
    Released { doc: DocId },
    /// Another program changed the file. `text` is what it holds now.
    /// `clean` says the buffer had nothing unsaved as far as the store knew,
    /// with `edit` the newest edit number it had seen: if that is still the
    /// buffer's number, replace the buffer with `text` (through
    /// `Doc::replace` and [`Store::record`], then [`Store::save`]) and the
    /// store takes the outside version as saved. Otherwise both versions
    /// exist: answer with [`Store::resolve`], or take theirs the same way
    /// after keeping yours with [`Store::keep`] (which, while the outside
    /// version waits, keeps the version at once and answers [`News::Kept`]).
    /// Until then saves are held, and a save asked for meanwhile is answered
    /// with [`News::Held`]. `clean` is true only if the text the store had
    /// saved is itself kept as a version. `read_only` is set if the new
    /// contents can no longer be written.
    ChangedOutside { doc: DocId, text: String, format: Format, clean: bool, edit: u64, read_only: Option<ReadOnly> },
    /// The file was removed by another program and has stayed away.
    Removed { doc: DocId },
    /// A removed file is back under its name.
    Reappeared { doc: DocId },
    /// The file was renamed by another program and followed to this path.
    Moved { doc: DocId, rel: PathBuf },
    /// The writing folder itself was renamed or moved and followed here.
    FolderMoved { folder: PathBuf },
    /// The answer to [`Resolve::KeepBoth`]: the buffer was written here.
    KeptBeside { doc: DocId, rel: PathBuf },
    /// The text is closed. `beside` is set if its last text had to be
    /// written to another file than its own (inside the writing folder).
    /// `in_journal` is set if it could not be written to any file yet: it
    /// waits in the journal, which was asked and said it holds it, and
    /// opening the text again in this session brings it back. If the journal
    /// could not vouch for it either, the text was written beside its file
    /// as `<name> (unsaved <date time>)` where that could be done; where not
    /// even that could, `in_journal` is still set, [`News::JournalAlarm`] is
    /// up, the store keeps the text in memory, and [`Store::shutdown`] will
    /// say if it never reached a disk.
    Closed { doc: DocId, beside: Option<PathBuf>, in_journal: bool },
    /// The folder's listing, whenever it changes and once at the start.
    Tree(Arc<Tree>),
    /// The contents of one listed text, for link indexes and word counts.
    /// Sent once for every text and again when its size or time changes,
    /// except for changes made by this store's own saves.
    FileText { rel: PathBuf, text: String },
    /// `true`: typed text is, for now, not safe in the journal: its disk is
    /// failing or has stopped answering, the journal thread has ended, or
    /// the lengths handed to [`Store::record`] do not add up to the text the
    /// journal is rebuilding. `false`: every edit recorded is in a journal
    /// file again. A worker wakes the interface for both.
    JournalAlarm(bool),
    /// The volume refused the one-step replace; saves here are not atomic.
    NotAtomic,
}

/// Counters the journal thread publishes.
#[derive(Default)]
pub(crate) struct Shared {
    /// Edits the journal thread has put in a file.
    pub written: AtomicU64,
    /// A journal file could not be written, or does not add up.
    pub journal_failed: AtomicBool,
    /// The interface is showing the alarm for a journal that fell behind and
    /// polls only when woken: the journal thread wakes it at every step
    /// forward, so that it sees the alarm go down.
    pub watched: AtomicBool,
    /// The store is shutting down and the journal thread has been told to end.
    pub stopping: AtomicBool,
}

/// What the keystroke thread and the saver thread share about one text.
#[derive(Default)]
pub(crate) struct DocShared {
    /// The newest edit number recorded.
    pub latest: AtomicU64,
}

/// The way news reaches the interface: a queue, and one wake per batch.
#[derive(Clone)]
pub(crate) struct NewsOut {
    tx: Sender<News>,
    wake: Arc<dyn Fn() + Send + Sync>,
    woken: Arc<AtomicBool>,
}

impl NewsOut {
    pub(crate) fn send(&self, news: News) {
        let _ = self.tx.send(news);
        self.nudge();
    }

    /// Wakes the interface unless it has been woken and has not yet polled.
    pub(crate) fn nudge(&self) {
        if !self.woken.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// The saver thread's queue and the event that wakes it from its wait.
struct SaverLink {
    tx: Sender<SMsg>,
    event: Arc<sys::Event>,
}

impl SaverLink {
    fn send(&self, msg: SMsg) -> bool {
        let sent = self.tx.send(msg).is_ok();
        self.event.set();
        sent
    }
}

/// The two worker threads, by their place in [`Store::threads`].
const JOURNAL: usize = 0;
const SAVER: usize = 1;

struct Local {
    shared: Arc<DocShared>,
    edit: u64,
}

/// An open writing folder. Dropping it shuts it down as [`Store::shutdown`]
/// does, with a bound of three seconds.
pub struct Store {
    root: PathBuf,
    data: PathBuf,
    volume: Volume,
    bound: Duration,
    j: Sender<JMsg>,
    s: SaverLink,
    news: Receiver<News>,
    woken: Arc<AtomicBool>,
    shared: Arc<Shared>,
    docs: Vec<Option<Local>>,
    /// Edits handed to the journal thread so far.
    sent: u64,
    /// One edit not yet seen written, and when it was handed over.
    mark: Option<(u64, Instant)>,
    alarm: bool,
    journal_dead: bool,
    saver_dead: bool,
    /// News this thread makes itself: the answer to a request the saver
    /// thread is no longer there to answer.
    local: RefCell<VecDeque<News>>,
    states: HashMap<String, Vec<u8>>,
    threads: Vec<JoinHandle<()>>,
    stopped: bool,
    _lock: state::Lock,
}

impl Store {
    /// Opens a writing folder: takes its lock (or says who holds it), deals
    /// with whatever journals an earlier session left, and starts the two
    /// worker threads. The second value says what recovery did, one entry
    /// per text it had to act on; it is empty after a clean exit. A text
    /// whose journal could not be read or whose file could not be replaced
    /// just then is in it as [`Recovery::Waiting`], never left out.
    ///
    /// Journals of other writing folders that share the data directory are
    /// dealt with too, each under its own folder's lock, if no instance has
    /// that folder open; their entries name that folder. If such an instance
    /// is, at this moment, doing the same for the folder asked for here,
    /// this waits for it (five seconds at most) instead of answering
    /// [`OpenError::Locked`].
    ///
    /// This reads and may write files, so it takes a few milliseconds, and
    /// longer when there is much to recover or when another program is
    /// holding a journal (about a third of a second for each one held).
    pub fn open(setup: Setup) -> Result<(Store, Vec<Recovered>), OpenError> {
        let Setup { folder, data, wake, note, tuning } = setup;
        let data_error = |e: std::io::Error| OpenError::Data(sys::code(&e));
        for sub in ["journal", "history", "lock", "state"] {
            fs::create_dir_all(data.join(sub)).map_err(data_error)?;
        }
        let data = fs::canonicalize(&data).map_err(data_error)?;
        fs::create_dir_all(&folder).map_err(|e| OpenError::Folder(sys::code(&e)))?;
        let root = fs::canonicalize(&folder).map_err(|e| OpenError::Folder(sys::code(&e)))?;
        let asked = Instant::now();
        let lock = loop {
            match state::take_lock(&data, &root, &note, false) {
                Ok(lock) => break lock,
                // An instance started on another folder is bringing back a
                // crashed session's text here and has the folder for that
                // moment. It is not a window to hand over to: wait it out.
                Err(state::LockFail::Held(owner)) if owner.passing && asked.elapsed() < LOCK_WAIT => std::thread::sleep(Duration::from_millis(5)),
                Err(state::LockFail::Held(owner)) => return Err(OpenError::Locked(owner)),
                Err(state::LockFail::Io(code)) => return Err(OpenError::Data(code)),
            }
        };
        let volume = sys::volume(&root);
        let history = history::History::new(data.join("history"));
        let outcome = recover::run(&recover::Ctx { root: &root, data: &data, volume: &volume, history: &history, faults: &tuning.faults });
        let ids = history.load_ids(&root);

        let event = Arc::new(sys::Event::new().ok_or(OpenError::Data(0))?);
        let shared = Arc::new(Shared::default());
        let woken = Arc::new(AtomicBool::new(false));
        let (news_tx, news) = mpsc::channel();
        let out = NewsOut { tx: news_tx, wake, woken: woken.clone() };
        let (j, j_rx) = mpsc::channel();
        let journal = journal::spawn(data.join("journal"), tuning.clone(), shared.clone(), out.clone(), j_rx).map_err(data_error)?;
        let (s_tx, s_rx) = mpsc::channel();
        let bound = tuning.bound;
        let saver = saver::Saver::new(saver::Start {
            root: root.clone(),
            data: data.clone(),
            volume: volume.clone(),
            tuning,
            history,
            rx: s_rx,
            event: event.clone(),
            j: j.clone(),
            out,
            shared: shared.clone(),
            ids,
            pending: outcome.pending,
            unread: outcome.unread,
            note,
        });
        let saver = std::thread::Builder::new().name("ok-saver".to_string()).spawn(move || saver.run()).map_err(data_error)?;
        let store = Store {
            root,
            data,
            volume,
            bound,
            j,
            s: SaverLink { tx: s_tx, event },
            news,
            woken,
            shared,
            docs: Vec::new(),
            sent: 0,
            mark: None,
            alarm: false,
            journal_dead: false,
            saver_dead: false,
            local: RefCell::new(VecDeque::new()),
            states: HashMap::new(),
            threads: vec![journal, saver],
            stopped: false,
            _lock: lock,
        };
        Ok((store, outcome.report))
    }

    /// The writing folder, in the form Windows itself spells it. If the
    /// folder is renamed while the store has it open and is followed, this
    /// is its new name from the moment [`Store::poll`] has handed over
    /// [`News::FolderMoved`].
    pub fn folder(&self) -> &Path {
        &self.root
    }

    /// What the volume under the writing folder can do. If
    /// [`Volume::atomic_replace`] is false, saves here are not atomic and the
    /// interface should say so.
    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Asks the saver thread for something and waits for the answer, for
    /// [`Tuning::bound`] at most. A request the saver has not begun by then
    /// is withdrawn, and is then certain never to be carried out. One it has
    /// begun is waited on for as long again.
    fn ask<T>(&self, make: impl FnOnce(saver::Reply<T>) -> SMsg) -> Result<T, StoreError> {
        let (reply, asked) = saver::Reply::new();
        if !self.s.send(make(reply)) {
            return Err(StoreError::Stopped);
        }
        match asked.rx.recv_timeout(self.bound) {
            Ok(result) => result,
            Err(RecvTimeoutError::Disconnected) => Err(StoreError::Stopped),
            Err(RecvTimeoutError::Timeout) if asked.withdraw() => Err(StoreError::Timeout),
            Err(RecvTimeoutError::Timeout) => match asked.rx.recv_timeout(self.bound) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => Err(StoreError::Unfinished),
                Err(RecvTimeoutError::Disconnected) => Err(StoreError::Stopped),
            },
        }
    }

    /// Queues the answer to a request the saver thread is not there to take.
    fn unanswered(&self, doc: DocId) {
        if matches!(self.docs.get(doc.0 as usize), Some(Some(_))) {
            self.local.borrow_mut().push_back(News::Held { doc, why: Hold::Stopped });
        }
    }

    fn open_at(&mut self, rel: &Path, create: bool) -> Result<Opened, StoreError> {
        let id = DocId(self.docs.len() as u32);
        self.docs.push(None);
        match self.ask(|reply| SMsg::Open { id, rel: rel.to_path_buf(), create, reply }) {
            Ok((opened, shared)) => {
                self.docs[id.0 as usize] = Some(Local { shared, edit: opened.edit });
                Ok(opened)
            }
            Err(error) => {
                if error == StoreError::Unfinished {
                    // It may yet be opened by the saver thread: have it closed again.
                    self.s.send(SMsg::Close { id, last: None, caret: None });
                }
                Err(error)
            }
        }
    }

    /// Opens the text at `rel`, a path inside the writing folder. Waits at
    /// most [`Tuning::bound`].
    ///
    /// A file that is not valid UTF-8, carries the read-only attribute, has
    /// several names, or is held open for writing by another program is
    /// still opened, with [`Opened::read_only`] saying why it cannot be
    /// written. A file another program is in the middle of rewriting (it is
    /// empty, or changing under our eyes) is not shown as the text: the
    /// answer is [`StoreError::Busy`], and asking again a moment later
    /// works. If an earlier session, or an earlier close in this one, left
    /// unsaved text for this file, that text is what is returned, with
    /// [`Opened::restored`] set.
    ///
    /// The text is opened under the name the disk spells: asking for
    /// `the orchard.md` when the file is `The Orchard.md` opens that file
    /// and never changes its capitals.
    pub fn open_text(&mut self, rel: impl AsRef<Path>) -> Result<Opened, StoreError> {
        self.open_at(rel.as_ref(), false)
    }

    /// Starts a new, empty text at `rel`, making the folders on the way.
    /// The name is refused if it is taken, is a reserved device name, holds
    /// a character Windows forbids, ends in a dot or a space, starts with a
    /// dot, or does not end in `.md`, `.markdown` or `.txt`.
    ///
    /// No file is written until the first [`Store::save`], and that save
    /// never replaces a file that appeared under the name in the meantime
    /// (the text is then held with [`Hold::NameTaken`]).
    pub fn create_text(&mut self, rel: impl AsRef<Path>) -> Result<Opened, StoreError> {
        self.open_at(rel.as_ref(), true)
    }

    /// Records one edit: the [`Change`] that `Doc::replace` returned, and the
    /// document's length in bytes afterwards (`Doc::len_bytes`). Returns the
    /// edit's number, which counts up by one per edit from [`Opened::edit`].
    /// A change that changed nothing is not recorded and returns the current
    /// number; an unknown handle returns 0.
    ///
    /// This is the keystroke path. It encodes about forty bytes and hands
    /// them to the journal thread; it makes no system call that touches a
    /// file and never waits.
    ///
    /// The journal thread keeps its own count of the text's length. If
    /// `len_after` is not what that count gives, the journal may no longer
    /// be rebuilding the text on the screen: [`News::JournalAlarm`] goes up
    /// until the two agree again, which they do at the next edit if only the
    /// number was wrong, and at the next [`Store::save`] otherwise (the
    /// journal then starts over from the text it is handed).
    pub fn record(&mut self, doc: DocId, change: &Change, len_after: usize) -> u64 {
        let Some(Some(local)) = self.docs.get_mut(doc.0 as usize) else { return 0 };
        if change.is_noop() {
            return local.edit;
        }
        local.edit += 1;
        let edit = local.edit;
        local.shared.latest.store(edit, Ordering::Release);
        let payload = wal::edit_payload(edit, change, len_after);
        self.sent += 1;
        if self.mark.is_none() {
            self.mark = Some((self.sent, Instant::now()));
        }
        if self.j.send(JMsg::Edit { id: doc, edit, payload }).is_err() {
            self.journal_dead = true;
        }
        edit
    }

    /// Notes where the caret is, so that a restart after a crash lands
    /// there. Costs one message; only the latest is ever written.
    pub fn caret(&mut self, doc: DocId, at: Pos) {
        if matches!(self.docs.get(doc.0 as usize), Some(Some(_))) {
            let _ = self.j.send(JMsg::Caret { id: doc, at });
        }
    }

    /// Asks for the text to be saved to its file: the whole text (`\n` line
    /// breaks) and the edit number it corresponds to. Returns at once. The
    /// store decides nothing about when to save, but a request a newer one
    /// overtakes is skipped. Every request that is not overtaken is
    /// answered: with [`News::Saved`]; with [`News::Held`] and the reason;
    /// or, when the file turns out to have been changed, removed or renamed
    /// by another program, with the news that says so
    /// ([`News::ChangedOutside`], [`News::Removed`], or [`News::Moved`]
    /// followed by the answer to the save itself).
    pub fn save(&self, doc: DocId, text: String, edit: u64) {
        if !self.s.send(SMsg::Save { id: doc, text, edit, keep: None }) {
            self.unanswered(doc);
        }
    }

    /// Saves as [`Store::save`] does and also keeps the text as a labelled
    /// version that is never thinned out. The answer is [`News::Saved`]
    /// followed by [`News::Kept`], whose byte count is read back from the
    /// file system: `Kept 14:02. 2,481 bytes on disk.` (or by
    /// [`News::NotKept`] if no version could be written).
    ///
    /// If the save cannot be made the answer is [`News::Held`]. Where the
    /// reason is one that waiting does not cure (another program's version
    /// is waiting to be taken or refused, the file is read-only, not UTF-8,
    /// linked or removed, its name was taken) the version is kept at once
    /// all the same, from this very text, and [`News::Kept`] says so before
    /// the `Held`. Otherwise it is kept when the save goes through, or when
    /// the text is closed.
    pub fn keep(&self, doc: DocId, text: String, edit: u64, label: &str) {
        if !self.s.send(SMsg::Save { id: doc, text, edit, keep: Some(label.to_string()) }) {
            self.unanswered(doc);
        }
    }

    /// Settles a [`News::ChangedOutside`] that found unsaved edits.
    /// [`Resolve::KeepMine`] is answered by the save that follows it (or by
    /// [`News::Held`] with [`Hold::NoHistory`] if the other version could be
    /// kept nowhere, in which case nothing goes over it);
    /// [`Resolve::KeepBoth`] by [`News::KeptBeside`].
    pub fn resolve(&self, doc: DocId, how: Resolve) {
        if !self.s.send(SMsg::Resolve { id: doc, how }) {
            self.unanswered(doc);
        }
    }

    /// The next piece of news, or `None`. Never waits.
    ///
    /// While [`Store::unwritten`] is true, call this at least every quarter
    /// of a second: that is how a journal disk that has stopped answering is
    /// noticed and reported as [`News::JournalAlarm`]. Once that alarm is up
    /// the journal thread wakes the interface at every step forward, so the
    /// alarm going down is heard without a timer.
    pub fn poll(&mut self) -> Option<News> {
        self.woken.store(false, Ordering::Release);
        if let Some(news) = self.local.borrow_mut().pop_front() {
            return Some(news);
        }
        if let Ok(news) = self.news.try_recv() {
            match &news {
                News::Closed { doc, .. } => {
                    if let Some(slot) = self.docs.get_mut(doc.0 as usize) {
                        *slot = None;
                    }
                }
                News::FolderMoved { folder } => self.root = folder.clone(),
                _ => {}
            }
            return Some(news);
        }
        if !self.stopped {
            // A worker thread that has ended without being told to is said
            // at once: no request to it will be answered again.
            if !self.saver_dead && self.threads.get(SAVER).is_some_and(|thread| thread.is_finished()) {
                self.saver_dead = true;
                let open = (0..self.docs.len()).filter(|&n| self.docs[n].is_some()).map(|n| News::Held { doc: DocId(n as u32), why: Hold::Stopped });
                self.local.borrow_mut().extend(open);
                if let Some(news) = self.local.borrow_mut().pop_front() {
                    return Some(news);
                }
            }
            if !self.journal_dead && self.threads.get(JOURNAL).is_some_and(|thread| thread.is_finished()) {
                self.journal_dead = true;
            }
        }
        self.alarm_news()
    }

    /// Works out whether the journal alarm should be up, and says so if that
    /// is a change.
    fn alarm_news(&mut self) -> Option<News> {
        let mut stalled = false;
        if let Some((mark, since)) = self.mark {
            let late = since.elapsed() > JOURNAL_LAG;
            if late {
                // Said before the count is read: either the journal thread's
                // next step forward sees this and wakes us, or this read
                // already sees that step.
                self.shared.watched.store(true, Ordering::SeqCst);
            }
            let written = self.shared.written.load(Ordering::SeqCst);
            if written >= mark {
                self.mark = if self.sent > written { Some((self.sent, Instant::now())) } else { None };
            } else {
                stalled = late;
            }
        }
        // Once up for a journal that fell behind, the alarm stays up until
        // the journal has caught up with everything, not only with the edit
        // that was being timed.
        let behind = self.alarm && self.shared.watched.load(Ordering::SeqCst) && self.unwritten();
        let alarm = stalled || behind || self.journal_dead || self.shared.journal_failed.load(Ordering::Acquire);
        if !alarm {
            self.shared.watched.store(false, Ordering::SeqCst);
        }
        if alarm != self.alarm {
            self.alarm = alarm;
            return Some(News::JournalAlarm(alarm));
        }
        None
    }

    /// True while an edit handed to [`Store::record`] has not yet been
    /// written to a journal file. Normally false again within a millisecond.
    pub fn unwritten(&self) -> bool {
        self.sent > self.shared.written.load(Ordering::SeqCst)
    }

    /// Flushes the journal to the disk now, without waiting for it: for
    /// focus loss and suspend.
    pub fn flush(&self) {
        let _ = self.j.send(JMsg::Flush { reply: None });
    }

    /// Tells the store whether the window is active. Becoming active looks
    /// at every open text and the whole folder once and starts watching the
    /// folder for changes; becoming inactive stops watching, because a watch
    /// held on the folder stops other programs renaming the folders above
    /// it. A store starts out active.
    pub fn set_active(&self, active: bool) {
        self.s.send(SMsg::Active(active));
    }

    /// Asks for the folder to be listed again now. [`News::Tree`] follows if
    /// anything changed.
    pub fn rescan(&self) {
        self.s.send(SMsg::Rescan);
    }

    /// The kept versions of an open text, newest first.
    pub fn versions(&self, doc: DocId) -> Result<Vec<Version>, StoreError> {
        self.ask(|reply| SMsg::Versions { id: doc, reply })
    }

    /// The text of one version, by [`Version::name`].
    pub fn read_version(&self, doc: DocId, name: &str) -> Result<String, StoreError> {
        self.ask(|reply| SMsg::ReadVersion { id: doc, name: name.to_string(), reply })
    }

    /// Brings a version back: keeps `present` (the buffer as it stands) as a
    /// version labelled `replaced`, and returns the old version's text for
    /// the interface to put in the buffer through `Doc::replace`,
    /// [`Store::record`] and [`Store::save`] like any other edit, so that
    /// undo takes it back.
    pub fn bring_back(&self, doc: DocId, name: &str, present: &str) -> Result<String, StoreError> {
        self.ask(|reply| SMsg::BringBack { id: doc, name: name.to_string(), present: present.to_string(), reply })
    }

    /// Renames or moves a text inside the writing folder, open or not,
    /// making folders on the way and never replacing anything. For an open
    /// text the journal is told, durably, before the file moves. The text's
    /// history goes with it.
    pub fn rename(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), StoreError> {
        self.ask(|reply| SMsg::Rename { from: from.as_ref().to_path_buf(), to: to.as_ref().to_path_buf(), reply })
    }

    /// Replaces the contents of a text that is not open: for mending links
    /// after a rename. `was` is the text as the interface last saw it; if
    /// the file reads differently now nothing is written and
    /// [`StoreError::Changed`] comes back. The old contents are kept as a
    /// version labelled `before-mend` first.
    pub fn rewrite(&self, rel: impl AsRef<Path>, was: &str, now: &str) -> Result<(), StoreError> {
        self.ask(|reply| SMsg::Rewrite { rel: rel.as_ref().to_path_buf(), was: was.to_string(), now: now.to_string(), reply })
    }

    /// Moves a text out of sight, into `.discarded` inside the writing
    /// folder, under the same path it had. Nothing is erased, and a file
    /// already there is never replaced (a number is added to the name).
    /// Returns where it went, inside the writing folder.
    ///
    /// If the text is open, pass its last text and edit number: it is saved
    /// first, then closed. If that save cannot be made, nothing is moved.
    pub fn discard(&self, rel: impl AsRef<Path>, last: Option<(String, u64)>) -> Result<PathBuf, StoreError> {
        self.ask(|reply| SMsg::Discard { rel: rel.as_ref().to_path_buf(), last, reply })
    }

    /// Reads a small named state file from the data directory (window
    /// placement, the last text and its caret). `None` if it has never been
    /// written. Meant for the start of the program: it reads the disk.
    pub fn read_state(&self, name: &str) -> Option<Vec<u8>> {
        match self.states.get(name) {
            Some(bytes) => Some(bytes.clone()),
            None => state::read_state(&self.data, name),
        }
    }

    /// Replaces a named state file. Returns at once; the file is replaced
    /// whole by the saver thread, and again at shutdown if it changed. The
    /// name must be a plain file name.
    pub fn write_state(&mut self, name: &str, bytes: &[u8]) {
        if state::state_path(&self.data, name).is_some() {
            self.states.insert(name.to_string(), bytes.to_vec());
            self.s.send(SMsg::State { name: name.to_string(), bytes: bytes.to_vec() });
        }
    }

    /// Closes a text. Pass its last text and edit number if anything was
    /// typed since the last [`News::Saved`]: it is saved before closing. If
    /// its own file cannot take it, it is written beside it where that is
    /// safe (`(mine ...)` after an outside change, `(edited ...)` for a
    /// read-only file, back under its name if the file was removed), and
    /// otherwise stays in the journal for the next start, and with the
    /// store for the rest of this session: opening the text again brings it
    /// back. `caret` is remembered for the next [`Store::open_text`].
    /// [`News::Closed`] follows and says which of these happened.
    pub fn close(&mut self, doc: DocId, last: Option<(String, u64)>, caret: Option<Pos>) {
        if let Some(slot) = self.docs.get_mut(doc.0 as usize) {
            if slot.take().is_some() && !self.s.send(SMsg::Close { id: doc, last, caret }) {
                // No saver thread to close it. The journal has every edit,
                // unless the journal thread is gone as well, and then the
                // alarm is up.
                self.local.borrow_mut().push_back(News::Closed { doc, beside: None, in_journal: true });
            }
        }
    }

    /// Shuts everything down within `bound`: first the journal is flushed,
    /// which alone makes everything typed safe; then every text still open
    /// is closed as [`Store::close`] does with no last text (so close each
    /// one first to have its last text saved); then the threads end.
    ///
    /// Returns true only if all of that finished in time and everything
    /// typed is on a disk: in its file, in a file beside it, or whole in its
    /// journal. If it returns false because time ran out, whatever was not
    /// saved is in the journal and comes back at the next start. If it
    /// returns false although both threads answered, some text could be
    /// written neither to a file nor to its journal (the disks are full or
    /// gone) and goes with the process: the journal alarm was up.
    pub fn shutdown(mut self, bound: Duration) -> bool {
        self.stop(bound)
    }

    fn stop(&mut self, bound: Duration) -> bool {
        if self.stopped {
            return true;
        }
        self.stopped = true;
        let started = Instant::now();
        let left = || bound.saturating_sub(started.elapsed());
        let (tx, rx) = mpsc::channel();
        let flushed = self.j.send(JMsg::Flush { reply: Some(tx) }).is_ok() && rx.recv_timeout(left()).is_ok();
        let (tx, rx) = mpsc::channel();
        let saved = self.s.send(SMsg::Shutdown { reply: tx }) && rx.recv_timeout(left()) == Ok(true);
        // From here a save still under way must not touch the folder: the
        // journal is about to close with everything in it, and could no
        // longer record what such a save did.
        self.shared.stopping.store(true, Ordering::Release);
        let (tx, rx) = mpsc::channel();
        let ended = self.j.send(JMsg::Stop { reply: tx }).is_ok() && rx.recv_timeout(left().max(Duration::from_millis(50))) == Ok(true);
        let all = flushed && saved && ended;
        for thread in self.threads.drain(..) {
            if all {
                let _ = thread.join();
            }
        }
        all
    }

    /// Stops both threads at once, as a kill would: nothing is flushed,
    /// saved or closed, and every journal is left as it stands. For tests of
    /// recovery.
    #[doc(hidden)]
    pub fn abandon(mut self) {
        self.stopped = true;
        self.s.send(SMsg::Abandon);
        let _ = self.j.send(JMsg::Abandon);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.stop(Duration::from_secs(3));
    }
}

/// Replaces a file whole: written under a temporary name beside it, flushed,
/// renamed into place and flushed again, so that after a crash or a power
/// cut the file holds the old bytes or the new and never a mixture. This is
/// what the state files use; it is offered for the settings file as well.
/// On failure the Win32 error number comes back and the old file is untouched.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), u32> {
    fsio::write_plain(path, bytes, true).map(|_| ()).map_err(|fail| match fail {
        fsio::Fail::Io(code) | fsio::Fail::NotDurable(code) => code,
        fsio::Fail::FolderGone | fsio::Fail::Missing => 3,
        fsio::Fail::DiskFull => 112,
        fsio::Fail::Busy => 32,
        fsio::Fail::BadPath => 123,
        _ => 5,
    })
}
