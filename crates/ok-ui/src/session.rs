//! The session: where the text comes from and where it goes.
//!
//! # Brief for the team that fills this file
//!
//! The interface never touches a file itself. Everything about the writing
//! folder, the journal, versions, recovery and links between texts belongs
//! behind the [`Session`] trait.
//!
//! Every method of the trait has a body that does nothing, and [`Memory`]
//! is the session that stands here now: one text in memory, read once from
//! the file the options name if they name one, never written anywhere, and
//! always "on disk". If the options name a folder it is read once, plainly,
//! so that frames can show a contents page and a links block. Write your
//! own type, implement the methods, and hand it out from [`start`]; nothing
//! outside this file has to change. The call sites in `app.rs` and the
//! painters are in place and are exercised by `tests/seams.rs` with a
//! stand-in session. Two other teams are filling `line.rs` and `flow.rs`
//! beside you.
//!
//! What you receive:
//! - [`start`] at launch, with the options and the host, and then
//!   [`Session::first`] once, for the text to show.
//! - [`Session::changed`] for every [`Change`], before the window message
//!   that made it has been answered, and before `keep`, `follow`, `open`,
//!   `tick` or `close` is called. An undo or a redo arrives as the changes
//!   it replayed, in order. For a step of several changes the text handed
//!   over is the text after the last of them, so journal what each change
//!   says and not what the text reads at that instant.
//! - [`Session::keep`] for Ctrl S and [`Session::follow`] for Ctrl Enter.
//! - [`Session::open`] when a second launch hands this one a file.
//! - [`Session::tick`] whenever the shell ticks: after the delay you asked
//!   for through [`Session::wake_in_ms`], and whenever one of your threads
//!   calls the host's waker. Say what happened through [`News`].
//! - [`Session::close`] when the window is going, by Ctrl Q or from
//!   outside. Everything must be on disk before it returns.
//!
//! What you must answer, cheaply, because the painters ask on every frame:
//! - [`Session::on_disk`]: is the file on disk exactly what is on screen?
//!   `ok` is printed while this is true and at no other time.
//! - [`Session::failing`]: is a write failing right now? The machine line
//!   goes inverse while it is.
//! - [`Session::name`], [`Session::contents`], [`Session::links`],
//!   [`Session::exists`], [`Session::words_today`].
//! - [`Session::today`]: which writing day a moment falls in. Change bars
//!   and the map's ink mark the lines stamped with today's number. The
//!   interface stamps a line when it changes; [`Opened::stamps`] carries
//!   the stamps you remembered from earlier sittings. The body the trait
//!   comes with already begins the day at 04:00.
//!
//! What you may not touch: the page, the cursor, the grid, the look. To
//! put another text on the page, hand back an [`Opened`]; to speak, hand
//! back a sentence. The interface decides where both go.

use std::path::{Path, PathBuf};

use ok_host::Host;
use ok_text::count;
use ok_text::links::{self, links_in, LinkIndex};
use ok_text::{Change, Doc, Pos};

use crate::contents::Entry;
use crate::machine::thousands;
use crate::page::Day;

/// What the program is started with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// A text to open.
    pub file: Option<PathBuf>,
    /// The writing folder.
    pub folder: Option<PathBuf>,
    /// A built-in look to start in. Manila when `None` or not known.
    pub look: Option<String>,
    /// Keep the texts in the writing folder through the save layer. Off for
    /// frames and tests, which must never write; on in the program.
    pub store: bool,
}

/// A text handed to the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    pub text: String,
    /// Where the cursor was left. The end of the text when `None`.
    pub cursor: Option<Pos>,
    /// The row of the text that was at the top of the sheet. When `None`
    /// the cursor's row is brought to the platen.
    pub top: Option<usize>,
    /// For each line, the writing day it was last changed. Lines the list
    /// does not reach count as older than any day.
    pub stamps: Vec<Day>,
    /// A text that must not be edited: one that is not UTF-8, say.
    pub read_only: bool,
    /// A sentence for the say row.
    pub say: Option<String>,
}

/// One text the open text links to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRow {
    pub name: String,
    /// False for a link to a text that has not been started yet.
    pub exists: bool,
}

/// The links block: what the open text points at, and what points at it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub see_also: Vec<LinkRow>,
    pub linked_from: Vec<String>,
}

