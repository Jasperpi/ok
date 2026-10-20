//! The line the program runs: everything DESIGN.md says under "The line".
//!
//! What is typed after `ok` is read afresh at every key (by `ask`), and what
//! the line shows follows from that reading alone: which pane answers and
//! with what rows, where the bar is, what the page is to show, and the one
//! sentence that says what Enter will do now. Nothing is done to the text
//! or the folder until Enter; until then the page and the settings are
//! only tried, and Esc puts back exactly what was there.
//!
//! When Enter could do nothing, the sentence starts `Enter does nothing
//! yet.` and gives the reason; pressing Enter then answers with `?` and the
//! same reason, as ed would, and the next key clears it.

use std::path::Path;

use ok_host::{Host, Key, Mods};
use ok_text::fuzzy::fuzzy;
use ok_text::history::{Cursor, EditKind};
use ok_text::markup::{self, LineKind};
use ok_text::search::find_all;
use ok_text::{count, str_width, Pos, Span};

use super::settings::{self, Kind, Opt, Read, Snapshot, OPTIONS};
use super::{pages, Answer, Line, Pane};
use crate::app::App;
use crate::canvas::fit_words;
use crate::contents::Entry;
use crate::machine::{thousands, words};
use crate::page::{Day, Way};

/// How long the bar rests on a row before the page shows what is under it.
const REST_MS: u64 = 250;

/// How long the line is left empty before it shows what it understands.
const EMPTY_MS: u64 = 1200;

/// What the empty line shows, faint, when it has been left alone.
pub const UNDERSTOOD: &str = "a title   # heading   / phrase   rename   set   earlier   discard   copy   keys";

/// How a sentence starts when Enter could do nothing as things stand.
const NOT_YET: &str = "Enter does nothing yet.";

/// The most that can be typed on the line, in bytes.
const LONGEST: usize = 512;

/// The most cells a value takes in the table of options.
const VALUE_CELLS: usize = 20;

/// How much of a paragraph's opening names it in a sentence: the width of
/// the labels at a full screen, where the same words are its label.
const OPENING_CELLS: usize = 24;

/// The longest stretch after `[[` that is still taken for the start of a
/// name, in bytes.
const LONGEST_LINK: usize = 80;

/// What is said when a change is asked of a text that takes none.
const READ_ONLY: &str = "? This text is open for reading only.";

/// What was typed on the line, read: the first character or word decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ask<'a> {
    /// Nothing: Enter goes back to where he last was.
    Back,
    /// A title, to find a text by or to start one under.
    Title(&'a str),
    /// `#` and the letters after it.
    Heading(&'a str),
    /// `/` and whatever follows: a phrase to find, or a change to make.
    Slash(&'a str),
    /// `rename` and the new title.
    Rename(&'a str),
    /// `set` and whatever follows: an option, and perhaps a value.
    Set(&'a str),
    Earlier,
    Discard,
    Copy,
    Keys,
    Notices,
    Quit,
}

/// Reads the line. `set` and `rename` are known by the first word; the
/// words that take nothing after them only when they stand alone, so that
/// a text called "Copy of the plan" can still be found by its title.
fn ask(typed: &str) -> Ask<'_> {
    let typed = typed.trim_start();
    let whole = typed.trim_end();
    if whole.is_empty() {
        return Ask::Back;
    }
    if let Some(rest) = typed.strip_prefix('#') {
        return Ask::Heading(rest);
    }
    if let Some(rest) = typed.strip_prefix('/') {
        return Ask::Slash(rest);
    }
    let (word, rest) = typed.split_once(char::is_whitespace).unwrap_or((typed, ""));
    match word.to_ascii_lowercase().as_str() {
        "set" => return Ask::Set(rest),
        "rename" => return Ask::Rename(rest.trim()),
        _ => {}
    }
    match whole.to_ascii_lowercase().as_str() {
        "earlier" => Ask::Earlier,
        "discard" => Ask::Discard,
        "copy" => Ask::Copy,
        "keys" => Ask::Keys,
        "notices" => Ask::Notices,
        "quit" => Ask::Quit,
        _ => Ask::Title(whole),
    }
}

/// What follows a `/`, read.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Slash {
    /// `/phrase`
    Find(String),
    /// `/old/new`, and with `all` set `/old/new/g`.
    Change { old: String, new: String, all: bool },
    /// Something after the last slash that is not `g`.
    Bad,
}

/// Reads what follows the first `/`. A backslash before a slash makes the
/// slash part of the phrase.
fn slash(rest: &str) -> Slash {
    let mut parts: Vec<String> = Vec::new();
    let mut part = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'/') => {
                chars.next();
                part.push('/');
            }
            '/' => parts.push(std::mem::take(&mut part)),
            ch => part.push(ch),
        }
    }
    parts.push(part);
    let mut parts = parts.into_iter();
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(phrase), None, ..) => Slash::Find(phrase),
        (Some(old), Some(new), None, _) => Slash::Change { old, new, all: false },
        (Some(old), Some(new), Some(flag), None) if flag.is_empty() || flag == "g" => Slash::Change { old, new, all: flag == "g" },
        _ => Slash::Bad,
    }
}

/// Case is ignored in a find unless a capital was typed.
fn has_capital(phrase: &str) -> bool {
    phrase.chars().any(char::is_uppercase)
}

/// True when every one of `words` is written somewhere in `name`, whatever
/// the case. This is what decides whether a name is found; how well it
/// matches, which decides the order, is the fuzzy matcher's business.
fn has_words(name: &str, words: &[&str]) -> bool {
    let name = name.to_lowercase();
    words.iter().all(|word| name.contains(&word.to_lowercase()))
}

/// The last part of a text's name: the name without the folders it is in.
fn leaf(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// What one row of the line's list stands for: what Enter would act on
/// with the bar there.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Stands {
    /// A folder's name, or anything else the bar passes over.
    Nothing,
    /// A text, by the name links use.
    Text(String),
    /// A heading or a paragraph, by the line it is on.
    Line(usize),
    /// An option, by name.
    Option(&'static str),
    /// One of the values an option is walked through.
    Choice(String),
    /// A kept version of the open text, by its row.
    Version(usize),
}

/// The folder as the line sees it: the texts by the names links use, in
/// the order of the contents, with their word counts and which is open.
struct Texts {
    names: Vec<String>,
    counts: Vec<String>,
    open: Option<usize>,
}

impl Texts {
    fn read(app: &App) -> Texts {
        let session = app.session();
        let names = session.names();
        // The contents list the same texts in the same order, between the
        // names of the folders; that is where the counts are.
        let listed: Vec<&Entry> = session.contents().iter().filter(|entry| !entry.value.is_empty()).collect();
        let agree = listed.len() == names.len();
        let mut counts: Vec<String> = if agree { listed.iter().map(|entry| entry.value.clone()).collect() } else { vec![String::new(); names.len()] };
        let open = if agree { listed.iter().position(|entry| entry.lit) } else { names.iter().position(|name| leaf(name) == session.name()) };
        // The open text's count is the page's own, which is always current.
        if let Some(count) = open.and_then(|open| counts.get_mut(open)).filter(|_| agree) {
            *count = thousands(app.text().words());
        }
        Texts { names, counts, open }
    }

    /// The texts a title finds, best first: every one whose name has all
    /// of its words, in any order and any case. A name that has the whole
    /// title in order comes before one that only has its words, and within
    /// each the better match first, ties staying in the folder's order. An
    /// empty title finds them all.
    fn find(&self, title: &str) -> Vec<usize> {
        let title = title.trim().replace('\\', "/");
        if title.is_empty() {
            return (0..self.names.len()).collect();
        }
        let parts: Vec<&str> = title.split_whitespace().collect();
        let mut found: Vec<(bool, i64, usize)> = Vec::new();
        for (at, name) in self.names.iter().enumerate() {
            if !has_words(name, &parts) {
                continue;
            }
            let in_order = [leaf(name), name.as_str()].into_iter().filter_map(|candidate| fuzzy(&title, candidate)).map(|found| found.score as i64).max();
            let scored = match in_order {
                Some(score) => Some((true, score)),
                None => parts.iter().map(|part| fuzzy(part, name).map(|found| found.score as i64)).sum::<Option<i64>>().map(|score| (false, score)),
            };
            if let Some((in_order, score)) = scored {
                found.push((in_order, score, at));
            }
        }
        found.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        found.into_iter().map(|(_, _, at)| at).collect()
    }

    /// The rows of a contents page for some of the texts: each under the
    /// folders it is in, in the folder's own order.
    fn rows(&self, keep: impl Fn(usize) -> bool) -> (Vec<Entry>, Vec<Stands>) {
        let (mut rows, mut stands) = (Vec::new(), Vec::new());
        let mut above: Vec<&str> = Vec::new();
        for (at, name) in self.names.iter().enumerate().filter(|(at, _)| keep(*at)) {
            let mut folders: Vec<&str> = name.split('/').collect();
            let title = folders.pop().unwrap_or(name);
            let shared = above.iter().zip(&folders).take_while(|(a, b)| a == b).count();
            for (depth, folder) in folders.iter().enumerate().skip(shared) {
                rows.push(Entry { depth, name: folder.to_string(), value: String::new(), lit: false });
                stands.push(Stands::Nothing);
            }
            rows.push(Entry { depth: folders.len(), name: title.to_string(), value: self.counts.get(at).cloned().unwrap_or_default(), lit: self.open == Some(at) });
            stands.push(Stands::Text(name.clone()));
            above = folders;
        }
        (rows, stands)
    }

    /// The rows for the texts a search found, and the row of the best.
    fn found(&self, found: &[usize]) -> (Vec<Entry>, Vec<Stands>, Option<usize>) {
        let mut kept = vec![false; self.names.len()];
        for &at in found {
            kept[at] = true;
        }
        let (rows, stands) = self.rows(|at| kept[at]);
        let best = found.first().and_then(|&best| stands.iter().position(|row| matches!(row, Stands::Text(name) if *name == self.names[best])));
        (rows, stands, best)
    }

    /// What a link to a text is written as: its name without its folders
    /// when no other text has that name, and its whole name otherwise.
    fn link_name(&self, at: usize) -> String {
        let name = &self.names[at];
        let alone = self.names.iter().filter(|other| leaf(other).eq_ignore_ascii_case(leaf(name))).count() == 1;
        if alone { leaf(name).to_string() } else { name.clone() }
    }
}

/// What the line is doing, as last read from what is typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    #[default]
    Back,
    Title,
    Heading,
    Find,
    Change,
    Rename,
    Set,
    Earlier,
    /// One of the words that act at Enter and show nothing before it.
    Word,
}

