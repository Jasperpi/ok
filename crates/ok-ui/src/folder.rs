//! The session that keeps the text in the writing folder.
//!
//! Every change goes to the save layer's journal the moment it is made. The
//! file itself is asked for two seconds after typing stops, and at least
//! every twenty seconds while it does not stop. `ok` is printed only when
//! the save layer reports the latest edit written and flushed.
//!
//! The session keeps its own copy of the open text, brought up to date
//! change by change, and that copy is what is journaled and saved. A text
//! handed in with a call is only ever compared with it: while the line shows
//! some other text on the page, what is handed in is that other text, and it
//! must never be written into this one's file.
//!
//! What is remembered between sittings is kept in small state files through
//! the save layer, under ok's own folder and never in the writing folder:
//! one for each writing folder (the last text, the one before it, the last
//! sitting, the words typed today, and for each text its word limit and the
//! lines changed on which day), `settings.txt`, and `once.txt` for the two
//! things that are said once.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_host::{Host, LocalTime};
use ok_store::{DocId, Hold, News as StoreNews, OpenError, ReadOnly, Recovered, Recovery, Resolve, Setup, Store, StoreError, Tree, Version};
use ok_text::links::{self, link_at, links_in, retarget, LinkIndex};
use ok_text::markup::{classify_line, pieces, LineKind, Style};
use ok_text::{count, Change, Doc, Pos, Span};

use crate::contents::Entry;
use crate::machine::{thousands, words};
use crate::page::Day;
use crate::session::{LinkRow, Links, News, Opened, Options, Session, NOT_HERE};

/// The file is written this long after typing stops.
const PAUSE_MS: u64 = 2_000;
/// And at least this often while typing does not stop.
const LONGEST_MS: u64 = 20_000;
/// A sitting ends after this long without a key.
const SITTING_MS: u64 = 30 * 60 * 1_000;
/// How long launch waits for the folder's first listing before it goes on
/// without one.
const FIRST_LISTING: Duration = Duration::from_millis(400);
/// How long a close waits for the file itself. The journal already holds
/// everything by then, so running out of time loses nothing.
const LAST_SAVE: Duration = Duration::from_millis(1_500);
/// How often a text that will not open just now is asked for again, and
/// the pause between.
const OPEN_TRIES: usize = 4;
const OPEN_PAUSE: Duration = Duration::from_millis(60);
/// The longest name the first line gives a text, in characters.
const NAME_LIMIT: usize = 60;
/// The most versions `earlier` lists.
const MOST_VERSIONS: usize = 200;
/// How many renames are remembered for mending texts that are read later.
const MOST_MENDS: usize = 16;

/// The state file that holds the settings text, and the one for what is
/// said once.
const SETTINGS: &str = "settings.txt";
const ONCE: &str = "once.txt";

const START: &str = "Start typing. Each word is saved as you finish it.";
const HINT: &str = "Esc goes to the line. Hold Ctrl or Alt to see what they do.";
const SAVED_AGAIN: &str = "Saved again. Nothing was lost.";
const NEWER: &str = "This text changed on disk. The newer one is shown; yours is kept.";
const NOT_UTF8: &str = "This file is not UTF-8, so it is open for reading only.";
const DISCARDED: &str = "Moved to .discarded. Nothing was erased.";
const READING_ONLY: &str = "This text is open for reading only.";
const NOT_KEPT: &str = "? That version could not be kept. The text itself is safe.";
const JOURNAL_DOWN: &str = "Not saved. The journal cannot be written. Still trying.";
const WAITING: &str = "Words typed after the last save are waiting in the journal.";
const REMOVED: &str = "Another program removed this file. Your words are still here.";
const NOT_ATOMIC: &str = "This drive cannot swap a file in one step. The journal covers it.";

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The number of the day a date is, counted from 1 January 1970.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let (month, day) = (month.clamp(1, 12), day.clamp(1, 31));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// The date of a day counted from 1 January 1970.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days.clamp(-100_000_000, 100_000_000) + 719_468;
    let era = shifted.div_euclid(146_097);
    let of_era = shifted.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// A moment as the person at the keyboard reads it: the seconds on his own
/// clock since 1970, from which the date and the time of day follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Moment {
    local: i64,
}

impl Moment {
    fn of(now: &LocalTime) -> Moment {
        let days = days_from_civil(now.year as i64, now.month as i64, now.day as i64);
        Moment { local: days * 86_400 + now.hour.min(23) as i64 * 3_600 + now.minute.min(59) as i64 * 60 + now.second.min(59) as i64 }
    }

    /// A moment given in milliseconds since 1970 UTC, read on a clock that
    /// is `offset` seconds ahead of UTC.
    fn at(unix_ms: u64, offset: i64) -> Moment {
        Moment { local: (unix_ms / 1000).min(i64::MAX as u64 / 2) as i64 + offset }
    }

    /// The writing day it falls in. Days begin at 04:00.
    fn writing_day(self) -> i64 {
        (self.local - 4 * 3_600).div_euclid(86_400)
    }

    /// `23:41`.
    fn time(self) -> String {
        let of_day = self.local.rem_euclid(86_400);
        format!("{:02}:{:02}", of_day / 3_600, of_day % 3_600 / 60)
    }

    /// The day as it is named from `now`: `Today`, the weekday for the six
    /// days before, and after that the date. The day is the writing day, so
    /// half past one at night still belongs to the evening before.
    fn day(self, now: Moment) -> String {
        let (day, today) = (self.writing_day(), now.writing_day());
        let (year, month, date) = civil_from_days(day);
        match today - day {
            ago if ago <= 0 => "Today".to_string(),
            1..=6 => WEEKDAYS[(day + 3).rem_euclid(7) as usize].to_string(),
            _ if year == civil_from_days(today).0 => format!("{date} {}", MONTHS[(month - 1).clamp(0, 11) as usize]),
            _ => format!("{date} {} {year}", MONTHS[(month - 1).clamp(0, 11) as usize]),
        }
    }
}

/// How far the clock on the wall is ahead of UTC, in seconds.
fn offset_of(now: &LocalTime) -> i64 {
    Moment::of(now).local - now.unix
}

/// The sitting he was last in: when its last key was pressed, and how many
/// words were typed in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Here {
    at: Moment,
    added: usize,
}

impl Here {
    fn sentence(&self, now: Moment) -> String {
        let day = match self.at.day(now) {
            day if day == "Today" => "today".to_string(),
            day => day,
        };
        format!("Last here {day} at {}. That sitting added {}.", self.at.time(), words(self.added))
    }
}

fn full_name(rel: &Path) -> String {
    rel.with_extension("").to_string_lossy().replace('\\', "/")
}

fn leaf(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// The key a path inside the folder is looked up by: lower case, since the
/// disk does not tell capitals apart.
fn rel_key(rel: &Path) -> String {
    rel.to_string_lossy().replace('/', "\\").to_lowercase()
}

/// The ending of a text's file, `md` when it has none.
fn ending_of(rel: &Path) -> String {
    rel.extension().map(|ending| ending.to_string_lossy().into_owned()).filter(|ending| !ending.is_empty()).unwrap_or_else(|| "md".to_string())
}

/// A path as a person writes it, without the `\\?\` Windows puts in front
/// of the paths it spells out itself.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match (text.strip_prefix(r"\\?\UNC\"), text.strip_prefix(r"\\?\")) {
        (Some(share), _) => PathBuf::from(format!(r"\\{share}")),
        (None, Some(rest)) => PathBuf::from(rest),
        (None, None) => path.to_path_buf(),
    }
}

/// Where `path` lies inside `root`, if it does, whatever the capitals and
/// however either of them is spelled.
fn inside(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = plain(root);
    let by_parent = path.parent().and_then(|parent| std::fs::canonicalize(parent).ok()).zip(path.file_name()).map(|(parent, name)| parent.join(name));
    for candidate in [Some(path.to_path_buf()), std::fs::canonicalize(path).ok(), by_parent, std::path::absolute(path).ok()].into_iter().flatten() {
        let candidate = plain(&candidate);
        let mut rest = candidate.components();
        let within = root.components().all(|part| rest.next().is_some_and(|other| part.as_os_str().to_string_lossy().to_lowercase() == other.as_os_str().to_string_lossy().to_lowercase()));
        let rel = rest.as_path();
        if within && !rel.as_os_str().is_empty() {
            return Some(rel.to_path_buf());
        }
    }
    None
}

/// A title someone typed, or a first line, as one part of a file name: the
/// marks that make a heading, emphasis or a link are dropped along with
/// everything Windows forbids, because a name holding `#`, `|` or a bracket
/// could never be linked to.
fn clean_part(title: &str) -> String {
    let unmarked: String = title.trim().trim_start_matches(['#', '>', ' ']).chars().map(|ch| if matches!(ch, '#' | '|' | '[' | ']' | '`' | '*') { ' ' } else { ch }).collect();
    let words: Vec<&str> = unmarked.split_whitespace().map(|word| word.trim_matches('_')).filter(|word| !word.is_empty()).collect();
    ok_store::safe_file_name(&words.join(" "))
}

/// The name a finished first line gives its text: at most [`NAME_LIMIT`]
/// characters, ending at a whole word where it can. `None` for a line with
/// nothing in it to name a text by.
fn title_of(line: &str) -> Option<String> {
    if !line.chars().any(char::is_alphanumeric) {
        return None;
    }
    let name = clean_part(line);
    let mut short: String = name.chars().take(NAME_LIMIT).collect();
    if short.len() < name.len() {
        let cut_in_a_word = name[short.len()..].chars().next().is_some_and(|next| !next.is_whitespace());
        match short.rfind(' ') {
            Some(space) if cut_in_a_word && space >= NAME_LIMIT / 3 => short.truncate(space),
            _ => {}
        }
    }
    let short = ok_store::safe_file_name(short.trim_end_matches([' ', ',', ';', ':', '-', '.']));
    short.chars().any(char::is_alphanumeric).then_some(short)
}

/// True for the names ok itself gives a text nobody has named: `Untitled`
/// and `Untitled 2`, `Untitled 3` and so on.
fn is_untitled(leaf: &str) -> bool {
    match leaf.strip_prefix("Untitled") {
        Some("") => true,
        Some(rest) => rest.strip_prefix(' ').is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// A fingerprint of a text, to tell whether what was remembered about its
/// lines still belongs to it.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The value of one setting in a settings text: `name value` to a line.
fn setting(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (word, value) = line.trim().split_once(char::is_whitespace)?;
        (word == name).then(|| value.trim().to_string())
    })
}

/// A settings text with one setting put in, in place of its old line or at
/// the end.
fn with_setting(text: &str, name: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().filter(|line| line.split_whitespace().next() != Some(name)).map(str::to_string).collect();
    lines.push(format!("{name} {value}"));
    lines.join("\n") + "\n"
}

/// How many links in `text` lead to the name with this key, leaving out
/// fenced code as the link index does.
fn count_links(text: &str, target: &str) -> usize {
    let mut in_fence = false;
    let mut found = 0;
    for line in text.split('\n') {
        let kind = classify_line(line, in_fence);
        in_fence ^= kind == LineKind::Fence;
        if !matches!(kind, LineKind::Fence | LineKind::Code) && line.contains("[[") {
            found += links_in(line).iter().filter(|link| links::key(link.target) == target).count();
        }
    }
    found
}

/// Whether the first and the last of the lines a change touched read the
/// same after it as before. `doc` is the text as that change left it. This
/// is the page's own rule for which lines a change bar goes beside, kept
/// here as well because the stamps are remembered here.
fn ends_untouched(doc: &Doc, change: &Change) -> (bool, bool) {
    let (removed, inserted) = (change.removed.as_str(), change.inserted.as_str());
    let (removed_broke, inserted_broke) = (removed.contains('\n'), inserted.contains('\n'));
    fn first_of(text: &str) -> &str {
        text.split('\n').next().unwrap_or("")
    }
    fn last_of(text: &str) -> &str {
        text.rsplit('\n').next().unwrap_or("")
    }
    let head = doc.line(change.start.line).get(..change.start.col).unwrap_or("");
    let tail = doc.line(change.end.line).get(change.end.col..).unwrap_or("");
    let alike = |a: (&str, &str), b: (&str, &str)| a.0.len() + a.1.len() == b.0.len() + b.1.len() && a.0.bytes().chain(a.1.bytes()).eq(b.0.bytes().chain(b.1.bytes()));
    let first_same = alike((first_of(inserted), if inserted_broke { "" } else { tail }), (first_of(removed), if removed_broke { "" } else { tail }));
    let last_same = alike((if inserted_broke { "" } else { head }, last_of(inserted)), (if removed_broke { "" } else { head }, last_of(removed)));
    (first_same, last_same)
}