/// What happened since the last tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct News {
    /// A sentence for the say row: `Saved again. Nothing was lost.`
    pub say: Option<String>,
    /// Another text to put on the page in place of the one there: the file
    /// changed on disk, a journal was replayed.
    pub replaced: Option<Opened>,
}

/// The number of the day a date is, counting from 1 March of year 0, so
/// that consecutive dates are consecutive numbers.
fn day_number(year: i32, month: u8, day: u8) -> i64 {
    let (year, month, day) = (year as i64, month.clamp(1, 12) as i64, day.clamp(1, 31) as i64);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year
}

/// Where the text comes from and goes to, as the interface sees it.
pub trait Session {
    /// The text to show at launch. Called once, straight after [`start`].
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        Opened::default()
    }

    /// A second launch asks this one to open a file; `doc` is the text on
    /// the page now. `None` leaves the page as it is.
    fn open(&mut self, _path: &Path, _doc: &Doc, _host: &mut dyn Host) -> Option<Opened> {
        None
    }

    /// One change has just been made to the open text; `doc` is the text
    /// after it.
    fn changed(&mut self, _doc: &Doc, _change: &Change, _host: &mut dyn Host) {}

    /// True when the file on disk is exactly what is on screen: the whole
    /// text written and flushed, and nothing changed since. `ok` is printed
    /// while this holds.
    fn on_disk(&self) -> bool {
        true
    }

    /// True while a write is failing. The machine line goes inverse.
    fn failing(&self) -> bool {
        false
    }

    /// The name of the open text: its file's name without the ending.
    /// Empty for a text that has no name yet.
    fn name(&self) -> &str {
        ""
    }

    /// The writing folder as a contents page: folder names and texts in
    /// alphabetical order, each with its depth, texts with their word count
    /// as it is to be read, the open text lit. The lit row's count is drawn
    /// from the page's own running tally, so it need not be kept current.
    fn contents(&self) -> &[Entry] {
        &[]
    }

    /// The links block for the open text: `None` when there is nothing to
    /// show.
    fn links(&self) -> Option<&Links> {
        None
    }

    /// Whether the text a `[[link]]` names has been started. A link that
    /// leads nowhere yet is drawn at half strength.
    fn exists(&self, _target: &str) -> bool {
        true
    }

    /// The writing day `host`'s clock is in. Days begin at 04:00, so the
    /// small hours belong to the evening before. Never 0, which stands for
    /// "before any day this sitting knows of".
    fn today(&self, host: &dyn Host) -> Day {
        let now = host.local_time();
        let day = day_number(now.year, now.month, now.day) - i64::from(now.hour < 4);
        day.clamp(1, Day::MAX as i64) as Day
    }

    /// Words typed today across the folder. Shown among the facts when it
    /// is not 0.
    fn words_today(&self) -> usize {
        0
    }

    /// Ctrl S: keeps a version and answers with the truth, such as
    /// `Kept 14:02. 2,481 bytes on disk.`
    fn keep(&mut self, _doc: &Doc, _host: &mut dyn Host) -> Option<String> {
        None
    }

    /// Ctrl Enter with the cursor at `at`: follows the link there, starting
    /// its text if it does not exist, or opens a web address in the
    /// browser. Hands back the text to show, if the page is to change.
    fn follow(&mut self, _doc: &Doc, _at: Pos, _host: &mut dyn Host) -> Option<Opened> {
        None
    }

    /// The shell ticked. Says what has happened since the last time.
    fn tick(&mut self, _doc: &Doc, _host: &mut dyn Host) -> News {
        News::default()
    }

    /// How long from now the session wants a tick, if it wants one: the
    /// moment after a pause when the file is written, say.
    fn wake_in_ms(&self, _host: &dyn Host) -> Option<u64> {
        None
    }

    /// The window is closing. Everything must be on disk before this
    /// returns.
    fn close(&mut self, _doc: &Doc, _host: &mut dyn Host) {}

    // What the line asks of the folder. Wherever a `Result` comes back, the
    // error is the sentence for the say row, `?` included. `doc` is always
    // the text on the page at that moment.

    /// Every text in the folder by the name links use (folders and name,
    /// forward slashes, no ending), in the order of the contents.
    fn names(&self) -> Vec<String> {
        Vec::new()
    }

    /// The text of one of them without opening it, for the page to show
    /// while the line's bar rests on its row.
    fn peek(&mut self, _name: &str) -> Option<String> {
        None
    }

    /// Puts the text called `name` on the page. With `start` set, a name
    /// that leads nowhere starts a new text by that title (a slash or a
    /// backslash in it makes or uses a folder).
    fn open_named(&mut self, _name: &str, _start: bool, _doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        Err(NOT_HERE.to_string())
    }

    /// The name of the text he was in before this one, for
    /// `Enter goes back to where you were, in Opening statement.`
    fn previous(&self) -> Option<&str> {
        None
    }

    /// Renames the open text, or moves it when the title has a slash or a
    /// backslash, and mends every link to it. Hands back how many links
    /// were mended.
    fn rename(&mut self, _title: &str, _doc: &Doc, _host: &mut dyn Host) -> Result<usize, String> {
        Err(NOT_HERE.to_string())
    }

    /// How many links in other texts lead to the open one.
    fn links_here(&self) -> usize {
        0
    }

    /// Moves the open text into `.discarded` and hands back the text to
    /// show in its place. Nothing is erased.
    fn discard(&mut self, _doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        Err(NOT_HERE.to_string())
    }

    /// The kept versions of the open text, newest first, as rows to show:
    /// the name says when and why (`Today 14:02, kept`), the value how many
    /// words.
    fn versions(&mut self) -> Vec<Entry> {
        Vec::new()
    }

    /// The text of row `n` of [`Session::versions`].
    fn version(&mut self, _n: usize) -> Option<String> {
        None
    }

    /// Makes version `n` the text again. The present text is kept as a
    /// version first.
    fn bring_back(&mut self, _n: usize, _doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        Err(NOT_HERE.to_string())
    }

    /// The settings text as it was last written, one `name value` to a
    /// line, and writing it.
    fn settings(&mut self) -> Option<String> {
        None
    }

    fn write_settings(&mut self, _text: &str) {}

    /// The open text's word limit, which belongs to that text alone.
    fn limit(&self) -> Option<usize> {
        None
    }

    fn set_limit(&mut self, _limit: Option<usize>) {}

    /// The writing folder, and writing somewhere else from now on.
    fn folder(&self) -> Option<&Path> {
        None
    }

    fn use_folder(&mut self, _folder: &Path, _doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        Err(NOT_HERE.to_string())
    }
}

