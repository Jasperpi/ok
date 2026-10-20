//! The journal's bytes: the file header, the record frame, the payload of
//! each kind of record, the reader that stops at the first thing that is not
//! a whole good record, and the replay that turns a journal back into a text.
//!
//! A journal file is `OKJ1\r\n\x1a\n`, the document id, the generation and a
//! format version, then records:
//!
//! ```text
//! u32 len | u32 crc32 | u8 kind | u64 seq | payload
//! ```
//!
//! `len` counts kind, seq and payload, so the smallest record (an empty
//! payload) has len 9, and the checksum covers exactly those `len` bytes.
//! `seq` starts at 0 in each file and has no gaps. All integers are little
//! endian.

use crate::sys::{wide_path, wide_units};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ok_text::{Change, Doc, Pos, Span};

use crate::format::{Eol, Format};
use crate::hash::{self, State};

pub(crate) const MAGIC: [u8; 8] = *b"OKJ1\r\n\x1a\n";
pub(crate) const HEADER_LEN: usize = 24;
const VERSION: u32 = 1;
/// The frame before the checksummed part: len and crc.
const FRAME: usize = 8;
/// Kind and seq: the least a record's `len` can be.
const MIN_LEN: usize = 9;
/// Nothing sane is larger; a bigger `len` is a damaged tail.
pub(crate) const MAX_LEN: usize = 64 << 20;
/// How many of its own earlier saves a journal remembers across rotations.
pub(crate) const KNOWN_STATES: usize = 16;

const KIND_OPEN: u8 = 1;
const KIND_EDIT: u8 = 2;
const KIND_BEGIN: u8 = 3;
const KIND_DONE: u8 = 4;
const KIND_PATH: u8 = 5;
const KIND_CLOSE: u8 = 6;
const KIND_CARET: u8 = 7;
const KIND_FOLDER: u8 = 8;
const KIND_LINEAGE: u8 = 9;
/// Bytes of an edit's payload before the inserted text.
const EDIT_HEAD: usize = 36;

/// Size, last-write time and file index of a file just after we wrote it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Stamp {
    pub size: u64,
    pub modified: u64,
    pub index: u64,
}

/// The first record of every journal file: enough to rebuild the text with
/// nothing else, and to recognise the file in the folder as ours.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Open {
    /// The edit number `text` corresponds to. Edits in this file follow it.
    pub edit: u64,
    pub wall_ms: u64,
    /// `text` was on disk (flushed after its rename, or just read) when this
    /// record was written.
    pub on_disk: bool,
    pub format: Format,
    /// Fingerprint of the file's bytes when `on_disk`.
    pub disk: State,
    pub stamp: Stamp,
    /// Fingerprints of earlier saves, newest last.
    pub known: Vec<State>,
    /// The writing folder, absolute.
    pub folder: PathBuf,
    /// The text's path inside it.
    pub rel: PathBuf,
    pub caret: Option<Pos>,
    /// The whole text, `\n` line breaks.
    pub text: Arc<String>,
}

/// One edit as journaled: the span [`Doc::replace`] was given, what went in,
/// and two checks for replay.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Edit {
    pub edit: u64,
    pub start: Pos,
    pub old_end: Pos,
    pub removed_len: u32,
    pub len_after: u64,
    pub inserted: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Record {
    Open(Open),
    Edit(Edit),
    /// A save of the text at `edit` is about to touch the folder. `format`
    /// is the line-ending style and byte-order mark its bytes are written in.
    Begin { edit: u64, state: State, format: Option<Format>, temp: String },
    /// That save is on disk, flushed after its rename.
    Done { edit: u64, stamp: Stamp, format: Option<Format> },
    /// The text is about to be renamed inside the folder.
    Path { old: PathBuf, new: PathBuf },
    /// Closed with nothing unsaved.
    Close,
    Caret(Pos),
    /// The writing folder itself was found at a new place.
    Folder(PathBuf),
    /// The session this file belongs to. Every file made by starting one
    /// journal afresh carries the same number, so a later file is known to
    /// hold all that an earlier one of the same number did.
    Lineage(u64),
}

/// A format as one byte of a record: bit 0 for CRLF line breaks, bit 1 for
/// the byte-order mark, and the top bit to say the byte is there at all.
fn format_byte(format: Format) -> u8 {
    0x80 | matches!(format.eol, Eol::CrLf) as u8 | (format.bom as u8) << 1
}

fn byte_format(byte: u8) -> Option<Format> {
    (byte & 0x80 != 0).then_some(Format { eol: if byte & 1 != 0 { Eol::CrLf } else { Eol::Lf }, bom: byte & 2 != 0 })
}

pub(crate) fn header(docid: u64, generation: u32) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..8].copy_from_slice(&MAGIC);
    h[8..16].copy_from_slice(&docid.to_le_bytes());
    h[16..20].copy_from_slice(&generation.to_le_bytes());
    h[20..24].copy_from_slice(&VERSION.to_le_bytes());
    h
}

/// Appends one framed record to `out`.
pub(crate) fn push(out: &mut Vec<u8>, kind: u8, seq: u64, payload: &[u8]) {
    let seq_bytes = seq.to_le_bytes();
    let len = (MIN_LEN + payload.len()) as u32;
    let crc = hash::crc32(&[&[kind], &seq_bytes, payload]);
    out.reserve(FRAME + MIN_LEN + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&seq_bytes);
    out.extend_from_slice(payload);
}

