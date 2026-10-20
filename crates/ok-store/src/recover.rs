//! What is done at start with the journals an earlier session left behind.
//!
//! A journal is there because a text was open when the program stopped
//! without closing it: killed, crashed, or the power went. Each one is
//! replayed to the text as last typed and compared with the file in the
//! folder. The file is brought forward only when its bytes are a state this
//! program itself once saved; anything else there is left alone and the
//! journal's text is put beside it. A journal is removed only after its text
//! is durably in a file of his, so running this twice, or dying in the middle
//! of it, changes nothing.
//!
//! Whose a journal is, is decided by the writing folder it names and by that
//! folder's lock, never by who happens to have the journal file open. Every
//! starting instance reads every journal, so the files are opened in ways
//! that keep nobody out who only reads: an instance looking through another
//! folder's journals, a scanner, a backup tool. A journal that cannot be
//! read at all just now is said to be waiting, never passed by in silence.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use crate::sys::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::format::{self, Decoded};
use crate::fsio::{self, Fail};
use crate::hash;
use crate::history::History;
use crate::names;
use crate::state;
use crate::sys::{self, Volume};
use crate::wal::{self, Replay};
use crate::Faults;

/// Largest file recovery will read to compare.
const LIMIT: u64 = 64 << 20;
/// Pauses, in milliseconds, while another program is writing the file.
const PAUSES: [u64; 3] = [10, 30, 100];
/// Pauses, in milliseconds, while another program keeps a journal file from
/// being taken up. After the last, the journal is said to be waiting.
pub(crate) const TAKE_PAUSES: [u64; 4] = [5, 15, 40, 80];

/// One thing recovery did, for the interface to say in a sentence.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recovered {
    /// The writing folder the text belongs to: the one being opened, or
    /// another one whose journal was found waiting.
    pub folder: PathBuf,
    /// The text's path inside that folder.
    pub rel: PathBuf,
    /// What was done with the text the journal held.
    pub how: Recovery,
    /// How many words the journal held that the file did not: the number in
    /// "Brought back 41 words typed after the last save".
    pub words: usize,
}

/// What became of a journal's text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Recovery {
    /// The file was an earlier save of ours; it now holds everything typed.
    BroughtBack,
    /// The file was gone while text was still unsaved; it was written again.
    /// The path is where, inside the folder (its old name unless that was taken).
    WroteBack(PathBuf),
    /// Another program had changed the file, so it was left alone and the
    /// typed text was written beside it at this path inside the folder.
    KeptBeside(PathBuf),
    /// The folder no longer exists, or the journal names no place inside it;
    /// the text was written to this full path under the data directory.
    Elsewhere(PathBuf),
    /// The text could not be put back just now: its file could not be read
    /// or replaced, its journal is held by another program, or its folder is
    /// on a drive that is not there. Nothing is lost: the journal is kept,
    /// opening the text brings back what was typed where that can be done,
    /// and the next start tries again.
    Waiting,
}

/// A journal recovery could not finish: its text comes back when the text is
/// next opened. The handles stay open, denying writers, so that the journal
/// is as it was read.
pub(crate) struct Pending {
    pub docid: u64,
    pub generation: u32,
    pub replay: Replay,
    pub files: Vec<File>,
}

pub(crate) struct Ctx<'a> {
    pub root: &'a Path,
    pub data: &'a Path,
    pub volume: &'a Volume,
    pub history: &'a History,
    pub faults: &'a Faults,
}

pub(crate) struct Outcome {
    pub report: Vec<Recovered>,
    /// Journals kept, by the lower-case path their text's file has inside the folder.
    pub pending: HashMap<String, Pending>,
    /// Journals of this folder that could not be read at all, by the same
    /// key, with their text's id: tried again when the text is opened.
    pub unread: HashMap<String, u64>,
}

