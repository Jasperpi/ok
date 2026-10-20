//! Earlier versions of a text: plain copies under `history\<docid>\` that any
//! editor can open, the rule for which of them are thinned out as they age,
//! and the two small notes kept beside them (where the text lives, and where
//! the caret was left).
//!
//! A version's file name is its time in UTC, an optional count for versions
//! kept in the same second, an optional label, and the text's own ending:
//! `20261011-143205.md`, `20261011-143205-2.before-cut.md`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use ok_text::Pos;

use crate::fsio::{self, Fail};
use crate::names;
use crate::sys;

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

/// One kept version of a text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Version {
    /// The name to hand back to read this version or to return to it.
    pub name: String,
    /// When it was kept, in milliseconds since 1970, UTC.
    pub at_ms: u64,
    /// Why it was kept, when there was a reason beyond time passing: `kept`
    /// (he asked), `before-cut`, `before-reload`, `outside`, `recovery`,
    /// `replaced`, `before-mend`, `before-discard`. Labelled versions are
    /// never thinned out.
    pub label: Option<String>,
    /// Its size on disk.
    pub bytes: u64,
}

/// The history folder under the data directory.
pub(crate) struct History {
    dir: PathBuf,
}

/// Reads a version's file name back into its time, its count within that
/// second, and its label.
fn parse_name(name: &str) -> Option<(u64, u32, Option<String>)> {
    let (stamp, rest) = name.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if stamp.len() < 15 || !stamp.is_char_boundary(8) || !stamp.is_char_boundary(9) || !stamp.is_char_boundary(15) {
        return None;
    }
    let (date, time) = (&stamp[..8], &stamp[9..15]);
    if &stamp[8..9] != "-" || !digits(date) || !digits(time) {
        return None;
    }
    let n = match &stamp[15..] {
        "" => 1,
        more => {
            let count = more.strip_prefix('-')?;
            if !digits(count) {
                return None;
            }
            count.parse().ok()?
        }
    };
    let field = |s: &str| s.parse::<u16>().ok();
    let at = sys::unix_ms_from_utc(field(&date[..4])?, field(&date[4..6])? as u8, field(&date[6..8])? as u8, field(&time[..2])? as u8, field(&time[2..4])? as u8, field(&time[4..6])? as u8)?;
    let label = rest.rsplit_once('.').map(|(label, _)| label.to_string());
    Some((at, n, label))
}