/// A text of the program's own that is on the page for reading.
#[derive(Clone, Debug)]
struct Reading {
    keys: bool,
    /// Where each of its sections begins: the line, and the title.
    sections: Vec<(usize, &'static str)>,
}

/// A value of an option being tried.
#[derive(Clone, Debug)]
struct Tried {
    /// Everything as it was before.
    was: Snapshot,
    option: &'static str,
    /// The value the option had before.
    value: String,
}

/// A name offered after `[[` on the page.
#[derive(Clone, Debug)]
struct Offer {
    line: usize,
    /// The byte just after the brackets.
    start: usize,
    /// What is drawn after the cursor.
    rest: String,
    /// What Tab writes between the brackets.
    name: String,
}

/// The ok line.
#[derive(Default)]
pub struct Prompt {
    open: bool,
    typed: String,
    /// The line's cursor, as a byte of `typed`.
    caret: usize,
    mode: Mode,
    /// When the line was last left empty, and the list of what it
    /// understands once that has lasted.
    empty_since: u64,
    hint: Option<String>,
    /// A mistake's `?` and its sentence, until the next key.
    mistake: Option<String>,
    /// What Enter will do now.
    sentence: String,
    pane: Option<Pane>,
    rows: Vec<Entry>,
    /// What each row stands for, row for row.
    stands: Vec<Stands>,
    bar: Option<usize>,
    hits: Vec<Span>,
    /// Which hit the cursor is on.
    at_hit: usize,
    /// The hits as the page is to show them: all in inverse video but the
    /// first letter of the one the cursor is on, which the hollow box
    /// marks instead, so that one hit among several can be told apart.
    marked: Vec<Span>,
    /// Where the cursor was when the line took it.
    origin: Pos,
    /// The name of the open text, as links have it.
    own: Option<String>,
    /// The cursor has been sent somewhere in the text since.
    away: bool,
    /// What is on the page in place of the text.
    shown: Option<Stands>,
    /// When the page is to show what is under the bar.
    rest_until: Option<u64>,
    reading: Option<Reading>,
    tried: Option<Tried>,
    /// The option last set, where the bar waits in the table.
    last_set: Option<&'static str>,
    /// The phrase last looked for, for F3 afterwards.
    found: String,
    /// Changes made one at a time since the line was opened.
    changed: usize,
    offer: Option<Offer>,
}

/// The sentence for a state in which Enter could do nothing.
fn not_yet(why: impl AsRef<str>) -> String {
    format!("{NOT_YET} {}", why.as_ref())
}

/// DESIGN.md's `Enter keeps measure at 66. Esc puts it back to 72. The
/// default is 66.`, with as much of it as the say row has room for.
fn keeps(name: &str, value: &str, was: &str, default: &str, room: usize) -> String {
    let keep = format!("Enter keeps {name} at {value}.");
    let back = if value == was { String::new() } else { format!(" Esc puts it back to {was}.") };
    let whole = format!("{keep}{back} The default is {default}.");
    let shorter = format!("{keep}{back}");
    [whole, shorter].into_iter().find(|sentence| str_width(sentence) <= room).unwrap_or(keep)
}

/// As much of [`UNDERSTOOD`] as the machine line has room for, cut between
/// two of the things it lists and never inside one.
fn understood(app: &App) -> String {
    let grid = app.grid();
    let room = (grid.text.last + 1 - grid.typing_col()).max(0) as usize;
    let mut shown = String::new();
    for entry in UNDERSTOOD.split("   ") {
        let longer = if shown.is_empty() { entry.to_string() } else { format!("{shown}   {entry}") };
        if str_width(&longer) > room {
            break;
        }
        shown = longer;
    }
    shown
}

/// A heading or the opening of a paragraph as a sentence names it: as many
/// whole words as there is room for, without the stop it may end in.
fn named(title: &str, room: usize) -> &str {
    fit_words(title, room).trim_end_matches(['.', ',', ';', ':', '?'])
}

/// What a line of the text is called when it is somewhere to go: a
/// heading by its title, a paragraph by its opening words, as the labels
/// call them.
fn place_name(line: &str) -> &str {
    match markup::heading(line) {
        Some((_, title)) => title,
        None => line.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '>' | '-' | '*' | '+' | '#')),
    }
}

impl Prompt {
    /// The cells the say row has for a sentence.
    fn room(app: &App) -> usize {
        app.grid().measure.max(1) as usize
    }

    /// The sentence for a place in the text the bar is on. A heading is
    /// named whole when there is room; a paragraph by as much of its
    /// opening as its label shows.
    fn goes_to(app: &App, line: usize) -> String {
        let text = app.page().doc().line(line);
        let room = if markup::heading(text).is_some() { Prompt::room(app).saturating_sub(16) } else { OPENING_CELLS };
        format!("Enter goes to {}.", named(place_name(text), room))
    }

    /// A row for the labels: the words cut where the pane ends, between
    /// two of them, as the labels themselves are.
    fn label(app: &App, depth: usize, words: &str, lit: bool) -> Entry {
        let grid = app.grid();
        let wide = grid.labels.map_or(grid.measure, |cols| cols.width()).max(0) as usize;
        Entry { depth, name: fit_words(words, wide.saturating_sub(depth * 2)).to_string(), value: String::new(), lit }
    }

    /// What the row under the bar stands for.
    fn under_bar(&self) -> Option<&Stands> {
        self.bar.and_then(|bar| self.stands.get(bar))
    }

    /// The text under the bar, by name.
    fn text_under_bar(&self) -> Option<String> {
        match self.under_bar() {
            Some(Stands::Text(name)) => Some(name.clone()),
            _ => None,
        }
    }

    /// Puts the text, the cursor and the paper back as they were when the
    /// line took the cursor, and goes on holding them.
    fn home(&mut self, app: &mut App) {
        if self.away || self.shown.is_some() || self.reading.is_some() {
            app.restore();
            app.hold_place();
        }
        self.away = false;
        self.shown = None;
        self.reading = None;
        self.rest_until = None;
    }