/// Opens a journal to take it up: for reading, and for removing it through
/// this handle afterwards. Writers are kept out, so a journal a running
/// instance is still writing cannot be taken; readers are not.
fn open_take(path: &Path) -> Result<File, u32> {
    OpenOptions::new().access_mode(sys::GENERIC_READ | sys::DELETE | sys::SYNCHRONIZE).share_mode(sys::SHARE_READ | sys::SHARE_DELETE).open(path).map_err(|e| sys::code(&e).max(1))
}

/// Opens a journal only to look at it, in nobody's way.
fn open_peek(path: &Path) -> Result<File, u32> {
    OpenOptions::new().read(true).share_mode(sys::SHARE_ALL).open(path).map_err(|e| sys::code(&e).max(1))
}

/// Moves a journal that will not parse into `journal\bad\`, where it is kept.
fn to_bad(dir: &Path, path: &Path, file: &File) {
    let bad = dir.join("bad");
    if fs::create_dir_all(&bad).is_err() {
        return;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = bad.join(format!("{} {name}", sys::unix_ms_now()));
    for n in 1..=9 {
        if let Some(to) = names::verbatim(&names::numbered(&base, n)) {
            if sys::rename_handle(file, &to, false, false).is_ok() {
                return;
            }
        }
    }
}

fn text_of(bytes: &[u8]) -> String {
    match format::decode(bytes) {
        Decoded::Text { text, .. } => text,
        Decoded::Other { lossy } => lossy,
    }
}

/// Looks for a file that left its name, by the index it had when last saved.
/// Only a text inside the folder whose bytes are a state of ours counts.
fn find_moved(ctx: &Ctx, replay: &Replay) -> Option<(PathBuf, fsio::Guarded)> {
    let index = replay.done.map(|(_, stamp)| stamp.index).unwrap_or(replay.open.stamp.index);
    if !ctx.volume.open_by_id || index == 0 {
        return None;
    }
    let hint = sys::open_dir(ctx.root).ok()?;
    let full = sys::final_path(&sys::open_by_id(&hint, index)?)?;
    let rel = names::check_rel(full.strip_prefix(ctx.root).ok()?, false).ok()?;
    if rel.components().any(|part| part.as_os_str().to_string_lossy().starts_with('.')) {
        return None;
    }
    let found = fsio::read_patiently(&ctx.root.join(&rel), LIMIT, &PAUSES).ok()?;
    if found.bytes == format::encode_like(&found.bytes, &replay.text, replay.format) || replay.known.contains(&hash::state(&found.bytes)) {
        Some((rel, found))
    } else {
        None
    }
}

/// Clears save temp files out of the folder a recovered text lives in: the
/// ones its journal names, and any other of the same shape. With the folder
/// lock held and no saver running, a temp file there belongs to a process
/// that is dead. They are moved to the data directory's trash, not erased.
fn trash_temps(ctx: &Ctx, replay: &Replay, path: &Path) {
    let Some(dir) = path.parent() else { return };
    let now = sys::unix_ms_now();
    let mut leftovers: Vec<PathBuf> = replay.temps.iter().filter(|temp| fsio::is_temp_name(temp)).map(|temp| dir.join(temp)).collect();
    if let Ok(entries) = fs::read_dir(dir) {
        leftovers.extend(entries.flatten().filter(|entry| fsio::is_temp_name(&entry.file_name().to_string_lossy())).map(|entry| entry.path()));
    }
    leftovers.sort();
    leftovers.dedup();
    for leftover in leftovers {
        if sys::stat(&leftover).is_ok() {
            fsio::to_trash(ctx.data, &leftover, now);
        }
    }
}

fn inside(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).map(Path::to_path_buf).unwrap_or_else(|_| path.to_path_buf())
}

