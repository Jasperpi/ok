//! The journal thread: the only code that writes under `journal\`.
//!
//! Every edit is handed to this thread over a channel and written to the
//! text's journal file as soon as it arrives; the file is flushed a second
//! after the first unflushed record, or a third of a second into a pause, or
//! when a save is about to touch the writing folder. The thread keeps in
//! memory every edit that no finished save covers, so it can start a fresh
//! journal file at any time: when the old one has grown long, when a write to
//! it failed, or when the text is first typed in.
//!
//! The flushes themselves are not made on this thread. A flush can take a
//! tenth of a second when the disk is busy, and an edit that waited behind
//! one would be lost to a kill in that time. They are handed to a thread of
//! their own, the flush thread, which flushes through a second handle on each
//! journal file: Windows makes every call on one handle wait for the call
//! before it, so a flush through the handle edits are written with would hold
//! the edits up all the same. A save waiting at the barrier is answered by
//! the flush thread, once what it waits for is on the disk. A journal started
//! afresh because the old one grew long is written whole under a `.part`
//! name and from then on given every record the old one is, until it has been
//! flushed, renamed into place and flushed again; only then is the old one
//! let go. At every instant the journal file recovery would choose holds
//! everything written so far.
//!
//! It also keeps count. From the text a journal starts with and the edits
//! since, it knows how long the text is after each edit, whatever the
//! interface says the length is. A save whose text has another length than
//! its edit number gives is refused before it touches the folder, and a text
//! is only ever counted as safely in a file when the lengths agree.
//!
//! A text that is closed with something still unsaved is not forgotten: its
//! journal stays open here, with the edits in memory, until the text is
//! opened again or the store shuts down. If the journal's disk is failing at
//! that moment the memory is all there is, and it is kept and written out as
//! soon as the disk takes it.
//!
//! Nothing in here panics on what a disk does. A failed write marks the
//! journal broken, raises the alarm, and is retried as a whole new file.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use crate::sys::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_text::Pos;

use crate::format::Format;
use crate::hash::{self, State};
use crate::names;
use crate::sys;
use crate::wal::{self, Open, Stamp};
use crate::{DocId, Faults, NewsOut, Shared, Tuning};

/// Most messages taken from the channel before what they produced is written.
const DRAIN: usize = 128;
const RESERVE_BYTES: u64 = 1 << 20;
/// How often the journal thread looks for the flush thread's answers while
/// some are owed. Answers somebody is waiting for go straight to them.
const ANSWERS_EVERY: Duration = Duration::from_millis(1);

pub(crate) enum JMsg {
    /// A text has been opened. No file is made until its first edit, unless
    /// the text came back from an earlier journal.
    Attach(Box<Attach>),
    /// One edit, already encoded. This is what a keystroke sends.
    Edit { id: DocId, edit: u64, payload: Vec<u8> },
    /// The caret moved. Only the latest is written, at the next flush.
    Caret { id: DocId, at: Pos },
    /// A save of `text`, said to be the text at `edit`, is about to touch
    /// the folder: check that it is, write the record, flush, then answer.
    Begin { id: DocId, edit: u64, state: State, format: Format, text: Arc<String>, temp: String, reply: Sender<Barrier> },
    /// That save is on disk, flushed after its rename.
    Done { id: DocId, edit: u64, state: State, stamp: Stamp, format: Format, text: Arc<String> },
    /// The text is about to be renamed inside the folder: write, flush, answer.
    Path { id: DocId, old: PathBuf, new: PathBuf, reply: Sender<bool> },
    /// The writing folder itself was found at a new place: write that into
    /// every journal, flush, answer.
    Folder { folder: PathBuf, reply: Sender<bool> },
    /// The text is closed. `covered` is the highest edit number whose text
    /// is safely in a file of his, and that text's length; the journal is
    /// removed only if no later edit was ever recorded and the length is the
    /// one that edit gives. Otherwise the journal is kept, here in memory
    /// too, and the answer says whether it is whole on the disk.
    Close { id: DocId, covered: Option<(u64, u64)>, reply: Option<Sender<bool>> },
    /// A text closed with something unsaved is being opened again: say what
    /// is held for it. Nothing is let go of until the text's new handle is
    /// attached and names this one as the one it replaces.
    Held { id: DocId, reply: Sender<Option<Held>> },
    /// Write and flush everything now. The answer says whether all of it is
    /// on the disk.
    Flush { reply: Option<Sender<bool>> },
    /// Flush, leave every remaining journal in place, and end the thread.
    Stop { reply: Sender<bool> },
    /// End the thread at once, as a kill would: nothing flushed, nothing
    /// closed. Only a new journal generation already being put in place is
    /// let finish.
    Abandon,
}

/// What the journal answers when a save asks to go ahead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Barrier {
    /// Every edit up to the one being saved is on the journal disk, and the
    /// text has the length those edits give.
    Ready,
    /// The journal could not be written or flushed. The save goes ahead all
    /// the same: getting the text into the file is then the best protection.
    Unconfirmed,
    /// The text is not the text at that edit number: its length is another.
    /// Nothing may be written or reported saved for it.
    Mismatch,
}

pub(crate) struct Attach {
    pub id: DocId,
    pub docid: u64,
    /// What an `Open` record for the text as opened would say.
    pub base: Open,
    /// Journal generations up to this one are spoken for.
    pub floor: u32,
    /// Make the file now and flush it, instead of waiting for an edit.
    pub eager: bool,
    /// Earlier journals this one replaces: removed once it is on disk.
    pub spent: Vec<File>,
    /// The handle the same text had when it was closed with something
    /// unsaved earlier in this session. What is still held under it (its
    /// journal file above all) passes to this one.
    pub replaces: Option<DocId>,
}