    /// Sends the cursor somewhere in the text, to be put back by Esc.
    fn go(&mut self, app: &mut App, pos: Pos) {
        if self.shown.is_some() || self.reading.is_some() {
            self.home(app);
        }
        app.go(pos);
        self.away = true;
    }

    /// Takes back a value that was only being tried. If one of the chords
    /// that keep a setting was pressed in the meantime, what it wrote had
    /// the tried value in it, so the settings are written again as they
    /// now stand.
    fn untry(&mut self, app: &mut App, host: &mut dyn Host) {
        if let Some(tried) = self.tried.take() {
            settings::put_back(app, &tried.was, host);
            app.keep_settings();
        }
    }

    /// Lets go of the page and closes the line. With `keep` what was
    /// tried on the page stays; otherwise everything is put back.
    fn finish(&mut self, app: &mut App, keep: bool) {
        if keep {
            app.keep_place();
        } else {
            app.restore();
        }
        self.reset();
    }

    /// Everything as it is while the line is closed. The phrase last
    /// looked for and the option last set outlast the closing.
    fn reset(&mut self) {
        let (last_set, found) = (self.last_set, std::mem::take(&mut self.found));
        *self = Prompt { last_set, found, ..Prompt::default() };
    }

    /// Answers a dead end with `?` and its reason.
    fn wrong(&mut self) {
        let reason = self.sentence.strip_prefix(NOT_YET).map_or(self.sentence.as_str(), str::trim);
        self.mistake = Some(format!("? {reason}"));
    }

    /// Works out afresh, from what is typed, what the line shows.
    fn understand(&mut self, app: &mut App, host: &mut dyn Host) {
        let now = host.now_ms();
        self.rows.clear();
        self.stands.clear();
        self.hits.clear();
        self.marked.clear();
        self.bar = None;
        self.pane = None;
        self.at_hit = 0;
        let typed = std::mem::take(&mut self.typed);
        let asked = ask(&typed);
        if !matches!(asked, Ask::Set(_)) {
            self.untry(app, host);
        }
        if asked != Ask::Back && self.reading.is_some() {
            self.home(app);
        }
        match asked {
            Ask::Back => self.back(app),
            Ask::Title(title) => self.title(title, app, now),
            Ask::Heading(letters) => self.heading(letters, app),
            Ask::Slash(rest) => self.slash(rest, app),
            Ask::Rename(title) => self.rename(title, app),
            Ask::Set(rest) => self.set(rest, app, host),
            Ask::Earlier => self.earlier(app, now),
            Ask::Discard => self.word(app, String::from("Enter moves this text to .discarded. Nothing is erased.")),
            Ask::Copy => {
                let copied = count::words(&pages::plain(app.text().doc()));
                self.word(app, format!("Enter copies {} with the marks removed.", words(copied)));
            }
            Ask::Keys => self.word(app, String::from("Enter shows the keys on the page.")),
            Ask::Notices => self.word(app, String::from("Enter shows the licences of the faces and code in the .exe.")),
            Ask::Quit => self.word(app, String::from("Enter leaves. Nothing can be lost.")),
        }
        self.typed = typed;
    }

    /// One of the words that act at Enter: nothing is shown but what Enter
    /// will do.
    fn word(&mut self, app: &mut App, sentence: String) {
        self.mode = Mode::Word;
        self.home(app);
        self.sentence = sentence;
    }

    /// The empty line: the contents as they are, the bar on the text he
    /// was in before this one.
    fn back(&mut self, app: &mut App) {
        self.mode = Mode::Back;
        if self.reading.is_some() {
            let name = app.session().name();
            self.sentence = if name.is_empty() { String::from("Enter goes back to where you were.") } else { format!("Enter goes back to where you were, in {name}.") };
            self.sections(app);
            return;
        }
        self.home(app);
        let texts = Texts::read(app);
        let (rows, stands) = texts.rows(|_| true);
        let previous = app.session().previous().map(str::to_string);
        self.bar = previous.as_ref().and_then(|previous| stands.iter().position(|row| matches!(row, Stands::Text(name) if name == previous)));
        self.own = texts.open.map(|open| texts.names[open].clone());
        (self.rows, self.stands) = (rows, stands);
        self.pane = Some(Pane::Contents);
        self.sentence = match (&previous, self.bar) {
            (Some(previous), Some(_)) => format!("Enter goes back to where you were, in {}.", leaf(previous)),
            _ => String::from("Enter goes back to where you were."),
        };
    }

    /// A title: the texts it finds in the contents, the bar on the best.
    fn title(&mut self, title: &str, app: &mut App, now: u64) {
        self.mode = Mode::Title;
        self.pane = Some(Pane::Contents);
        let texts = Texts::read(app);
        self.own = texts.open.map(|open| texts.names[open].clone());
        let found = texts.find(title);
        if found.is_empty() {
            self.home(app);
            self.sentence = format!("Enter starts a new text called {}.", title.trim());
            return;
        }
        (self.rows, self.stands, self.bar) = texts.found(&found);
        self.say_bar();
        self.rest(now);
    }

    /// The sentence for the row the bar is on, in a list of texts or of
    /// versions.
    fn say_bar(&mut self) {
        let sentence = match (self.under_bar(), self.bar.and_then(|bar| self.rows.get(bar))) {
            (Some(Stands::Text(name)), _) => format!("Enter opens {}.", name.replace('/', "\\")),
            (Some(Stands::Version(_)), Some(row)) => format!("Enter brings back {}. The present one is kept too.", row.name),
            _ => return,
        };
        self.sentence = sentence;
    }

    /// Starts the wait before the page shows what is under the bar, unless
    /// that is what it shows already.
    fn rest(&mut self, now: u64) {
        let own = self.own.clone().map(Stands::Text);
        let wanted = self.under_bar().filter(|under| Some(*under) != own.as_ref()).cloned();
        self.rest_until = (wanted != self.shown).then(|| now.saturating_add(REST_MS));
    }

    /// The bar has rested: the page shows what is under it.
    fn show_under_bar(&mut self, app: &mut App) {
        let label = self.bar.and_then(|bar| self.rows.get(bar)).map(|row| row.name.clone()).unwrap_or_default();
        match self.under_bar().cloned() {
            Some(Stands::Text(name)) if Some(&name) != self.own.as_ref() => match app.session_mut().peek(&name) {
                Some(text) => {
                    app.show(leaf(&name), &text, -1);
                    self.shown = Some(Stands::Text(name));
                }
                None => self.home(app),
            },
            Some(Stands::Version(row)) => match app.session_mut().version(row) {
                Some(text) => {
                    app.show(&label, &text, -1);
                    self.shown = Some(Stands::Version(row));
                }
                None => self.home(app),
            },
            _ => self.home(app),
        }
    }

    /// `#`: the headings of the text in the labels, or with none its
    /// paragraphs, and the page following the bar.
    fn heading(&mut self, letters: &str, app: &mut App) {
        self.mode = Mode::Heading;
        self.home(app);
        self.pane = Some(Pane::Labels);
        let page = app.page();
        let doc = page.doc();
        let mut places: Vec<(usize, String)> = markup::outline(doc.lines()).into_iter().filter(|heading| !heading.title.is_empty()).map(|heading| (heading.line, heading.title)).collect();
        let headed = !places.is_empty();
        if !headed {
            // A text with no headings is walked by its paragraphs, each
            // called by its opening words, as the labels call them.
            let blank = |line: usize| page.kind(line) == LineKind::Blank;
            places = (0..doc.line_count()).filter(|&line| !blank(line) && (line == 0 || blank(line - 1))).map(|line| (line, place_name(doc.line(line)).to_string())).filter(|(_, words)| !words.is_empty()).collect();
        }
        let current = places.iter().rposition(|(line, _)| *line <= self.origin.line);
        let letters = letters.trim();
        let parts: Vec<&str> = letters.split_whitespace().collect();
        let scores: Vec<Option<i32>> = places.iter().map(|(_, title)| has_words(title, &parts).then(|| fuzzy(letters, title).map_or(0, |found| found.score))).collect();
        // With nothing typed the bar waits on the section he is in.
        let best = if letters.is_empty() {
            current.or((!places.is_empty()).then_some(0))
        } else {
            let top = scores.iter().flatten().max().copied();
            top.and_then(|top| scores.iter().position(|score| *score == Some(top)))
        };
        for (at, (line, title)) in places.iter().enumerate().filter(|(at, _)| scores[*at].is_some()) {
            if Some(at) == best {
                self.bar = Some(self.rows.len());
            }
            self.rows.push(Prompt::label(app, 0, title, Some(at) == current));
            self.stands.push(Stands::Line(*line));
        }
        match best.and_then(|best| places.get(best)) {
            Some((line, _)) => {
                let line = *line;
                self.sentence = Prompt::goes_to(app, line);
                self.go(app, Pos::new(line, 0));
            }
            None if places.is_empty() => self.sentence = not_yet("This text has nothing to go to."),
            None if headed => self.sentence = not_yet(format!("No heading reads {letters}.")),
            None => self.sentence = not_yet(format!("No paragraph starts {letters}.")),
        }
    }