/// What to do with a journal's text that has no file to go to: its folder is
/// gone for good, or the journal names no place inside the folder (a damaged
/// or forged record: a name read from a file is never followed out of the
/// writing folder). Unsaved text is written under the data directory, where
/// it can be found and where nothing of his can be in its way; text that was
/// saved is kept as a version. Returns what to tell, and whether the journal
/// must be kept.
fn aside(ctx: &Ctx, docid: u64, replay: &Replay, folder: &Path) -> (Option<Recovered>, bool) {
    let plain = format::encode(&replay.text, replay.format);
    if !replay.unsaved() {
        return (None, ctx.history.snapshot(docid, &names::ending_of(&replay.rel), &plain, Some("recovery"), sys::unix_ms_now(), false).is_err());
    }
    let dir = ctx.data.join("recovered");
    let name = replay.rel.file_name().and_then(|name| name.to_str()).filter(|name| names::check_name(name).is_ok()).unwrap_or("recovered.md");
    if fs::create_dir_all(&dir).is_err() {
        return (None, true);
    }
    match fsio::write_numbered(&dir.join(name), &plain, &Faults::default()) {
        Ok(written) => {
            let words = format::words_added(&replay.open.text, &replay.text);
            (Some(Recovered { folder: folder.to_path_buf(), rel: replay.rel.clone(), how: Recovery::Elsewhere(written), words }), false)
        }
        Err(_) => (None, true),
    }
}