fn put_path(out: &mut Vec<u8>, path: &Path) {
    let wide = wide_units(path);
    out.extend_from_slice(&(wide.len() as u32).to_le_bytes());
    for unit in wide {
        out.extend_from_slice(&unit.to_le_bytes());
    }
}

fn pos32(p: Pos) -> (u32, u32) {
    (u32::try_from(p.line).unwrap_or(u32::MAX), u32::try_from(p.col).unwrap_or(u32::MAX))
}

fn put_pos(out: &mut Vec<u8>, p: Pos) {
    let (line, col) = pos32(p);
    out.extend_from_slice(&line.to_le_bytes());
    out.extend_from_slice(&col.to_le_bytes());
}

pub(crate) fn push_open(out: &mut Vec<u8>, seq: u64, open: &Open) {
    let mut p = Vec::with_capacity(160 + open.text.len());
    p.extend_from_slice(&open.edit.to_le_bytes());
    p.extend_from_slice(&open.wall_ms.to_le_bytes());
    p.push(open.on_disk as u8);
    p.push(matches!(open.format.eol, Eol::CrLf) as u8);
    p.push(open.format.bom as u8);
    p.extend_from_slice(&open.disk.0.to_le_bytes());
    p.extend_from_slice(&open.disk.1.to_le_bytes());
    p.extend_from_slice(&open.stamp.size.to_le_bytes());
    p.extend_from_slice(&open.stamp.modified.to_le_bytes());
    p.extend_from_slice(&open.stamp.index.to_le_bytes());
    let known = &open.known[open.known.len().saturating_sub(KNOWN_STATES)..];
    p.extend_from_slice(&(known.len() as u16).to_le_bytes());
    for (len, fnv) in known {
        p.extend_from_slice(&len.to_le_bytes());
        p.extend_from_slice(&fnv.to_le_bytes());
    }
    put_path(&mut p, &open.folder);
    put_path(&mut p, &open.rel);
    match open.caret {
        Some(c) => {
            p.push(1);
            put_pos(&mut p, c);
        }
        None => p.push(0),
    }
    p.extend_from_slice(open.text.as_bytes());
    push(out, KIND_OPEN, seq, &p);
}

/// The payload of an edit record. This runs on the keystroke path: one small
/// allocation and a few copies.
pub(crate) fn edit_payload(edit: u64, change: &Change, len_after: usize) -> Vec<u8> {
    let mut p = Vec::with_capacity(36 + change.inserted.len());
    p.extend_from_slice(&edit.to_le_bytes());
    put_pos(&mut p, change.start);
    put_pos(&mut p, change.old_end);
    p.extend_from_slice(&u32::try_from(change.removed.len()).unwrap_or(u32::MAX).to_le_bytes());
    p.extend_from_slice(&(len_after as u64).to_le_bytes());
    p.extend_from_slice(change.inserted.as_bytes());
    p
}

pub(crate) fn push_edit(out: &mut Vec<u8>, seq: u64, payload: &[u8]) {
    push(out, KIND_EDIT, seq, payload);
}

/// What an edit's payload says about lengths, read without decoding its
/// text: the edit's number, how many bytes it took out, how many it put in,
/// and the length the document was said to have afterwards.
pub(crate) fn edit_lengths(payload: &[u8]) -> Option<(u64, u64, u64, u64)> {
    let mut c = Cur { bytes: payload };
    let edit = c.u64()?;
    c.take(16)?;
    let removed = c.u32()? as u64;
    let len_after = c.u64()?;
    Some((edit, removed, c.bytes.len() as u64, len_after))
}

/// Writes the length afterwards into an edit's payload.
pub(crate) fn set_len_after(payload: &mut [u8], len: u64) {
    if let Some(field) = payload.get_mut(EDIT_HEAD - 8..EDIT_HEAD) {
        field.copy_from_slice(&len.to_le_bytes());
    }
}

pub(crate) fn push_begin(out: &mut Vec<u8>, seq: u64, edit: u64, state: State, format: Format, temp: &str) {
    let mut p = Vec::with_capacity(25 + temp.len());
    p.extend_from_slice(&edit.to_le_bytes());
    p.extend_from_slice(&state.0.to_le_bytes());
    p.extend_from_slice(&state.1.to_le_bytes());
    p.push(format_byte(format));
    p.extend_from_slice(temp.as_bytes());
    push(out, KIND_BEGIN, seq, &p);
}

pub(crate) fn push_done(out: &mut Vec<u8>, seq: u64, edit: u64, stamp: Stamp, format: Format) {
    let mut p = Vec::with_capacity(33);
    p.extend_from_slice(&edit.to_le_bytes());
    p.extend_from_slice(&stamp.size.to_le_bytes());
    p.extend_from_slice(&stamp.modified.to_le_bytes());
    p.extend_from_slice(&stamp.index.to_le_bytes());
    p.push(format_byte(format));
    push(out, KIND_DONE, seq, &p);
}

pub(crate) fn push_folder(out: &mut Vec<u8>, seq: u64, folder: &Path) {
    let mut p = Vec::new();
    put_path(&mut p, folder);
    push(out, KIND_FOLDER, seq, &p);
}