    /// `/`: a phrase found in the text, or a change to make to it.
    fn slash(&mut self, rest: &str, app: &mut App) {
        self.home(app);
        self.pane = Some(Pane::Page);
        let asked = slash(rest);
        let phrase = match &asked {
            Slash::Find(phrase) => {
                self.mode = Mode::Find;
                phrase.as_str()
            }
            Slash::Change { old, .. } => {
                self.mode = Mode::Change;
                old.as_str()
            }
            Slash::Bad => {
                self.mode = Mode::Change;
                self.sentence = not_yet("Only g may follow the last slash.");
                return;
            }
        };
        if phrase.is_empty() {
            self.sentence = not_yet("Type the phrase to find after the slash.");
            return;
        }
        let doc = app.page().doc();
        self.hits = find_all(doc, phrase, has_capital(phrase));
        if self.hits.is_empty() {
            self.sentence = not_yet(format!("Nothing here reads {phrase}."));
            return;
        }
        // A change that would leave every hit reading as it does is none.
        if matches!(&asked, Slash::Change { new, .. } if self.hits.iter().all(|hit| doc.slice(*hit) == *new)) {
            self.hits.clear();
            self.sentence = not_yet("That would change nothing.");
            return;
        }
        // The nearest after the cursor comes to the platen.
        self.at_hit = self.hits.iter().position(|hit| hit.start >= self.origin).unwrap_or(0);
        let to = self.hits[self.at_hit].start;
        self.go(app, to);
        self.mark_hits(app);
        self.say_hits(&asked);
    }

    /// Works out how the hits are shown, now that the cursor is on one.
    fn mark_hits(&mut self, app: &App) {
        self.marked = self.hits.clone();
        if let Some(hit) = self.marked.get_mut(self.at_hit) {
            let line = app.page().doc().line(hit.start.line);
            let first = line.get(hit.start.col..).and_then(|rest| rest.chars().next()).map_or(0, char::len_utf8);
            hit.start.col = (hit.start.col + first).min(hit.end.col);
        }
    }

    /// The sentence for the hit the cursor is on.
    fn say_hits(&mut self, asked: &Slash) {
        let (at, all) = (self.at_hit + 1, self.hits.len());
        self.sentence = match asked {
            Slash::Find(_) if all == 1 => String::from("1 of 1. Enter stays here."),
            Slash::Find(_) => format!("{at} of {all}. Up and Down step through them. Enter stays here."),
            Slash::Change { new, all: false, .. } if new.is_empty() => format!("{at} of {all}. Enter takes this one out and moves on."),
            Slash::Change { new, all: false, .. } => format!("{at} of {all}. Enter changes this one to {new} and moves on."),
            Slash::Change { new, .. } if new.is_empty() && all == 1 => String::from("Enter takes the only one out. Ctrl Z takes it back."),
            Slash::Change { new, .. } if new.is_empty() => format!("Enter takes all {all} out. Ctrl Z takes them all back."),
            Slash::Change { new, .. } if all == 1 => format!("Enter changes the only one to {new}. Ctrl Z takes it back."),
            Slash::Change { new, .. } => format!("Enter changes all {all} to {new}. Ctrl Z takes them all back."),
            Slash::Bad => return,
        };
    }

    /// `rename`: the contents with the open text's row under the bar,
    /// already reading as it will.
    fn rename(&mut self, title: &str, app: &mut App) {
        self.mode = Mode::Rename;
        self.home(app);
        self.pane = Some(Pane::Contents);
        let texts = Texts::read(app);
        let (mut rows, stands) = texts.rows(|_| true);
        self.bar = rows.iter().position(|row| row.lit);
        if let Some(row) = self.bar.and_then(|bar| rows.get_mut(bar)).filter(|_| !title.is_empty()) {
            row.name = leaf(&title.replace('\\', "/")).to_string();
        }
        (self.rows, self.stands) = (rows, stands);
        self.sentence = match app.session().links_here() {
            _ if title.is_empty() => not_yet("Type the new title after rename."),
            0 => String::from("Enter renames this text."),
            1 => String::from("Enter renames this text and mends 1 link to it."),
            links => format!("Enter renames this text and mends {links} links to it."),
        };
    }

    /// `earlier`: the kept versions in the contents, newest first.
    fn earlier(&mut self, app: &mut App, now: u64) {
        self.mode = Mode::Earlier;
        if !matches!(self.shown, Some(Stands::Version(_))) {
            self.home(app);
        }
        self.own = None;
        self.pane = Some(Pane::Contents);
        self.rows = app.session_mut().versions();
        self.stands = (0..self.rows.len()).map(Stands::Version).collect();
        if self.rows.is_empty() {
            self.home(app);
            self.sentence = not_yet("Nothing earlier is kept of this text.");
            return;
        }
        self.bar = Some(0);
        self.say_bar();
        self.rest(now);
    }

    /// What was typed for an option, read. Nothing typed yet stands for
    /// the value the option had, so that Enter keeps it as it was.
    fn read(app: &App, opt: &Opt, typed: &str, was: &str, host: &mut dyn Host) -> Read {
        if typed.is_empty() && opt.kind != Kind::Folder {
            return Read::Value(was.to_string());
        }
        settings::read(app, opt, typed, host)
    }

    /// The table of options, with the values one of them is walked
    /// through set in under it when `open` names it and the value it had.
    fn table(&mut self, app: &App, open: Option<(&Opt, &str)>, host: &mut dyn Host) {
        for opt in &OPTIONS {
            self.rows.push(Entry { depth: 0, name: opt.name.to_string(), value: settings::shown(&settings::value(app, opt), VALUE_CELLS), lit: false });
            self.stands.push(Stands::Option(opt.name));
            let Some((_, kept)) = open.filter(|(open, _)| open.name == opt.name) else { continue };
            for choice in settings::choices(app, opt, host).unwrap_or_default() {
                self.rows.push(Entry { depth: 1, lit: choice == kept, name: choice.clone(), value: String::new() });
                self.stands.push(Stands::Choice(choice));
            }
        }
    }

    /// `set`: the table of options in the contents, and the value typed
    /// for one of them tried on the whole screen as soon as it is one the
    /// option can take.
    fn set(&mut self, rest: &str, app: &mut App, host: &mut dyn Host) {
        self.mode = Mode::Set;
        self.home(app);
        self.pane = Some(Pane::Contents);
        let rest = rest.trim_start();
        let Some((name, typed)) = rest.split_once(char::is_whitespace) else {
            // Still naming the option: the bar is on the one the letters
            // are heading for, or waits where the last one was set.
            self.untry(app, host);
            let heading_for = if rest.is_empty() { self.last_set.and_then(settings::find).or(OPTIONS.first()) } else { settings::starting(rest) };
            self.table(app, None, host);
            self.bar = heading_for.and_then(|opt| self.stands.iter().position(|row| *row == Stands::Option(opt.name)));
            self.sentence = match heading_for {
                Some(opt) => match settings::shown(&settings::value(app, opt), 30) {
                    value if value.is_empty() => format!("Enter changes {}.", opt.name),
                    value => format!("Enter changes {}. It is {value} now.", opt.name),
                },
                None => not_yet(format!("No option is called {rest}.")),
            };
            return;
        };
        let Some(opt) = settings::find(name) else {
            self.untry(app, host);
            self.table(app, None, host);
            self.sentence = not_yet(format!("No option is called {name}."));
            return;
        };
        if self.tried.as_ref().is_some_and(|tried| tried.option != opt.name) {
            self.untry(app, host);
        }
        let was = self.tried.as_ref().map_or_else(|| settings::value(app, opt), |tried| tried.value.clone());
        let mut read = Prompt::read(app, opt, typed.trim(), &was, host);
        match &read {
            // The folder is not tried: writing somewhere else is an act.
            Read::Value(_) if opt.kind == Kind::Folder => {}
            Read::Value(value) if settings::value(app, opt) != *value => {
                let before = settings::snapshot(app);
                if settings::apply(app, opt, value, host) {
                    self.tried.get_or_insert(Tried { was: before, option: opt.name, value: was.clone() });
                } else {
                    read = Read::No;
                }
            }
            Read::Value(_) => {}
            Read::NewLook(_) | Read::No => self.untry(app, host),
        }
        self.table(app, Some((opt, &was)), host);
        let at_value = match &read {
            Read::Value(value) => self.stands.iter().position(|row| matches!(row, Stands::Choice(choice) if choice == value)),
            _ => None,
        };
        self.bar = at_value.or_else(|| self.stands.iter().position(|row| *row == Stands::Option(opt.name)));
        self.sentence = match &read {
            Read::Value(folder) if opt.kind == Kind::Folder => format!("Enter writes in {} from now on.", settings::leaf_of(folder)),
            Read::Value(value) => keeps(opt.name, value, &was, &settings::shown(&settings::default(opt, host), 30), Prompt::room(app)),
            Read::NewLook(name) => format!("Enter keeps these colours as {name}."),
            Read::No => not_yet(settings::takes(opt)),
        };
    }