/// Decides what one journal means for the folder and does it. Returns what
/// to tell the interface, whether the journal must be kept, and the path
/// inside the folder its text's file was found at.
///
/// `branch` is set for a journal that is not the newest of its text: an
/// earlier session's, left unread at the time, with a later one beside it.
/// Its text is never put over the file, which belongs to the later session;
/// what it holds that was never saved goes beside the file.
fn recover_one(ctx: &Ctx, docid: u64, replay: &Replay, journal_ms: u64, branch: bool) -> (Option<Recovered>, bool, PathBuf) {
    let Ok(mut rel) = names::check_rel(&replay.rel, false) else {
        let (told, keep) = aside(ctx, docid, replay, ctx.root);
        return (told, keep, replay.rel.clone());
    };
    let plain = format::encode(&replay.text, replay.format);
    let ending = names::ending_of(&rel);
    let now = sys::unix_ms_now();

    // Where the file is: its new name first, then the old one if a rename was under way.
    let mut read = fsio::read_patiently(&ctx.root.join(&rel), LIMIT, &PAUSES);
    if matches!(read, Err(Fail::Missing | Fail::FolderGone)) {
        if let Some(old) = replay.old_rel.as_ref().and_then(|old| names::check_rel(old, false).ok()) {
            let at_old = fsio::read_patiently(&ctx.root.join(&old), LIMIT, &PAUSES);
            if !matches!(at_old, Err(Fail::Missing | Fail::FolderGone)) {
                rel = old;
                read = at_old;
            }
        }
    }
    if matches!(read, Err(Fail::Missing | Fail::FolderGone)) {
        if let Some((moved, found)) = find_moved(ctx, replay) {
            rel = moved;
            read = Ok(found);
        }
    }
    let path = ctx.root.join(&rel);
    let since_save = format::words_added(&replay.open.text, &replay.text);
    let report = |how: Recovery, words: usize| Some(Recovered { folder: ctx.root.to_path_buf(), rel: rel.clone(), how, words });
    if !branch {
        ctx.history.set_origin(docid, ctx.root, &rel);
        if let Some(caret) = replay.caret {
            ctx.history.set_caret(docid, caret);
        }
    }

    let (told, keep) = match read {
        Ok(found) => {
            // The journal's text as this file would hold it: line breaks
            // nobody touched are the file's own, whatever kinds they are.
            let mine = format::encode_like(&found.bytes, &replay.text, replay.format);
            let info = found.info;
            let words = format::words_added(&text_of(&found.bytes), &replay.text);
            let ours = replay.known.contains(&hash::state(&found.bytes));
            let replaceable = info.attrs & sys::ATTR_READONLY == 0 && info.links <= 1;
            if found.bytes == mine {
                // Nothing was lost. But unless a finished save vouches for
                // this text, the file may have been renamed into place and
                // never flushed: make it durable before the journal is let go.
                drop(found);
                if replay.unsaved() && !fsio::flush_in_place(&path) {
                    (None, true)
                } else {
                    trash_temps(ctx, replay, &path);
                    (None, false)
                }
            } else if ours && replaceable && !branch {
                // The file is merely behind: one of our own earlier saves.
                let _ = ctx.history.snapshot(docid, &ending, &found.bytes, Some("recovery"), now, false);
                let posix = ctx.volume.atomic_replace && ctx.faults.no_one_step.load(Ordering::Relaxed) == 0;
                // Written where the file really is, under the name it has on
                // the disk: the journal's spelling of it may be an older one.
                let target = sys::final_path(&found.file).unwrap_or_else(|| path.clone());
                let mut guard = Some(found.file);
                let write = fsio::Write { target: &target, first_temp: None, bytes: &mine, created: Some(info.created), replace: true, posix, faults: ctx.faults };
                let result = fsio::write_file(&write, &mut |look| fsio::verify_unchanged(&target, &info, posix, &mut guard, look));
                drop(guard);
                match result {
                    Ok(_) => {
                        trash_temps(ctx, replay, &path);
                        (report(Recovery::BroughtBack, words), false)
                    }
                    Err(_) => (report(Recovery::Waiting, words), true),
                }
            } else if replay.unsaved() || ours {
                // Something else is there now. It stays; what was typed goes beside it.
                drop(found);
                let beside = names::sibling(&path, "recovered", journal_ms, 1);
                match fsio::write_numbered(&beside, &plain, ctx.faults) {
                    Ok(written) => {
                        trash_temps(ctx, replay, &path);
                        (report(Recovery::KeptBeside(inside(ctx.root, &written)), words), false)
                    }
                    Err(_) => (report(Recovery::Waiting, words), true),
                }
            } else {
                // Nothing of ours was unsaved: the journal's text is an older version now.
                drop(found);
                match ctx.history.snapshot(docid, &ending, &plain, Some("recovery"), now, false) {
                    Ok(_) => {
                        trash_temps(ctx, replay, &path);
                        (None, false)
                    }
                    Err(_) => (None, true),
                }
            }
        }
        Err(Fail::Missing | Fail::FolderGone) => {
            let never_saved = replay.done.is_none() && !replay.open.on_disk;
            if replay.unsaved() || never_saved {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                match fsio::write_numbered(&path, &plain, ctx.faults) {
                    Ok(written) => {
                        trash_temps(ctx, replay, &path);
                        (report(Recovery::WroteBack(inside(ctx.root, &written)), since_save), false)
                    }
                    Err(_) => (report(Recovery::Waiting, since_save), true),
                }
            } else {
                // It was saved and then removed by someone: that was their choice. Keep a copy only.
                match ctx.history.snapshot(docid, &ending, &plain, Some("recovery"), now, false) {
                    Ok(_) => (None, false),
                    Err(_) => (None, true),
                }
            }
        }
        Err(_) => (report(Recovery::Waiting, since_save), true),
    };
    (told, keep, rel)
}

/// A journal of one text that is not the newest and that the newest does
/// not carry on from: an earlier session's, never read at the time.
pub(crate) struct Orphan {
    pub replay: Replay,
    pub written_ms: u64,
    pub file: File,
}

/// One text's journal, taken up: its generations opened, and the highest
/// whole one replayed.
pub(crate) struct Taken {
    pub generation: u32,
    pub replay: Replay,
    /// When the journal file was last written, in milliseconds since 1970.
    pub written_ms: u64,
    /// The winning generation and every lower one it carries on from.
    pub files: Vec<File>,
    /// Lower generations holding unsaved text the winner does not have.
    pub orphans: Vec<Orphan>,
}