pub(crate) fn push_lineage(out: &mut Vec<u8>, seq: u64, lineage: u64) {
    push(out, KIND_LINEAGE, seq, &lineage.to_le_bytes());
}

pub(crate) fn push_path(out: &mut Vec<u8>, seq: u64, old: &Path, new: &Path) {
    let mut p = Vec::new();
    put_path(&mut p, old);
    put_path(&mut p, new);
    push(out, KIND_PATH, seq, &p);
}

pub(crate) fn push_close(out: &mut Vec<u8>, seq: u64) {
    push(out, KIND_CLOSE, seq, &[]);
}

pub(crate) fn push_caret(out: &mut Vec<u8>, seq: u64, caret: Pos) {
    let mut p = Vec::with_capacity(8);
    put_pos(&mut p, caret);
    push(out, KIND_CARET, seq, &p);
}

/// A cursor over a payload that answers `None` instead of running off the end.
struct Cur<'a> {
    bytes: &'a [u8],
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.bytes.len() < n {
            return None;
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes)
    }

    fn pos(&mut self) -> Option<Pos> {
        Some(Pos { line: self.u32()? as usize, col: self.u32()? as usize })
    }

    fn path(&mut self) -> Option<PathBuf> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(2)?)?;
        let wide: Vec<u16> = raw.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        Some(wide_path(&wide))
    }

    fn rest_str(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.bytes);
        std::str::from_utf8(rest).ok().map(str::to_string)
    }
}

fn decode(kind: u8, payload: &[u8]) -> Option<Record> {
    let mut c = Cur { bytes: payload };
    match kind {
        KIND_OPEN => {
            let edit = c.u64()?;
            let wall_ms = c.u64()?;
            let on_disk = c.u8()? != 0;
            let eol = if c.u8()? != 0 { Eol::CrLf } else { Eol::Lf };
            let bom = c.u8()? != 0;
            let disk = (c.u64()?, c.u64()?);
            let stamp = Stamp { size: c.u64()?, modified: c.u64()?, index: c.u64()? };
            let count = c.u16()? as usize;
            let mut known = Vec::with_capacity(count.min(KNOWN_STATES));
            for _ in 0..count {
                known.push((c.u64()?, c.u64()?));
            }
            let folder = c.path()?;
            let rel = c.path()?;
            let caret = match c.u8()? {
                0 => None,
                _ => Some(c.pos()?),
            };
            let text = Arc::new(c.rest_str()?);
            Some(Record::Open(Open { edit, wall_ms, on_disk, format: Format { eol, bom }, disk, stamp, known, folder, rel, caret, text }))
        }
        KIND_EDIT => {
            let edit = c.u64()?;
            let start = c.pos()?;
            let old_end = c.pos()?;
            let removed_len = c.u32()?;
            let len_after = c.u64()?;
            let inserted = c.rest_str()?;
            Some(Record::Edit(Edit { edit, start, old_end, removed_len, len_after, inserted }))
        }
        KIND_BEGIN => {
            let edit = c.u64()?;
            let state = (c.u64()?, c.u64()?);
            let format = byte_format(c.u8()?);
            let temp = c.rest_str()?;
            Some(Record::Begin { edit, state, format, temp })
        }
        KIND_DONE => {
            let edit = c.u64()?;
            let stamp = Stamp { size: c.u64()?, modified: c.u64()?, index: c.u64()? };
            let format = c.u8().and_then(byte_format);
            Some(Record::Done { edit, stamp, format })
        }
        KIND_PATH => {
            let old = c.path()?;
            let new = c.path()?;
            Some(Record::Path { old, new })
        }
        KIND_CLOSE => Some(Record::Close),
        KIND_CARET => Some(Record::Caret(c.pos()?)),
        KIND_FOLDER => Some(Record::Folder(c.path()?)),
        KIND_LINEAGE => Some(Record::Lineage(c.u64()?)),
        _ => None,
    }
}

/// A journal file as far as it could be read.
#[derive(Debug)]
pub(crate) struct Parsed {
    pub docid: u64,
    pub generation: u32,
    pub records: Vec<Record>,
    /// Bytes of the file that were whole, good records (header included).
    #[cfg(test)]
    pub good_len: usize,
}

/// Reads a journal file. `None` means the header itself is missing or wrong.
/// Reading stops, without complaint, at the first record that is short, has
/// an impossible length, fails its checksum, is out of sequence, or is of a
/// kind this version does not know; everything after it is ignored.
pub(crate) fn parse(bytes: &[u8]) -> Option<Parsed> {
    if bytes.len() < HEADER_LEN || bytes[..8] != MAGIC {
        return None;
    }
    let docid = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let generation = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    let mut records = Vec::new();
    let mut at = HEADER_LEN;
    let mut seq = 0u64;
    while let Some(frame) = bytes.get(at..at + FRAME) {
        let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        let crc = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        if !(MIN_LEN..=MAX_LEN).contains(&len) {
            break;
        }
        let Some(body) = bytes.get(at + FRAME..at + FRAME + len) else { break };
        if hash::crc32(&[body]) != crc {
            break;
        }
        let record_seq = u64::from_le_bytes(body[1..9].try_into().ok()?);
        if record_seq != seq {
            break;
        }
        let Some(record) = decode(body[0], &body[9..]) else { break };
        records.push(record);
        at += FRAME + len;
        seq += 1;
    }
    Some(Parsed {
        docid,
        generation,
        records,
        #[cfg(test)]
        good_len: at,
    })
}