    /// The sections of the text being read, in the labels, the one at the
    /// top of the sheet in ink.
    fn sections(&mut self, app: &App) {
        let Some(reading) = &self.reading else { return };
        self.rows.clear();
        self.stands.clear();
        self.bar = None;
        self.pane = None;
        if reading.sections.is_empty() {
            return;
        }
        let top = app.view().top() + 1;
        let layout = app.page().layout();
        let lines = app.page().doc().line_count();
        let current = reading.sections.iter().rposition(|(line, _)| *line < lines && layout.first_row(*line) as i64 <= top);
        // A section named for the one above it, as "Gallant, from the
        // FreeBSD Project" is, is set in under it by what it adds.
        let mut above = "";
        for (at, (_, title)) in reading.sections.iter().enumerate() {
            let added = title.strip_prefix(above).and_then(|rest| rest.strip_prefix(", ")).filter(|_| !above.is_empty());
            match added {
                Some(added) => self.rows.push(Prompt::label(app, 1, added, Some(at) == current)),
                None => {
                    above = title;
                    self.rows.push(Prompt::label(app, 0, title, Some(at) == current));
                }
            }
        }
        self.stands = vec![Stands::Nothing; self.rows.len()];
        self.pane = Some(Pane::Labels);
    }

    /// Puts the keys or the notices on the page for reading. The line is
    /// left empty under them, so Enter and Esc both go back.
    fn read_page(&mut self, keys: bool, app: &mut App, host: &mut dyn Host) {
        self.home(app);
        let sections = if keys {
            app.show("Keys", pages::KEYS, -1);
            Vec::new()
        } else {
            let (text, sections) = pages::notices();
            app.show("Notices", &text, pages::NOTICES_TOP);
            sections
        };
        self.reading = Some(Reading { keys, sections });
        self.typed.clear();
        self.caret = 0;
        self.left_empty(host.now_ms());
        self.understand(app, host);
    }

    /// The line has just become empty: the wait for the list of what it
    /// understands starts again.
    fn left_empty(&mut self, now: u64) {
        self.empty_since = now;
        self.hint = None;
    }

    /// What is typed has changed.
    fn edited(&mut self, was_empty: bool, app: &mut App, host: &mut dyn Host) {
        if self.typed.is_empty() && !was_empty {
            self.left_empty(host.now_ms());
        } else if !self.typed.is_empty() {
            self.hint = None;
        }
        self.understand(app, host);
    }

    /// Replaces what is typed, the cursor after it.
    fn retype(&mut self, typed: String, app: &mut App, host: &mut dyn Host) {
        let was_empty = self.typed.is_empty();
        self.typed = typed;
        self.caret = self.typed.len();
        self.edited(was_empty, app, host);
    }

    /// The row the bar goes to from where it is: the next one, that way,
    /// that stands for something. From nowhere it sets out from the open
    /// text's row.
    fn next_row(&self, way: Way) -> Option<usize> {
        let from = self.bar.or_else(|| self.rows.iter().position(|row| row.lit));
        let stands = |at: &usize| self.stands.get(*at).is_some_and(|row| *row != Stands::Nothing);
        match (from, way) {
            (Some(from), Way::On) => (from + 1..self.rows.len()).find(stands),
            (Some(from), Way::Back) => (0..from).rev().find(stands),
            (None, Way::On) => (0..self.rows.len()).find(stands),
            (None, Way::Back) => (0..self.rows.len()).rev().find(stands),
        }
    }

    /// Up and Down: they try.
    fn arrow(&mut self, way: Way, app: &mut App, host: &mut dyn Host) {
        if self.reading.is_some() {
            app.scroll_page(if way == Way::Back { -1 } else { 1 });
            self.sections(app);
            return;
        }
        match self.mode {
            Mode::Back | Mode::Title | Mode::Earlier => {
                let Some(next) = self.next_row(way) else { return };
                self.bar = Some(next);
                self.say_bar();
                self.rest(host.now_ms());
            }
            Mode::Heading => {
                let Some(next) = self.next_row(way) else { return };
                self.bar = Some(next);
                if let Some(Stands::Line(line)) = self.stands.get(next).cloned() {
                    self.sentence = Prompt::goes_to(app, line);
                    self.go(app, Pos::new(line, 0));
                }
            }
            Mode::Find | Mode::Change if !self.hits.is_empty() => {
                let all = self.hits.len();
                self.at_hit = if way == Way::Back { (self.at_hit + all - 1) % all } else { (self.at_hit + 1) % all };
                let to = self.hits[self.at_hit].start;
                self.go(app, to);
                self.mark_hits(app);
                if let Ask::Slash(rest) = ask(&self.typed) {
                    let asked = slash(rest);
                    self.say_hits(&asked);
                }
            }
            Mode::Set => self.set_arrow(way, app, host),
            _ => {}
        }
    }

    /// Up and Down in `set`: the bar walks the table and the name on the
    /// line follows it; in a list it walks the values; a number is nudged.
    fn set_arrow(&mut self, way: Way, app: &mut App, host: &mut dyn Host) {
        let typed = self.typed.clone();
        let Ask::Set(rest) = ask(&typed) else { return };
        let rest = rest.trim_start();
        let next = match rest.split_once(char::is_whitespace) {
            None => {
                let at = self.bar.map_or(0, |bar| if way == Way::Back { bar.saturating_sub(1) } else { bar + 1 });
                format!("set {}", OPTIONS[at.min(OPTIONS.len() - 1)].name)
            }
            Some((name, value)) => {
                let Some(opt) = settings::find(name) else { return };
                match settings::choices(app, opt, host) {
                    Some(choices) if !choices.is_empty() => {
                        let at = match self.under_bar() {
                            Some(Stands::Choice(choice)) => choices.iter().position(|other| other == choice),
                            _ => None,
                        };
                        let to = match (at, way) {
                            (Some(at), Way::Back) => at.saturating_sub(1),
                            (Some(at), Way::On) => (at + 1).min(choices.len() - 1),
                            (None, _) => 0,
                        };
                        format!("set {} {}", opt.name, choices[to])
                    }
                    // Up is more, as it is on every dial.
                    _ => match settings::nudge(app, opt, value, way == Way::Back) {
                        Some(value) => format!("set {} {value}", opt.name),
                        None => return,
                    },
                }
            }
        };
        self.retype(next, app, host);
    }