/// Stamps the lines a change wrote with today, as the page does: the lines
/// at either end keep the day they had when they read as they did.
fn stamp(stamps: &mut Vec<Day>, doc: &Doc, change: &Change, today: Day) {
    let (first, old, new) = change.lines();
    if old == 0 || first + old > stamps.len() || first + new > doc.line_count() {
        stamps.resize(doc.line_count(), today);
        return;
    }
    let (was_first, was_last) = (stamps[first], stamps[first + old - 1]);
    let (first_same, last_same) = ends_untouched(doc, change);
    stamps.splice(first..first + old, std::iter::repeat_n(today, new));
    if last_same && !(first_same && new > old) {
        stamps[first + new - 1] = was_last;
    }
    if first_same {
        stamps[first] = was_first;
    }
}

/// The stamps for a text put in place of another: the lines the two share
/// at the top and at the foot keep their day, the rest are today's.
fn restamp(old: &Doc, stamps: &[Day], new: &Doc, today: Day) -> Vec<Day> {
    let (was, now) = (old.lines(), new.lines());
    let head = was.iter().zip(now).take_while(|(a, b)| a == b).count();
    let most = was.len().min(now.len()) - head;
    let tail = was.iter().rev().zip(now.iter().rev()).take(most).take_while(|(a, b)| a == b).count();
    let mut out = vec![today; now.len()];
    for line in 0..head {
        out[line] = stamps.get(line).copied().unwrap_or(0);
    }
    for back in 1..=tail {
        out[now.len() - back] = was.len().checked_sub(back).and_then(|line| stamps.get(line)).copied().unwrap_or(0);
    }
    out
}

/// True when the change is one typed character that began a word: the
/// stretch between spaces it went into had no letter or digit before. This
/// is the count behind `163 today`, so it never falls, and what is pasted,
/// which arrives as more than one character, does not add to it.
fn began_word(doc: &Doc, change: &Change) -> bool {
    let mut typed = change.inserted.chars();
    let (Some(ch), None) = (typed.next(), typed.next()) else { return false };
    if !ch.is_alphanumeric() {
        return false;
    }
    let line = doc.line(change.start.line);
    let before = line.get(..change.start.col).unwrap_or("");
    let after = line.get(change.end.col..).unwrap_or("");
    let lettered = |part: &mut dyn Iterator<Item = char>| part.take_while(|ch| !ch.is_whitespace()).any(char::is_alphanumeric);
    !lettered(&mut before.chars().rev()) && !lettered(&mut after.chars())
}

/// What a version's label reads as beside its time.
fn label_words(label: &str) -> &str {
    match label {
        "kept" => "kept",
        "mine" => "yours",
        "before-cut" => "before a cut",
        "before-reload" => "set aside",
        "outside" => "from outside",
        "recovery" => "recovered",
        "replaced" => "replaced",
        "before-mend" => "before mending",
        "before-discard" => "before discard",
        other => other,
    }
}

/// The sentence for a text whose saves are not reaching its file.
fn held_sentence(why: Hold) -> String {
    let reason = match why {
        Hold::NotUtf8 => return "Not saved. This file is not UTF-8. Leaving keeps yours beside it.".to_string(),
        Hold::Removed => return "Not saved. This file was removed. Leaving ok writes it again.".to_string(),
        Hold::NameTaken => return "Not saved. Another file has this name. Rename this text.".to_string(),
        Hold::Stopped => return "Not saved. ok has stopped writing files. The journal has it all.".to_string(),
        Hold::ReadOnly => "This file is marked read-only.",
        Hold::Linked => "This file has more than one name.",
        Hold::Busy => "Another program has this file open.",
        Hold::FolderGone => "The writing folder is not there.",
        Hold::DiskFull => "The disk is full.",
        Hold::CannotWrite => "This folder cannot be written.",
        Hold::NoHistory => "The earlier text could not be kept first.",
        _ => "The file could not be written.",
    };
    format!("Not saved. {reason} Still trying.")
}

/// The sentence for a text that opens for reading only.
fn reading_sentence(why: ReadOnly) -> &'static str {
    match why {
        ReadOnly::NotUtf8 => NOT_UTF8,
        ReadOnly::Attribute => "This file is marked read-only, so it is open for reading only.",
        ReadOnly::Busy => "Another program has this file open, so it is for reading only.",
        ReadOnly::Linked => "This file has more than one name, so it is for reading only.",
    }
}

/// The sentence for what recovery did at launch.
fn recovery_sentence(recovered: &[Recovered]) -> Option<String> {
    let done: Vec<&Recovered> = recovered.iter().filter(|r| !matches!(r.how, Recovery::Waiting)).collect();
    if done.is_empty() {
        return (!recovered.is_empty()).then(|| WAITING.to_string());
    }
    let back: usize = done.iter().map(|r| r.words).sum();
    Some(if done.iter().any(|r| matches!(r.how, Recovery::KeptBeside(_))) {
        format!("Brought back {} into a file beside the changed one.", words(back))
    } else if done.iter().any(|r| matches!(r.how, Recovery::Elsewhere(_))) {
        format!("Brought back {}. Their folder is gone, so they are in ok's own.", words(back))
    } else {
        brought_back(back)
    })
}

fn brought_back(count: usize) -> String {
    match count {
        0 => "Brought back what was typed after the last save.".to_string(),
        count => format!("Brought back {} typed after the last save.", words(count)),
    }
}

/// Why a text could not be opened or started, as a sentence.
fn open_sentence(why: &StoreError, name: &str, starting: bool) -> String {
    match why {
        StoreError::Missing => format!("? No text called {name}."),
        StoreError::TooLarge => format!("? {name} is too large to open."),
        StoreError::FolderGone => "? The writing folder is not there.".to_string(),
        StoreError::Name(_) if starting => format!("? A text called {name} could not be started."),
        _ if starting => format!("? A text called {name} could not be started just now."),
        _ => format!("? {name} could not be opened just now."),
    }
}

/// Opens a writing folder through the save layer, or says why not.
fn open_store(folder: &Path, data: &Path, host: &dyn Host) -> Result<(Store, Vec<Recovered>), String> {
    let mut setup = Setup::new(folder, data);
    setup.wake = host.waker();
    Store::open(setup).map_err(|why| match why {
        OpenError::Locked(_) => "? ok is already open on this folder in another window.".to_string(),
        OpenError::Folder(code) => format!("? The writing folder could not be opened (Windows error {code})."),
        OpenError::Data(code) => format!("? ok's own folder could not be opened (Windows error {code})."),
    })
}

/// Takes a listed text out of a listing.
fn unlist(folder: &mut ok_store::Folder, key: &str) {
    folder.texts.retain(|text| rel_key(&text.rel) != key);
    for inner in &mut folder.folders {
        unlist(inner, key);
    }
}

/// Puts a text into a listing where the alphabet has it, making the folders
/// on the way.
fn place(root: &mut ok_store::Folder, rel: &Path) {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let Some((name, folders)) = parts.split_last() else { return };
    let mut at = root;
    for part in folders {
        let lower = part.to_lowercase();
        let found = match at.folders.iter().position(|folder| folder.name.to_lowercase() == lower) {
            Some(found) => found,
            None => {
                at.folders.push(ok_store::Folder { name: part.clone(), rel: at.rel.join(part), folders: Vec::new(), texts: Vec::new() });
                at.folders.sort_by_key(|folder| folder.name.to_lowercase());
                at.folders.iter().position(|folder| folder.name.to_lowercase() == lower).unwrap_or(0)
            }
        };
        at = &mut at.folders[found];
    }
    at.texts.push(ok_store::Text { name: name.clone(), rel: rel.to_path_buf(), bytes: 0, modified_ms: 0, created_ms: 0, placeholder: false, read_only: false });
    at.texts.sort_by_key(|text| text.name.to_lowercase());
}

/// What is remembered about one text between sittings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Kept {
    rel: PathBuf,
    limit: Option<usize>,
    /// The fingerprint of the text the stamps were taken from.
    print: u64,
    /// For each line, the writing day it was last changed.
    stamps: Vec<Day>,
}

/// What is remembered about one writing folder between sittings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Memory {
    last: Option<PathBuf>,
    before: Option<PathBuf>,
    here: Option<Here>,
    /// The writing day and the words typed in it.
    typed: (Day, usize),
    texts: HashMap<String, Kept>,
    /// The words in every text when it was last read, by the key of its
    /// path, so that the contents can be shown before the folder has been
    /// read again.
    counts: HashMap<String, usize>,
}

/// The furthest a remembered moment may lie from 1970, in seconds: about
/// thirty thousand years, and far inside what the arithmetic can hold.
const FAR: i64 = 1_000_000_000_000;

/// Runs of the same day, written `day*count`.
fn pack_stamps(stamps: &[Day]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < stamps.len() {
        let run = stamps[at..].iter().take_while(|&&day| day == stamps[at]).count();
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&format!("{}*{run}", stamps[at]));
        at += run;
    }
    out
}

fn unpack_stamps(packed: &str) -> Vec<Day> {
    let mut out = Vec::new();
    for run in packed.split(',') {
        let Some((day, count)) = run.split_once('*') else { continue };
        let (Ok(day), Ok(count)) = (day.parse::<Day>(), count.parse::<usize>()) else { continue };
        // A file that claims more lines than any text has is not believed.
        if out.len().saturating_add(count) > 1_000_000 {
            break;
        }
        out.extend(std::iter::repeat_n(day, count));
    }
    out
}

impl Memory {
    /// One fact to a line, its parts set apart by tabs, which no file name
    /// can hold.
    fn write(&self) -> String {
        let mut out = String::from("ok folder 1\n");
        if let Some(last) = &self.last {
            out.push_str(&format!("last\t{}\n", last.display()));
        }
        if let Some(before) = &self.before {
            out.push_str(&format!("before\t{}\n", before.display()));
        }
        if let Some(here) = &self.here {
            out.push_str(&format!("here\t{}\t{}\n", here.at.local, here.added));
        }
        out.push_str(&format!("typed\t{}\t{}\n", self.typed.0, self.typed.1));
        let mut texts: Vec<&Kept> = self.texts.values().collect();
        texts.sort_by(|a, b| a.rel.cmp(&b.rel));
        for text in texts {
            let limit = text.limit.map_or("-".to_string(), |limit| limit.to_string());
            out.push_str(&format!("text\t{}\t{limit}\t{:016x}\t{}\n", text.rel.display(), text.print, pack_stamps(&text.stamps)));
        }
        let mut counts: Vec<(&String, &usize)> = self.counts.iter().collect();
        counts.sort();
        for (key, count) in counts {
            out.push_str(&format!("count\t{key}\t{count}\n"));
        }
        out
    }

    /// Reads what [`Memory::write`] wrote. A line that does not read is
    /// passed over, so a damaged file costs what it held and nothing more.
    fn read(text: &str) -> Memory {
        let mut memory = Memory::default();
        for line in text.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                ["last", rel] => memory.last = Some(PathBuf::from(rel)),
                ["before", rel] => memory.before = Some(PathBuf::from(rel)),
                ["here", local, added] => {
                    if let (Ok(local), Ok(added)) = (local.parse::<i64>(), added.parse()) {
                        // No moment anyone was here is further off than this.
                        memory.here = Some(Here { at: Moment { local: local.clamp(-FAR, FAR) }, added });
                    }
                }
                ["typed", day, count] => {
                    if let (Ok(day), Ok(count)) = (day.parse(), count.parse()) {
                        memory.typed = (day, count);
                    }
                }
                ["text", rel, limit, print, stamps] => {
                    let kept = Kept { rel: PathBuf::from(rel), limit: limit.parse().ok(), print: u64::from_str_radix(print, 16).unwrap_or(0), stamps: unpack_stamps(stamps) };
                    memory.texts.insert(rel_key(&kept.rel), kept);
                }
                ["count", key, count] => {
                    if let Ok(count) = count.parse() {
                        memory.counts.insert(key.to_string(), count);
                    }
                }
                _ => {}
            }
        }
        memory
    }
}

/// The name of the state file a writing folder's memory is kept in.
fn memory_name(folder: &Path) -> String {
    format!("folder-{:016x}.txt", fnv(plain(folder).to_string_lossy().to_lowercase().as_bytes()))
}