/// A label as it may appear in a file name: lower-case letters, digits and
/// hyphens, at most 40 of them. `None` when nothing is left.
fn clean_label(label: &str) -> Option<String> {
    let mapped: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).take(40).collect();
    let trimmed = mapped.trim_matches('-');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn stamp_of(unix_ms: u64) -> String {
    let (y, mo, d, h, mi, s) = sys::utc_fields(unix_ms);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Which versions stay. `items` are (time, labelled) pairs, oldest first.
/// Kept: the first and the newest, every labelled one, everything younger
/// than two hours, the last of each clock hour for 48 hours, the last of each
/// day for 60 days, and the last of each month for ever. A version dated
/// after `now_ms` (the clock was set back) counts as new and stays.
pub(crate) fn keep_set(items: &[(u64, bool)], now_ms: u64) -> Vec<bool> {
    let mut keep = vec![false; items.len()];
    let Some(last) = items.len().checked_sub(1) else { return keep };
    keep[0] = true;
    keep[last] = true;
    let (mut hour, mut day, mut month) = (None, None, None);
    for (i, &(at, labelled)) in items.iter().enumerate().rev() {
        let age = now_ms.saturating_sub(at);
        if labelled || age < 2 * HOUR {
            keep[i] = true;
        }
        let this_hour = Some(at / HOUR);
        if age < 48 * HOUR && hour != this_hour {
            keep[i] = true;
        }
        hour = this_hour;
        let this_day = Some(at / DAY);
        if age < 60 * DAY && day != this_day {
            keep[i] = true;
        }
        day = this_day;
        let (y, mo, ..) = sys::utc_fields(at);
        let this_month = Some((y, mo));
        if month != this_month {
            keep[i] = true;
        }
        month = this_month;
    }
    keep
}

impl History {
    pub(crate) fn new(dir: PathBuf) -> History {
        History { dir }
    }

    fn doc_dir(&self, docid: u64) -> PathBuf {
        self.dir.join(format!("{docid:016x}"))
    }

    /// Every text of the writing folder `root` that has a history, by the
    /// lower-case form of its path inside the folder.
    pub(crate) fn load_ids(&self, root: &Path) -> HashMap<String, u64> {
        let mut ids = HashMap::new();
        let Ok(entries) = fs::read_dir(&self.dir) else { return ids };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() != 16 {
                continue;
            }
            let Ok(docid) = u64::from_str_radix(&name, 16) else { continue };
            let Ok(origin) = fs::read_to_string(entry.path().join("origin.txt")) else { continue };
            let mut lines = origin.lines();
            let (Some(folder), Some(rel)) = (lines.next(), lines.next()) else { continue };
            if names::same_folder(Path::new(folder), root) {
                ids.insert(names::key(Path::new(rel)), docid);
            }
        }
        ids
    }

    /// Where the text with this id was last noted to live: the writing
    /// folder and the path inside it.
    pub(crate) fn origin(&self, docid: u64) -> Option<(PathBuf, PathBuf)> {
        let origin = fs::read_to_string(self.doc_dir(docid).join("origin.txt")).ok()?;
        let mut lines = origin.lines();
        let (folder, rel) = (lines.next()?, lines.next()?);
        Some((PathBuf::from(folder), PathBuf::from(rel)))
    }

    /// Notes where the text with this id lives, so its history is found
    /// again the next time the folder is opened.
    pub(crate) fn set_origin(&self, docid: u64, root: &Path, rel: &Path) -> bool {
        let dir = self.doc_dir(docid);
        if fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let note = format!("{}\n{}\n", root.display(), rel.display());
        fsio::write_plain(&dir.join("origin.txt"), note.as_bytes(), true).is_ok()
    }

    /// The writing folder was found at a new place: every note that named
    /// the old place is rewritten, so the histories stay attached.
    pub(crate) fn move_folder(&self, old: &Path, new: &Path) {
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let note = entry.path().join("origin.txt");
            let Ok(origin) = fs::read_to_string(&note) else { continue };
            let mut lines = origin.lines();
            let (Some(folder), Some(rel)) = (lines.next(), lines.next()) else { continue };
            if names::same_folder(Path::new(folder), old) {
                let _ = fsio::write_plain(&note, format!("{}\n{rel}\n", new.display()).as_bytes(), true);
            }
        }
    }

    /// Where the caret was when the text was last closed.
    pub(crate) fn caret(&self, docid: u64) -> Option<Pos> {
        let note = fs::read_to_string(self.doc_dir(docid).join("caret.txt")).ok()?;
        let mut parts = note.split_whitespace();
        Some(Pos { line: parts.next()?.parse().ok()?, col: parts.next()?.parse().ok()? })
    }

    pub(crate) fn set_caret(&self, docid: u64, caret: Pos) {
        let dir = self.doc_dir(docid);
        if fs::create_dir_all(&dir).is_ok() {
            let _ = fsio::write_plain(&dir.join("caret.txt"), format!("{} {}\n", caret.line, caret.col).as_bytes(), true);
        }
    }

    /// Versions with their count within the second, newest first.
    fn list_counted(&self, docid: u64) -> Vec<(Version, u32)> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.doc_dir(docid)) else { return out };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((at_ms, n, label)) = parse_name(&name) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() {
                out.push((Version { name, at_ms, label, bytes: meta.len() }, n));
            }
        }
        out.sort_by(|a, b| (b.0.at_ms, b.1, &b.0.name).cmp(&(a.0.at_ms, a.1, &a.0.name)));
        out
    }

    /// The versions of a text, newest first.
    pub(crate) fn list(&self, docid: u64) -> Vec<Version> {
        self.list_counted(docid).into_iter().map(|(version, _)| version).collect()
    }

    /// The bytes of one version. `None` if there is no such version; a name
    /// that is not a version's name is never looked up.
    pub(crate) fn read(&self, docid: u64, name: &str) -> Option<Vec<u8>> {
        parse_name(name)?;
        if name.contains(['\\', '/', ':']) {
            return None;
        }
        fs::read(self.doc_dir(docid).join(name)).ok()
    }

    /// Keeps `bytes` as a version. Unless `force`d, nothing is written when
    /// the newest version already holds exactly these bytes (and, for a
    /// labelled version, already carries the label): `Ok(None)`. A version is
    /// never dated before the newest one, so a clock set back cannot reorder
    /// them, and it never replaces a file.
    pub(crate) fn snapshot(&self, docid: u64, ending: &str, bytes: &[u8], label: Option<&str>, now_ms: u64, force: bool) -> Result<Option<Version>, Fail> {
        let dir = self.doc_dir(docid);
        fs::create_dir_all(&dir).map_err(|e| fsio::open_fail(sys::code(&e)))?;
        let label = label.and_then(clean_label);
        let all = self.list_counted(docid);
        if let (false, Some((newest, _))) = (force, all.first()) {
            let same_label = label.is_none() || newest.label == label;
            if same_label && newest.bytes == bytes.len() as u64 && fs::read(dir.join(&newest.name)).map(|there| there == bytes).unwrap_or(false) {
                return Ok(None);
            }
        }
        let newest_at = all.first().map(|(v, _)| v.at_ms).unwrap_or(0);
        let at_ms = (now_ms / 1000 * 1000).max(newest_at);
        let first = all.iter().filter(|(v, _)| v.at_ms == at_ms).map(|(_, n)| *n).max().unwrap_or(0) + 1;
        let tag = label.as_ref().map(|l| format!(".{l}")).unwrap_or_default();
        for n in first..first + 50 {
            let count = if n > 1 { format!("-{n}") } else { String::new() };
            let name = format!("{}{count}{tag}.{ending}", stamp_of(at_ms));
            match fsio::write_plain(&dir.join(&name), bytes, false) {
                // The size is the one the file system gives for the file just written.
                Ok(written) => return Ok(Some(Version { name, at_ms, label, bytes: written.stamp.size })),
                Err(Fail::NameTaken) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(Fail::NameTaken)
    }

    /// Removes the versions [`keep_set`] does not keep, and any half-written
    /// copy a kill left behind more than ten minutes ago. These redundant
    /// copies are the only thing this crate ever erases. Returns how many
    /// versions went.
    pub(crate) fn thin(&self, docid: u64, now_ms: u64) -> usize {
        let mut all = self.list(docid);
        all.reverse();
        let items: Vec<(u64, bool)> = all.iter().map(|v| (v.at_ms, v.label.is_some())).collect();
        let keep = keep_set(&items, now_ms);
        let dir = self.doc_dir(docid);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let old = entry.metadata().ok().and_then(|meta| meta.modified().ok()).and_then(|at| at.elapsed().ok()).is_some_and(|age| age.as_secs() > 600);
                if old && fsio::is_temp_name(&entry.file_name().to_string_lossy()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let mut gone = 0;
        for (version, keep) in all.iter().zip(keep) {
            if !keep && fs::remove_file(dir.join(&version.name)).is_ok() {
                gone += 1;
            }
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    const NOW: u64 = 1_791_729_125_000;

    #[test]
    fn version_names_round_trip() {
        assert_eq!(parse_name("20261011-143205.md"), Some((NOW, 1, None)));
        assert_eq!(parse_name("20261011-143205-3.md"), Some((NOW, 3, None)));
        assert_eq!(parse_name("20261011-143205.outside.md"), Some((NOW, 1, Some("outside".to_string()))));
        assert_eq!(parse_name("20261011-143205-2.before-cut.txt"), Some((NOW, 2, Some("before-cut".to_string()))));
        assert_eq!(stamp_of(NOW), "20261011-143205");
        for bad in ["origin.txt", "caret.txt", "~20261011-143205.md.0a0a0a0a.tmp", "20261311-143205.md", "20261011-143205", "20261011_143205.md", "20261011-143205x.md", "20261011-143205-.md", "\u{00E9}0261011-143205.md"] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
        assert_eq!(clean_label("Before Cut!"), Some("before-cut".to_string()));
        assert_eq!(clean_label("..\\x"), Some("x".to_string()));
        assert_eq!(clean_label("  "), None);
    }

    #[test]
    fn identical_consecutive_saves_make_one_version() {
        let dir = Scratch::new("history-dedupe");
        let h = History::new(dir.path().join("history"));
        assert!(h.snapshot(7, "md", b"one", None, NOW, false).unwrap().is_some());
        assert!(h.snapshot(7, "md", b"one", None, NOW + 5000, false).unwrap().is_none());
        // A label is a reason to keep a second copy; the same label again is not.
        assert!(h.snapshot(7, "md", b"one", Some("before-cut"), NOW + 6000, false).unwrap().is_some());
        assert!(h.snapshot(7, "md", b"one", Some("before-cut"), NOW + 7000, false).unwrap().is_none());
        assert!(h.snapshot(7, "md", b"one", None, NOW + 8000, false).unwrap().is_none());
        // Asked for by hand: always kept.
        assert!(h.snapshot(7, "md", b"one", Some("kept"), NOW + 9000, true).unwrap().is_some());
        assert!(h.snapshot(7, "md", b"one", Some("kept"), NOW + 9500, true).unwrap().is_some());
        let all = h.list(7);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].label.as_deref(), Some("kept"));
        assert_eq!(all[3].label, None);
        assert_eq!(h.read(7, &all[3].name).unwrap(), b"one");
        assert_eq!(h.read(7, "origin.txt"), None);
        assert_eq!(h.read(7, r"..\..\20261011-143205.md"), None);
    }

    #[test]
    fn versions_in_one_second_keep_their_order_and_never_overwrite() {
        let dir = Scratch::new("history-order");
        let h = History::new(dir.path().join("history"));
        for (i, label) in [None, Some("outside"), None, Some("kept")].into_iter().enumerate() {
            h.snapshot(1, "md", format!("text {i}").as_bytes(), label, NOW + i as u64, true).unwrap().unwrap();
        }
        // The clock goes back an hour: the next version still sorts last.
        h.snapshot(1, "md", b"text 4", None, NOW - HOUR, false).unwrap().unwrap();
        let all = h.list(1);
        assert_eq!(all.len(), 5);
        for (i, version) in all.iter().rev().enumerate() {
            assert_eq!(h.read(1, &version.name).unwrap(), format!("text {i}").as_bytes(), "{}", version.name);
        }
    }

    #[test]
    fn the_origin_note_finds_a_text_again() {
        let dir = Scratch::new("history-origin");
        let h = History::new(dir.path().join("history"));
        let root = Path::new(r"\\?\C:\Users\x\Documents\ok");
        assert!(h.set_origin(0xAB, root, Path::new(r"College\The Orchard.md")));
        assert!(h.set_origin(0xCD, Path::new(r"\\?\C:\elsewhere"), Path::new("other.md")));
        let ids = h.load_ids(Path::new(r"\\?\c:\users\X\documents\OK"));
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get(r"college\the orchard.md"), Some(&0xAB));
        assert_eq!(h.caret(0xAB), None);
        h.set_caret(0xAB, Pos { line: 12, col: 34 });
        assert_eq!(h.caret(0xAB), Some(Pos { line: 12, col: 34 }));
    }

    #[test]
    fn thinning_keeps_the_first_the_newest_and_every_labelled_one() {
        // One version every 20 minutes for 90 days, a labelled one every 10 days.
        let mut items = Vec::new();
        let start = NOW - 90 * DAY;
        let mut at = start;
        while at <= NOW {
            items.push((at, (at - start).is_multiple_of(10 * DAY) && at != start));
            at += 20 * 60_000;
        }
        let keep = keep_set(&items, NOW);
        assert!(keep[0] && keep[items.len() - 1]);
        for (i, &(at, labelled)) in items.iter().enumerate() {
            let age = NOW - at;
            if labelled {
                assert!(keep[i], "labelled at {} days", age / DAY);
            }
            if age < 2 * HOUR {
                assert!(keep[i], "younger than two hours");
            }
        }
        // 61 days old and labelled: stays.
        let old = items.iter().position(|&(at, labelled)| labelled && NOW - at > 61 * DAY).unwrap();
        assert!(keep[old]);
        // Between 3 and 59 days old: exactly one unlabelled version a day survives.
        let mut per_day: HashMap<u64, usize> = HashMap::new();
        for (i, &(at, labelled)) in items.iter().enumerate() {
            let age = NOW - at;
            if keep[i] && !labelled && age > 3 * DAY && age < 59 * DAY {
                *per_day.entry(at / DAY).or_default() += 1;
            }
        }
        assert!(per_day.len() >= 55 && per_day.values().all(|&n| n == 1), "{per_day:?}");
        // Thinning what is left changes nothing.
        let left: Vec<(u64, bool)> = items.iter().zip(&keep).filter(|(_, k)| **k).map(|(item, _)| *item).collect();
        assert!(keep_set(&left, NOW).iter().all(|k| *k));
        // A clock set back to before every version removes nothing.
        assert!(keep_set(&left, start - DAY).iter().all(|k| *k));
        assert!(keep_set(&[], NOW).is_empty());
        assert_eq!(keep_set(&[(5, false)], NOW), vec![true]);
    }

    #[test]
    fn random_histories_never_lose_what_must_stay() {
        let mut seed = 0x9E37_79B9u32;
        let mut rnd = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u64 % n.max(1)
        };
        for _ in 0..200 {
            let mut items: Vec<(u64, bool)> = (0..rnd(60)).map(|_| (NOW - rnd(400 * DAY), rnd(7) == 0)).collect();
            items.sort();
            let keep = keep_set(&items, NOW);
            if let Some(last) = items.len().checked_sub(1) {
                assert!(keep[0] && keep[last]);
            }
            for (i, &(_, labelled)) in items.iter().enumerate() {
                assert!(!labelled || keep[i]);
            }
            let left: Vec<(u64, bool)> = items.iter().zip(&keep).filter(|(_, k)| **k).map(|(item, _)| *item).collect();
            assert!(keep_set(&left, NOW).iter().all(|k| *k), "thinning twice must change nothing");
        }
    }

    #[test]
    fn thinning_on_disk_removes_only_the_redundant() {
        let dir = Scratch::new("history-thin");
        let h = History::new(dir.path().join("history"));
        let old = NOW - 200 * DAY;
        // Ten unlabelled versions inside one long-past hour, one labelled among them.
        for i in 0..10u64 {
            let label = if i == 4 { Some("before-cut") } else { None };
            h.snapshot(9, "md", format!("v{i}").as_bytes(), label, old + i * 60_000, false).unwrap().unwrap();
        }
        h.snapshot(9, "md", b"today", None, NOW - 1000, false).unwrap().unwrap();
        assert_eq!(h.list(9).len(), 11);
        let gone = h.thin(9, NOW);
        let left: Vec<Vec<u8>> = h.list(9).iter().rev().map(|v| h.read(9, &v.name).unwrap()).collect();
        // The first, the labelled one, the last of that month, and the newest.
        assert_eq!(left, vec![b"v0".to_vec(), b"v4".to_vec(), b"v9".to_vec(), b"today".to_vec()]);
        assert_eq!(gone, 7);
        assert_eq!(h.thin(9, NOW), 0);
    }
}