/// What the journal thread holds for a text that was closed unsaved.
pub(crate) struct Held {
    /// The generation of its journal file; a new one must be numbered above it.
    pub generation: u32,
    /// The text as last typed, rebuilt from memory.
    pub text: String,
    pub caret: Option<Pos>,
    /// Fingerprints of every state of ours the file in the folder may show.
    pub known: Vec<State>,
}

/// One edit no finished save covers.
struct Entry {
    edit: u64,
    /// The text's length after it, as worked out here.
    len_after: u64,
    payload: Vec<u8>,
}

/// The next generation of a text's journal while it is put in place: written
/// whole under a `.part` name, then flushed, renamed and flushed again by
/// turns of the flush thread. Until then every record the text's journal
/// gets is written to it as well.
struct Next {
    file: File,
    /// A second handle on it, for the flush thread.
    sync: Option<File>,
    generation: u32,
    seq: u64,
    written: u64,
    /// Records framed for it but not yet written.
    out: Vec<u8>,
    /// The name it is to have.
    to: PathBuf,
}

impl Next {
    fn flush_handle(&self) -> Option<File> {
        self.sync.as_ref().unwrap_or(&self.file).try_clone().ok()
    }
}

struct Doc {
    docid: u64,
    /// The session this journal belongs to: the same in every file made by
    /// starting it afresh.
    lineage: u64,
    /// The generation of `file`, or the last one used.
    generation: u32,
    /// The highest generation number taken for this text, kept or not.
    used: u32,
    file: Option<File>,
    /// A second handle on `file`, for the flush thread alone, so that a
    /// flush never holds up a write.
    sync: Option<File>,
    /// Bytes written to `file`.
    written: u64,
    /// The next record's sequence number in `file`.
    seq: u64,
    base: Open,
    /// Fingerprints of every state of ours the file in the folder may show.
    known: Vec<State>,
    /// Every edit no finished save covers, in order.
    tail: Vec<Entry>,
    last_edit: u64,
    /// The text's length after `last_edit`.
    text_len: u64,
    /// The interface's count of the text's length and ours have parted: the
    /// journal may not be rebuilding the text that is on the screen.
    doubt: bool,
    /// Records framed but not yet written.
    out: Vec<u8>,
    caret_moved: bool,
    /// Written to since a flush was last asked for.
    unflushed: bool,
    /// Flushes of this text handed to the flush thread and not yet answered.
    syncing: usize,
    /// A write failed: `file` may end in half a record and must be replaced.
    broken: bool,
    eager: bool,
    /// Earlier journals this one replaces, let go once it is on the disk.
    spent: Vec<File>,
    next: Option<Next>,
}

impl Doc {
    fn healthy(&self) -> bool {
        self.file.is_some() && !self.broken
    }

    /// True when there is something to journal and no sound file to hold it.
    fn needs_file(&self) -> bool {
        self.broken || (self.file.is_none() && (self.eager || !self.tail.is_empty()))
    }

    /// True when everything recorded for this text is in its file and flushed.
    fn settled(&self) -> bool {
        !self.needs_file() && !self.unflushed && self.out.is_empty()
    }

    /// The text's length after edit `edit`, if that edit is still in view.
    fn len_at(&self, edit: u64) -> Option<u64> {
        if edit == self.base.edit {
            return Some(self.base.text.len() as u64);
        }
        self.tail.iter().rev().find(|entry| entry.edit == edit).map(|entry| entry.len_after)
    }

    /// Frames one record for the journal file, and for the next generation
    /// too while one is being put in place.
    fn frame(&mut self, put: impl Fn(&mut Vec<u8>, u64)) {
        put(&mut self.out, self.seq);
        self.seq += 1;
        if let Some(next) = &mut self.next {
            put(&mut next.out, next.seq);
            next.seq += 1;
        }
    }

    /// Gives up a next generation on its way in: the present file still
    /// holds everything.
    fn drop_next(&mut self) {
        if let Some(next) = self.next.take() {
            sys::delete_on_close(&next.file);
        }
    }

    /// What the flush thread flushes `file` through.
    fn flush_handle(&self) -> Option<File> {
        self.sync.as_ref().or(self.file.as_ref()).and_then(|file| file.try_clone().ok())
    }

    /// Starts over from `text` as the text at `edit`: the edits after it are
    /// kept and counted on from its length.
    fn rebase(&mut self, edit: u64, text: Arc<String>) {
        self.tail.retain(|entry| entry.edit > edit);
        let mut len = text.len() as u64;
        for entry in &mut self.tail {
            if let Some((_, removed, inserted, _)) = wal::edit_lengths(&entry.payload) {
                len = (len + inserted).saturating_sub(removed);
            }
            entry.len_after = len;
            wal::set_len_after(&mut entry.payload, len);
        }
        self.base.edit = edit;
        self.base.on_disk = false;
        self.base.text = text;
        self.text_len = len;
        self.doubt = false;
    }
}

fn remember(known: &mut Vec<State>, state: State) {
    if known.last() != Some(&state) {
        known.push(state);
    }
    if known.len() > wal::KNOWN_STATES {
        let extra = known.len() - wal::KNOWN_STATES;
        known.drain(..extra);
    }
}

fn io_code(result: io::Result<()>) -> Result<(), u32> {
    result.map_err(|e| sys::code(&e).max(1))
}

fn write(file: &mut File, bytes: &[u8], faults: &Faults) -> Result<(), u32> {
    match faults.journal_write.load(Ordering::Relaxed) {
        0 => io_code(file.write_all(bytes)),
        code => Err(code),
    }
}

/// Makes a journal file whose name must be free. Other programs may read it
/// while it is open; writing is shared only so that the flush thread can
/// have a handle of its own on it.
fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).access_mode(sys::GENERIC_WRITE | sys::DELETE | sys::SYNCHRONIZE).share_mode(sys::SHARE_READ | sys::SHARE_WRITE).open(path)
}