/// Another program's version of the open text, waiting to be shown.
struct Outside {
    text: String,
    read_only: Option<ReadOnly>,
    /// His own text has something in it that no file holds, so it must be
    /// kept as a version before the other one takes its place.
    mine: bool,
    /// The edit number his text was asked to be kept at, while the answer
    /// is awaited.
    keeping: Option<u64>,
    /// The version could not be kept, so a file beside the text was asked
    /// for instead.
    beside: bool,
    /// The name of that file, once it is written.
    beside_as: Option<String>,
}

/// The text on the page, as the save layer knows it.
struct Open {
    doc: DocId,
    rel: PathBuf,
    /// Its name as links spell it: folders and name, forward slashes, no ending.
    full: String,
    /// The file's name without its ending.
    leaf: String,
    /// The text itself, kept in step with the page change by change. This,
    /// and never a text handed in with a call, is what is saved.
    text: Doc,
    /// Its length in bytes, each line break one.
    len: usize,
    /// The page's own count of changes when the last one was taken in; 0
    /// for a text nothing has been changed in since it went on the page.
    revision: u64,
    /// The number of the latest edit recorded.
    edit: u64,
    /// The latest edit reported written and flushed. `None` while the file
    /// is known not to hold the text: brought back from a journal and not
    /// yet written, changed or removed by another program.
    saved: Option<u64>,
    /// The latest edit a save has been asked for, and when.
    asked: Option<u64>,
    asked_ms: u64,
    changed_ms: u64,
    unsaved_since_ms: u64,
    read_only: Option<ReadOnly>,
    /// Why saves are not reaching the file, while they are not.
    held: Option<Hold>,
    outside: Option<Outside>,
    /// Where the cursor was last known to be.
    caret: Option<Pos>,
    /// For each line, the writing day it was last changed.
    stamps: Vec<Day>,
    limit: Option<usize>,
    /// The page may be written on again and has yet to be told.
    writable_due: bool,
    /// The text has, or has had, a file: it was there to open, or a save of
    /// it has been reported.
    on_file: bool,
}

impl Open {
    fn on_disk(&self) -> bool {
        self.saved == Some(self.edit) && self.outside.is_none()
    }

    fn whole(&self) -> String {
        self.text.to_text("\n")
    }

    /// The text for the page as it stands here.
    fn opened(&self) -> Opened {
        Opened { text: self.whole(), cursor: self.caret, top: None, stamps: self.stamps.clone(), read_only: self.read_only.is_some(), say: None }
    }
}

/// A rename whose links are still mended in texts as they are read: one
/// that was not read yet when it happened, or that another program had just
/// changed.
#[derive(Clone, Debug)]
struct Mend {
    /// Each old spelling a link may carry and what it is to read instead.
    pairs: Vec<(String, String)>,
}

/// The writing folder, through the save layer.
pub struct Folder {
    /// `None` only after the session has been dropped.
    store: Option<Store>,
    /// The writing folder as a person writes it.
    shown: PathBuf,
    /// The state file this folder's memory is kept in.
    memory_name: String,
    tree: Arc<Tree>,
    listed: bool,
    /// Texts that have just been moved away and are still in the listing.
    gone: Vec<String>,
    /// Texts this session wrote and left that the listing does not show
    /// yet.
    born: Vec<PathBuf>,
    index: LinkIndex,
    /// Every listed text as it was last read, by the key of its path.
    bodies: HashMap<String, String>,
    /// Word counts by the key of each text's path.
    counts: HashMap<String, usize>,
    entries: Vec<Entry>,
    /// Every text by the name links use, in the order of the contents, and
    /// beside each its path inside the folder.
    names: Vec<String>,
    rels: Vec<PathBuf>,
    by_name: HashMap<String, usize>,
    links: Links,
    links_here: usize,
    open: Option<Open>,
    /// What has been typed while no text at all could be opened or started.
    adrift: Option<Doc>,
    adrift_tried_ms: u64,
    /// Texts put away whose last save has not been answered for yet.
    closing: Vec<(DocId, String)>,
    /// The file asked for at launch, as a path inside the folder.
    wanted: Option<PathBuf>,
    /// Something to say at the next tick.
    later: Option<String>,
    /// A tick is wanted at once: something was put off until one.
    nudge: bool,
    /// Typed text is not reaching the journal.
    alarm: bool,
    memory: Memory,
    /// The memory has changed since it was last written.
    stale: bool,
    /// When the last key of this sitting was pressed, on the host's clock.
    last_key_ms: Option<u64>,
    /// The words typed in this sitting.
    added: usize,
    /// The program has been started before, and the hint has been given.
    started: bool,
    hinted: bool,
    /// Something has been typed since launch, so the hint may follow the
    /// next pause.
    hint_due: bool,
    /// This launch made the folder's first text.
    first_run: bool,
    /// The first line has just been finished in a text nobody has named.
    name_due: bool,
    /// The open text may hold links to a name it has just lost.
    mend_own: Option<Mend>,
    mends: Vec<Mend>,
    /// The rows `versions` last listed, and the word counts already read.
    versions: Vec<Version>,
    version_words: HashMap<String, usize>,
    /// The wall clock at the last call that had a host.
    clock: LocalTime,
}