    /// Enter: it keeps.
    fn enter(&mut self, app: &mut App, host: &mut dyn Host) {
        let typed = self.typed.clone();
        let asked = ask(&typed);
        if let Ask::Set(rest) = asked {
            return self.enter_set(rest, app, host);
        }
        if self.sentence.starts_with(NOT_YET) {
            return self.wrong();
        }
        match asked {
            Ask::Back if self.reading.is_some() => self.finish(app, false),
            Ask::Back | Ask::Title(_) if self.text_under_bar().is_some_and(|name| Some(&name) == self.own.as_ref()) => {
                // The text that is open already: going to it is going back
                // to where he was in it, with everything it can undo.
                self.finish(app, false);
            }
            Ask::Back => match self.text_under_bar() {
                Some(name) => self.open_named(&name, false, app, host),
                None => self.finish(app, false),
            },
            Ask::Title(title) => match self.text_under_bar() {
                Some(name) => self.open_named(&name, false, app, host),
                None => self.open_named(title, true, app, host),
            },
            Ask::Heading(_) => self.finish(app, true),
            Ask::Slash(rest) => match slash(rest) {
                Slash::Find(phrase) => {
                    self.found = phrase;
                    self.finish(app, true);
                }
                Slash::Change { old, new, all: false } => self.change_one(&old, &new, app, host),
                Slash::Change { old, new, all: true } => self.change_all(&old, &new, app, host),
                Slash::Bad => self.wrong(),
            },
            Ask::Rename(title) => match app.rename(title, host) {
                Ok(mended) => {
                    let now = leaf(&title.replace('\\', "/")).to_string();
                    self.finish(app, false);
                    app.say(match mended {
                        0 => format!("Renamed to {now}."),
                        1 => format!("Renamed to {now} and mended 1 link."),
                        links => format!("Renamed to {now} and mended {links} links."),
                    });
                }
                Err(sentence) => self.mistake = Some(sentence),
            },
            Ask::Earlier => {
                let Some(Stands::Version(row)) = self.under_bar().cloned() else { return self.wrong() };
                let label = self.rows.get(row).map(|row| row.name.clone()).unwrap_or_default();
                match app.bring_back(row, host) {
                    Ok(()) => {
                        self.finish(app, false);
                        app.say(format!("Brought back {label}. The present one is kept too."));
                    }
                    Err(sentence) => self.mistake = Some(sentence),
                }
            }
            Ask::Discard => match app.discard(host) {
                Ok(()) => {
                    self.finish(app, false);
                    app.say("Moved to .discarded. Nothing was erased.");
                }
                Err(sentence) => self.mistake = Some(sentence),
            },
            Ask::Copy => {
                let copy = pages::plain(app.text().doc());
                // Line breaks go to the clipboard the way Windows writes them.
                host.clipboard_set(&copy.replace('\n', "\r\n"));
                self.finish(app, false);
                app.say(format!("Copied {} with the marks removed.", words(count::words(&copy))));
            }
            Ask::Keys => self.read_page(true, app, host),
            Ask::Notices => self.read_page(false, app, host),
            Ask::Quit => {
                self.finish(app, false);
                app.leave(host);
            }
            Ask::Set(_) => {}
        }
    }

    /// Opens a text for good, or says why it could not be.
    fn open_named(&mut self, name: &str, start: bool, app: &mut App, host: &mut dyn Host) {
        match app.open_named(name, start, host) {
            Ok(()) => self.finish(app, false),
            Err(sentence) => self.mistake = Some(sentence),
        }
    }

    /// Enter in `set`: on an option's row it goes on to its value; with a
    /// value it keeps it, and the line stays at `ok set` for the next one.
    fn enter_set(&mut self, rest: &str, app: &mut App, host: &mut dyn Host) {
        let rest = rest.trim_start();
        let nameless = |name: &str| Some(format!("? No option called {name}. The nearest is {}.", settings::nearest(name)));
        let Some((name, typed)) = rest.split_once(char::is_whitespace) else {
            match self.under_bar() {
                Some(Stands::Option(option)) => {
                    let next = format!("set {option} ");
                    self.retype(next, app, host);
                }
                _ => self.mistake = nameless(rest),
            }
            return;
        };
        let Some(opt) = settings::find(name) else {
            self.mistake = nameless(name);
            return;
        };
        if self.sentence.starts_with(NOT_YET) {
            return self.wrong();
        }
        let was = self.tried.as_ref().map_or_else(|| settings::value(app, opt), |tried| tried.value.clone());
        match Prompt::read(app, opt, typed.trim(), &was, host) {
            Read::Value(folder) if opt.kind == Kind::Folder => match app.use_folder(Path::new(&folder), host) {
                Ok(()) => {
                    // Another text is on the page now: that is the place to
                    // put back from here on.
                    app.hold_place();
                    self.origin = app.page().cursor.head;
                    (self.away, self.shown) = (false, None);
                }
                Err(sentence) => {
                    self.mistake = Some(sentence);
                    return;
                }
            },
            // Already on the screen since it was typed.
            Read::Value(_) => {}
            Read::NewLook(name) => app.keep_look(&name),
            Read::No => return self.wrong(),
        }
        self.tried = None;
        app.keep_settings();
        self.last_set = Some(opt.name);
        self.retype(String::from("set"), app, host);
    }

    /// `/old/new` and Enter: this one is changed, and the cursor moves on
    /// to the next.
    fn change_one(&mut self, old: &str, new: &str, app: &mut App, host: &mut dyn Host) {
        let Some(hit) = self.hits.get(self.at_hit).copied() else { return self.wrong() };
        if app.page().read_only {
            self.mistake = Some(String::from(READ_ONLY));
            return;
        }
        self.found = old.to_string();
        if app.edit(host, |page, clock| page.replace(hit, new, EditKind::Other, None, clock)) {
            self.changed += 1;
        }
        // What was changed is kept: this is the place to put back to now.
        app.keep_place();
        app.hold_place();
        self.origin = app.page().cursor.head.max(hit.end);
        self.away = false;
        self.hits = find_all(app.page().doc(), old, has_capital(old));
        if self.hits.is_empty() {
            let changed = self.changed;
            self.finish(app, true);
            app.say(if changed == 1 { String::from("Changed 1. That was the only one.") } else { format!("Changed {changed}. That was the last one.") });
            return;
        }
        self.at_hit = self.hits.iter().position(|next| next.start >= self.origin).unwrap_or(0);
        let to = self.hits[self.at_hit].start;
        self.go(app, to);
        self.mark_hits(app);
        self.say_hits(&Slash::Change { old: old.to_string(), new: new.to_string(), all: false });
    }

    /// `/old/new/g` and Enter: every one is changed in a single step, so
    /// that one Ctrl Z takes them all back.
    fn change_all(&mut self, old: &str, new: &str, app: &mut App, host: &mut dyn Host) {
        let hits = std::mem::take(&mut self.hits);
        let (Some(first), Some(last)) = (hits.first().map(|hit| hit.start), hits.last().map(|hit| hit.end)) else { return self.wrong() };
        if app.page().read_only {
            self.hits = hits;
            self.mistake = Some(String::from(READ_ONLY));
            return;
        }
        // From where he was, so that the step begins and ends there.
        self.home(app);
        let origin = self.origin;
        let doc = app.page().doc();
        let mut text = String::new();
        let mut at = first;
        for hit in &hits {
            text.push_str(&doc.slice(Span { start: at, end: hit.start }));
            text.push_str(new);
            at = hit.end;
        }
        // The cursor keeps its place among the words of its own line.
        let mut col = origin.col as isize;
        for hit in hits.iter().filter(|hit| hit.start.line == origin.line && hit.start.col < origin.col) {
            let taken = origin.col.min(hit.end.col) - hit.start.col;
            col += new.len() as isize - taken as isize;
        }
        let cursor = Cursor::at(Pos::new(origin.line, col.max(0) as usize));
        let touched: Vec<usize> = hits.iter().map(|hit| hit.start.line).collect();
        let changed = app.edit(host, |page, clock| {
            let stamps: Vec<Day> = (0..page.doc().line_count()).map(|line| page.stamp(line)).collect();
            if !page.replace(Span { start: first, end: last }, &text, EditKind::Other, Some(cursor), clock) {
                return false;
            }
            // One change reaches from the first hit to the last and would
            // mark every line between as written today. Only the lines that
            // held a hit were.
            let stamps: Vec<Day> = stamps.into_iter().enumerate().map(|(line, was)| if touched.binary_search(&line).is_ok() { clock.today } else { was }).collect();
            page.set_stamps(&stamps);
            true
        });
        if !changed {
            self.hits = hits;
            self.mistake = Some(String::from("? That would change nothing."));
            return;
        }
        self.found = old.to_string();
        self.finish(app, true);
        app.say(if hits.len() == 1 { String::from("Changed 1. Ctrl Z takes it back.") } else { format!("Changed {}. Ctrl Z takes them all back.", hits.len()) });
    }

