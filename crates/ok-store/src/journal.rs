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
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
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
    /// End the thread at once, as a kill would: nothing flushed, nothing closed.
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

struct Doc {
    docid: u64,
    /// The session this journal belongs to: the same in every file made by
    /// starting it afresh.
    lineage: u64,
    /// The generation of `file`, or the last one used.
    generation: u32,
    file: Option<File>,
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
    unflushed: bool,
    /// A write failed: `file` may end in half a record and must be replaced.
    broken: bool,
    eager: bool,
    spent: Vec<File>,
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
}

impl Journal {
    /// Starts the next generation of a text's journal holding its `Open`
    /// record and every uncovered edit. With `durable` the file is written
    /// under a `.part` name, flushed, renamed and flushed again before the
    /// old generation is given up, so that at every instant one complete
    /// journal exists; without it (a text's very first edit, when there is
    /// nothing older to protect) the file is simply written.
    fn fresh(&mut self, id: DocId, durable: bool) -> Result<(), u32> {
        let Some(doc) = self.docs.get(&id) else { return Ok(()) };
        let on_disk = if doc.file.is_none() { highest_on_disk(&self.dir, doc.docid) } else { 0 };
        let Some(doc) = self.docs.get_mut(&id) else { return Ok(()) };
        let mut open = doc.base.clone();
        open.known = doc.known.clone();
        open.wall_ms = sys::unix_ms_now();
        let mut generation = doc.generation.max(on_disk);
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
            let made = OpenOptions::new().write(true).create_new(true).access_mode(sys::GENERIC_WRITE | sys::DELETE | sys::SYNCHRONIZE).share_mode(sys::SHARE_READ).open(&first_path);
            let mut file = match made {
                Ok(file) => file,
                Err(e) => match sys::code(&e) {
                    // Taken, or a name whose file is on its way out while a
                    // reader still holds it: the next number is free.
                    80 | 183 | 5 => continue,
                    code => return Err(code.max(1)),
                },
            };
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
                    for old in doc.spent.drain(..) {
                        sys::delete_on_close(&old);
                    }
                    doc.file = Some(file);
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

    /// Gives a text that needs one a sound journal file, releasing the
    /// reserve and trying once more if the disk is full.
    fn repair(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.get(&id) else { return true };
        if !doc.needs_file() {
            return true;
        }
        let durable = doc.broken || doc.eager || !doc.spent.is_empty();
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

    /// Brings one text's journal fully onto the disk: repaired if broken,
    /// written, flushed. True when everything recorded for it is durable.
    fn settle(&mut self, id: DocId) -> bool {
        if !self.repair(id) || !self.write_out(id) {
            return self.repair(id);
        }
        let Some(doc) = self.docs.get_mut(&id) else { return true };
        if !doc.unflushed {
            return true;
        }
        match doc.file.as_ref().map(File::sync_all) {
            Some(Err(_)) => {
                doc.broken = true;
                self.repair(id)
            }
            _ => {
                doc.unflushed = false;
                true
            }
        }
    }

    /// Settles every text. True when all of them are whole on the disk.
    fn settle_all(&mut self) -> bool {
        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        let mut all = true;
        for id in ids {
            if let Some(doc) = self.docs.get_mut(&id) {
                if doc.caret_moved && doc.healthy() {
                    if let Some(caret) = doc.base.caret {
                        wal::push_caret(&mut doc.out, doc.seq, caret);
                        doc.seq += 1;
                    }
                    doc.caret_moved = false;
                }
            }
            all &= self.settle(id);
        }
        self.first_unflushed = None;
        all
    }

    /// The barrier before a save: is `text` the text at `edit`, and is every
    /// edit up to it on the disk?
    fn begin(&mut self, id: DocId, edit: u64, state: State, format: Format, text: Arc<String>, temp: &str) -> Barrier {
        let Some(doc) = self.docs.get_mut(&id) else { return Barrier::Unconfirmed };
        match doc.len_at(edit) {
            Some(expected) if expected != text.len() as u64 => {
                if !doc.doubt {
                    return Barrier::Mismatch;
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
            return Barrier::Ready;
        }
        if doc.healthy() && (news || !doc.settled()) {
            wal::push_begin(&mut doc.out, doc.seq, edit, state, format, temp);
            doc.seq += 1;
        }
        if self.settle(id) {
            Barrier::Ready
        } else {
            Barrier::Unconfirmed
        }
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
                    floor = floor.max(old.generation);
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
                    file: None,
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
                    broken: false,
                    eager,
                    spent,
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
                    wal::push_edit(&mut doc.out, doc.seq, &payload);
                    doc.seq += 1;
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
                let answer = self.begin(id, edit, state, format, text, &temp);
                let _ = reply.send(answer);
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
                    let long = doc.written + doc.out.len() as u64 > limit;
                    if (doc.broken || long) && self.fresh(id, true).is_ok() {
                        return true;
                    }
                    if let Some(doc) = self.docs.get_mut(&id) {
                        if doc.healthy() {
                            wal::push_done(&mut doc.out, doc.seq, edit, stamp, format);
                            doc.seq += 1;
                        }
                    }
                }
            }
            JMsg::Path { id, old, new, reply } => {
                let ok = match self.docs.get_mut(&id) {
                    None => true,
                    Some(doc) => {
                        doc.base.rel = new.clone();
                        if doc.file.is_none() && !doc.needs_file() {
                            true
                        } else {
                            if doc.healthy() {
                                wal::push_path(&mut doc.out, doc.seq, &old, &new);
                                doc.seq += 1;
                            }
                            self.settle(id)
                        }
                    }
                };
                let _ = reply.send(ok);
            }
            JMsg::Folder { folder, reply } => {
                // A journal that went on naming the folder's old path would,
                // after a kill, be taken for the journal of a folder that is
                // gone, and its text put aside instead of into its file.
                let ids: Vec<DocId> = self.docs.keys().copied().collect();
                let mut ok = true;
                for id in ids {
                    let Some(doc) = self.docs.get_mut(&id) else { continue };
                    doc.base.folder = folder.clone();
                    if doc.file.is_none() && !doc.needs_file() {
                        continue;
                    }
                    if doc.healthy() {
                        wal::push_folder(&mut doc.out, doc.seq, &folder);
                        doc.seq += 1;
                    }
                    ok &= self.settle(id);
                }
                let _ = reply.send(ok);
            }
            JMsg::Close { id, covered, reply } => {
                let clean = self.docs.get(&id).is_some_and(|doc| covered.is_some_and(|(edit, len)| edit >= doc.last_edit && doc.len_at(edit).is_none_or(|ours| ours == len)));
                let whole = if clean {
                    // Every edit is in a file of his: the journal has done its work.
                    if let Some(mut doc) = self.docs.remove(&id) {
                        if let Some(mut file) = doc.file.take() {
                            if !doc.broken {
                                wal::push_close(&mut doc.out, doc.seq);
                                let _ = write(&mut file, &doc.out, &self.tuning.faults);
                                let _ = file.sync_all();
                            }
                            sys::delete_on_close(&file);
                        }
                        for old in doc.spent.drain(..) {
                            sys::delete_on_close(&old);
                        }
                    }
                    true
                } else {
                    // Something typed is in the journal alone. It stays, in
                    // its file for the next start and here in memory for the
                    // rest of this session: if the file cannot be written
                    // just now, memory is where the text is, and letting go
                    // of it would be losing it.
                    if let Some(doc) = self.docs.get_mut(&id) {
                        if doc.caret_moved && doc.healthy() {
                            if let Some(caret) = doc.base.caret {
                                wal::push_caret(&mut doc.out, doc.seq, caret);
                                doc.seq += 1;
                            }
                            doc.caret_moved = false;
                        }
                    }
                    self.docs.contains_key(&id) && self.settle(id)
                };
                if let Some(reply) = reply {
                    let _ = reply.send(whole);
                }
            }
            JMsg::Held { id, reply } => {
                let held = self.docs.get(&id).map(|doc| {
                    let text = wal::rebuild(&doc.base.text, doc.tail.iter().map(|entry| entry.payload.as_slice()));
                    Held { generation: doc.generation, text, caret: doc.base.caret, known: doc.known.clone() }
                });
                let _ = reply.send(held);
            }
            JMsg::Flush { reply } => {
                let all = self.settle_all();
                self.publish();
                if let Some(reply) = reply {
                    let _ = reply.send(all);
                }
            }
            JMsg::Stop { reply } => {
                let all = self.settle_all();
                self.publish();
                let _ = reply.send(all);
                return false;
            }
            JMsg::Abandon => {
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
        let next = match (flush, self.retry_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        next.map(|at| at.saturating_duration_since(now))
    }

    /// Does what has come due: the group flush, and another try at a broken journal.
    fn tick(&mut self) {
        let now = Instant::now();
        if let Some(first) = self.first_unflushed {
            if now >= first + self.tuning.flush_after || now >= self.last_record + self.tuning.flush_pause {
                self.settle_all();
                self.publish();
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
        self.settle_all();
    }
}

/// Starts the journal thread on `dir`.
pub(crate) fn spawn(dir: PathBuf, tuning: Tuning, shared: Arc<Shared>, out: NewsOut, rx: Receiver<JMsg>) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("ok-journal".to_string()).spawn(move || {
        let journal = Journal { dir, tuning, shared, out, docs: HashMap::new(), received: 0, first_unflushed: None, last_record: Instant::now(), retry_at: None, reserve: false };
        journal.run(rx);
    })
}