impl Folder {
    /// Opens the writing folder: the one the options name, the one a file
    /// they name lies in, the one chosen with `set folder`, or `ok` in
    /// Documents. `Err` carries the sentence that says why it could not.
    pub fn open(options: &Options, host: &mut dyn Host) -> Result<Folder, String> {
        let data = host.local_data_dir();
        let file = options.file.as_ref().map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone()));
        let named = options.folder.clone().or_else(|| file.as_ref().and_then(|file| file.parent()).filter(|parent| !parent.as_os_str().is_empty()).map(Path::to_path_buf));
        let usual = host.documents_dir().join("ok");
        let mut sorry = None;
        let (store, recovered) = match named {
            Some(folder) => open_store(&folder, &data, host)?,
            None => {
                // The settings are read before any folder is open, straight
                // from where the save layer keeps its state files.
                let settings = std::fs::read(data.join("state").join(SETTINGS)).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
                match setting(&settings, "folder").filter(|folder| !folder.is_empty()).map(PathBuf::from) {
                    Some(chosen) => match open_store(&chosen, &data, host) {
                        Ok(opened) => opened,
                        // A folder on a drive that is away must not leave
                        // him with nowhere to write.
                        Err(_) => {
                            sorry = Some("? The folder in settings could not be opened. This is Documents\\ok.".to_string());
                            open_store(&usual, &data, host)?
                        }
                    },
                    None => open_store(&usual, &data, host)?,
                }
            }
        };
        let mut folder = Folder::over(store, &recovered, host);
        match file.as_deref().map(|file| inside(&folder.shown, file).ok_or(())) {
            Some(Ok(rel)) => folder.wanted = Some(rel),
            Some(Err(())) => sorry = Some("? That file is outside the writing folder.".to_string()),
            None => {}
        }
        folder.later = sorry.or(folder.later.take());
        Ok(folder)
    }

    /// The session for a store that has just been opened.
    fn over(store: Store, recovered: &[Recovered], host: &dyn Host) -> Folder {
        let shown = plain(store.folder());
        let memory_name = memory_name(&shown);
        let memory = store.read_state(&memory_name).map(|bytes| Memory::read(&String::from_utf8_lossy(&bytes))).unwrap_or_default();
        let once = store.read_state(ONCE).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
        let not_atomic = !store.volume().atomic_replace;
        let mut folder = Folder {
            store: Some(store),
            shown,
            memory_name,
            tree: Arc::new(Tree::default()),
            listed: false,
            gone: Vec::new(),
            born: Vec::new(),
            index: LinkIndex::new(),
            bodies: HashMap::new(),
            counts: memory.counts.clone(),
            entries: Vec::new(),
            names: Vec::new(),
            rels: Vec::new(),
            by_name: HashMap::new(),
            links: Links::default(),
            links_here: 0,
            open: None,
            adrift: None,
            adrift_tried_ms: 0,
            closing: Vec::new(),
            wanted: None,
            later: recovery_sentence(recovered).or_else(|| not_atomic.then(|| NOT_ATOMIC.to_string())),
            nudge: false,
            alarm: false,
            memory,
            stale: false,
            last_key_ms: None,
            added: 0,
            started: once.lines().any(|line| line.trim() == "started"),
            hinted: once.lines().any(|line| line.trim() == "hinted"),
            hint_due: false,
            first_run: false,
            name_due: false,
            mend_own: None,
            mends: Vec::new(),
            versions: Vec::new(),
            version_words: HashMap::new(),
            clock: host.local_time(),
        };
        // A sitting carries on when he is back within the half hour.
        let now = Moment::of(&folder.clock);
        if let Some(here) = folder.memory.here.filter(|here| (0..(SITTING_MS / 1000) as i64).contains(&(now.local - here.at.local))) {
            folder.added = here.added;
        }
        let today = folder.today(host);
        if folder.memory.typed.0 != today {
            folder.memory.typed = (today, 0);
        }
        folder
    }

    /// The listing as it is to be shown: what the save layer last listed,
    /// less what has just been moved away, and with the open text in it
    /// even before it has a file, as are the texts written and left since
    /// the last listing was made.
    fn shelf(&self) -> Tree {
        let mut tree = (*self.tree).clone();
        for key in &self.gone {
            unlist(&mut tree.root, key);
        }
        for rel in self.born.iter().chain(self.open.as_ref().map(|open| &open.rel)) {
            if tree.find(rel).is_none() {
                place(&mut tree.root, rel);
            }
        }
        tree
    }

    /// Rebuilds the contents rows from the listing: in each folder its
    /// texts, then the folders inside it with theirs.
    fn list(&mut self) {
        struct Out {
            entries: Vec<Entry>,
            names: Vec<String>,
            rels: Vec<PathBuf>,
        }
        fn walk(folder: &ok_store::Folder, depth: usize, open: Option<&str>, counts: &HashMap<String, usize>, out: &mut Out) {
            for text in &folder.texts {
                let key = rel_key(&text.rel);
                let full = full_name(&text.rel);
                // A text that has not been read yet has no count to show.
                let value = counts.get(&key).map(|&count| thousands(count)).unwrap_or_default();
                out.entries.push(Entry { depth, name: leaf(&full), value, lit: open == Some(key.as_str()) });
                out.names.push(full);
                out.rels.push(text.rel.clone());
            }
            for inner in &folder.folders {
                out.entries.push(Entry { depth, name: inner.name.clone(), value: String::new(), lit: false });
                walk(inner, depth + 1, open, counts, out);
            }
        }
        let open = self.open.as_ref().map(|open| rel_key(&open.rel));
        let mut out = Out { entries: Vec::new(), names: Vec::new(), rels: Vec::new() };
        walk(&self.shelf().root, 0, open.as_deref(), &self.counts, &mut out);
        self.by_name.clear();
        for (at, name) in out.names.iter().enumerate() {
            self.by_name.entry(links::key(name)).or_insert(at);
        }
        self.entries = out.entries;
        self.names = out.names;
        self.rels = out.rels;
    }

    /// The path inside the folder of the text links call `name`.
    fn rel_of(&self, name: &str) -> Option<PathBuf> {
        self.by_name.get(&links::key(name)).and_then(|&at| self.rels.get(at)).cloned()
    }

    /// Brings what is known of the folder in step with a new listing. A text
    /// the last listing had and this one lacks has gone; one that neither
    /// has is a text of this session's own that the listing has yet to
    /// catch up with, and is left alone. Every listed text can be linked to
    /// before it has been read.
    fn reindex(&mut self, before: &Tree) {
        let now: Vec<String> = self.tree.texts().into_iter().map(|text| rel_key(&text.rel)).collect();
        // What was moved away is hidden for as long as a listing made
        // before the move still shows it.
        self.gone.retain(|key| now.contains(key));
        self.born.retain(|rel| !now.contains(&rel_key(rel)));
        let open = self.open.as_ref().map(|open| rel_key(&open.rel));
        for text in before.texts() {
            let key = rel_key(&text.rel);
            if !now.contains(&key) && open.as_ref() != Some(&key) {
                self.bodies.remove(&key);
                self.counts.remove(&key);
                self.index.remove(&full_name(&text.rel));
            }
        }
        let tree = self.tree.clone();
        for text in tree.texts() {
            let full = full_name(&text.rel);
            if !self.gone.contains(&rel_key(&text.rel)) && !self.index.contains(&full) {
                self.index.set(&full, "");
            }
        }
    }

    /// Works out the links block and the count of links that lead here.
    fn relink(&mut self) {
        let Some(open) = &self.open else {
            self.links = Links::default();
            self.links_here = 0;
            return;
        };
        let see_also = self.index.outgoing(&open.full).into_iter().map(|link| LinkRow { exists: link.resolved.is_some(), name: leaf(link.resolved.as_deref().unwrap_or(&link.target)) }).collect();
        let from: Vec<String> = self.index.incoming(&open.full).into_iter().map(str::to_string).collect();
        let (whole, short) = (links::key(&open.full), links::key(&open.leaf));
        let by_leaf = whole != short && self.index.resolve(&open.leaf).map(links::key) == Some(whole.clone());
        let mut here = 0;
        for name in &from {
            let Some(body) = self.rel_of(name).and_then(|rel| self.bodies.get(&rel_key(&rel))) else { continue };
            here += count_links(body, &whole);
            if by_leaf {
                here += count_links(body, &short);
            }
        }
        self.links_here = here;
        self.links = Links { see_also, linked_from: from.iter().map(|name| leaf(name)).collect() };
    }

    /// Notes the wall clock, and starts the count of words afresh when a
    /// new writing day has begun.
    fn tell_time(&mut self, host: &dyn Host) {
        self.clock = host.local_time();
        let today = self.today(host);
        if self.memory.typed.0 != today {
            self.memory.typed = (today, 0);
            self.stale = true;
        }
    }

    /// Writes down what is to be remembered about the folder, if any of it
    /// has changed.
    fn remember(&mut self) {
        if !self.stale {
            return;
        }
        if let Some(open) = &self.open {
            let (key, text) = (rel_key(&open.rel), open.whole());
            let kept = Kept { rel: open.rel.clone(), limit: open.limit, print: fnv(text.as_bytes()), stamps: open.stamps.clone() };
            self.memory.texts.insert(key.clone(), kept);
            self.counts.insert(key, count::words(&text));
            self.memory.last = Some(open.rel.clone());
        }
        // Counts are kept for the texts that are there; before the folder
        // has been listed nobody knows which those are.
        if self.listed {
            let rels = &self.rels;
            self.memory.counts = self.counts.iter().filter(|(key, _)| rels.iter().any(|rel| rel_key(rel) == **key)).map(|(key, count)| (key.clone(), *count)).collect();
        }
        if let Some(store) = self.store.as_mut() {
            store.write_state(&self.memory_name, self.memory.write().as_bytes());
            self.stale = false;
        }
    }

    fn write_once(&mut self) {
        let text = format!("{}{}", if self.started { "started\n" } else { "" }, if self.hinted { "hinted\n" } else { "" });
        if let Some(store) = self.store.as_mut() {
            store.write_state(ONCE, text.as_bytes());
        }
    }

    /// Asks the save layer for a text, a few times over when another
    /// program is in the middle of writing it. The second answer is whether
    /// the text was there to open; a text that had to be started has no
    /// file until it is first saved.
    fn fetch(&mut self, rel: &Path, make: bool) -> Result<(ok_store::Opened, bool), String> {
        let name = leaf(&full_name(rel));
        let store = self.store.as_mut().ok_or_else(|| NOT_HERE.to_string())?;
        let mut tries = 0;
        loop {
            match store.open_text(rel) {
                Ok(got) => return Ok((got, true)),
                // A text in a folder that is not there yet is as missing as
                // one whose folder is; starting it makes the folder.
                Err(StoreError::Missing | StoreError::FolderGone) if make => return store.create_text(rel).map(|got| (got, false)).map_err(|why| open_sentence(&why, &name, true)),
                Err(StoreError::Busy | StoreError::Timeout) if tries < OPEN_TRIES => {
                    tries += 1;
                    std::thread::sleep(OPEN_PAUSE);
                }
                Err(why) => return Err(open_sentence(&why, &name, false)),
            }
        }
    }

    /// Puts the open text away: its last text is handed to the save layer
    /// if the file does not hold it yet, with where the cursor was. A text
    /// that was started and never written leaves nothing behind.
    fn put_away(&mut self) {
        let Some(old) = self.open.take() else { return };
        let text = old.whole();
        let key = rel_key(&old.rel);
        let unsaved = old.saved != Some(old.edit) || old.outside.is_some();
        self.versions.clear();
        self.version_words.clear();
        self.name_due = false;
        self.mend_own = None;
        self.stale = true;
        if let Some(store) = self.store.as_mut() {
            store.close(old.doc, unsaved.then(|| (text.clone(), old.edit)), old.caret);
            self.closing.push((old.doc, old.leaf));
        }
        if old.on_file || unsaved {
            self.index.set(&old.full, &text);
            self.counts.insert(key.clone(), count::words(&text));
            self.memory.texts.insert(key.clone(), Kept { rel: old.rel.clone(), limit: old.limit, print: fnv(text.as_bytes()), stamps: old.stamps });
            if self.tree.find(&old.rel).is_none() {
                self.born.push(old.rel.clone());
            }
            self.memory.before = Some(old.rel);
            self.bodies.insert(key, text);
        } else {
            self.index.remove(&old.full);
            self.counts.remove(&key);
            self.bodies.remove(&key);
            self.memory.texts.remove(&key);
        }
    }

    /// Makes `got` the text on the page.
    fn settle(&mut self, rel: &Path, got: ok_store::Opened, on_file: bool, host: &dyn Host) -> Opened {
        // The name is the one the disk spells, whatever was asked for.
        let rel = self.tree.find(rel).map(|text| text.rel.clone()).unwrap_or_else(|| rel.to_path_buf());
        let full = full_name(&rel);
        let key = rel_key(&rel);
        let now = host.now_ms();
        let text = Doc::from_text(&got.text);
        let kept = self.memory.texts.get(&key);
        let mut stamps = kept.filter(|kept| kept.print == fnv(got.text.as_bytes())).map(|kept| kept.stamps.clone()).unwrap_or_default();
        stamps.resize(text.line_count(), 0);
        let mut open = Open {
            doc: got.doc,
            leaf: leaf(&full),
            full,
            len: text.len_bytes(),
            text,
            revision: 0,
            edit: got.edit,
            saved: Some(got.edit),
            asked: Some(got.edit),
            asked_ms: now,
            changed_ms: now,
            unsaved_since_ms: now,
            read_only: got.read_only,
            held: None,
            outside: None,
            caret: got.caret,
            stamps,
            limit: kept.and_then(|kept| kept.limit),
            writable_due: false,
            on_file,
            rel,
        };
        if let Some(store) = self.store.as_mut() {
            // A text brought back from a journal is not in its file yet.
            if got.restored.is_some() {
                open.saved = None;
                store.save(got.doc, got.text.clone(), got.edit);
            }
        }
        self.gone.retain(|gone| *gone != key);
        self.born.retain(|born| rel_key(born) != key);
        self.index.set(&open.full, &got.text);
        // From here on the text is known from the copy kept with it.
        self.bodies.remove(&key);
        self.counts.insert(key, count::words(&got.text));
        let opened = Opened {
            say: match (got.restored, got.read_only) {
                (Some(back), _) => Some(brought_back(back)),
                (None, Some(why)) => Some(reading_sentence(why).to_string()),
                (None, None) => None,
            },
            ..open.opened()
        };
        self.open = Some(open);
        self.memory.last = self.open.as_ref().map(|open| open.rel.clone());
        self.stale = true;
        self.list();
        self.relink();
        self.remember();
        opened
    }

    /// Puts another text on the page. The new one is opened before the old
    /// one is put away, so a text that will not open leaves the page, and
    /// its journal, exactly as they were. `Ok(None)` when the text asked for
    /// is the one already there.
    fn switch(&mut self, rel: &Path, make: bool, host: &dyn Host) -> Result<Option<Opened>, String> {
        if self.open.as_ref().is_some_and(|open| rel_key(&open.rel) == rel_key(rel)) {
            return Ok(None);
        }
        let (got, on_file) = self.fetch(rel, make)?;
        self.put_away();
        Ok(Some(self.settle(rel, got, on_file, host)))
    }

    /// The path for a text that does not exist yet. A name with a slash or
    /// a backslash in it says which folder; one without goes beside the open
    /// text when a link asks for it, and into the writing folder itself when
    /// a title was typed on the line.
    fn rel_for_new(&self, title: &str, beside: bool, ending: &str) -> PathBuf {
        let mut parts: Vec<String> = title.split(['/', '\\']).filter(|part| !part.trim().is_empty()).map(clean_part).collect();
        // An ending typed with the title is not part of the name.
        if let Some(last) = parts.last_mut() {
            if let Some((stem, typed)) = last.rsplit_once('.') {
                if ok_store::TEXT_ENDINGS.iter().any(|known| typed.eq_ignore_ascii_case(known)) && !stem.trim().is_empty() {
                    *last = clean_part(stem);
                }
            }
        }
        let mut rel = match (&self.open, parts.len()) {
            (Some(open), 0 | 1) if beside => open.rel.parent().map(Path::to_path_buf).unwrap_or_default(),
            _ => PathBuf::new(),
        };
        if parts.is_empty() {
            parts.push("Untitled".to_string());
        }
        let last = parts.len() - 1;
        for (at, part) in parts.iter().enumerate() {
            rel.push(if at == last { format!("{part}.{ending}") } else { part.clone() });
        }
        rel
    }

    /// The text to show when a folder has just been opened: the file asked
    /// for, the text he was last in, the first in the contents, or a new
    /// empty one.
    fn begin(&mut self, host: &dyn Host) -> Opened {
        let mut tries: Vec<(PathBuf, bool, bool)> = Vec::new();
        if let Some(wanted) = self.wanted.take() {
            tries.push((wanted, true, true));
        }
        if let Some(last) = self.memory.last.clone() {
            tries.push((last, false, false));
        }
        if let Some(first) = self.rels.first() {
            tries.push((first.clone(), false, true));
        }
        for (rel, make, tell) in tries {
            match self.switch(&rel, make, host) {
                Ok(Some(opened)) => return opened,
                Ok(None) => {}
                Err(why) if tell => self.later = self.later.take().or(Some(why)),
                Err(_) => {}
            }
        }
        // Nothing to open: an empty text under the first free name.
        let nothing_here = self.listed && self.rels.is_empty();
        for n in 1..50 {
            let name = if n == 1 { "Untitled.md".to_string() } else { format!("Untitled {n}.md") };
            let Some(store) = self.store.as_mut() else { break };
            match store.create_text(&name) {
                Ok(got) => {
                    let (doc, edit) = (got.doc, got.edit);
                    let opened = self.settle(Path::new(&name), got, false, host);
                    if nothing_here {
                        // The folder is made with one empty text in it, so
                        // the file is asked for before a word is typed.
                        self.first_run = !self.started;
                        if let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_mut()) {
                            store.save(doc, String::new(), edit);
                            open.saved = None;
                        }
                    }
                    return opened;
                }
                Err(StoreError::Name(_)) => continue,
                Err(_) => break,
            }
        }
        // Not even that. What he types is kept here until a text can be
        // started for it, and the screen says so for as long as it is true.
        self.adrift = Some(Doc::new());
        self.adrift_tried_ms = host.now_ms();
        Opened { say: Some(held_sentence(Hold::CannotWrite)), ..Opened::default() }
    }

    /// Waits a moment for the folder's first listing and then opens the
    /// first text, with what there is to say about coming back.
    fn arrive(&mut self, host: &mut dyn Host) -> Opened {
        self.tell_time(host);
        // The first listing arrives from the saver thread within a few
        // milliseconds; wait for it once so the last text can be found.
        let started = Instant::now();
        let mut news = News::default();
        while !self.listed && started.elapsed() < FIRST_LISTING {
            self.drain(false, host, &mut news);
            if !self.listed {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.drain(false, host, &mut news);
        let mut opened = self.begin(host);
        // What must be known comes first: words brought back, a file that
        // cannot be written. Then the first run's one sentence, or when he
        // was last here.
        let greeting = if self.first_run { Some(START.to_string()) } else { self.memory.here.map(|here| here.sentence(Moment::of(&self.clock))) };
        opened.say = self.later.take().or(opened.say.take()).or(news.say).or(greeting);
        if !self.started {
            self.started = true;
            self.write_once();
        }
        opened
    }

    /// Leaves this writing folder for another, opening `wanted` there if a
    /// file is named. With `persist` the choice is written into the
    /// settings, so that the next launch starts there too.
    fn relocate(&mut self, folder: &Path, wanted: Option<PathBuf>, persist: bool, host: &mut dyn Host) -> Result<Opened, String> {
        let same = std::fs::canonicalize(folder).is_ok_and(|real| plain(&real).to_string_lossy().to_lowercase() == self.shown.to_string_lossy().to_lowercase());
        if same {
            return Err("? That is the writing folder already.".to_string());
        }
        // The new folder is opened first, so one that will not open leaves
        // everything here as it was.
        let (store, recovered) = open_store(folder, &host.local_data_dir(), host)?;
        self.put_away();
        self.remember();
        if persist {
            let settings = with_setting(&self.settings().unwrap_or_default(), "folder", &plain(store.folder()).to_string_lossy());
            self.write_settings(&settings);
        }
        if let Some(old) = self.store.take() {
            old.shutdown(Duration::from_secs(3));
        }
        let mut next = Folder::over(store, &recovered, host);
        next.wanted = wanted;
        next.hint_due = self.hint_due;
        *self = next;
        Ok(self.arrive(host))
    }

    /// Makes sure the copy kept here reads as the page does, when the text
    /// handed in is known to be the page of the open text. The page is what
    /// he sees, so if the two ever differ the copy and the journal are
    /// brought to it as one change.
    fn verify(&mut self, doc: &Doc, host: &dyn Host) {
        let (Some(store), Some(open)) = (self.store.as_mut(), self.open.as_mut()) else { return };
        if open.revision == 0 || doc.revision() != open.revision || doc.lines() == open.text.lines() {
            return;
        }
        let whole = Span::new(Pos::default(), open.text.end());
        let change = open.text.replace(whole, &doc.to_text("\n"));
        open.len = open.text.len_bytes();
        open.edit = store.record(open.doc, &change, open.len);
        open.stamps.resize(open.text.line_count(), 0);
        open.changed_ms = host.now_ms();
    }

    /// Puts another program's version on the page in place of his, which by
    /// now is in a file or a version of its own.
    fn take_theirs(&mut self, host: &dyn Host, news: &mut News) {
        let today = self.today(host);
        let (Some(store), Some(open)) = (self.store.as_mut(), self.open.as_mut()) else { return };
        let Some(theirs) = open.outside.take() else { return };
        let fresh = Doc::from_text(&theirs.text);
        let stamps = restamp(&open.text, &open.stamps, &fresh, today);
        let whole = Span::new(Pos::default(), open.text.end());
        let change = open.text.replace(whole, &theirs.text);
        open.len = open.text.len_bytes();
        open.edit = store.record(open.doc, &change, open.len);
        // Saving their text is how the save layer learns it has been taken.
        store.save(open.doc, open.whole(), open.edit);
        open.asked = Some(open.edit);
        open.asked_ms = host.now_ms();
        let now_writable = open.read_only.is_some() && theirs.read_only.is_none();
        open.read_only = theirs.read_only;
        if change.is_noop() && !now_writable {
            return;
        }
        open.revision = 0;
        open.stamps = stamps;
        open.caret = open.caret.map(|caret| open.text.clamp(caret));
        news.replaced = Some(open.opened());
        news.say = Some(match theirs.beside_as {
            Some(name) => format!("This text changed on disk. Yours is beside it as {name}."),
            None => NEWER.to_string(),
        });
        let (full, key, text) = (open.full.clone(), rel_key(&open.rel), open.whole());
        self.index.set(&full, &text);
        self.counts.insert(key, count::words(&text));
        self.relink();
    }

    /// Does the next thing a waiting outside version needs: shows it at
    /// once when nothing of his is unsaved, and otherwise asks for his own
    /// text to be kept first.
    fn settle_outside(&mut self, host: &dyn Host, news: &mut News) {
        let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_mut()) else { return };
        let Some(theirs) = open.outside.as_mut() else { return };
        if theirs.read_only == Some(ReadOnly::NotUtf8) {
            if theirs.mine {
                // It can no longer be written, and his text is not in it:
                // his stays on the page and goes beside the file at the end.
                open.outside = None;
                open.held = Some(Hold::NotUtf8);
                news.say = Some(held_sentence(Hold::NotUtf8));
            } else {
                // Shown as best it can be, and never written.
                let lossy = std::mem::take(&mut theirs.text);
                open.outside = None;
                open.text = Doc::from_text(&lossy);
                open.len = open.text.len_bytes();
                open.revision = 0;
                open.read_only = Some(ReadOnly::NotUtf8);
                open.saved = Some(open.edit);
                open.asked = Some(open.edit);
                open.stamps = vec![0; open.text.line_count()];
                open.caret = open.caret.map(|caret| open.text.clamp(caret));
                news.replaced = Some(open.opened());
                news.say = Some(NOT_UTF8.to_string());
            }
            return;
        }
        if !theirs.mine {
            self.take_theirs(host, news);
        } else if theirs.keeping.is_none() {
            theirs.keeping = Some(open.edit);
            if theirs.beside {
                store.resolve(open.doc, Resolve::KeepBoth { text: open.whole(), edit: open.edit });
            } else {
                store.keep(open.doc, open.whole(), open.edit, "mine");
            }
        }
    }

    /// Takes in everything the save layer has to tell. With `page` unset
    /// nothing may be put on the page from here, so what would be is left
    /// for the next tick.
    fn drain(&mut self, page: bool, host: &mut dyn Host, news: &mut News) {
        let mut relist = false;
        loop {
            let Some(item) = self.store.as_mut().and_then(Store::poll) else { break };
            relist |= self.hear(item, host, news);
        }
        if relist {
            self.list();
            self.relink();
        }
        if page {
            self.settle_outside(host, news);
            if let Some(open) = self.open.as_mut().filter(|open| open.writable_due) {
                open.writable_due = false;
                news.replaced = news.replaced.take().or_else(|| Some(open.opened()));
            }
        } else if self.open.as_ref().is_some_and(|open| open.outside.is_some() || open.writable_due) {
            self.nudge = true;
        }
    }

    /// Takes in one piece of news. True when the contents must be listed
    /// again.
    fn hear(&mut self, item: StoreNews, host: &mut dyn Host, news: &mut News) -> bool {
        let mine = |open: &Option<Open>, doc: DocId| open.as_ref().is_some_and(|open| open.doc == doc);
        match item {
            StoreNews::Saved { doc, edit, .. } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                if open.saved.is_none_or(|saved| edit >= saved) {
                    open.saved = Some(edit);
                }
                open.on_file = true;
                if open.held.take().is_some() {
                    news.say = Some(SAVED_AGAIN.to_string());
                }
            }
            StoreNews::Kept { doc, edit, bytes, .. } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                match open.outside.as_mut() {
                    // His text is kept: the other one may take its place,
                    // unless he has typed since, and then it is kept again.
                    Some(theirs) if theirs.keeping == Some(edit) => {
                        theirs.keeping = None;
                        theirs.mine = open.edit != edit;
                    }
                    Some(_) => {}
                    None => {
                        let now = host.local_time();
                        news.say = Some(format!("Kept {:02}:{:02}. {} bytes on disk.", now.hour, now.minute, thousands(bytes as usize)));
                    }
                }
            }
            StoreNews::NotKept { doc, edit } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                match open.outside.as_mut() {
                    // No version could be written: a file beside the text
                    // is asked for instead.
                    Some(theirs) if theirs.keeping == Some(edit) => {
                        theirs.keeping = None;
                        theirs.beside = true;
                    }
                    Some(_) => {}
                    None => news.say = Some(NOT_KEPT.to_string()),
                }
            }
            StoreNews::KeptBeside { doc, rel } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                if let Some(theirs) = open.outside.as_mut().filter(|theirs| theirs.beside) {
                    theirs.mine = theirs.keeping != Some(open.edit);
                    theirs.keeping = None;
                    theirs.beside_as = Some(leaf(&full_name(&rel)));
                }
            }
            StoreNews::Held { doc, why } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                match (why, open.outside.as_mut()) {
                    // Not even a file beside it could be written. His text
                    // stays on the page; the other version is the one that
                    // goes into the history.
                    (Hold::CannotWrite, Some(theirs)) if theirs.beside => {
                        open.outside = None;
                        if let Some(store) = self.store.as_ref() {
                            store.resolve(doc, Resolve::KeepMine);
                        }
                        open.asked = None;
                        news.say = Some("This text changed on disk too. Yours stays on the page.".to_string());
                    }
                    // Said while another program's version waits: that is
                    // being dealt with, and is not a write that failed.
                    (Hold::ChangedOutside, _) => {}
                    // The text handed over was not the text at its edit
                    // number. It is asked for again as it stands.
                    (Hold::WrongEdit, _) => {
                        open.asked = None;
                        open.changed_ms = host.now_ms();
                    }
                    (Hold::NameTaken, _) => {
                        if !self.step_aside() {
                            self.hold(Hold::NameTaken, news);
                        }
                        return true;
                    }
                    (why, _) => self.hold(why, news),
                }
            }
            StoreNews::Released { doc } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                // A file another program had open, or that was marked
                // read-only, can be written again.
                if matches!(open.read_only, Some(ReadOnly::Busy | ReadOnly::Attribute)) {
                    open.read_only = None;
                    open.writable_due = true;
                }
                if open.saved == Some(open.edit) && open.held.take().is_some() {
                    news.say = Some(SAVED_AGAIN.to_string());
                }
            }
            StoreNews::ChangedOutside { doc, text, clean, edit, read_only, .. } => {
                let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) else { return false };
                let mine = !(clean && edit == open.edit);
                open.outside = Some(Outside { text, read_only, mine, keeping: None, beside: false, beside_as: None });
            }
            StoreNews::Removed { doc } => {
                if let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) {
                    open.saved = None;
                    news.say = Some(REMOVED.to_string());
                }
            }
            StoreNews::Reappeared { doc } => {
                // The file is back. Whether it holds the text is found out
                // by asking for it again.
                if let Some(open) = self.open.as_mut().filter(|open| open.doc == doc) {
                    open.held = None;
                    open.asked = None;
                }
            }
            StoreNews::Moved { doc, rel } => {
                if mine(&self.open, doc) {
                    self.renamed(rel);
                    return true;
                }
            }
            StoreNews::Closed { doc, beside, in_journal } => {
                let Some(at) = self.closing.iter().position(|(closed, _)| *closed == doc) else { return false };
                let (_, name) = self.closing.remove(at);
                if let Some(beside) = beside {
                    self.later = Some(format!("{name} could not take your last words. They are in {}.", leaf(&full_name(&beside))));
                } else if in_journal {
                    self.later = Some(format!("{name} is not in its file yet. The journal has its words."));
                }
                self.nudge |= self.later.is_some();
            }
            StoreNews::Tree(tree) => {
                let before = std::mem::replace(&mut self.tree, tree);
                self.listed = true;
                self.reindex(&before);
                return true;
            }
            StoreNews::FileText { rel, text } => {
                let key = rel_key(&rel);
                // The open text is known from the page, which is newer.
                if self.open.as_ref().is_some_and(|open| rel_key(&open.rel) == key) {
                    return false;
                }
                let text = self.mend_late(&rel, text);
                self.index.set(&full_name(&rel), &text);
                self.counts.insert(key.clone(), count::words(&text));
                self.bodies.insert(key, text);
                return true;
            }
            StoreNews::JournalAlarm(up) => {
                news.say = Some(if up { JOURNAL_DOWN } else { SAVED_AGAIN }.to_string());
                self.alarm = up;
            }
            StoreNews::FolderMoved { folder } => {
                // The memory goes with the folder to its new name.
                self.shown = plain(&folder);
                self.memory_name = memory_name(&self.shown);
                self.stale = true;
            }
            StoreNews::NotAtomic => news.say = Some(NOT_ATOMIC.to_string()),
        }
        false
    }

    /// Notes that saves are not reaching the file, and says why.
    fn hold(&mut self, why: Hold, news: &mut News) {
        if let Some(open) = self.open.as_mut() {
            open.held = Some(why);
            news.say = Some(held_sentence(why));
        }
    }

    /// A new text's name was taken by another file before its first save:
    /// the text moves to the first free name beside it, and the save layer
    /// tries again there. False when no name could be had.
    fn step_aside(&mut self) -> bool {
        let Some(rel) = self.open.as_ref().map(|open| open.rel.clone()) else { return false };
        let stem = rel.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let ending = ending_of(&rel);
        for n in 2..10 {
            let to = rel.with_file_name(format!("{stem} {n}.{ending}"));
            let moved = self.store.as_ref().is_some_and(|store| store.rename(&rel, &to).is_ok());
            if moved {
                self.renamed(to);
                return true;
            }
        }
        false
    }

    /// The open text has a new path: everything kept by the old one moves.
    fn renamed(&mut self, rel: PathBuf) {
        let Some(open) = self.open.as_mut() else { return };
        let (old_key, old_full) = (rel_key(&open.rel), open.full.clone());
        open.full = full_name(&rel);
        open.leaf = leaf(&open.full);
        open.rel = rel;
        let (key, full) = (rel_key(&open.rel), open.full.clone());
        self.index.rename(&old_full, &full);
        if let Some(count) = self.counts.remove(&old_key) {
            self.counts.insert(key.clone(), count);
        }
        self.bodies.remove(&old_key);
        self.memory.texts.remove(&old_key);
        if old_key != key {
            self.gone.push(old_key);
        }
        self.gone.retain(|gone| *gone != key);
        self.stale = true;
        self.remember();
    }

    /// Mends the links to a name in a text that has only now been read, and
    /// hands back the text as it then stands.
    fn mend_late(&mut self, rel: &Path, text: String) -> String {
        let mut mended = text.clone();
        for mend in &self.mends {
            for (old, new) in &mend.pairs {
                // A name that leads somewhere again is linked to on purpose.
                if self.index.resolve(old).is_none() {
                    if let Some(next) = retarget(&mended, old, new) {
                        mended = next;
                    }
                }
            }
        }
        if mended == text {
            return text;
        }
        match self.store.as_ref().map(|store| store.rewrite(rel, &text, &mended)) {
            Some(Ok(())) => mended,
            _ => text,
        }
    }

    /// Gives the open text a new path and mends every link to its old name
    /// in the other texts. Hands back how many links were mended.
    fn rename_to(&mut self, to: &Path) -> Result<usize, String> {
        let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_ref()) else { return Err(NOT_HERE.to_string()) };
        let new_leaf = leaf(&full_name(to));
        if open.rel == to {
            return Err(format!("? This text is already called {new_leaf}."));
        }
        store.rename(&open.rel, to).map_err(|why| match why {
            StoreError::Name(ok_store::NameError::Taken) => format!("? A text called {new_leaf} is already there."),
            StoreError::Name(_) => "? That name cannot be used for a file.".to_string(),
            StoreError::Busy => "? Another program has this file open. Try again in a moment.".to_string(),
            StoreError::Missing => "? This file is not there to rename.".to_string(),
            StoreError::FolderGone => "? The writing folder is not there.".to_string(),
            _ => "? This text could not be renamed just now.".to_string(),
        })?;
        let (old_full, old_leaf) = (open.full.clone(), open.leaf.clone());
        let new_full = full_name(to);
        // A link may name the text in full or by its last part alone, when
        // that part led here.
        let by_leaf = links::key(&old_full) != links::key(&old_leaf) && self.index.resolve(&old_leaf).map(links::key) == Some(links::key(&old_full));
        self.renamed(to.to_path_buf());
        let short = if self.index.resolve(&new_leaf).map(links::key) == Some(links::key(&new_full)) { new_leaf.clone() } else { new_full.clone() };
        let mut pairs = vec![(old_full, new_full)];
        if by_leaf {
            pairs.push((old_leaf, short));
        }
        let mut mended = 0;
        let mut failed = None;
        let own = self.open.as_ref().map(|open| rel_key(&open.rel));
        let others: Vec<(String, PathBuf)> = self.rels.iter().map(|rel| (rel_key(rel), rel.clone())).filter(|(key, _)| self.bodies.contains_key(key) && own.as_ref() != Some(key)).collect();
        for (key, rel) in others {
            let Some(body) = self.bodies.get(&key) else { continue };
            let mut text = body.clone();
            let mut found = 0;
            for (old, new) in &pairs {
                let here = count_links(&text, &links::key(old));
                if here == 0 {
                    continue;
                }
                if let Some(next) = retarget(&text, old, new) {
                    text = next;
                    found += here;
                }
            }
            if text == *body {
                continue;
            }
            match self.store.as_ref().map(|store| store.rewrite(&rel, body, &text)) {
                Some(Ok(())) => {
                    mended += found;
                    self.index.set(&full_name(&rel), &text);
                    self.counts.insert(key.clone(), count::words(&text));
                    self.bodies.insert(key, text);
                }
                // Another program has just changed it: it is mended when
                // its new contents are read.
                Some(Err(StoreError::Changed)) => {}
                _ => failed = Some(leaf(&full_name(&rel))),
            }
        }
        if let Some(name) = failed {
            self.later = Some(format!("? The links in {name} could not be mended. It cannot be written."));
        }
        let mend = Mend { pairs };
        self.mend_own = Some(mend.clone());
        self.mends.push(mend);
        if self.mends.len() > MOST_MENDS {
            self.mends.remove(0);
        }
        self.nudge = true;
        self.list();
        self.relink();
        Ok(mended)
    }

    /// Mends the links the open text holds to a name it has just lost. The
    /// page is handed the mended text whole, since the session is given no
    /// way to change a stretch of it.
    fn mend_open(&mut self, host: &dyn Host, news: &mut News) {
        let Some(mend) = self.mend_own.take() else { return };
        let today = self.today(host);
        let (Some(store), Some(open)) = (self.store.as_mut(), self.open.as_mut()) else { return };
        if open.read_only.is_some() || open.outside.is_some() {
            return;
        }
        let was = open.whole();
        let mut text = was.clone();
        for (old, new) in &mend.pairs {
            if let Some(next) = retarget(&text, old, new) {
                text = next;
            }
        }
        if text == was {
            return;
        }
        let fresh = Doc::from_text(&text);
        open.stamps = restamp(&open.text, &open.stamps, &fresh, today);
        let whole = Span::new(Pos::default(), open.text.end());
        let change = open.text.replace(whole, &text);
        open.len = open.text.len_bytes();
        if open.saved == Some(open.edit) {
            open.unsaved_since_ms = host.now_ms();
        }
        open.edit = store.record(open.doc, &change, open.len);
        open.changed_ms = host.now_ms();
        open.revision = 0;
        open.caret = open.caret.map(|caret| open.text.clamp(caret));
        news.replaced = Some(open.opened());
        let full = open.full.clone();
        self.index.set(&full, &text);
        self.relink();
    }

    /// Names a text nobody has named after its first line, now that the
    /// line is finished.
    fn name_from_first_line(&mut self) {
        let Some(open) = self.open.as_ref().filter(|open| is_untitled(&open.leaf) && open.read_only.is_none()) else { return };
        let Some(title) = title_of(open.text.line(0)) else { return };
        let ending = ending_of(&open.rel);
        let rel = open.rel.clone();
        for n in 1..10 {
            let name = if n == 1 { format!("{title}.{ending}") } else { format!("{title} {n}.{ending}") };
            match self.rename_to(&rel.with_file_name(name)) {
                Err(why) if why.contains("already there") => continue,
                _ => break,
            }
        }
        // Naming a text is not something to be told about.
        if self.later.as_ref().is_some_and(|later| later.starts_with("? The links")) {
            self.later = None;
        }
    }

    /// Starts a text for what was typed while none could be opened.
    fn come_ashore(&mut self, doc: &Doc, host: &dyn Host) {
        let now = host.now_ms();
        if self.open.is_some() || self.adrift.is_none() || now.saturating_sub(self.adrift_tried_ms) < PAUSE_MS {
            return;
        }
        self.adrift_tried_ms = now;
        for n in 1..50 {
            let name = if n == 1 { "Untitled.md".to_string() } else { format!("Untitled {n}.md") };
            let Some(store) = self.store.as_mut() else { return };
            match store.create_text(&name) {
                Ok(got) => {
                    self.settle(Path::new(&name), got, false, host);
                    break;
                }
                Err(StoreError::Name(_)) => continue,
                Err(_) => return,
            }
        }
        let Some(typed) = self.adrift.take() else { return };
        let (Some(store), Some(open)) = (self.store.as_mut(), self.open.as_mut()) else {
            self.adrift = Some(typed);
            return;
        };
        let change = open.text.replace(Span::at(Pos::default()), &typed.to_text("\n"));
        open.len = open.text.len_bytes();
        open.edit = store.record(open.doc, &change, open.len);
        open.stamps = vec![0; open.text.line_count()];
        open.revision = if doc.lines() == open.text.lines() { doc.revision() } else { 0 };
        open.changed_ms = now.saturating_sub(PAUSE_MS);
        open.unsaved_since_ms = now;
        open.asked = None;
    }
}