    /// Ctrl Z and Ctrl Y while changes are being made from the line: the
    /// last one is taken back, or made again, and the hits are found anew.
    fn redo_or_undo(&mut self, redo: bool, app: &mut App, host: &mut dyn Host) {
        self.home(app);
        if !(if redo { app.redo(host) } else { app.undo(host) }) {
            return;
        }
        app.keep_place();
        app.hold_place();
        self.origin = app.page().cursor.head;
        self.changed = self.changed.saturating_sub(usize::from(!redo));
        self.understand(app, host);
    }

    /// Moves the line's own cursor by a letter.
    fn caret_to(&mut self, way: Way) {
        let typed = &self.typed;
        self.caret = match way {
            Way::Back => typed.get(..self.caret).and_then(|before| before.char_indices().next_back()).map_or(0, |(at, _)| at),
            Way::On => typed.get(self.caret..).and_then(|after| after.chars().next()).map_or(typed.len(), |ch| self.caret + ch.len_utf8()),
        };
    }

    /// Takes out what is typed between two bytes.
    fn erase(&mut self, from: usize, to: usize, app: &mut App, host: &mut dyn Host) {
        if from >= to || self.typed.get(from..to).is_none() {
            return;
        }
        self.typed.replace_range(from..to, "");
        self.caret = from;
        self.edited(false, app, host);
    }

    /// Puts the name of a text after `[[`, if the cursor is in brackets
    /// that have not been closed. `typing` is set when a character has
    /// just been typed there, which is what wakes the contents; anything
    /// else only keeps an offer that is already made in step.
    fn link(&mut self, app: &mut App, typing: bool) {
        let had = self.offer.take();
        if had.is_some() {
            self.rows.clear();
            self.stands.clear();
            self.pane = None;
            self.bar = None;
        }
        let page = app.page();
        let head = page.cursor.head;
        if page.read_only || page.cursor.has_selection() || matches!(page.kind(head.line), LineKind::Code | LineKind::Fence) {
            return;
        }
        let line = page.doc().line(head.line);
        let Some(before) = line.get(..head.col) else { return };
        let Some(start) = before.rfind("[[").map(|at| at + 2) else { return };
        let since = &before[start..];
        if since.len() > LONGEST_LINK || since.contains(['[', ']', '|', '#']) {
            return;
        }
        let in_step = had.is_some_and(|offer| offer.line == head.line && offer.start == start);
        if !(typing || in_step) {
            return;
        }
        let texts = Texts::read(app);
        let found = texts.find(since);
        let Some(&best) = found.first() else { return };
        (self.rows, self.stands, self.bar) = texts.found(&found);
        self.pane = Some(Pane::Contents);
        let name = texts.link_name(best);
        // Letters that begin the name are completed; anything else is
        // answered with the whole name, set off by a space.
        let begun = name.get(..since.len()).is_some_and(|begun| begun.to_lowercase() == since.to_lowercase());
        let rest = if begun { name[since.len()..].to_string() } else { format!(" {name}") };
        self.offer = Some(Offer { line: head.line, start, rest, name });
    }
}

impl Line for Prompt {
    fn is_open(&self) -> bool {
        self.open
    }

    fn open(&mut self, with: &str, app: &mut App, host: &mut dyn Host) {
        self.reset();
        self.open = true;
        self.typed = with.to_string();
        self.caret = self.typed.len();
        self.origin = app.page().cursor.head;
        app.hold_place();
        self.left_empty(host.now_ms());
        self.understand(app, host);
    }

    fn key(&mut self, key: Key, mods: Mods, app: &mut App, host: &mut dyn Host) -> bool {
        // Whatever was wrong, the next key clears it.
        self.mistake = None;
        if mods.alt {
            return false;
        }
        if mods.ctrl {
            let changing = matches!(self.mode, Mode::Find | Mode::Change);
            match key {
                Key::Char('z') if changing && !mods.shift => self.redo_or_undo(false, app, host),
                Key::Char('y') if changing && !mods.shift => self.redo_or_undo(true, app, host),
                Key::Char('z') if changing => self.redo_or_undo(true, app, host),
                Key::Char('f') if !mods.shift => self.retype(String::from("/"), app, host),
                Key::Char('v') if !mods.shift => {
                    // The first line of the clipboard, as if it were typed.
                    let pasted = host.clipboard_get().unwrap_or_default();
                    let pasted: String = pasted.lines().next().unwrap_or("").chars().filter(|ch| !ch.is_control()).collect();
                    if !pasted.is_empty() && self.typed.len() + pasted.len() <= LONGEST {
                        let was_empty = self.typed.is_empty();
                        self.typed.insert_str(self.caret, &pasted);
                        self.caret += pasted.len();
                        self.edited(was_empty, app, host);
                    }
                }
                Key::Backspace => {
                    // Back over the word before the cursor.
                    let before = self.typed.get(..self.caret).unwrap_or("");
                    let word = before.trim_end().rfind(char::is_whitespace).map_or(0, |at| at + 1);
                    self.erase(word, self.caret, app, host);
                }
                _ => return false,
            }
            return true;
        }
        let reading = self.reading.is_some();
        let screen = (app.grid().text_rows as i64 - 1).max(1);
        match key {
            Key::Escape => {
                self.untry(app, host);
                self.finish(app, false);
            }
            Key::Enter if !mods.shift => self.enter(app, host),
            Key::Up => self.arrow(Way::Back, app, host),
            Key::Down => self.arrow(Way::On, app, host),
            Key::F(3) => self.arrow(if mods.shift { Way::Back } else { Way::On }, app, host),
            Key::F(1) if self.reading.as_ref().is_some_and(|reading| reading.keys) => self.finish(app, false),
            Key::F(1) => self.read_page(true, app, host),
            // A text that is only being shown is read a screen at a time.
            Key::PageUp | Key::PageDown if app.showing() => {
                app.scroll_page(if key == Key::PageUp { -screen } else { screen });
                self.sections(app);
            }
            Key::Home | Key::End if reading => {
                app.scroll_page(if key == Key::Home { -(1 << 40) } else { 1 << 40 });
                self.sections(app);
            }
            Key::Home => self.caret = 0,
            Key::End => self.caret = self.typed.len(),
            Key::Left => self.caret_to(Way::Back),
            Key::Right => self.caret_to(Way::On),
            Key::Backspace => {
                let to = self.caret;
                self.caret_to(Way::Back);
                self.erase(self.caret, to, app, host);
            }
            Key::Delete => {
                let from = self.caret;
                self.caret_to(Way::On);
                let to = self.caret;
                self.caret = from;
                self.erase(from, to, app, host);
            }
            _ => return false,
        }
        true
    }

    fn text(&mut self, ch: char, app: &mut App, host: &mut dyn Host) {
        self.mistake = None;
        if ch.is_control() || self.typed.len() + ch.len_utf8() > LONGEST || self.typed.get(..self.caret).is_none() {
            return;
        }
        let was_empty = self.typed.is_empty();
        self.typed.insert(self.caret, ch);
        self.caret += ch.len_utf8();
        self.edited(was_empty, app, host);
    }

    /// F3 and Shift F3 on the page: on to the next place the phrase last
    /// looked for is written, or back to the one before, going round.
    fn step(&mut self, way: Way, app: &mut App, _host: &mut dyn Host) {
        if self.found.is_empty() {
            app.say("Nothing has been looked for yet. Ctrl F finds a phrase.");
            return;
        }
        let hits = find_all(app.page().doc(), &self.found, has_capital(&self.found));
        let head = app.page().cursor.head;
        let at = match way {
            Way::On => hits.iter().position(|hit| hit.start > head).or((!hits.is_empty()).then_some(0)),
            Way::Back => hits.iter().rposition(|hit| hit.start < head).or(hits.len().checked_sub(1)),
        };
        match at.and_then(|at| hits.get(at).map(|hit| (at, hit.start))) {
            Some((at, start)) => {
                app.go(start);
                app.say(format!("{} of {}. F3 and Shift F3 step through them.", at + 1, hits.len()));
            }
            None => app.say(format!("Nothing here reads {}.", self.found)),
        }
    }

    fn keys(&mut self, app: &mut App, host: &mut dyn Host) {
        self.open("", app, host);
        self.read_page(true, app, host);
    }