/// What became of an attempt to take one text's journal up.
pub(crate) enum Take {
    Taken(Box<Taken>),
    /// No generation of it starts with a whole `Open` record (those there
    /// were are now in `bad\`), or none is there any more.
    Empty,
    /// Another program has it open in a way that keeps us out.
    Unreadable,
}

/// The journal generations of one text that lie in `dir`.
pub(crate) fn generations(dir: &Path, docid: u64) -> Vec<(u32, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| match wal::parse_name(&entry.file_name().to_string_lossy()) {
            Some((id, generation, false)) if id == docid => Some((generation, entry.path())),
            _ => None,
        })
        .collect()
}

/// Reads a journal file through an open handle and replays it. A file whose
/// header disagrees with its name is not the journal it claims to be.
fn read_journal(file: &mut File, docid: u64, generation: u32) -> Option<wal::Parsed> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    wal::parse(&bytes).filter(|parsed| parsed.docid == docid && parsed.generation == generation)
}

/// Takes up one text's journal, waiting out for `pauses` (milliseconds)
/// whatever keeps a file of it from being opened.
///
/// The highest generation that starts with a whole `Open` record speaks for
/// the text. A lower one is spent, and goes with it, when the winner carries
/// on from it: it is of the same session, or the winner's text passed
/// through the lower one's last text, or the lower one holds nothing
/// unsaved. A lower one of which none of that is true is an earlier
/// session's that was never read, and is handed back apart, so that being
/// outnumbered never costs it its text.
pub(crate) fn take(dir: &Path, docid: u64, mut generations: Vec<(u32, PathBuf)>, pauses: &[u64]) -> Take {
    generations.sort_by(|a, b| b.0.cmp(&a.0));
    let mut opened = Vec::new();
    for (generation, path) in generations {
        let mut waits = pauses.iter();
        let file = loop {
            match open_take(&path) {
                Ok(file) => break Some(file),
                // Gone since it was listed: someone else has dealt with it.
                Err(2 | 3) => break None,
                Err(_) => match waits.next() {
                    Some(&ms) => std::thread::sleep(Duration::from_millis(ms)),
                    None => {
                        if opened.is_empty() {
                            // The highest cannot be read, so nothing below it can speak.
                            return Take::Unreadable;
                        }
                        // A lower one that cannot be read stays where it is, for a later start.
                        break None;
                    }
                },
            }
        };
        if let Some(file) = file {
            opened.push((generation, path, file));
        }
    }
    let mut winner: Option<(u32, wal::Parsed, Replay, u64)> = None;
    let mut files = Vec::new();
    let mut orphans = Vec::new();
    for (generation, path, mut file) in opened {
        let parsed = read_journal(&mut file, docid, generation);
        let Some((parsed, replay)) = parsed.and_then(|parsed| wal::replay(&parsed).map(|replay| (parsed, replay))) else {
            to_bad(dir, &path, &file);
            continue;
        };
        let written_ms = sys::info(&file).map(|info| sys::filetime_to_unix_ms(info.modified)).unwrap_or_else(|_| sys::unix_ms_now());
        match &winner {
            None => {
                winner = Some((generation, parsed, replay, written_ms));
                files.push(file);
            }
            Some((_, top, top_replay, _)) => {
                let same_session = replay.lineage != 0 && replay.lineage == top_replay.lineage;
                let spent = !replay.unsaved() || same_session || wal::replay_seeking(top, Some(&replay.text)).is_some_and(|(_, passed)| passed);
                if spent {
                    files.push(file);
                } else {
                    orphans.push(Orphan { replay, written_ms, file });
                }
            }
        }
    }
    match winner {
        Some((generation, _, replay, written_ms)) => Take::Taken(Box::new(Taken { generation, replay, written_ms, files, orphans })),
        None => Take::Empty,
    }
}