/// `<docid as 16 hex>.<generation>.wal`.
pub(crate) fn file_name(docid: u64, generation: u32) -> String {
    format!("{docid:016x}.{generation}.wal")
}

/// Reads a journal file name back: document id, generation, and whether it is
/// a `.part` (a generation that was still being written).
pub(crate) fn parse_name(name: &str) -> Option<(u64, u32, bool)> {
    let (name, part) = match name.strip_suffix(".part") {
        Some(rest) => (rest, true),
        None => (name, false),
    };
    let rest = name.strip_suffix(".wal")?;
    let (id, generation) = rest.split_once('.')?;
    if id.len() != 16 {
        return None;
    }
    Some((u64::from_str_radix(id, 16).ok()?, generation.parse().ok()?, part))
}

/// What a journal says once its edits have been applied in order.
#[derive(Debug)]
pub(crate) struct Replay {
    pub open: Open,
    /// The text as last journaled, `\n` line breaks.
    pub text: String,
    /// The highest edit number that was applied.
    pub last_edit: u64,
    /// The last save known to be on disk: its edit number and stamp.
    pub done: Option<(u64, Stamp)>,
    /// Every fingerprint this journal knows the file in the folder may
    /// rightly have: the opening state if it was on disk, the remembered
    /// earlier saves, and every save that was begun.
    pub known: Vec<State>,
    /// Names of the temp files its saves used.
    pub temps: Vec<String>,
    /// Where the text is now, and where it was before an in-app rename.
    pub rel: PathBuf,
    pub old_rel: Option<PathBuf>,
    /// The writing folder, where it was last known to be.
    pub folder: PathBuf,
    /// The line endings and byte-order mark the file was last written in.
    pub format: Format,
    /// The session the file belongs to; 0 if it does not say.
    pub lineage: u64,
    pub closed: bool,
    pub caret: Option<Pos>,
}

impl Replay {
    /// True when the journal holds text that no finished save covers.
    pub(crate) fn unsaved(&self) -> bool {
        match self.done {
            Some((edit, _)) => self.last_edit > edit,
            None => self.last_edit > self.open.edit || !self.open.on_disk,
        }
    }
}

/// A document holding exactly `text`. `Doc::from_text` drops a leading
/// U+FEFF; the buffer in the interface keeps one that was typed or pasted
/// there, and the journal has to rebuild that buffer to the byte.
pub(crate) fn doc_of(text: &str) -> Doc {
    let mut doc = Doc::new();
    doc.replace(Span::at(Pos::default()), text);
    doc
}

/// Applies one journaled edit. False, with the document as it was, when the
/// edit does not land the way it was recorded.
fn apply(doc: &mut Doc, len: &mut u64, e: &Edit) -> bool {
    if doc.clamp(e.start) != e.start || doc.clamp(e.old_end) != e.old_end || e.start > e.old_end {
        return false;
    }
    let change = doc.replace(Span { start: e.start, end: e.old_end }, &e.inserted);
    let len_now = (*len + change.inserted.len() as u64).saturating_sub(change.removed.len() as u64);
    if change.removed.len() as u64 != e.removed_len as u64 || len_now != e.len_after {
        // Take the half-fitting edit back out: the text before it is the last good one.
        doc.revert(&change);
        return false;
    }
    *len = len_now;
    true
}

/// The text that `base` becomes when the edits in `payloads` are applied to
/// it in order, stopping at the first that does not fit. This is how the
/// journal thread hands back a text it still holds in memory.
pub(crate) fn rebuild<'a>(base: &str, payloads: impl Iterator<Item = &'a [u8]>) -> String {
    let mut doc = doc_of(base);
    let mut len = doc.len_bytes() as u64;
    for payload in payloads {
        let Some(Record::Edit(e)) = decode(KIND_EDIT, payload) else { break };
        if !apply(&mut doc, &mut len, &e) {
            break;
        }
    }
    doc.to_text("\n")
}

/// Rebuilds the text from a parsed journal by replaying its edits through
/// [`Doc::replace`]. `None` when the file does not start with an `Open`.
/// An edit that is out of order, or does not land the way it was recorded,
/// ends the replay there: what came before it is still good.
pub(crate) fn replay(parsed: &Parsed) -> Option<Replay> {
    replay_seeking(parsed, None).map(|(replay, _)| replay)
}