    fn typed_on_page(&mut self, app: &mut App, _host: &mut dyn Host) {
        self.link(app, true);
    }

    fn tab(&mut self, app: &mut App, host: &mut dyn Host) -> bool {
        let Some(offer) = self.offer.take() else { return false };
        self.rows.clear();
        self.stands.clear();
        self.pane = None;
        self.bar = None;
        let head = app.page().cursor.head;
        if head.line != offer.line || head.col < offer.start {
            return false;
        }
        // Brackets he has closed already are not closed twice.
        let closed = app.page().doc().line(head.line).get(head.col..).is_some_and(|after| after.starts_with("]]"));
        let after = Cursor::at(Pos::new(offer.line, offer.start + offer.name.len() + 2));
        let text = if closed { offer.name } else { format!("{}]]", offer.name) };
        let clock = app.clock(host);
        app.page_mut().replace(Span { start: Pos::new(offer.line, offer.start), end: head }, &text, EditKind::Other, Some(after), clock)
    }

    fn moved_on_page(&mut self, app: &mut App, _host: &mut dyn Host) {
        if self.open {
            // The wheel may have turned under a text being read.
            self.sections(app);
        } else if self.offer.is_some() {
            self.link(app, false);
        }
    }

    fn tick(&mut self, app: &mut App, host: &mut dyn Host) {
        let now = host.now_ms();
        if self.open && self.typed.is_empty() && self.hint.is_none() && now >= self.empty_since.saturating_add(EMPTY_MS) {
            self.hint = Some(understood(app));
        }
        if self.rest_until.is_some_and(|at| now >= at) {
            self.rest_until = None;
            self.show_under_bar(app);
        }
    }

    fn wake_in_ms(&self, now_ms: u64) -> Option<u64> {
        let hint = (self.open && self.typed.is_empty() && self.hint.is_none()).then(|| self.empty_since.saturating_add(EMPTY_MS).saturating_sub(now_ms));
        let rest = self.rest_until.map(|at| at.saturating_sub(now_ms));
        [hint, rest].into_iter().flatten().min()
    }

    fn typed(&self) -> &str {
        &self.typed
    }

    fn caret(&self) -> usize {
        str_width(self.typed.get(..self.caret).unwrap_or(&self.typed))
    }

    fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    fn say(&self) -> Option<&str> {
        if !self.open {
            return None;
        }
        self.mistake.as_deref().or(Some(self.sentence.as_str()))
    }

    fn answer(&self) -> Option<Answer<'_>> {
        self.pane.map(|pane| Answer { pane, rows: &self.rows, bar: self.bar })
    }

    fn hits(&self) -> &[Span] {
        &self.marked
    }

    fn offer(&self) -> Option<&str> {
        self.offer.as_ref().map(|offer| offer.rest.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_character_or_word_decides() {
        assert_eq!(ask(""), Ask::Back);
        assert_eq!(ask("   "), Ask::Back);
        assert_eq!(ask("the orchard"), Ask::Title("the orchard"));
        assert_eq!(ask("#weak"), Ask::Heading("weak"));
        assert_eq!(ask("/old/new"), Ask::Slash("old/new"));
        assert_eq!(ask("rename Rebuttal notes "), Ask::Rename("Rebuttal notes"));
        assert_eq!(ask("rename"), Ask::Rename(""));
        assert_eq!(ask("set"), Ask::Set(""));
        assert_eq!(ask("SET look amber"), Ask::Set("look amber"));
        assert_eq!(ask("set measure "), Ask::Set("measure "));
        for (word, asked) in [("earlier", Ask::Earlier), ("discard", Ask::Discard), ("copy", Ask::Copy), ("keys", Ask::Keys), ("notices", Ask::Notices), ("quit", Ask::Quit), (" Quit ", Ask::Quit)] {
            assert_eq!(ask(word), asked);
        }
        // A word that acts alone is a title as soon as anything follows it,
        // and the two that take something are not titles at all.
        assert_eq!(ask("copy of the plan"), Ask::Title("copy of the plan"));
        assert_eq!(ask("keys to the orchard"), Ask::Title("keys to the orchard"));
        assert_eq!(ask("settle"), Ask::Title("settle"));
        assert_eq!(ask("renamed"), Ask::Title("renamed"));
    }

    #[test]
    fn what_follows_a_slash_is_a_phrase_or_a_change() {
        let change = |old: &str, new: &str, all: bool| Slash::Change { old: old.to_string(), new: new.to_string(), all };
        assert_eq!(slash("apricots"), Slash::Find(String::from("apricots")));
        assert_eq!(slash(""), Slash::Find(String::new()));
        assert_eq!(slash("old/new"), change("old", "new", false));
        assert_eq!(slash("old/"), change("old", "", false));
        assert_eq!(slash("old/new/"), change("old", "new", false));
        assert_eq!(slash("old/new/g"), change("old", "new", true));
        assert_eq!(slash("old/new/x"), Slash::Bad);
        assert_eq!(slash("old/new/g/"), Slash::Bad);
        assert_eq!(slash("and\\/or/or"), change("and/or", "or", false));
        assert_eq!(slash("a\\b"), Slash::Find(String::from("a\\b")));
    }

    #[test]
    fn a_title_finds_texts_by_its_words_in_any_order() {
        let names = ["Essay plan", "College/Grandfather interview", "College/Orchard notes", "College/The orchard", "Mock trial/Opening statement"];
        let texts = Texts { names: names.iter().map(|name| name.to_string()).collect(), counts: vec![String::from("1"); names.len()], open: Some(3) };
        let found = |title: &str| -> Vec<&str> { texts.find(title).into_iter().map(|at| names[at]).collect() };
        assert_eq!(found("orchard"), ["College/Orchard notes", "College/The orchard"]);
        assert_eq!(found("ORCHARD THE"), ["College/The orchard"]);
        assert_eq!(found("notes orchard"), ["College/Orchard notes"]);
        assert_eq!(found("college orch"), ["College/Orchard notes", "College/The orchard"]);
        assert_eq!(found("college\\orch"), ["College/Orchard notes"]);
        assert_eq!(found("Rebuttal notes"), Vec::<&str>::new());
        // Letters that happen to come in order somewhere in a name find
        // nothing: it is words that are looked for.
        assert_eq!(found("gr"), ["College/Grandfather interview"]);
        assert_eq!(found("cgi"), Vec::<&str>::new());
        assert_eq!(found("").len(), 5);
        // The rows are a contents page of what was found, and the bar is
        // on the best of them wherever that falls.
        let (rows, stands, best) = texts.found(&texts.find("orchard"));
        let read: Vec<(usize, &str, bool)> = rows.iter().map(|row| (row.depth, row.name.as_str(), row.lit)).collect();
        assert_eq!(read, [(0, "College", false), (1, "Orchard notes", false), (1, "The orchard", true)]);
        assert_eq!(stands[0], Stands::Nothing);
        assert_eq!(best, Some(1));
        assert_eq!(texts.link_name(2), "Orchard notes");
    }

    #[test]
    fn a_link_is_written_by_the_whole_name_only_when_two_texts_share_one() {
        let texts = Texts { names: vec![String::from("College/Plan"), String::from("Mock trial/Plan"), String::from("Mock trial/Closing")], counts: Vec::new(), open: None };
        assert_eq!(texts.link_name(0), "College/Plan");
        assert_eq!(texts.link_name(2), "Closing");
    }

    #[test]
    fn the_sentence_for_a_value_gives_way_from_its_end() {
        // DESIGN.md's own sentence is 69 cells, three more than the default
        // measure has, so there it loses its last part and is never cut
        // inside a word.
        assert_eq!(keeps("measure", "66", "72", "66", 72), "Enter keeps measure at 66. Esc puts it back to 72. The default is 66.");
        assert_eq!(keeps("measure", "66", "72", "66", 66), "Enter keeps measure at 66. Esc puts it back to 72.");
        assert_eq!(keeps("measure", "66", "66", "66", 66), "Enter keeps measure at 66. The default is 66.");
        assert_eq!(keeps("look", "register night", "amber", "manila", 66), "Enter keeps look at register night. Esc puts it back to amber.");
        assert_eq!(keeps("look", "register night", "wordperfect", "manila", 66), "Enter keeps look at register night.");
    }
}