/// What a look at a text's journal files, taken without getting in anyone's
/// way, says: where the text lives and what it holds unsaved.
struct Glance {
    folder: PathBuf,
    rel: PathBuf,
    unsaved: bool,
    words: usize,
}

fn glance(docid: u64, generations: &[(u32, PathBuf)]) -> Option<Glance> {
    let mut sorted: Vec<&(u32, PathBuf)> = generations.iter().collect();
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    for (generation, path) in sorted {
        let mut file = match open_peek(path) {
            Ok(file) => file,
            Err(2 | 3) => continue,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(5));
                match open_peek(path) {
                    Ok(file) => file,
                    // The highest cannot be looked at, so nothing below it can speak.
                    Err(_) => return None,
                }
            }
        };
        if let Some(replay) = read_journal(&mut file, docid, *generation).and_then(|parsed| wal::replay(&parsed)) {
            let words = format::words_added(&replay.open.text, &replay.text);
            return Some(Glance { unsaved: replay.unsaved(), words, folder: replay.folder, rel: replay.rel });
        }
    }
    None
}

/// Whose a journal is, by the writing folder it names.
enum Whose {
    /// The folder being opened.
    Here,
    /// Another folder that exists and that no instance has open: it is
    /// recovered there, under its lock, which is held for that long.
    There(PathBuf, state::Lock),
    /// Another folder that a running instance has open: theirs to look after.
    Theirs,
    /// A folder on a drive that is not there.
    Away,
    /// A folder that is gone for good, or no folder at all.
    Gone,
}

fn whose(ctx: &Ctx, folder: &Path, origin: Option<&(PathBuf, PathBuf)>) -> Whose {
    if names::same_folder(folder, ctx.root) {
        return Whose::Here;
    }
    if !folder.is_absolute() {
        return Whose::Gone;
    }
    match fs::canonicalize(folder) {
        Ok(root) if names::same_folder(&root, ctx.root) => Whose::Here,
        Ok(root) if sys::stat(&root).is_ok_and(|info| info.attrs & sys::ATTR_DIRECTORY != 0) => match state::take_lock(ctx.data, &root, "recovery", true) {
            Ok(lock) => Whose::There(root, lock),
            Err(_) => Whose::Theirs,
        },
        Ok(_) => Whose::Gone,
        // The folder was renamed while a store had it open, and followed:
        // the note kept with the text's history says where it is now, even
        // if the journal never got to hear of it.
        Err(_) if origin.is_some_and(|(noted, _)| names::same_folder(noted, ctx.root)) => Whose::Here,
        Err(_) if sys::stat(&sys::volume_root(folder)).is_err() => Whose::Away,
        Err(_) => Whose::Gone,
    }
}