impl Session for Folder {
    fn first(&mut self, host: &mut dyn Host) -> Opened {
        self.arrive(host)
    }

    fn open(&mut self, path: &Path, _doc: &Doc, host: &mut dyn Host) -> Option<Opened> {
        self.tell_time(host);
        let result = match inside(&self.shown, path) {
            Some(rel) if ok_store::is_text_name(&rel.to_string_lossy()) => self.switch(&rel, true, host),
            Some(_) => Err("? That file is not a text.".to_string()),
            // A file somewhere else is opened where it lies: its folder is
            // the writing folder until the program is started again.
            None => match (path.parent(), path.file_name()) {
                (Some(folder), Some(name)) if ok_store::is_text_name(&name.to_string_lossy()) => self.relocate(folder, Some(PathBuf::from(name)), false, host).map(Some),
                _ => Err("? That file is not a text.".to_string()),
            },
        };
        match result {
            Ok(opened) => opened,
            Err(why) => {
                self.later = Some(why);
                self.nudge = true;
                None
            }
        }
    }

    fn changed(&mut self, doc: &Doc, change: &Change, host: &mut dyn Host) {
        let now = host.now_ms();
        self.tell_time(host);
        let today = self.today(host);
        let Some(open) = self.open.as_mut() else {
            // No text could be opened or started: what he types is kept
            // until one can be.
            if let Some(typed) = self.adrift.as_mut() {
                typed.replace(Span { start: change.start, end: change.old_end }, &change.inserted);
            }
            return;
        };
        let Some(store) = self.store.as_mut() else { return };
        // A change that arrives alone is typing; an undo, a redo or a
        // change made everywhere at once arrives several at a time.
        let alone = doc.revision() == open.revision + 1;
        let applied = open.text.replace(Span { start: change.start, end: change.old_end }, &change.inserted);
        if applied.is_noop() {
            open.revision = doc.revision();
            return;
        }
        open.len = open.len + applied.inserted.len() - applied.removed.len().min(open.len + applied.inserted.len());
        if open.saved == Some(open.edit) {
            open.unsaved_since_ms = now;
        }
        open.edit = store.record(open.doc, &applied, open.len);
        store.caret(open.doc, applied.end);
        open.caret = Some(applied.end);
        open.changed_ms = now;
        open.revision = doc.revision();
        stamp(&mut open.stamps, &open.text, &applied, today);

        // The sitting and the day's count.
        if self.last_key_ms.is_some_and(|last| now.saturating_sub(last) >= SITTING_MS) {
            self.added = 0;
        }
        self.last_key_ms = Some(now);
        if alone && began_word(&open.text, &applied) {
            self.memory.typed.1 = self.memory.typed.1.saturating_add(1);
            self.added = self.added.saturating_add(1);
        }
        self.memory.here = Some(Here { at: Moment::of(&self.clock), added: self.added });
        self.stale = true;
        self.hint_due = !self.hinted;

        // The first line he finishes names a text nobody has named.
        if applied.start.line == 0 && applied.inserted.contains('\n') && is_untitled(&open.leaf) {
            self.name_due = true;
            self.nudge = true;
        }
        if applied.inserted.contains(['[', ']']) || applied.removed.contains(['[', ']']) {
            let (full, text) = (open.full.clone(), open.whole());
            self.index.set(&full, &text);
            self.relink();
        }
    }