/// [`replay`], also saying whether the text read exactly `probe` at any
/// point on the way: at its opening or after any edit. That is how a journal
/// is known to carry on from another one of the same text.
pub(crate) fn replay_seeking(parsed: &Parsed, probe: Option<&str>) -> Option<(Replay, bool)> {
    let mut records = parsed.records.iter();
    let Some(Record::Open(open)) = records.next() else { return None };
    let mut doc = doc_of(&open.text);
    let mut out = Replay {
        open: open.clone(),
        text: String::new(),
        last_edit: open.edit,
        done: None,
        known: open.known.clone(),
        temps: Vec::new(),
        rel: open.rel.clone(),
        old_rel: None,
        folder: open.folder.clone(),
        format: open.format,
        lineage: 0,
        closed: false,
        caret: open.caret,
    };
    if open.on_disk {
        out.known.push(open.disk);
    }
    // The text's length is kept in step by arithmetic: measuring the whole
    // text after every edit would make a long journal slow to replay.
    let mut len = doc.len_bytes() as u64;
    // Whole texts are compared only where the length already fits, and not
    // without end: a journal of edits that all keep the length would
    // otherwise cost a comparison apiece.
    let (mut passed, mut looks) = (false, 0);
    let mut look = |doc: &Doc, len: u64| {
        if let Some(probe) = probe {
            if !passed && len == probe.len() as u64 && looks < 64 {
                looks += 1;
                passed = doc.to_text("\n") == probe;
            }
        }
    };
    look(&doc, len);
    for record in records {
        out.closed = false;
        match record {
            Record::Open(_) => break,
            Record::Edit(e) => {
                if e.edit != out.last_edit + 1 || !apply(&mut doc, &mut len, e) {
                    break;
                }
                out.last_edit = e.edit;
                look(&doc, len);
            }
            Record::Begin { state, format, temp, .. } => {
                out.known.push(*state);
                if let Some(format) = format {
                    out.format = *format;
                }
                if !temp.is_empty() {
                    out.temps.push(temp.clone());
                }
            }
            Record::Done { edit, stamp, format } => {
                out.done = Some((*edit, *stamp));
                if let Some(format) = format {
                    out.format = *format;
                }
            }
            Record::Path { old, new } => {
                out.old_rel = Some(old.clone());
                out.rel = new.clone();
            }
            Record::Close => out.closed = true,
            Record::Caret(c) => out.caret = Some(*c),
            Record::Folder(folder) => out.folder = folder.clone(),
            Record::Lineage(lineage) => out.lineage = *lineage,
        }
    }
    out.text = doc.to_text("\n");
    Some((out, passed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_open(text: &str) -> Open {
        Open {
            edit: 7,
            wall_ms: 1_791_729_125_000,
            on_disk: true,
            format: Format { eol: Eol::CrLf, bom: true },
            disk: (text.len() as u64, hash::fnv1a(text.as_bytes())),
            stamp: Stamp { size: 5, modified: 99, index: 1234 },
            known: vec![(1, 2), (3, 4)],
            folder: PathBuf::from(r"\\?\C:\Users\x\Documents\ok"),
            rel: PathBuf::from("College\\mitsqanaqan\u{0313}.md"),
            caret: Some(Pos { line: 2, col: 3 }),
            text: Arc::new(text.to_string()),
        }
    }

    fn change(doc: &mut Doc, at: usize, del: usize, text: &str) -> Change {
        let a = doc.pos_at(at);
        let b = doc.pos_at(at + del);
        doc.replace(Span::new(a, b), text)
    }

    /// A journal with every kind of record in it, and the records it should read back as.
    fn sample_journal() -> (Vec<u8>, Vec<Record>, String) {
        let mut doc = Doc::from_text("alpha\nbeta");
        let open = sample_open("alpha\nbeta");
        let mut bytes = header(0xABCD_EF01_2345_6789, 3).to_vec();
        let mut want = vec![Record::Open(open.clone())];
        push_open(&mut bytes, 0, &open);
        let c = change(&mut doc, 5, 0, " \u{00E9}t\u{00E9}");
        push_edit(&mut bytes, 1, &edit_payload(8, &c, doc.len_bytes()));
        want.push(Record::Edit(Edit { edit: 8, start: c.start, old_end: c.old_end, removed_len: 0, len_after: doc.len_bytes() as u64, inserted: c.inserted.clone() }));
        let marked = Format { eol: Eol::Lf, bom: true };
        push_begin(&mut bytes, 2, 8, (11, 22), marked, "~essay.md.0a1b2c3d.tmp");
        want.push(Record::Begin { edit: 8, state: (11, 22), format: Some(marked), temp: "~essay.md.0a1b2c3d.tmp".to_string() });
        push_done(&mut bytes, 3, 8, Stamp { size: 11, modified: 5, index: 6 }, Format::default());
        want.push(Record::Done { edit: 8, stamp: Stamp { size: 11, modified: 5, index: 6 }, format: Some(Format::default()) });
        let c = change(&mut doc, 0, 6, "");
        push_edit(&mut bytes, 4, &edit_payload(9, &c, doc.len_bytes()));
        want.push(Record::Edit(Edit { edit: 9, start: c.start, old_end: c.old_end, removed_len: 6, len_after: doc.len_bytes() as u64, inserted: String::new() }));
        push_path(&mut bytes, 5, Path::new("a.md"), Path::new("sub\\b.md"));
        want.push(Record::Path { old: PathBuf::from("a.md"), new: PathBuf::from("sub\\b.md") });
        push_caret(&mut bytes, 6, Pos { line: 1, col: 0 });
        want.push(Record::Caret(Pos { line: 1, col: 0 }));
        push_folder(&mut bytes, 7, Path::new(r"\\?\C:\Users\x\Documents\ok renamed"));
        want.push(Record::Folder(PathBuf::from(r"\\?\C:\Users\x\Documents\ok renamed")));
        push_lineage(&mut bytes, 8, 0xFEED);
        want.push(Record::Lineage(0xFEED));
        push_close(&mut bytes, 9);
        want.push(Record::Close);
        (bytes, want, doc.to_text("\n"))
    }

    /// Byte offsets at which each record of a journal ends.
    fn record_ends(bytes: &[u8]) -> Vec<usize> {
        let mut ends = Vec::new();
        let mut at = HEADER_LEN;
        while at + FRAME <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            at += FRAME + len;
            ends.push(at);
        }
        ends
    }

    #[test]
    fn every_kind_of_record_round_trips() {
        let (bytes, want, text) = sample_journal();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.docid, 0xABCD_EF01_2345_6789);
        assert_eq!(parsed.generation, 3);
        assert_eq!(parsed.records, want);
        assert_eq!(parsed.good_len, bytes.len());
        let replayed = replay(&parsed).unwrap();
        assert_eq!(replayed.text, text);
        assert_eq!(replayed.last_edit, 9);
        assert_eq!(replayed.done, Some((8, Stamp { size: 11, modified: 5, index: 6 })));
        assert!(replayed.closed);
        assert!(replayed.unsaved());
        assert_eq!(replayed.rel, PathBuf::from("sub\\b.md"));
        assert_eq!(replayed.old_rel, Some(PathBuf::from("a.md")));
        assert_eq!(replayed.caret, Some(Pos { line: 1, col: 0 }));
        assert_eq!(replayed.folder, PathBuf::from(r"\\?\C:\Users\x\Documents\ok renamed"));
        assert_eq!(replayed.lineage, 0xFEED);
        // The save that was begun wrote a mark; the one that finished wrote none.
        assert_eq!(replayed.format, Format::default());
        assert_eq!(replayed.temps, vec!["~essay.md.0a1b2c3d.tmp".to_string()]);
        // Remembered states, then the opening state, then the begun save.
        assert_eq!(replayed.known, vec![(1, 2), (3, 4), want_open_state(), (11, 22)]);
    }

    fn want_open_state() -> State {
        (10, hash::fnv1a(b"alpha\nbeta"))
    }

    #[test]
    fn an_empty_payload_is_a_real_record_not_the_end() {
        // The first design read "len 0" as end of log, which swallowed Close.
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &sample_open(""));
        push_close(&mut bytes, 1);
        push_caret(&mut bytes, 2, Pos { line: 0, col: 0 });
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.records.len(), 3);
        assert_eq!(parsed.records[1], Record::Close);
        // A Close followed by anything is no longer a clean close.
        assert!(!replay(&parsed).unwrap().closed);
        let mut only_close = header(1, 1).to_vec();
        push_open(&mut only_close, 0, &sample_open(""));
        push_close(&mut only_close, 1);
        assert!(replay(&parse(&only_close).unwrap()).unwrap().closed);
    }

    #[test]
    fn truncation_at_every_byte_keeps_exactly_the_whole_records() {
        let (bytes, want, _) = sample_journal();
        let ends = record_ends(&bytes);
        assert_eq!(ends.len(), want.len());
        for cut in 0..=bytes.len() {
            let parsed = parse(&bytes[..cut]);
            if cut < HEADER_LEN {
                assert!(parsed.is_none(), "cut {cut}");
                continue;
            }
            let parsed = parsed.unwrap();
            let whole = ends.iter().filter(|&&e| e <= cut).count();
            assert_eq!(parsed.records.len(), whole, "cut {cut}");
            assert_eq!(parsed.records[..], want[..whole], "cut {cut}");
            assert_eq!(parsed.good_len, if whole == 0 { HEADER_LEN } else { ends[whole - 1] }, "cut {cut}");
        }
    }

    #[test]
    fn a_zero_filled_or_garbage_tail_is_the_end_of_the_log() {
        let (bytes, want, _) = sample_journal();
        let mut zeros = bytes.clone();
        zeros.extend_from_slice(&[0u8; 4096]);
        assert_eq!(parse(&zeros).unwrap().records, want);
        let mut noise = bytes.clone();
        let mut x = 0x9E37_79B9u32;
        for _ in 0..4096 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            noise.push(x as u8);
        }
        assert_eq!(parse(&noise).unwrap().records, want);
        // A length that claims more than any record may hold.
        let mut huge = bytes.clone();
        huge.extend_from_slice(&((MAX_LEN + 1) as u32).to_le_bytes());
        huge.extend_from_slice(&[0u8; 64]);
        assert_eq!(parse(&huge).unwrap().records, want);
        // A length below the smallest record.
        let mut tiny = bytes;
        tiny.extend_from_slice(&8u32.to_le_bytes());
        tiny.extend_from_slice(&[0u8; 64]);
        assert_eq!(parse(&tiny).unwrap().records, want);
    }

    #[test]
    fn a_flipped_bit_stops_the_reader_at_that_record() {
        let (bytes, want, _) = sample_journal();
        let ends = record_ends(&bytes);
        let mut start = HEADER_LEN;
        for (i, &end) in ends.iter().enumerate() {
            // One flip in the checksum, one in the kind, one in the seq, one in the last byte.
            for at in [start + 4, start + 8, start + 9, end - 1] {
                let mut bad = bytes.clone();
                bad[at] ^= 0x10;
                let parsed = parse(&bad).unwrap();
                assert_eq!(parsed.records[..], want[..i], "record {i}, byte {at}");
            }
            start = end;
        }
    }

    #[test]
    fn a_record_out_of_sequence_stops_the_reader() {
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &sample_open("x"));
        push_caret(&mut bytes, 1, Pos::default());
        push_caret(&mut bytes, 3, Pos::default());
        push_caret(&mut bytes, 4, Pos::default());
        assert_eq!(parse(&bytes).unwrap().records.len(), 2);
        // A repeated seq (a record written twice) stops it too.
        let mut twice = header(1, 1).to_vec();
        push_open(&mut twice, 0, &sample_open("x"));
        push_caret(&mut twice, 1, Pos::default());
        push_caret(&mut twice, 1, Pos::default());
        assert_eq!(parse(&twice).unwrap().records.len(), 2);
        // A journal whose first record is not seq 0 has nothing in it.
        let mut late = header(1, 1).to_vec();
        push_open(&mut late, 1, &sample_open("x"));
        assert!(parse(&late).unwrap().records.is_empty());
    }

    #[test]
    fn a_wrong_header_is_not_a_journal() {
        assert!(parse(b"").is_none());
        assert!(parse(b"OKJ1\r\n\x1a\n").is_none());
        let mut bytes = header(1, 1).to_vec();
        bytes[0] = b'X';
        assert!(parse(&bytes).is_none());
        assert!(parse(&header(1, 1)).unwrap().records.is_empty());
        assert!(replay(&parse(&header(1, 1)).unwrap()).is_none());
    }

    #[test]
    fn replay_stops_at_an_edit_that_does_not_fit() {
        let mut doc = Doc::from_text("hello");
        let mut open = sample_open("hello");
        open.edit = 0;
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        let c1 = change(&mut doc, 5, 0, " world");
        push_edit(&mut bytes, 1, &edit_payload(1, &c1, doc.len_bytes()));
        // Edit 2 is missing: edit 3 must not be applied on top of edit 1.
        let _c2 = change(&mut doc, 0, 0, ">> ");
        let c3 = change(&mut doc, 0, 3, "");
        push_edit(&mut bytes, 2, &edit_payload(3, &c3, doc.len_bytes()));
        let r = replay(&parse(&bytes).unwrap()).unwrap();
        assert_eq!(r.text, "hello world");
        assert_eq!(r.last_edit, 1);

        // A recorded length that the replayed text does not reach ends replay
        // and leaves the text as it was before that edit.
        let mut doc = Doc::from_text("hello");
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        let c1 = change(&mut doc, 5, 0, "!");
        push_edit(&mut bytes, 1, &edit_payload(1, &c1, doc.len_bytes() + 1));
        let r = replay(&parse(&bytes).unwrap()).unwrap();
        assert_eq!(r.text, "hello");
        assert_eq!(r.last_edit, 0);

        // A position past the end of the text ends replay too.
        let far = Change { start: Pos { line: 9, col: 0 }, old_end: Pos { line: 9, col: 0 }, end: Pos { line: 9, col: 1 }, removed: String::new(), inserted: "x".into() };
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        push_edit(&mut bytes, 1, &edit_payload(1, &far, 6));
        assert_eq!(replay(&parse(&bytes).unwrap()).unwrap().text, "hello");
    }

    #[test]
    fn random_edits_replay_to_the_same_text() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize % n.max(1)
        };
        let pieces = ["a", "\u{00E9}", "word ", "\n", "\n\n", "x\ny", "n\u{0313}", "\u{4E00}", "end\n", "\r\n", "tab\there"];
        let mut open = sample_open("");
        open.edit = 0;
        open.on_disk = false;
        let mut doc = Doc::new();
        let mut bytes = header(9, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        let mut seq = 1;
        for edit in 1..=4000u64 {
            let len = doc.len_bytes();
            let at = rnd(len + 1);
            let del = if rnd(4) == 0 { rnd(12) } else { 0 };
            let text = if rnd(5) == 0 { "" } else { pieces[rnd(pieces.len())] };
            let c = change(&mut doc, at, del.min(len - at), text);
            if c.is_noop() {
                push_caret(&mut bytes, seq, c.start);
                seq += 1;
                // The edit number is not used up by a no-op.
                let c = change(&mut doc, 0, 0, "z");
                push_edit(&mut bytes, seq, &edit_payload(edit, &c, doc.len_bytes()));
            } else {
                push_edit(&mut bytes, seq, &edit_payload(edit, &c, doc.len_bytes()));
            }
            seq += 1;
        }
        let r = replay(&parse(&bytes).unwrap()).unwrap();
        assert_eq!(r.last_edit, 4000);
        assert_eq!(r.text, doc.to_text("\n"));
        assert!(r.unsaved());
    }

    #[test]
    fn a_large_paste_survives_the_round_trip() {
        let mut open = sample_open("start");
        open.edit = 0;
        let mut doc = Doc::from_text("start");
        let mut bytes = header(9, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        let big = "five megabytes of pasted text, line after line\n".repeat(110_000);
        assert!(big.len() > 5_000_000);
        let c = change(&mut doc, 5, 0, &big);
        push_edit(&mut bytes, 1, &edit_payload(1, &c, doc.len_bytes()));
        let r = replay(&parse(&bytes).unwrap()).unwrap();
        assert_eq!(r.text.len(), doc.len_bytes());
        assert_eq!(r.text, doc.to_text("\n"));
    }

    #[test]
    fn unsaved_follows_the_last_finished_save() {
        let mut open = sample_open("x");
        open.edit = 4;
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        assert!(!replay(&parse(&bytes).unwrap()).unwrap().unsaved());
        open.on_disk = false;
        let mut bytes = header(1, 1).to_vec();
        push_open(&mut bytes, 0, &open);
        assert!(replay(&parse(&bytes).unwrap()).unwrap().unsaved());
        let mut doc = Doc::from_text("x");
        let c = change(&mut doc, 1, 0, "y");
        push_edit(&mut bytes, 1, &edit_payload(5, &c, 2));
        push_begin(&mut bytes, 2, 5, (2, 0), Format::default(), "");
        assert!(replay(&parse(&bytes).unwrap()).unwrap().unsaved());
        push_done(&mut bytes, 3, 5, Stamp::default(), Format::default());
        assert!(!replay(&parse(&bytes).unwrap()).unwrap().unsaved());
    }

    #[test]
    fn a_text_that_begins_with_a_zero_width_no_break_space_replays_to_the_byte() {
        // The character is kept by Doc::replace and dropped by Doc::from_text.
        // A journal started afresh from such a text must rebuild it whole,
        // or every edit after it fails its length check.
        let text = "\u{FEFF}Pasted from a web page.";
        let mut open = sample_open(text);
        open.edit = 0;
        let mut doc = Doc::new();
        doc.replace(Span::at(Pos::default()), text);
        assert_eq!(doc.len_bytes(), text.len());
        let mut bytes = header(1, 2).to_vec();
        push_open(&mut bytes, 0, &open);
        let c1 = change(&mut doc, text.len(), 0, " And typed on.");
        push_edit(&mut bytes, 1, &edit_payload(1, &c1, doc.len_bytes()));
        let c2 = change(&mut doc, 0, 3, "");
        push_edit(&mut bytes, 2, &edit_payload(2, &c2, doc.len_bytes()));
        let r = replay(&parse(&bytes).unwrap()).unwrap();
        assert_eq!((r.last_edit, r.text.as_str()), (2, "Pasted from a web page. And typed on."));
        assert_eq!(doc_of(text).to_text("\n"), text);
        assert_eq!(doc_of("a\r\nb\rc").to_text("\n"), "a\nb\nc");
        assert_eq!(doc_of("").to_text("\n"), "");
    }

    #[test]
    fn a_journal_is_known_to_carry_on_from_a_text_it_passes_through() {
        let mut open = sample_open("Saved.");
        open.edit = 0;
        let mut doc = Doc::from_text("Saved.");
        let mut bytes = header(1, 2).to_vec();
        push_open(&mut bytes, 0, &open);
        let mut payloads = Vec::new();
        for (n, piece) in [" One.", " Two.", " Three."].iter().enumerate() {
            let end = doc.len_bytes();
            let c = change(&mut doc, end, 0, piece);
            let payload = edit_payload(n as u64 + 1, &c, doc.len_bytes());
            push_edit(&mut bytes, n as u64 + 1, &payload);
            payloads.push(payload);
        }
        let parsed = parse(&bytes).unwrap();
        for (probe, passed) in [("Saved.", true), ("Saved. One.", true), ("Saved. One. Two. Three.", true), ("Saved. Two.", false), ("Saved. One!", false), ("", false)] {
            assert_eq!(replay_seeking(&parsed, Some(probe)).unwrap().1, passed, "{probe:?}");
        }
        assert!(!replay_seeking(&parsed, None).unwrap().1);

        // The same edits, as the journal thread holds them in memory.
        assert_eq!(rebuild("Saved.", payloads.iter().map(Vec::as_slice)), "Saved. One. Two. Three.");
        assert_eq!(rebuild("Saved.", payloads[..1].iter().map(Vec::as_slice)), "Saved. One.");
        // One that does not fit ends it there, with what came before intact.
        assert_eq!(rebuild("Saved.", [payloads[1].as_slice(), payloads[0].as_slice()].into_iter()), "Saved.");

        // What the journal thread reads from a payload without decoding it,
        // and the one field it may put right.
        assert_eq!(edit_lengths(&payloads[2]), Some((3, 0, 7, 23)));
        let mut wrong = edit_payload(4, &change(&mut doc, 0, 6, "S"), 999);
        assert_eq!(edit_lengths(&wrong), Some((4, 6, 1, 999)));
        set_len_after(&mut wrong, doc.len_bytes() as u64);
        assert_eq!(edit_lengths(&wrong), Some((4, 6, 1, 18)));
        assert_eq!(edit_lengths(&wrong[..20]), None);
    }

    #[test]
    fn journal_file_names_round_trip() {
        assert_eq!(file_name(0xAB, 12), "00000000000000ab.12.wal");
        assert_eq!(parse_name("00000000000000ab.12.wal"), Some((0xAB, 12, false)));
        assert_eq!(parse_name("00000000000000ab.13.wal.part"), Some((0xAB, 13, true)));
        for bad in ["reserve.bin", "ab.1.wal", "00000000000000ab.wal", "00000000000000ab.x.wal", "00000000000000zz.1.wal", "00000000000000ab.1.wal.tmp"] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }
}