/// Goes through every journal under the data directory. A journal for a
/// folder a running instance has open is left to that instance.
pub(crate) fn run(ctx: &Ctx) -> Outcome {
    let dir = ctx.data.join("journal");
    let mut out = Outcome { report: Vec::new(), pending: HashMap::new(), unread: HashMap::new() };
    let Ok(entries) = fs::read_dir(&dir) else { return out };
    let mut by_doc: BTreeMap<u64, Vec<(u32, PathBuf)>> = BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((docid, generation, part)) = wal::parse_name(&name) else { continue };
        if part {
            // A generation that was still being written when the program
            // stopped: the one before it is whole. One a running instance is
            // writing this moment cannot be opened this way and is left.
            if let Ok(file) = open_take(&entry.path()) {
                sys::delete_on_close(&file);
            }
            continue;
        }
        by_doc.entry(docid).or_default().push((generation, entry.path()));
    }
    for (docid, generations) in by_doc {
        let origin = ctx.history.origin(docid);
        // A first try without waiting for anyone. It usually succeeds, and
        // then what was read says whose the journal is. Where it does not (a
        // running instance is writing the journal, or some other program
        // holds it), a look that is in nobody's way says whose it is, or
        // failing that the note kept with the text's history does.
        let first = take(&dir, docid, generations.clone(), &[]);
        let seen = match &first {
            Take::Taken(taken) => {
                let replay = &taken.replay;
                Some(Glance { folder: replay.folder.clone(), rel: replay.rel.clone(), unsaved: replay.unsaved(), words: format::words_added(&replay.open.text, &replay.text) })
            }
            // What would not parse is in bad\ now; there is nothing else.
            Take::Empty => continue,
            Take::Unreadable => glance(docid, &generations),
        };
        let (folder, rel) = match (&seen, &origin) {
            (Some(seen), _) => (seen.folder.clone(), seen.rel.clone()),
            (None, Some((folder, rel))) => (folder.clone(), rel.clone()),
            // Nothing says whose it is: left for a later start.
            (None, None) => continue,
        };
        // What a first try took stays taken while the folder's lock is asked
        // for: the handles keep writers out, so what was read through them
        // is still what the files hold when the lock is ours.
        let owner = whose(ctx, &folder, origin.as_ref());
        let here = matches!(owner, Whose::Here);
        let there_volume: Volume;
        // Where the text is put back: this folder, another one that is free
        // for the moment, or no folder at all.
        let place: Option<Ctx> = match &owner {
            Whose::Here => Some(Ctx { root: ctx.root, data: ctx.data, volume: ctx.volume, history: ctx.history, faults: ctx.faults }),
            Whose::There(root, _lock) => {
                there_volume = sys::volume(root);
                Some(Ctx { root, data: ctx.data, volume: &there_volume, history: ctx.history, faults: ctx.faults })
            }
            Whose::Theirs => continue,
            Whose::Away => {
                // Kept for the day the drive is back, and said so: words
                // waiting in a file only this program reads must not wait unseen.
                if let Some(seen) = seen.filter(|seen| seen.unsaved) {
                    out.report.push(Recovered { folder, rel: seen.rel, how: Recovery::Waiting, words: seen.words });
                }
                continue;
            }
            Whose::Gone => None,
        };
        let second = match first {
            Take::Taken(taken) => Take::Taken(taken),
            _ => take(&dir, docid, generations, &TAKE_PAUSES),
        };
        let taken = match second {
            Take::Taken(taken) => *taken,
            Take::Empty => continue,
            Take::Unreadable => {
                // Another program has the journal file. It is this folder's
                // text all the same, and he is told it is waiting.
                if here {
                    out.report.push(Recovered { folder: ctx.root.to_path_buf(), rel: rel.clone(), how: Recovery::Waiting, words: seen.map(|seen| seen.words).unwrap_or(0) });
                    out.unread.insert(names::key(&rel), docid);
                }
                continue;
            }
        };
        let Taken { generation, replay, written_ms, files, orphans } = taken;
        let (keep, rel) = match &place {
            Some(place) => {
                let (told, keep, rel) = recover_one(place, docid, &replay, written_ms, false);
                out.report.extend(told);
                (keep, rel)
            }
            None => {
                let (told, keep) = aside(ctx, docid, &replay, &folder);
                out.report.extend(told);
                (keep, replay.rel.clone())
            }
        };
        // Earlier sessions' journals of the same text, never read until now.
        for orphan in orphans {
            let (told, keep) = match &place {
                Some(place) => {
                    let (told, keep, _) = recover_one(place, docid, &orphan.replay, orphan.written_ms, true);
                    (told, keep)
                }
                None => aside(ctx, docid, &orphan.replay, &folder),
            };
            out.report.extend(told);
            if !keep {
                sys::delete_on_close(&orphan.file);
            }
        }
        if !keep {
            for file in &files {
                sys::delete_on_close(file);
            }
        } else if here {
            out.pending.insert(names::key(&rel), Pending { docid, generation, replay, files });
        }
    }
    out
}