    fn on_disk(&self) -> bool {
        self.open.as_ref().is_some_and(Open::on_disk)
    }

    fn failing(&self) -> bool {
        self.alarm || self.open.as_ref().is_none_or(|open| open.held.is_some())
    }

    fn name(&self) -> &str {
        self.open.as_ref().map_or("", |open| open.leaf.as_str())
    }

    fn contents(&self) -> &[Entry] {
        &self.entries
    }

    fn links(&self) -> Option<&Links> {
        Some(&self.links)
    }

    fn exists(&self, target: &str) -> bool {
        self.index.resolve(target).is_some()
    }

    fn words_today(&self) -> usize {
        self.memory.typed.1
    }

    fn keep(&mut self, doc: &Doc, host: &mut dyn Host) -> Option<String> {
        self.tell_time(host);
        self.verify(doc, host);
        let now = host.now_ms();
        let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_mut()) else { return Some(NOT_HERE.to_string()) };
        if open.read_only.is_some() {
            return Some(READING_ONLY.to_string());
        }
        store.keep(open.doc, open.whole(), open.edit, "kept");
        open.asked = Some(open.edit);
        open.asked_ms = now;
        self.remember();
        // The answer, with the byte count read back from disk, comes at a tick.
        None
    }

    fn follow(&mut self, doc: &Doc, at: Pos, host: &mut dyn Host) -> Option<Opened> {
        self.tell_time(host);
        self.verify(doc, host);
        // The line is read from the page, where the cursor is.
        let line = doc.line(at.line);
        let Some(link) = link_at(line, at.col) else {
            // Not a link between texts: an address under the cursor opens in the browser.
            let address = pieces(line, classify_line(line, false)).into_iter().find(|piece| piece.style == Style::Url && piece.start <= at.col && at.col <= piece.end);
            if let Some(piece) = address {
                host.open_in_browser(&line[piece.start..piece.end]);
            }
            return None;
        };
        if let Some(open) = self.open.as_mut() {
            open.caret = Some(at);
        }
        let (target, heading) = (link.target.to_string(), link.heading.map(str::to_string));
        let rel = match self.index.resolve(&target).map(str::to_string).and_then(|name| self.rel_of(&name)) {
            Some(rel) => rel,
            None => self.rel_for_new(&target, true, "md"),
        };
        match self.switch(&rel, true, host) {
            Ok(Some(mut opened)) => {
                // A link to a heading lands on it.
                if let Some(heading) = heading {
                    let wanted = links::key(&heading);
                    let found = opened.text.split('\n').position(|line| matches!(classify_line(line, false), LineKind::Heading(_)) && links::key(line.trim_start_matches('#')) == wanted);
                    opened.cursor = found.map(|line| Pos::new(line, 0)).or(opened.cursor);
                }
                Some(opened)
            }
            Ok(None) => None,
            Err(why) => {
                self.later = Some(why);
                self.nudge = true;
                None
            }
        }
    }

    fn tick(&mut self, doc: &Doc, host: &mut dyn Host) -> News {
        self.nudge = false;
        self.tell_time(host);
        self.verify(doc, host);
        let mut news = News::default();
        self.drain(true, host, &mut news);
        self.come_ashore(doc, host);
        if self.name_due {
            self.name_due = false;
            self.name_from_first_line();
        }
        if news.replaced.is_none() {
            self.mend_open(host, &mut news);
        }
        let now = host.now_ms();
        let mut paused = false;
        if let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_mut()) {
            paused = now.saturating_sub(open.changed_ms) >= PAUSE_MS;
            let long = now.saturating_sub(open.unsaved_since_ms.max(open.asked_ms)) >= LONGEST_MS;
            if open.saved != Some(open.edit) && open.asked != Some(open.edit) && open.outside.is_none() && (paused || long) {
                store.save(open.doc, open.whole(), open.edit);
                open.asked = Some(open.edit);
                open.asked_ms = now;
            }
        }
        if paused {
            self.remember();
        }
        if news.say.is_none() {
            news.say = self.later.take();
        }
        // Once ever, after his first pause.
        if self.hint_due && !self.hinted && paused && news.say.is_none() {
            self.hinted = true;
            self.hint_due = false;
            self.write_once();
            news.say = Some(HINT.to_string());
        }
        news
    }

    fn wake_in_ms(&self, host: &dyn Host) -> Option<u64> {
        if self.later.is_some() || self.nudge {
            return Some(1);
        }
        let now = host.now_ms();
        let mut wake: Option<u64> = None;
        let mut sooner = |ms: u64| wake = Some(wake.map_or(ms, |so_far| so_far.min(ms)).max(1));
        if let Some(open) = &self.open {
            let pause = open.changed_ms.saturating_add(PAUSE_MS).saturating_sub(now);
            if open.saved != Some(open.edit) && open.asked != Some(open.edit) && open.outside.is_none() {
                let longest = open.unsaved_since_ms.max(open.asked_ms).saturating_add(LONGEST_MS).saturating_sub(now);
                sooner(pause.min(longest));
            }
            // The memory is written, and the hint given, at the pause too.
            if (self.stale || (self.hint_due && !self.hinted)) && pause > 0 {
                sooner(pause);
            }
        } else if self.adrift.is_some() {
            sooner(self.adrift_tried_ms.saturating_add(PAUSE_MS).saturating_sub(now));
        }
        // The count of words typed today goes at 04:00.
        if self.memory.typed.1 > 0 {
            let of_day = (Moment::of(&host.local_time()).local - 4 * 3_600).rem_euclid(86_400);
            sooner((86_400 - of_day) as u64 * 1_000);
        }
        // The save layer asks to be looked at while an edit is still on its
        // way to the journal file.
        if self.store.as_ref().is_some_and(Store::unwritten) {
            sooner(200);
        }
        wake
    }

    /// The window may be going. The journal is flushed, which alone makes
    /// every word safe, and the file is given a moment to be written too.
    /// The store keeps running, because a close can be called off; if it
    /// is, everything here carries on as it was.
    fn close(&mut self, doc: &Doc, host: &mut dyn Host) {
        self.tell_time(host);
        self.verify(doc, host);
        let now = host.now_ms();
        if let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_mut()) {
            if open.saved != Some(open.edit) && open.outside.is_none() {
                store.save(open.doc, open.whole(), open.edit);
                open.asked = Some(open.edit);
                open.asked_ms = now;
            }
            store.flush();
        }
        self.stale = true;
        self.remember();
        let started = Instant::now();
        let mut news = News::default();
        while self.open.as_ref().is_some_and(|open| open.saved != Some(open.edit) && open.held.is_none() && open.outside.is_none()) && started.elapsed() < LAST_SAVE {
            self.drain(false, host, &mut news);
            std::thread::sleep(Duration::from_millis(5));
        }
        // If the window stays after all, what was heard is still said.
        if let Some(sentence) = news.say {
            self.later = Some(sentence);
        }
    }

    fn names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn peek(&mut self, name: &str) -> Option<String> {
        let rel = self.rel_of(name).or_else(|| self.index.resolve(name).map(str::to_string).and_then(|found| self.rel_of(&found)))?;
        let key = rel_key(&rel);
        match &self.open {
            Some(open) if rel_key(&open.rel) == key => Some(open.whole()),
            _ => self.bodies.get(&key).cloned(),
        }
    }

    fn open_named(&mut self, name: &str, start: bool, doc: &Doc, host: &mut dyn Host) -> Result<Opened, String> {
        self.tell_time(host);
        self.verify(doc, host);
        // The name in full first; then the text he was in before, which the
        // line names by its last part alone; then whatever a link of that
        // name would lead to.
        let before = self.memory.before.clone().filter(|before| links::key(&leaf(&full_name(before))) == links::key(name));
        let found = self.rel_of(name).or(before).or_else(|| self.index.resolve(name).map(str::to_string).and_then(|found| self.rel_of(&found)));
        let (rel, make) = match found {
            Some(rel) => (rel, false),
            None if start => (self.rel_for_new(name, false, "md"), true),
            None => return Err(format!("? No text called {name}.")),
        };
        match self.switch(&rel, make, host)? {
            Some(opened) => Ok(opened),
            None => Err(format!("? {} is already on the page.", self.name())),
        }
    }

    fn previous(&self) -> Option<&str> {
        let before = self.memory.before.as_ref()?;
        let key = rel_key(before);
        let at = self.rels.iter().position(|rel| rel_key(rel) == key)?;
        // The name without its folders is the end of the name with them.
        self.names.get(at).map(|name| name.rsplit('/').next().unwrap_or(name))
    }

    fn rename(&mut self, title: &str, doc: &Doc, host: &mut dyn Host) -> Result<usize, String> {
        self.tell_time(host);
        self.verify(doc, host);
        let Some(open) = self.open.as_ref() else { return Err(NOT_HERE.to_string()) };
        if !title.chars().any(char::is_alphanumeric) {
            return Err("? A name needs a letter or a digit in it.".to_string());
        }
        let ending = ending_of(&open.rel);
        let moving = title.contains(['/', '\\']);
        let to = self.rel_for_new(title, !moving, &ending);
        self.rename_to(&to)
    }

    fn links_here(&self) -> usize {
        self.links_here
    }

    fn discard(&mut self, doc: &Doc, host: &mut dyn Host) -> Result<Opened, String> {
        self.tell_time(host);
        self.verify(doc, host);
        let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_ref()) else { return Err(NOT_HERE.to_string()) };
        let last = (open.saved != Some(open.edit)).then(|| (open.whole(), open.edit));
        store.discard(&open.rel, last).map_err(|why| match why {
            StoreError::Changed => "? This text changed on disk. Nothing was moved.".to_string(),
            StoreError::ReadOnly | StoreError::Linked => "? This file cannot be written, so nothing was moved.".to_string(),
            StoreError::Busy => "? Another program has this file open. Nothing was moved.".to_string(),
            StoreError::DiskFull => "? The disk is full. Nothing was moved.".to_string(),
            _ => "? This text could not be moved just now. Nothing was erased.".to_string(),
        })?;
        // The save layer has closed it; nothing of it is kept here.
        let Some(old) = self.open.take() else { return Err(NOT_HERE.to_string()) };
        let key = rel_key(&old.rel);
        let at = self.rels.iter().position(|rel| rel_key(rel) == key);
        self.closing.push((old.doc, old.leaf.clone()));
        self.index.remove(&old.full);
        self.bodies.remove(&key);
        self.counts.remove(&key);
        self.memory.texts.remove(&key);
        self.memory.last = None;
        self.gone.push(key);
        self.versions.clear();
        self.version_words.clear();
        self.stale = true;
        self.list();
        // In its place: the text he was in before, or the one that now
        // stands where this one did.
        let before = self.memory.before.take();
        let neighbour = at.and_then(|at| self.rels.get(at).or_else(|| self.rels.last())).cloned();
        self.memory.last = before.or(neighbour);
        let mut opened = self.begin(host);
        if let Some(other) = opened.say.replace(DISCARDED.to_string()) {
            self.later = Some(other);
            self.nudge = true;
        }
        Ok(opened)
    }

    fn versions(&mut self) -> Vec<Entry> {
        let (Some(store), Some(open)) = (self.store.as_ref(), self.open.as_ref()) else { return Vec::new() };
        let Ok(mut all) = store.versions(open.doc) else { return Vec::new() };
        all.truncate(MOST_VERSIONS);
        let (now, offset) = (Moment::of(&self.clock), offset_of(&self.clock));
        let rows = all
            .iter()
            .map(|version| {
                let count = *self.version_words.entry(version.name.clone()).or_insert_with(|| store.read_version(open.doc, &version.name).map(|text| count::words(&text)).unwrap_or(0));
                let at = Moment::at(version.at_ms, offset);
                let when = format!("{} {}", at.day(now), at.time());
                let name = match version.label.as_deref() {
                    Some(label) => format!("{when}, {}", label_words(label)),
                    None => when,
                };
                Entry { depth: 0, name, value: thousands(count), lit: false }
            })
            .collect();
        self.versions = all;
        rows
    }

    fn version(&mut self, n: usize) -> Option<String> {
        let (store, open) = (self.store.as_ref()?, self.open.as_ref()?);
        store.read_version(open.doc, &self.versions.get(n)?.name).ok()
    }

    fn bring_back(&mut self, n: usize, doc: &Doc, host: &mut dyn Host) -> Result<Opened, String> {
        self.tell_time(host);
        self.verify(doc, host);
        let today = self.today(host);
        let now = host.now_ms();
        let (Some(store), Some(open)) = (self.store.as_mut(), self.open.as_mut()) else { return Err(NOT_HERE.to_string()) };
        if open.read_only.is_some() {
            return Err(format!("? {READING_ONLY}"));
        }
        let Some(version) = self.versions.get(n) else { return Err("? There is no such version.".to_string()) };
        // The present text is kept as a version before the old one is
        // handed over.
        let text = store.bring_back(open.doc, &version.name, &open.whole()).map_err(|why| match why {
            StoreError::NoSuchVersion => "? That version is no longer there.".to_string(),
            _ => "? The present text could not be kept first, so nothing was changed.".to_string(),
        })?;
        let fresh = Doc::from_text(&text);
        open.stamps = restamp(&open.text, &open.stamps, &fresh, today);
        let whole = Span::new(Pos::default(), open.text.end());
        let change = open.text.replace(whole, &text);
        open.len = open.text.len_bytes();
        if open.saved == Some(open.edit) {
            open.unsaved_since_ms = now;
        }
        open.edit = store.record(open.doc, &change, open.len);
        store.save(open.doc, open.whole(), open.edit);
        open.asked = Some(open.edit);
        open.asked_ms = now;
        open.changed_ms = now;
        open.revision = 0;
        open.caret = open.caret.map(|caret| open.text.clamp(caret));
        let opened = open.opened();
        let (full, key) = (open.full.clone(), rel_key(&open.rel));
        self.index.set(&full, &text);
        self.counts.insert(key, count::words(&text));
        self.stale = true;
        self.relink();
        Ok(opened)
    }

    fn settings(&mut self) -> Option<String> {
        self.store.as_ref()?.read_state(SETTINGS).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn write_settings(&mut self, text: &str) {
        if let Some(store) = self.store.as_mut() {
            store.write_state(SETTINGS, text.as_bytes());
        }
    }

    fn limit(&self) -> Option<usize> {
        self.open.as_ref().and_then(|open| open.limit)
    }

    fn set_limit(&mut self, limit: Option<usize>) {
        if let Some(open) = self.open.as_mut() {
            open.limit = limit.filter(|&limit| limit > 0);
            self.stale = true;
            self.remember();
        }
    }

    fn folder(&self) -> Option<&Path> {
        Some(&self.shown)
    }

    fn use_folder(&mut self, folder: &Path, doc: &Doc, host: &mut dyn Host) -> Result<Opened, String> {
        self.tell_time(host);
        self.verify(doc, host);
        self.relocate(folder, None, true, host)
    }
}