/// The flush thread's own handle on a journal file just made. Without one
/// (another program opened the file in between, sharing nothing) flushes go
/// through the writing handle: as safe, only slower to type behind.
fn second_handle(path: &Path) -> Option<File> {
    OpenOptions::new().write(true).share_mode(sys::SHARE_ALL).open(path).ok()
}

/// The highest generation any journal file of this text has on the disk,
/// whole or half made. A new journal is always numbered above it, so that a
/// journal this session knows nothing of (one another program was holding
/// when the store started) is never outranked by accident and never reused.
fn highest_on_disk(dir: &Path, docid: u64) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter_map(|entry| match wal::parse_name(&entry.file_name().to_string_lossy()) {
            Some((id, generation, _)) if id == docid => Some(generation),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

// -------------------------------------------------------------- the flush thread

/// Who is told when a flush is done.
enum Then {
    Nothing,
    Barrier(Sender<Barrier>),
    Answer(Sender<bool>),
}

impl Then {
    fn give(self, whole: bool) {
        match self {
            Then::Nothing => {}
            Then::Barrier(reply) => {
                let _ = reply.send(if whole { Barrier::Ready } else { Barrier::Unconfirmed });
            }
            Then::Answer(reply) => {
                let _ = reply.send(whole);
            }
        }
    }
}

/// Which step of putting a next generation in place a flush is.
#[derive(Clone, Copy)]
enum Stage {
    Plain,
    /// The next generation is written whole; once flushed it may be named.
    Part(DocId, u32),
    /// It has its name; once flushed the one before it may go.
    Named(DocId, u32),
}

/// Files to flush, each with its text's handle and generation.
struct Job {
    files: Vec<(DocId, u32, File)>,
    /// False if something could not even be written: then nothing is whole.
    whole: bool,
    then: Then,
    stage: Stage,
}

/// What the flush thread reports back.
struct Synced {
    /// The texts this flush was for, each once.
    docs: Vec<DocId>,
    results: Vec<(DocId, u32, bool)>,
    stage: Stage,
}

fn docs_of(files: &[(DocId, u32, File)]) -> Vec<DocId> {
    let mut docs: Vec<DocId> = Vec::new();
    for (id, _, _) in files {
        if !docs.contains(id) {
            docs.push(*id);
        }
    }
    docs
}

/// Flushes what a batch of jobs asks for, each file once, and answers them.
/// Every job was handed over after the writes it waits for, so one flush
/// made once they have all arrived covers every one of them.
fn run_jobs(jobs: Vec<Job>, done: &Sender<Synced>) {
    let mut results: HashMap<(DocId, u32), bool> = HashMap::new();
    for job in &jobs {
        for (id, generation, file) in &job.files {
            results.entry((*id, *generation)).or_insert_with(|| file.sync_all().is_ok());
        }
    }
    for job in jobs {
        let mut whole = job.whole;
        let mut told = Vec::with_capacity(job.files.len());
        for (id, generation, _) in &job.files {
            let ok = results.get(&(*id, *generation)).copied().unwrap_or(false);
            whole &= ok;
            told.push((*id, *generation, ok));
        }
        // Whoever waits hears first; the journal thread's bookkeeping can follow.
        job.then.give(whole);
        let _ = done.send(Synced { docs: docs_of(&job.files), results: told, stage: job.stage });
    }
}

fn flusher(jobs: Receiver<Job>, done: Sender<Synced>, abandon: Arc<AtomicBool>) {
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        while let Ok(job) = jobs.try_recv() {
            batch.push(job);
        }
        if abandon.load(Ordering::Acquire) {
            return;
        }
        run_jobs(batch, &done);
    }
}

// ------------------------------------------------------------- the journal thread

struct Journal {
    dir: PathBuf,
    tuning: Tuning,
    shared: Arc<Shared>,
    out: NewsOut,
    docs: HashMap<DocId, Doc>,
    /// Edit messages taken from the channel so far.
    received: u64,
    first_unflushed: Option<Instant>,
    last_record: Instant,
    retry_at: Option<Instant>,
    reserve: bool,
    /// Work for the flush thread. `None` once it has been ended, or if it
    /// could not be started: flushes are then made on this thread.
    jobs: Option<Sender<Job>>,
    flush_thread: Option<std::thread::JoinHandle<()>>,
    done: Sender<Synced>,
    answers: Receiver<Synced>,
    /// Jobs handed over and not yet answered.
    in_flight: usize,
    abandon: Arc<AtomicBool>,
}

impl Journal {
    /// Hands flushes to the flush thread, or makes them here if there is none.
    fn queue(&mut self, job: Job) {
        self.in_flight += 1;
        for id in docs_of(&job.files) {
            if let Some(doc) = self.docs.get_mut(&id) {
                doc.syncing += 1;
            }
        }
        let job = match &self.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        self.jobs = None;
        run_jobs(vec![job], &self.done);
    }

    /// Takes in what the flush thread has finished.
    fn absorb(&mut self) {
        while let Ok(synced) = self.answers.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            self.synced(synced);
        }
    }

    fn synced(&mut self, synced: Synced) {
        for id in &synced.docs {
            if let Some(doc) = self.docs.get_mut(id) {
                doc.syncing = doc.syncing.saturating_sub(1);
            }
        }
        let mut broken = Vec::new();
        for (id, generation, ok) in synced.results {
            let Some(doc) = self.docs.get_mut(&id) else { continue };
            if doc.file.is_some() && generation == doc.generation {
                if !ok {
                    doc.broken = true;
                    broken.push(id);
                } else if doc.healthy() {
                    // The journal that replaces them is on the disk.
                    for old in doc.spent.drain(..) {
                        sys::delete_on_close(&old);
                    }
                }
            } else if !ok && doc.next.as_ref().is_some_and(|next| next.generation == generation) {
                doc.drop_next();
            }
        }
        match synced.stage {
            Stage::Plain => {}
            Stage::Part(id, generation) => self.name_next(id, generation),
            Stage::Named(id, generation) => self.take_next(id, generation),
        }
        for id in broken {
            self.repair(id);
        }
    }

    /// The next generation is on the disk whole: give it its name, and have
    /// it flushed once more so that the name is on the disk too.
    fn name_next(&mut self, id: DocId, generation: u32) {
        let Some(doc) = self.docs.get_mut(&id) else { return };
        let Some(next) = doc.next.as_ref().filter(|next| next.generation == generation) else { return };
        let named = match sys::rename_handle(&next.file, &next.to, false, true) {
            Err(87 | 50 | 1) => sys::rename_handle(&next.file, &next.to, false, false),
            other => other,
        };
        match (named, next.flush_handle()) {
            (Ok(()), Some(handle)) => self.queue(Job { files: vec![(id, generation, handle)], whole: true, then: Then::Nothing, stage: Stage::Named(id, generation) }),
            _ => doc.drop_next(),
        }
    }

    /// The next generation is in place: it becomes the text's journal, and
    /// the one before it is let go.
    fn take_next(&mut self, id: DocId, generation: u32) {
        let Some(doc) = self.docs.get_mut(&id) else { return };
        if doc.next.as_ref().is_none_or(|next| next.generation != generation) {
            return;
        }
        let Some(next) = doc.next.take() else { return };
        if let Some(old) = doc.file.replace(next.file) {
            sys::delete_on_close(&old);
        }
        doc.sync = next.sync;
        for old in doc.spent.drain(..) {
            sys::delete_on_close(&old);
        }
        doc.generation = next.generation;
        doc.seq = next.seq;
        doc.written = next.written;
        // What was framed for the old file and not yet written is framed here too.
        doc.out = next.out;
        doc.broken = false;
        doc.unflushed = true;
    }

    /// Waits for every next generation on its way in to be in place (or
    /// given up), so that what is on the disk afterwards is each text's one
    /// journal. Only where nothing is being typed: an answered flush is
    /// asked for by a store that is shutting down, and an abandon by a test.
    fn finish_rotations(&mut self) {
        let end = Instant::now() + self.tuning.barrier;
        while self.in_flight > 0 && self.docs.values().any(|doc| doc.next.is_some()) {
            match self.answers.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(synced) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    self.synced(synced);
                }
                Err(_) => break,
            }
        }
        for doc in self.docs.values_mut() {
            doc.drop_next();
        }
    }

    /// Ends the flush thread once it has done what it was given (at once,
    /// when `abandon`), and takes in its answers.
    fn end_flusher(&mut self, abandon: bool) {
        if abandon {
            self.abandon.store(true, Ordering::Release);
        }
        self.jobs = None;
        if let Some(thread) = self.flush_thread.take() {
            let _ = thread.join();
        }
        if abandon {
            return;
        }
        // An answer may ask for more (a rename, its flush); with the thread
        // gone that is done here and answered at once.
        for _ in 0..8 {
            self.absorb();
            if self.in_flight == 0 {
                break;
            }
        }
        for doc in self.docs.values_mut() {
            doc.drop_next();
        }
    }

    /// Starts the next generation of a text's journal holding its `Open`
    /// record and every uncovered edit. With `durable` (the file being
    /// replaced failed, and may hold edits the new one must not be seen
    /// without) the new file is written under a `.part` name, flushed,
    /// renamed and flushed again here before the old one is given up, so that
    /// at every instant one complete journal exists. Without it no file of
    /// this text holds anything the new one does not, and it is simply
    /// written under its name; earlier journals it replaces are let go once
    /// the flush thread has flushed it.
    fn fresh(&mut self, id: DocId, durable: bool) -> Result<(), u32> {
        let Some(doc) = self.docs.get(&id) else { return Ok(()) };
        let on_disk = if doc.file.is_none() { highest_on_disk(&self.dir, doc.docid) } else { 0 };
        let Some(doc) = self.docs.get_mut(&id) else { return Ok(()) };
        // A next generation on its way in is overtaken by this one.
        doc.drop_next();
        let mut open = doc.base.clone();
        open.known = doc.known.clone();
        open.wall_ms = sys::unix_ms_now();
        let mut generation = doc.generation.max(doc.used).max(on_disk);
        let mut bytes = Vec::new();
        for _ in 0..50 {
            generation += 1;
            let name = wal::file_name(doc.docid, generation);
            let last_path = self.dir.join(&name);
            if sys::stat(&last_path).is_ok() {
                continue;
            }
            let first_path = if durable { self.dir.join(format!("{name}.part")) } else { last_path.clone() };
            bytes.clear();
            bytes.extend_from_slice(&wal::header(doc.docid, generation));
            wal::push_open(&mut bytes, 0, &open);
            wal::push_lineage(&mut bytes, 1, doc.lineage);
            let mut seq = 2;
            for entry in &doc.tail {
                wal::push_edit(&mut bytes, seq, &entry.payload);
                seq += 1;
            }
            let mut file = match create(&first_path) {
                Ok(file) => file,
                Err(e) => match sys::code(&e) {
                    // Taken, or a name whose file is on its way out while a
                    // reader still holds it: the next number is free.
                    80 | 183 | 5 => continue,
                    code => return Err(code.max(1)),
                },
            };
            doc.used = doc.used.max(generation);
            let sync = second_handle(&first_path);
            let mut finish = || -> Result<(), u32> {
                write(&mut file, &bytes, &self.tuning.faults)?;
                if durable {
                    // Flushed before it gets its name: a file called .wal is
                    // then never a valid beginning with its end missing,
                    // whatever a power cut does, and "the highest whole
                    // generation wins" can be trusted. Flushed again after,
                    // so the name itself is on disk before the older
                    // generation is given up.
                    io_code(file.sync_all())?;
                    let to = names::verbatim(&last_path).ok_or(123u32)?;
                    match sys::rename_handle(&file, &to, false, true) {
                        Err(87 | 50 | 1) => sys::rename_handle(&file, &to, false, false)?,
                        other => other?,
                    }
                    io_code(file.sync_all())?;
                }
                Ok(())
            };
            match finish() {
                Ok(()) => {
                    // The new generation holds everything the old ones did.
                    if let Some(old) = doc.file.take() {
                        sys::delete_on_close(&old);
                    }
                    if durable {
                        for old in doc.spent.drain(..) {
                            sys::delete_on_close(&old);
                        }
                    }
                    doc.file = Some(file);
                    doc.sync = sync;
                    doc.generation = generation;
                    doc.written = bytes.len() as u64;
                    doc.seq = seq;
                    doc.out.clear();
                    doc.broken = false;
                    doc.eager = false;
                    doc.caret_moved = false;
                    doc.unflushed = !durable;
                    return Ok(());
                }
                Err(183 | 80) if durable => {
                    sys::delete_on_close(&file);
                    continue;
                }
                Err(code) => {
                    sys::delete_on_close(&file);
                    return Err(code);
                }
            }
        }
        Err(183)
    }

    /// Starts putting the next generation of a long journal in place without
    /// waiting on the disk: it is written whole under a `.part` name now, and
    /// the flush thread takes it on from there (see [`Journal::name_next`]).
    /// Meanwhile the present file goes on as before and the new one is given
    /// every record it is. If anything fails, the present file simply stays.
    fn rotate(&mut self, id: DocId) {
        let Some(doc) = self.docs.get_mut(&id) else { return };
        if doc.next.is_some() || !doc.healthy() {
            return;
        }
        let mut open = doc.base.clone();
        open.known = doc.known.clone();
        open.wall_ms = sys::unix_ms_now();
        let mut generation = doc.generation.max(doc.used);
        for _ in 0..50 {
            generation += 1;
            let name = wal::file_name(doc.docid, generation);
            let last_path = self.dir.join(&name);
            if sys::stat(&last_path).is_ok() {
                continue;
            }
            let part = self.dir.join(format!("{name}.part"));
            let mut file = match create(&part) {
                Ok(file) => file,
                Err(e) => match sys::code(&e) {
                    80 | 183 | 5 => continue,
                    _ => return,
                },
            };
            doc.used = doc.used.max(generation);
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&wal::header(doc.docid, generation));
            wal::push_open(&mut bytes, 0, &open);
            wal::push_lineage(&mut bytes, 1, doc.lineage);
            let mut seq = 2;
            for entry in &doc.tail {
                wal::push_edit(&mut bytes, seq, &entry.payload);
                seq += 1;
            }
            let wrote = write(&mut file, &bytes, &self.tuning.faults);
            let Some(to) = names::verbatim(&last_path) else {
                sys::delete_on_close(&file);
                return;
            };
            let sync = second_handle(&part);
            let next = Next { file, sync, generation, seq, written: bytes.len() as u64, out: Vec::new(), to };
            let handle = next.flush_handle();
            let (Ok(()), Some(handle)) = (wrote, handle) else {
                sys::delete_on_close(&next.file);
                return;
            };
            doc.next = Some(next);
            self.queue(Job { files: vec![(id, generation, handle)], whole: true, then: Then::Nothing, stage: Stage::Part(id, generation) });
            return;
        }
    }

    /// Gives a text that needs one a sound journal file, releasing the
    /// reserve and trying once more if the disk is full.
    fn repair(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.get(&id) else { return true };
        if !doc.needs_file() {
            return true;
        }
        let durable = doc.file.is_some();
        let mut result = self.fresh(id, durable);
        if matches!(result, Err(39 | 112)) && self.reserve {
            self.reserve = false;
            let _ = fs::remove_file(self.dir.join("reserve.bin"));
            result = self.fresh(id, durable);
        }
        if result.is_err() {
            if let Some(doc) = self.docs.get_mut(&id) {
                doc.broken = true;
            }
        }
        result.is_ok()
    }

    /// Writes what a text has framed. A failure marks its journal broken.
    fn write_out(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.get_mut(&id) else { return true };
        if let Some(next) = &mut doc.next {
            if !next.out.is_empty() {
                let result = write(&mut next.file, &next.out, &self.tuning.faults);
                next.written += next.out.len() as u64;
                next.out.clear();
                if result.is_err() {
                    doc.drop_next();
                }
            }
        }
        if doc.out.is_empty() {
            return true;
        }
        let Some(file) = doc.file.as_mut() else {
            doc.out.clear();
            return true;
        };
        let result = write(file, &doc.out, &self.tuning.faults);
        doc.written += doc.out.len() as u64;
        doc.out.clear();
        match result {
            Ok(()) => {
                doc.unflushed = true;
                true
            }
            Err(code) => {
                doc.broken = true;
                if matches!(code, 39 | 112) && self.reserve {
                    self.reserve = false;
                    let _ = fs::remove_file(self.dir.join("reserve.bin"));
                }
                false
            }
        }
    }

    /// Brings these texts' journals onto the disk without waiting for it:
    /// repaired if broken and written here, then flushed by the flush thread,
    /// which tells `then` whether all of it is durable.
    fn settle_then(&mut self, ids: &[DocId], then: Then) {
        let (whole, files) = self.ready_to_flush(ids);
        self.hand_over(whole, files, then);
    }

    /// The first half of [`Journal::settle_then`]: repairs and writes, and
    /// says whether that went well and which files want flushing.
    fn ready_to_flush(&mut self, ids: &[DocId]) -> (bool, Vec<(DocId, u32, File)>) {
        let mut whole = true;
        let mut files = Vec::new();
        for &id in ids {
            if (!self.repair(id) || !self.write_out(id)) && !self.repair(id) {
                whole = false;
                continue;
            }
            let Some(doc) = self.docs.get_mut(&id) else { continue };
            // A flush under way may have begun before the last write.
            if !doc.unflushed && doc.syncing == 0 {
                continue;
            }
            match doc.flush_handle() {
                Some(handle) => files.push((id, doc.generation, handle)),
                None if doc.file.is_some() => whole = false,
                None => {}
            }
            if let Some(handle) = doc.next.as_ref().and_then(Next::flush_handle) {
                files.push((id, doc.next.as_ref().map_or(0, |next| next.generation), handle));
            }
            doc.unflushed = false;
        }
        (whole, files)
    }

    /// The second half: the flush thread flushes `files` and answers `then`,
    /// or `then` is answered now if there is nothing to flush.
    fn hand_over(&mut self, whole: bool, files: Vec<(DocId, u32, File)>, then: Then) {
        if files.is_empty() {
            then.give(whole);
        } else {
            self.queue(Job { files, whole, then, stage: Stage::Plain });
        }
    }

    /// Frames every text's caret that moved.
    fn frame_carets(&mut self) {
        for doc in self.docs.values_mut() {
            if doc.caret_moved && doc.healthy() {
                if let Some(caret) = doc.base.caret {
                    doc.frame(|out, seq| wal::push_caret(out, seq, caret));
                }
                doc.caret_moved = false;
            }
        }
    }

    /// [`Journal::settle_then`] for every text. The other threads are told
    /// how far the journal has got before anyone is answered.
    fn settle_all_then(&mut self, then: Then) {
        self.frame_carets();
        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        let (whole, files) = self.ready_to_flush(&ids);
        self.first_unflushed = None;
        self.publish();
        self.hand_over(whole, files, then);
    }

    /// Brings one text's journal fully onto the disk here and now: repaired
    /// if broken, written, flushed. True when everything recorded for it is
    /// durable. Only for the end, when nothing more can be typed.
    fn settle(&mut self, id: DocId) -> bool {
        if !self.repair(id) || !self.write_out(id) {
            return self.repair(id);
        }
        let Some(doc) = self.docs.get_mut(&id) else { return true };
        if doc.unflushed {
            if let Some(Err(_)) = doc.sync.as_ref().or(doc.file.as_ref()).map(File::sync_all) {
                doc.broken = true;
                return self.repair(id);
            }
            doc.unflushed = false;
        }
        if doc.healthy() {
            // The journal that replaces them is on the disk.
            for old in doc.spent.drain(..) {
                sys::delete_on_close(&old);
            }
        }
        true
    }

    /// Settles every text here and now. True when all of them are whole on the disk.
    fn settle_all(&mut self) -> bool {
        self.frame_carets();
        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        let mut all = true;
        for id in ids {
            all &= self.settle(id);
        }
        self.first_unflushed = None;
        all
    }

    /// The barrier before a save: is `text` the text at `edit`, and is every
    /// edit up to it on the disk? The answer goes to `reply`, from the flush
    /// thread when a flush is needed for it.
    #[allow(clippy::too_many_arguments)]
    fn begin(&mut self, id: DocId, edit: u64, state: State, format: Format, text: Arc<String>, temp: &str, reply: Sender<Barrier>) {
        let Some(doc) = self.docs.get_mut(&id) else {
            let _ = reply.send(Barrier::Unconfirmed);
            return;
        };
        match doc.len_at(edit) {
            Some(expected) if expected != text.len() as u64 => {
                if !doc.doubt {
                    let _ = reply.send(Barrier::Mismatch);
                    return;
                }
                // The interface's own count had already parted from ours,
                // and now its text is not the length the journaled edits
                // give: this journal no longer rebuilds what is on the
                // screen. The text is the buffer itself, so start over from
                // it, in a new file.
                doc.rebase(edit, text);
                if doc.file.is_some() {
                    doc.broken = true;
                }
            }
            // Our count and the buffer agree after all: only a number was off.
            Some(_) if edit == doc.last_edit => doc.doubt = false,
            _ => {}
        }
        let news = doc.known.last() != Some(&state);
        remember(&mut doc.known, state);
        // The style the file is written in from here on, for any journal
        // started afresh before this save's own record of it.
        doc.base.format = format;
        if doc.file.is_none() && !doc.needs_file() {
            // Nothing was ever journaled for this text, so there is nothing to make safe.
            let _ = reply.send(Barrier::Ready);
            return;
        }
        if doc.healthy() && (news || !doc.settled()) {
            doc.frame(|out, seq| wal::push_begin(out, seq, edit, state, format, temp));
        }
        self.settle_then(&[id], Then::Barrier(reply));
    }

    /// Handles one message. False ends the thread.
    fn handle(&mut self, msg: JMsg) -> bool {
        match msg {
            JMsg::Attach(attach) => {
                let Attach { id, docid, base, mut floor, eager, mut spent, replaces } = *attach;
                if let Some(mut old) = replaces.and_then(|old| self.docs.remove(&old)) {
                    // The same text under its earlier handle: its journal is
                    // spent once the new one, which starts from the whole
                    // text, is on the disk.
                    floor = floor.max(old.generation).max(old.used);
                    if let Some(next) = old.next.take() {
                        floor = floor.max(next.generation);
                        spent.push(next.file);
                    }
                    spent.append(&mut old.spent);
                    spent.extend(old.file.take());
                    if !eager {
                        // The file in the folder already says everything
                        // that journal did: it has done its work.
                        for file in spent.drain(..) {
                            sys::delete_on_close(&file);
                        }
                    }
                }
                let mut known = base.known.clone();
                if base.on_disk {
                    remember(&mut known, base.disk);
                }
                let doc = Doc {
                    docid,
                    lineage: hash::random() | 1,
                    generation: floor,
                    used: floor,
                    file: None,
                    sync: None,
                    written: 0,
                    seq: 0,
                    last_edit: base.edit,
                    text_len: base.text.len() as u64,
                    doubt: false,
                    base,
                    known,
                    tail: Vec::new(),
                    out: Vec::new(),
                    caret_moved: false,
                    unflushed: false,
                    syncing: 0,
                    broken: false,
                    eager,
                    spent,
                    next: None,
                };
                self.docs.insert(id, doc);
                self.repair(id);
            }
            JMsg::Edit { id, edit, mut payload } => {
                self.received += 1;
                let Some(doc) = self.docs.get_mut(&id) else { return true };
                // The length after this edit by our own count, against the
                // length the interface says the document now has. What goes
                // into the file is ours, so that the file always replays; a
                // difference is remembered as doubt and raises the alarm
                // until the two agree again.
                match wal::edit_lengths(&payload) {
                    Some((_, removed, inserted, said)) => {
                        let len = (doc.text_len + inserted).saturating_sub(removed);
                        doc.doubt = said != len;
                        if doc.doubt {
                            wal::set_len_after(&mut payload, len);
                        }
                        doc.text_len = len;
                    }
                    None => doc.doubt = true,
                }
                doc.last_edit = edit;
                if doc.healthy() {
                    doc.frame(|out, seq| wal::push_edit(out, seq, &payload));
                }
                doc.tail.push(Entry { edit, len_after: doc.text_len, payload });
                self.last_record = Instant::now();
                self.repair(id);
            }
            JMsg::Caret { id, at } => {
                if let Some(doc) = self.docs.get_mut(&id) {
                    if doc.base.caret != Some(at) {
                        doc.base.caret = Some(at);
                        doc.caret_moved = true;
                    }
                }
            }
            JMsg::Begin { id, edit, state, format, text, temp, reply } => {
                self.begin(id, edit, state, format, text, &temp, reply);
            }
            JMsg::Done { id, edit, state, stamp, format, text } => {
                let Some(doc) = self.docs.get_mut(&id) else { return true };
                remember(&mut doc.known, state);
                // A text that is not the text at that edit covers nothing,
                // and an edit from before the journal's own start is past.
                if edit < doc.base.edit || doc.len_at(edit).is_some_and(|len| len != text.len() as u64) {
                    return true;
                }
                doc.base.edit = edit;
                doc.base.on_disk = true;
                doc.base.format = format;
                doc.base.disk = state;
                doc.base.stamp = stamp;
                doc.tail.retain(|entry| entry.edit > edit);
                let limit = self.tuning.rotate_at.max(4 * text.len() as u64);
                doc.base.text = text;
                if doc.file.is_some() {
                    if doc.broken && self.fresh(id, true).is_ok() {
                        return true;
                    }
                    let Some(doc) = self.docs.get_mut(&id) else { return true };
                    if doc.healthy() {
                        doc.frame(|out, seq| wal::push_done(out, seq, edit, stamp, format));
                        if doc.written + doc.out.len() as u64 > limit {
                            // Started afresh from this save, after the record of it.
                            self.rotate(id);
                        }
                    }
                }
            }
            JMsg::Path { id, old, new, reply } => match self.docs.get_mut(&id) {
                None => {
                    let _ = reply.send(true);
                }
                Some(doc) => {
                    doc.base.rel = new.clone();
                    if doc.file.is_none() && !doc.needs_file() {
                        let _ = reply.send(true);
                    } else {
                        if doc.healthy() {
                            doc.frame(|out, seq| wal::push_path(out, seq, &old, &new));
                        }
                        self.settle_then(&[id], Then::Answer(reply));
                    }
                }
            },
            JMsg::Folder { folder, reply } => {
                // A journal that went on naming the folder's old path would,
                // after a kill, be taken for the journal of a folder that is
                // gone, and its text put aside instead of into its file.
                let mut ids = Vec::new();
                for (&id, doc) in self.docs.iter_mut() {
                    doc.base.folder = folder.clone();
                    if doc.file.is_none() && !doc.needs_file() {
                        continue;
                    }
                    if doc.healthy() {
                        doc.frame(|out, seq| wal::push_folder(out, seq, &folder));
                    }
                    ids.push(id);
                }
                self.settle_then(&ids, Then::Answer(reply));
            }
            JMsg::Close { id, covered, reply } => {
                let clean = self.docs.get(&id).is_some_and(|doc| covered.is_some_and(|(edit, len)| edit >= doc.last_edit && doc.len_at(edit).is_none_or(|ours| ours == len)));
                if clean {
                    // Every edit is in a file of his: the journal has done its work.
                    if let Some(mut doc) = self.docs.remove(&id) {
                        doc.drop_next();
                        if let Some(mut file) = doc.file.take() {
                            if !doc.broken {
                                wal::push_close(&mut doc.out, doc.seq);
                                let _ = write(&mut file, &doc.out, &self.tuning.faults);
                                // Flushed on the flush thread; the file goes when that is done.
                                if let Ok(handle) = doc.sync.as_ref().unwrap_or(&file).try_clone() {
                                    self.queue(Job { files: vec![(id, doc.generation, handle)], whole: true, then: Then::Nothing, stage: Stage::Plain });
                                }
                            }
                            sys::delete_on_close(&file);
                        }
                        for old in doc.spent.drain(..) {
                            sys::delete_on_close(&old);
                        }
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(true);
                    }
                } else {
                    // Something typed is in the journal alone. It stays, in
                    // its file for the next start and here in memory for the
                    // rest of this session: if the file cannot be written
                    // just now, memory is where the text is, and letting go
                    // of it would be losing it.
                    let then = reply.map_or(Then::Nothing, Then::Answer);
                    match self.docs.get_mut(&id) {
                        None => then.give(false),
                        Some(doc) => {
                            if doc.caret_moved && doc.healthy() {
                                if let Some(caret) = doc.base.caret {
                                    doc.frame(|out, seq| wal::push_caret(out, seq, caret));
                                }
                                doc.caret_moved = false;
                            }
                            self.settle_then(&[id], then);
                        }
                    }
                }
            }
            JMsg::Held { id, reply } => {
                let held = self.docs.get(&id).map(|doc| {
                    let text = wal::rebuild(&doc.base.text, doc.tail.iter().map(|entry| entry.payload.as_slice()));
                    let generation = doc.generation.max(doc.used);
                    Held { generation, text, caret: doc.base.caret, known: doc.known.clone() }
                });
                let _ = reply.send(held);
            }
            JMsg::Flush { reply } => {
                if reply.is_some() {
                    self.finish_rotations();
                }
                self.settle_all_then(reply.map_or(Then::Nothing, Then::Answer));
            }
            JMsg::Stop { reply } => {
                self.end_flusher(false);
                let all = self.settle_all();
                self.publish();
                let _ = reply.send(all);
                return false;
            }
            JMsg::Abandon => {
                // A next generation already on its way in is let into place
                // first: the moment just after that is a moment a kill could
                // come, and tests that stand in for one can then say which
                // journal file is there.
                self.finish_rotations();
                self.end_flusher(true);
                let ids: Vec<DocId> = self.docs.keys().copied().collect();
                for id in ids {
                    self.write_out(id);
                }
                return false;
            }
        }
        true
    }

    /// After a batch of messages: write what they framed and tell the other
    /// threads how far the journal has got.
    fn after_batch(&mut self) {
        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        let now = Instant::now();
        for id in ids {
            self.write_out(id);
            if let Some(doc) = self.docs.get(&id) {
                if doc.unflushed || (doc.caret_moved && doc.healthy()) {
                    self.first_unflushed.get_or_insert(now);
                }
                if doc.needs_file() {
                    self.retry_at.get_or_insert(now + Duration::from_secs(1));
                }
            }
        }
        self.publish();
    }

    /// Tells the interface's side how far the journal has got, and wakes it
    /// when that is news: when a journal starts or stops failing, and, while
    /// the interface is showing the alarm for a journal that had stopped
    /// keeping up, whenever there is progress for it to see.
    fn publish(&mut self) {
        let failed = self.docs.values().any(|doc| doc.broken || doc.doubt);
        if !failed {
            self.shared.written.store(self.received, Ordering::SeqCst);
        }
        let changed = self.shared.journal_failed.swap(failed, Ordering::AcqRel) != failed;
        if changed || self.shared.watched.load(Ordering::SeqCst) {
            self.out.nudge();
        }
    }

    /// How long to sleep when no message comes.
    fn wait(&self) -> Option<Duration> {
        let now = Instant::now();
        let flush = self.first_unflushed.map(|first| (first + self.tuning.flush_after).min(self.last_record + self.tuning.flush_pause));
        let mut next = match (flush, self.retry_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if self.in_flight > 0 {
            let soon = now + ANSWERS_EVERY;
            next = Some(next.map_or(soon, |at| at.min(soon)));
        }
        next.map(|at| at.saturating_duration_since(now))
    }

    /// Does what has come due: the group flush, and another try at a broken journal.
    fn tick(&mut self) {
        let now = Instant::now();
        if let Some(first) = self.first_unflushed {
            if now >= first + self.tuning.flush_after || now >= self.last_record + self.tuning.flush_pause {
                self.settle_all_then(Then::Nothing);
            }
        }
        if self.retry_at.is_some_and(|at| now >= at) {
            self.retry_at = None;
            let ids: Vec<DocId> = self.docs.keys().copied().collect();
            for id in ids {
                if !self.repair(id) {
                    self.retry_at = Some(now + Duration::from_secs(1));
                }
            }
            self.publish();
        }
    }

    fn run(mut self, rx: Receiver<JMsg>) {
        // A megabyte set aside so that typing can go on being journaled for
        // hours after the disk fills. set_len really takes the clusters.
        let reserve = self.dir.join("reserve.bin");
        self.reserve = match sys::stat(&reserve) {
            Ok(info) => info.size >= RESERVE_BYTES,
            Err(_) => OpenOptions::new().write(true).create_new(true).open(&reserve).and_then(|file| file.set_len(RESERVE_BYTES)).is_ok(),
        };
        loop {
            let first = match self.wait() {
                Some(wait) => match rx.recv_timeout(wait) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                },
            };
            self.absorb();
            if let Some(msg) = first {
                if !self.handle(msg) {
                    return;
                }
                for _ in 0..DRAIN {
                    let Ok(msg) = rx.try_recv() else { break };
                    if !self.handle(msg) {
                        return;
                    }
                }
                self.after_batch();
            }
            self.tick();
        }
        // Every sender is gone without a goodbye: leave the journals whole.
        self.end_flusher(false);
        self.settle_all();
    }
}

/// Starts the journal thread on `dir`, with its flush thread.
pub(crate) fn spawn(dir: PathBuf, tuning: Tuning, shared: Arc<Shared>, out: NewsOut, rx: Receiver<JMsg>) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("ok-journal".to_string()).spawn(move || {
        let (done, answers) = mpsc::channel();
        let (jobs, work) = mpsc::channel();
        let abandon = Arc::new(AtomicBool::new(false));
        let flush_thread = {
            let (done, abandon) = (done.clone(), abandon.clone());
            std::thread::Builder::new().name("ok-journal-flush".to_string()).spawn(move || flusher(work, done, abandon)).ok()
        };
        let jobs = flush_thread.as_ref().map(|_| jobs);
        let journal = Journal {
            dir,
            tuning,
            shared,
            out,
            docs: HashMap::new(),
            received: 0,
            first_unflushed: None,
            last_record: Instant::now(),
            retry_at: None,
            reserve: false,
            jobs,
            flush_thread,
            done,
            answers,
            in_flight: 0,
            abandon,
        };
        journal.run(rx);
    })
}