/// What a session that has no folder answers to the calls that need one.
pub const NOT_HERE: &str = "? There is no writing folder open.";

/// The stand-in session: one text in memory, and one plain read of the
/// folder for the contents and the links.
#[derive(Debug, Default)]
pub struct Memory {
    name: String,
    text: String,
    contents: Vec<Entry>,
    links: Links,
    /// The texts of the folder that was read at launch, for answering
    /// whether a link leads anywhere. `None` when no folder was named.
    index: Option<LinkIndex>,
}

/// The endings of the files the contents list.
const ENDINGS: [&str; 3] = ["md", "markdown", "txt"];

/// How deep and how far the plain read of a folder goes, so a folder that
/// turns out to be a whole disk cannot hold up the first frame.
const DEEPEST: usize = 6;
const MOST: usize = 2000;

fn is_text(path: &Path) -> bool {
    path.extension().and_then(|ending| ending.to_str()).is_some_and(|ending| ENDINGS.iter().any(|known| ending.eq_ignore_ascii_case(known)))
}

/// The last part of a text's name: the name without the folders it is in.
fn leaf(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// Reads one folder into `entries`, texts first and then the folders
/// inside it, each alphabetically, and puts every text into `index` under
/// its name relative to the top.
fn read_folder(dir: &Path, prefix: &str, depth: usize, open: Option<&Path>, entries: &mut Vec<Entry>, index: &mut LinkIndex) {
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    let mut texts: Vec<(String, PathBuf)> = Vec::new();
    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    for item in listing.flatten() {
        let path = item.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            folders.push((name, path));
        } else if is_text(&path) {
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(&name).to_string();
            texts.push((stem, path));
        }
    }
    texts.sort_by_key(|(name, _)| name.to_lowercase());
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    for (name, path) in texts {
        if entries.len() >= MOST {
            return;
        }
        let text = std::fs::read(&path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
        index.set(&format!("{prefix}{name}"), &text);
        entries.push(Entry { depth, name, value: thousands(count::words(&text)), lit: open == Some(path.as_path()) });
    }
    for (name, path) in folders {
        if entries.len() >= MOST || depth >= DEEPEST {
            return;
        }
        entries.push(Entry { depth, name: name.clone(), value: String::new(), lit: false });
        read_folder(&path, &format!("{prefix}{name}/"), depth + 1, open, entries, index);
    }
}

impl Memory {
    /// Reads the file and the folder the options name, once each, and
    /// keeps nothing open.
    pub fn new(options: &Options) -> Memory {
        let mut session = Memory::default();
        if let Some(file) = &options.file {
            session.text = std::fs::read(file).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
            session.name = file.file_stem().and_then(|stem| stem.to_str()).unwrap_or("").to_string();
        }
        if let Some(folder) = &options.folder {
            let mut index = LinkIndex::new();
            read_folder(folder, "", 0, options.file.as_deref(), &mut session.contents, &mut index);
            let open = options.file.as_deref().and_then(|file| file.strip_prefix(folder).ok()).map(|inside| inside.with_extension(""));
            if let Some(open) = open.as_deref().and_then(Path::to_str) {
                let open = open.replace('\\', "/");
                let see_also = index.outgoing(&open).into_iter().map(|link| LinkRow { exists: link.resolved.is_some(), name: leaf(link.resolved.as_deref().unwrap_or(&link.target)) });
                session.links = Links { see_also: see_also.collect(), linked_from: index.incoming(&open).into_iter().map(leaf).collect() };
            }
            session.index = Some(index);
        }
        session
    }
}

impl Session for Memory {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        Opened { text: std::mem::take(&mut self.text), ..Opened::default() }
    }

    /// Only keeps "See also" true to the text, and only looks when a
    /// bracket came or went.
    fn changed(&mut self, doc: &Doc, change: &Change, _host: &mut dyn Host) {
        let Some(index) = &self.index else { return };
        if !change.inserted.contains(['[', ']']) && !change.removed.contains(['[', ']']) {
            return;
        }
        let mut seen: Vec<String> = Vec::new();
        self.links.see_also.clear();
        for link in doc.lines().iter().filter(|line| line.contains("[[")).flat_map(|line| links_in(line)) {
            let key = links::key(link.target);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let found = index.resolve(link.target);
            self.links.see_also.push(LinkRow { name: leaf(found.unwrap_or(link.target)), exists: found.is_some() });
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> &[Entry] {
        &self.contents
    }

    fn links(&self) -> Option<&Links> {
        Some(&self.links)
    }

    fn exists(&self, target: &str) -> bool {
        self.index.as_ref().is_none_or(|index| index.resolve(target).is_some())
    }
}

/// The session the program starts with. Replace the body to hand out
/// yours.
pub fn start(options: &Options, host: &mut dyn Host) -> Box<dyn Session> {
    if options.store {
        return match crate::folder::Folder::open(options, host) {
            Ok(folder) => Box::new(folder),
            Err(why) => Box::new(crate::folder::Unsaved::new(why)),
        };
    }
    Box::new(Memory::new(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_dates_are_consecutive_days() {
        assert_eq!(day_number(2026, 10, 12) - day_number(2026, 10, 11), 1);
        assert_eq!(day_number(2026, 3, 1) - day_number(2026, 2, 28), 1);
        assert_eq!(day_number(2024, 3, 1) - day_number(2024, 2, 28), 2);
        assert_eq!(day_number(2027, 1, 1) - day_number(2026, 12, 31), 1);
        assert_eq!(day_number(2000, 1, 1) - day_number(1970, 1, 1), 10_957);
        // Dates no calendar has are still numbers.
        day_number(i32::MIN, 0, 0);
        day_number(i32::MAX, 255, 255);
    }

    #[test]
    fn only_texts_are_listed() {
        for (name, listed) in [("a.md", true), ("a.MD", true), ("a.markdown", true), ("a.txt", true), ("a.rs", false), ("a", false), ("md", false)] {
            assert_eq!(is_text(Path::new(name)), listed, "{name}");
        }
    }
}