impl Drop for Folder {
    /// Ends the save layer. The journal is flushed, the cursor's place noted
    /// and what is remembered written down, and that is all: the file
    /// itself is asked for in
    /// [`Session::close`], which every orderly way out goes through. A
    /// session dropped without it, as when the program is unwinding from a
    /// fault, leaves what was typed since the last save in the journal, and
    /// the next start brings it back and says so.
    fn drop(&mut self) {
        self.stale = true;
        self.remember();
        if let Some(mut store) = self.store.take() {
            if let Some(open) = &self.open {
                store.close(open.doc, None, open.caret);
            }
            store.shutdown(Duration::from_secs(3));
        }
    }
}

/// The session when the writing folder could not be opened at all: the text
/// lives in memory only, and the screen says so for as long as it is true.
pub struct Unsaved {
    why: Option<String>,
}

impl Unsaved {
    pub fn new(why: String) -> Unsaved {
        Unsaved { why: Some(why) }
    }
}

impl Session for Unsaved {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        Opened { say: self.why.take(), ..Opened::default() }
    }

    fn on_disk(&self) -> bool {
        false
    }

    fn failing(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> Moment {
        Moment { local: days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 }
    }

    #[test]
    fn every_fixed_sentence_fits_the_measure() {
        // One sentence is left out: DESIGN.md gives it as it stands here and
        // it is 70 cells long, four more than DESIGN.md says every sentence
        // fits. It is used exactly, and the say row cuts it at the measure.
        assert_eq!(NEWER, "This text changed on disk. The newer one is shown; yours is kept.");
        let fixed = [START, HINT, SAVED_AGAIN, NOT_UTF8, DISCARDED, READING_ONLY, NOT_KEPT, JOURNAL_DOWN, WAITING, REMOVED, NOT_ATOMIC];
        let holds = [Hold::NotUtf8, Hold::Removed, Hold::NameTaken, Hold::Stopped, Hold::ReadOnly, Hold::Linked, Hold::Busy, Hold::FolderGone, Hold::DiskFull, Hold::CannotWrite, Hold::NoHistory, Hold::NotDurable, Hold::Io(5)];
        let reading = [ReadOnly::NotUtf8, ReadOnly::Attribute, ReadOnly::Busy, ReadOnly::Linked];
        let all = fixed.iter().map(|s| s.to_string()).chain(holds.iter().map(|&why| held_sentence(why))).chain(reading.iter().map(|&why| reading_sentence(why).to_string())).chain([brought_back(0), brought_back(12_345)]);
        for sentence in all {
            assert!(sentence.chars().count() <= 66, "{} cells: {sentence}", sentence.chars().count());
            assert!(!sentence.contains('!') && !sentence.contains('\u{b7}') && !sentence.contains("->"), "{sentence}");
        }
        assert_eq!(held_sentence(Hold::CannotWrite), "Not saved. This folder cannot be written. Still trying.");
        assert_eq!(brought_back(41), "Brought back 41 words typed after the last save.");
    }

    #[test]
    fn a_day_is_named_from_where_he_stands() {
        // 13 October 2026 is a Tuesday.
        let now = moment(2026, 10, 15, 9, 0);
        assert_eq!(moment(2026, 10, 15, 8, 5).day(now), "Today");
        assert_eq!(moment(2026, 10, 13, 23, 41).day(now), "Tuesday");
        // Half past one at night belongs to the evening before.
        assert_eq!(moment(2026, 10, 14, 1, 30).day(now), "Tuesday");
        assert_eq!(moment(2026, 10, 14, 1, 30).time(), "01:30");
        assert_eq!(moment(2026, 10, 15, 1, 30).day(moment(2026, 10, 15, 2, 0)), "Today");
        assert_eq!(moment(2026, 9, 14, 23, 10).day(now), "14 September");
        assert_eq!(moment(2025, 9, 14, 23, 10).day(now), "14 September 2025");
        // A clock that was set back does not make a day of the future.
        assert_eq!(moment(2026, 10, 20, 8, 0).day(now), "Today");
        let here = Here { at: moment(2026, 10, 13, 23, 41), added: 388 };
        assert_eq!(here.sentence(now), "Last here Tuesday at 23:41. That sitting added 388 words.");
        assert_eq!(Here { at: moment(2026, 10, 15, 8, 5), added: 1 }.sentence(now), "Last here today at 08:05. That sitting added 1 word.");
        for days in [-1, 0, 59, 60, 365, 11_016, 20_737, 100_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn a_first_line_names_its_text() {
        assert_eq!(title_of("# The orchard").as_deref(), Some("The orchard"));
        assert_eq!(title_of("  ## What: he *showed* me?  ").as_deref(), Some("What he showed me"));
        assert_eq!(title_of("See [[Grandfather interview|him]] and `code`").as_deref(), Some("See Grandfather interview him and code"));
        assert_eq!(title_of("_Draft_ of the opening.").as_deref(), Some("Draft of the opening"));
        assert_eq!(title_of("# "), None);
        assert_eq!(title_of("---"), None);
        assert_eq!(title_of(""), None);
        assert_eq!(title_of("CON").as_deref(), Some("CON_"));
        let long = title_of("I spent the summer I was fifteen on a ladder in those rows, thinning apricots").unwrap();
        assert_eq!(long, "I spent the summer I was fifteen on a ladder in those rows");
        assert!(long.chars().count() <= NAME_LIMIT);
        let unbroken = title_of(&"x".repeat(200)).unwrap();
        assert_eq!(unbroken.chars().count(), NAME_LIMIT);
        assert!(title_of(&"\u{e9}".repeat(200)).unwrap().chars().count() <= NAME_LIMIT);
        for (name, untitled) in [("Untitled", true), ("Untitled 2", true), ("Untitled 12", true), ("Untitled two", false), ("Untitled ", false), ("Untitled2", false), ("The orchard", false)] {
            assert_eq!(is_untitled(name), untitled, "{name}");
        }
    }

    #[test]
    fn only_a_typed_letter_that_begins_a_word_counts() {
        let mut doc = Doc::new();
        let mut typed = 0;
        for ch in "It's well-known: **two** words.".chars() {
            let at = doc.end();
            let change = doc.replace(Span::at(at), &ch.to_string());
            typed += usize::from(began_word(&doc, &change));
        }
        assert_eq!(typed, count::words(&doc.to_text("\n")));
        // A letter put into a word, or at its edge, begins nothing.
        let change = doc.replace(Span::at(Pos::new(0, 1)), "x");
        assert!(!began_word(&doc, &change));
        // What is pasted is not typed.
        let at = doc.end();
        let change = doc.replace(Span::at(at), " pasted words");
        assert!(!began_word(&doc, &change));
        let at = doc.end();
        let change = doc.replace(Span::at(at), " ");
        assert!(!began_word(&doc, &change));
    }

    #[test]
    fn the_memory_reads_back_what_it_wrote() {
        let mut memory = Memory { last: Some(PathBuf::from(r"College\The orchard.md")), before: Some(PathBuf::from("Essay plan.md")), here: Some(Here { at: moment(2026, 10, 13, 23, 41), added: 388 }), typed: (739_170, 163), texts: HashMap::new(), counts: HashMap::from([(r"college\the orchard.md".to_string(), 412)]) };
        let kept = Kept { rel: PathBuf::from(r"College\The orchard.md"), limit: Some(650), print: 0xdead_beef, stamps: vec![0, 0, 0, 739_170, 739_170, 0] };
        memory.texts.insert(rel_key(&kept.rel), kept);
        let plain = Kept { rel: PathBuf::from("Essay plan.md"), limit: None, print: 7, stamps: Vec::new() };
        memory.texts.insert(rel_key(&plain.rel), plain);
        assert_eq!(Memory::read(&memory.write()), memory);
        assert_eq!(pack_stamps(&[0, 0, 5, 5, 5, 0]), "0*2,5*3,0*1");
        // A file that is damaged, or not a memory at all, costs nothing.
        assert_eq!(Memory::read("here\tx\ty\ntext\tonly\nnonsense\n\u{0}\ttyped\t1"), Memory::default());
        assert!(unpack_stamps("1*99999999999,2*3").len() <= 3);
        assert_eq!(memory_name(Path::new(r"\\?\C:\Users\x\Documents\ok")), memory_name(Path::new(r"c:\users\X\documents\OK")));
    }

    #[test]
    fn stamps_follow_a_text_that_is_put_in_place_of_another() {
        let old = Doc::from_text("one\ntwo\nthree\nfour");
        let new = Doc::from_text("one\nTWO\nand a half\nthree\nfour");
        assert_eq!(restamp(&old, &[1, 2, 3, 4], &new, 9), [1, 9, 9, 3, 4]);
        assert_eq!(restamp(&old, &[1, 2, 3, 4], &old, 9), [1, 2, 3, 4]);
        assert_eq!(restamp(&old, &[], &Doc::new(), 9), [9]);
    }

    #[test]
    fn settings_are_one_name_and_value_to_a_line() {
        let text = "look manila\nfolder C:\\Users\\x\\My essays\nmeasure 66\n";
        assert_eq!(setting(text, "folder").as_deref(), Some("C:\\Users\\x\\My essays"));
        assert_eq!(setting(text, "scale"), None);
        let changed = with_setting(text, "folder", "D:\\Essays");
        assert_eq!(setting(&changed, "folder").as_deref(), Some("D:\\Essays"));
        assert_eq!(changed.lines().count(), 3);
        assert_eq!(setting(&with_setting("", "limit", "650"), "limit").as_deref(), Some("650"));
    }

    #[test]
    fn links_are_counted_outside_fenced_code() {
        let text = "See [[The orchard]] and [[college/the orchard#Thinning|there]].\n\n```\n[[The orchard]]\n```\n\n[[The orchard]] again.";
        assert_eq!(count_links(text, &links::key("The orchard")), 2);
        assert_eq!(count_links(text, &links::key("College/The orchard")), 1);
    }

    #[test]
    #[cfg(windows)] // Windows paths: drive letters and `\`.
    fn a_path_is_found_inside_the_folder_however_it_is_spelled() {
        let root = Path::new(r"\\?\C:\Users\x\Documents\ok");
        assert_eq!(inside(root, Path::new(r"c:\users\X\documents\OK\College\The orchard.md")), Some(PathBuf::from(r"College\The orchard.md")));
        assert_eq!(inside(root, Path::new(r"C:\Users\x\Documents\other\a.md")), None);
        assert_eq!(inside(root, Path::new(r"C:\Users\x\Documents\ok")), None);
        assert_eq!(plain(Path::new(r"\\?\UNC\server\share\ok")), PathBuf::from(r"\\server\share\ok"));
    }
}
