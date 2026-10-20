//! What the line's tests share: a writing folder that lives in memory and
//! answers everything the line asks of a session, and a rig with the real
//! line in it.
//!
//! The session the program runs with is being written elsewhere, so these
//! tests bring their own. [`Shelf`] keeps a set of texts by the names links
//! use, a text that is open, the one that was open before it, kept
//! versions, a settings text and a word limit for each text; a test looks
//! into it through the [`Folder`] it shares.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ok_host::Host;
use ok_text::links::{key, links_in, retarget};
use ok_text::{count, Change, Doc, Pos};
use ok_ui::contents::Entry;
use ok_ui::flow::Rest;
use ok_ui::harness::Rig;
use ok_ui::session::{News, Opened, Session};
use ok_ui::{line, Options};

/// An essay with a heading, two sections, a TK and a word that comes more
/// than once in two cases.
pub const ORCHARD: &str = "# The orchard\n\nThe orchard was eleven acres when my grandfather bought it.\n\n## Thinning\n\nYou take off two of every three apricots, TK how many that is. The apricots that stay have room.\n\n## What he showed me\n\nThe branches had split under the weight of the Apricots he kept.";

/// A second text, long enough to fill a sheet and scroll.
pub fn interview() -> String {
    let mut text = String::from("# Grandfather interview\n\nRecorded at the kitchen table. See [[The orchard]] for what came of it.");
    for answer in 1..=40 {
        text.push_str(&format!("\n\nAnswer {answer}. He said the trees were older than the fence."));
    }
    text
}

/// What is behind the stand-in session.
#[derive(Default)]
pub struct State {
    /// Every text by the name links use, with what it holds, in the order
    /// of the contents.
    pub texts: Vec<(String, String)>,
    pub open: usize,
    pub previous: Option<String>,
    pub discarded: Vec<(String, String)>,
    /// Kept versions of the open text, newest first: what the row says,
    /// and the text.
    pub versions: Vec<(String, String)>,
    pub settings: Option<String>,
    /// How many times the settings were written.
    pub settings_written: usize,
    pub limits: HashMap<String, usize>,
    pub folder: PathBuf,
    /// Every text the session was handed as "the text on the page", with
    /// what it was handed it for.
    pub handed: Vec<(&'static str, String)>,
    /// When set, everything asked of the folder is refused with this.
    pub refuse: Option<String>,
}

pub type Folder = Rc<RefCell<State>>;

impl State {
    pub fn text(&self, name: &str) -> Option<&str> {
        self.texts.iter().find(|(other, _)| other == name).map(|(_, text)| text.as_str())
    }

    pub fn names(&self) -> Vec<&str> {
        self.texts.iter().map(|(name, _)| name.as_str()).collect()
    }

    pub fn open_name(&self) -> &str {
        self.texts.get(self.open).map_or("", |(name, _)| name.as_str())
    }

    fn at(&self, name: &str) -> Option<usize> {
        let wanted = key(name);
        let whole = self.texts.iter().position(|(other, _)| key(other) == wanted);
        whole.or_else(|| self.texts.iter().position(|(other, _)| key(leaf(other)) == wanted))
    }

    /// Texts outside any folder first, then folder by folder, each in
    /// alphabetical order: the order of a contents page.
    fn sort(&mut self) {
        let open = self.open_name().to_string();
        self.texts.sort_by_key(|(name, _)| (name.contains('/'), name.to_lowercase()));
        self.open = self.texts.iter().position(|(name, _)| *name == open).unwrap_or(0);
    }

    fn links_to_open(&self) -> usize {
        let (whole, last) = (key(self.open_name()), key(leaf(self.open_name())));
        let leads_here = |target: &str| key(target) == whole || key(target) == last;
        self.texts.iter().enumerate().filter(|(at, _)| *at != self.open).map(|(_, (_, text))| text.lines().flat_map(links_in).filter(|link| leads_here(link.target)).count()).sum()
    }
}

fn leaf(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn thousands(n: usize) -> String {
    ok_ui::machine::thousands(n)
}

/// The stand-in session.
pub struct Shelf {
    state: Folder,
    // What the trait hands out by reference, kept in step with the state.
    name: String,
    contents: Vec<Entry>,
    previous: Option<String>,
    folder: PathBuf,
}

impl Shelf {
    /// A folder holding these texts, with the one called `open` open.
    pub fn new(texts: &[(&str, &str)], open: &str) -> (Shelf, Folder) {
        let mut state = State { texts: texts.iter().map(|(name, text)| (name.to_string(), text.to_string())).collect(), folder: PathBuf::from("C:\\Users\\jaspe\\Documents\\ok"), ..State::default() };
        state.open = state.texts.iter().position(|(name, _)| name == open).unwrap_or(0);
        state.sort();
        let state = Rc::new(RefCell::new(state));
        let mut shelf = Shelf { state: state.clone(), name: String::new(), contents: Vec::new(), previous: None, folder: PathBuf::new() };
        shelf.refresh();
        (shelf, state)
    }

    /// The texts of a real folder, read once and never written.
    pub fn read(dir: &Path, open: &str) -> (Shelf, Folder) {
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
            let Ok(listing) = std::fs::read_dir(dir) else { return };
            for item in listing.flatten() {
                let path = item.path();
                let Some(name) = path.file_stem().and_then(|name| name.to_str()).map(str::to_string) else { continue };
                if path.is_dir() {
                    walk(&path, &format!("{prefix}{name}/"), out);
                } else if path.extension().is_some_and(|ending| ending == "md") {
                    out.push((format!("{prefix}{name}"), std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n")));
                }
            }
        }
        let mut texts = Vec::new();
        walk(dir, "", &mut texts);
        let borrowed: Vec<(&str, &str)> = texts.iter().map(|(name, text)| (name.as_str(), text.as_str())).collect();
        Shelf::new(&borrowed, open)
    }

    fn refresh(&mut self) {
        let state = self.state.borrow();
        self.name = leaf(state.open_name()).to_string();
        self.previous = state.previous.clone();
        self.folder = state.folder.clone();
        self.contents.clear();
        let mut above: Vec<&str> = Vec::new();
        for (at, (name, text)) in state.texts.iter().enumerate() {
            let mut folders: Vec<&str> = name.split('/').collect();
            let title = folders.pop().unwrap_or(name);
            let shared = above.iter().zip(&folders).take_while(|(a, b)| a == b).count();
            for (depth, folder) in folders.iter().enumerate().skip(shared) {
                self.contents.push(Entry { depth, name: folder.to_string(), value: String::new(), lit: false });
            }
            self.contents.push(Entry { depth: folders.len(), name: title.to_string(), value: thousands(count::words(text)), lit: at == state.open });
            above = folders;
        }
    }

    /// Puts what is on the page into the open text, as a save would.
    fn save(&mut self, what: &'static str, doc: &Doc) {
        let mut state = self.state.borrow_mut();
        let text = doc.to_text("\n");
        state.handed.push((what, text.clone()));
        let open = state.open;
        if let Some(held) = state.texts.get_mut(open) {
            held.1 = text;
        }
    }

    fn refused(&self) -> Result<(), String> {
        match &self.state.borrow().refuse {
            Some(sentence) => Err(sentence.clone()),
            None => Ok(()),
        }
    }

    fn opened(&self) -> Opened {
        let state = self.state.borrow();
        Opened { text: state.texts.get(state.open).map(|(_, text)| text.clone()).unwrap_or_default(), ..Opened::default() }
    }
}

impl Session for Shelf {
    fn first(&mut self, _host: &mut dyn Host) -> Opened {
        self.opened()
    }

    fn changed(&mut self, doc: &Doc, _change: &Change, _host: &mut dyn Host) {
        self.save("changed", doc);
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> &[Entry] {
        &self.contents
    }

    fn exists(&self, target: &str) -> bool {
        self.state.borrow().at(target).is_some()
    }

    fn keep(&mut self, doc: &Doc, _host: &mut dyn Host) -> Option<String> {
        self.save("keep", doc);
        None
    }

    fn follow(&mut self, _doc: &Doc, _at: Pos, _host: &mut dyn Host) -> Option<Opened> {
        None
    }

    fn tick(&mut self, doc: &Doc, _host: &mut dyn Host) -> News {
        self.save("tick", doc);
        News::default()
    }

    fn close(&mut self, doc: &Doc, _host: &mut dyn Host) {
        self.save("close", doc);
    }

    fn names(&self) -> Vec<String> {
        self.state.borrow().texts.iter().map(|(name, _)| name.clone()).collect()
    }

    fn peek(&mut self, name: &str) -> Option<String> {
        self.state.borrow().text(name).map(str::to_string)
    }

    fn open_named(&mut self, name: &str, start: bool, doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        self.refused()?;
        self.save("open_named", doc);
        {
            let mut state = self.state.borrow_mut();
            let was = state.open_name().to_string();
            match state.at(name) {
                Some(at) => state.open = at,
                None if start => {
                    let name = name.trim().replace('\\', "/");
                    state.texts.push((name.clone(), String::new()));
                    state.open = state.texts.len() - 1;
                    state.sort();
                }
                None => return Err(format!("? There is no text called {name}.")),
            }
            if state.open_name() != was {
                state.previous = Some(was);
            }
            state.versions.clear();
        }
        self.refresh();
        Ok(self.opened())
    }

    fn previous(&self) -> Option<&str> {
        self.previous.as_deref()
    }

    fn rename(&mut self, title: &str, doc: &Doc, _host: &mut dyn Host) -> Result<usize, String> {
        self.refused()?;
        self.save("rename", doc);
        let mended = {
            let mut state = self.state.borrow_mut();
            let title = title.trim().replace('\\', "/");
            if state.at(&title).is_some() {
                return Err(format!("? There is a text called {} already.", leaf(&title)));
            }
            let old = state.open_name().to_string();
            // A bare title stays in the folder the text is in.
            let new = match (title.contains('/'), old.rsplit_once('/')) {
                (false, Some((folder, _))) => format!("{folder}/{title}"),
                _ => title,
            };
            let mended = state.links_to_open();
            let open = state.open;
            for (at, (_, text)) in state.texts.iter_mut().enumerate() {
                if at == open {
                    continue;
                }
                for was in [old.as_str(), leaf(&old)] {
                    if let Some(now) = retarget(text, was, leaf(&new)) {
                        *text = now;
                    }
                }
            }
            state.texts[open].0 = new;
            state.sort();
            mended
        };
        self.refresh();
        Ok(mended)
    }

    fn links_here(&self) -> usize {
        self.state.borrow().links_to_open()
    }

    fn discard(&mut self, doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        self.refused()?;
        self.save("discard", doc);
        {
            let mut state = self.state.borrow_mut();
            let open = state.open;
            let gone = state.texts.remove(open);
            state.discarded.push(gone);
            if state.texts.is_empty() {
                state.texts.push((String::from("Untitled"), String::new()));
            }
            state.open = open.min(state.texts.len() - 1);
            state.previous = None;
            state.versions.clear();
        }
        self.refresh();
        Ok(self.opened())
    }

    fn versions(&mut self) -> Vec<Entry> {
        self.state.borrow().versions.iter().map(|(says, text)| Entry { depth: 0, name: says.clone(), value: thousands(count::words(text)), lit: false }).collect()
    }

    fn version(&mut self, n: usize) -> Option<String> {
        self.state.borrow().versions.get(n).map(|(_, text)| text.clone())
    }

    fn bring_back(&mut self, n: usize, doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        self.refused()?;
        self.save("bring_back", doc);
        {
            let mut state = self.state.borrow_mut();
            let Some((_, back)) = state.versions.get(n).cloned() else { return Err(String::from("? There is no such version.")) };
            let present = doc.to_text("\n");
            state.versions.insert(0, (String::from("Today 14:02, before going back"), present));
            let open = state.open;
            state.texts[open].1 = back;
        }
        self.refresh();
        Ok(self.opened())
    }

    fn settings(&mut self) -> Option<String> {
        self.state.borrow().settings.clone()
    }

    fn write_settings(&mut self, text: &str) {
        let mut state = self.state.borrow_mut();
        state.settings = Some(text.to_string());
        state.settings_written += 1;
    }

    fn limit(&self) -> Option<usize> {
        let state = self.state.borrow();
        state.limits.get(state.open_name()).copied()
    }

    fn set_limit(&mut self, limit: Option<usize>) {
        let mut state = self.state.borrow_mut();
        let name = state.open_name().to_string();
        match limit {
            Some(limit) => state.limits.insert(name, limit),
            None => state.limits.remove(&name),
        };
    }

    fn folder(&self) -> Option<&Path> {
        Some(&self.folder)
    }

    fn use_folder(&mut self, folder: &Path, doc: &Doc, _host: &mut dyn Host) -> Result<Opened, String> {
        self.refused()?;
        if folder.to_string_lossy().contains("nowhere") {
            return Err(format!("? There is no folder at {}.", folder.display()));
        }
        self.save("use_folder", doc);
        {
            let mut state = self.state.borrow_mut();
            state.folder = folder.to_path_buf();
            state.texts = vec![(String::from("First text"), String::from("Written in the other folder."))];
            state.open = 0;
            state.previous = None;
            state.versions.clear();
        }
        self.refresh();
        Ok(self.opened())
    }
}

/// The six texts most of the line's tests use.
pub fn desk() -> Vec<(&'static str, String)> {
    vec![
        ("Essay plan", String::from("Plan.\n\n- Open with [[The orchard]]\n- Then [[College/The orchard|the trees]] again\n- End on [[Grandfather interview]]")),
        ("College/Grandfather interview", interview()),
        ("College/Orchard notes", String::from("Notes on thinning.\n\nTwo of every three.")),
        ("College/The orchard", String::from(ORCHARD)),
        ("Mock trial/Closing argument", String::from("# Closing\n\nMembers of the jury.")),
        ("Mock trial/Opening statement", String::from("# Opening\n\nThis case is about a promise.")),
    ]
}

/// A full-screen rig with the real line, the screen at rest, and the desk's
/// six texts behind it, `open` on the page with the cursor at its end.
pub fn rig_at(open: &str) -> (Rig, Folder) {
    let texts = desk();
    let borrowed: Vec<(&str, &str)> = texts.iter().map(|(name, text)| (*name, text.as_str())).collect();
    rig_with(&borrowed, open, 1920, 1080)
}

/// The same with The orchard open.
pub fn rig() -> (Rig, Folder) {
    rig_at("College/The orchard")
}

/// A rig of any size over any texts.
pub fn rig_with(texts: &[(&str, &str)], open: &str, width: usize, height: usize) -> (Rig, Folder) {
    let (shelf, folder) = Shelf::new(texts, open);
    let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    rig.size(width, height);
    (rig, folder)
}

/// What the say row reads.
pub fn said(rig: &mut Rig) -> String {
    let (text, say) = (rig.app.grid().text, rig.app.grid().say.unwrap_or(-1));
    rig.screen().span(text, say)
}

/// What a stretch of one row of the screen reads.
pub fn read(rig: &mut Rig, first: i32, last: i32, row: i32) -> String {
    rig.screen().span(ok_ui::grid::Cols { first, last }, row)
}

/// The rows of the contents pane from row 1 down, as they read, skipping
/// the empty rows between groups.
pub fn contents(rig: &mut Rig) -> Vec<String> {
    let cols = rig.app.grid().contents.unwrap_or(ok_ui::grid::Cols { first: 0, last: -1 });
    let screen = rig.screen().clone();
    (1..screen.rows()).map(|row| screen.span(cols, row)).filter(|row| !row.is_empty()).collect()
}

/// The rows of the labels pane from row 1 down, skipping empty ones.
pub fn labels(rig: &mut Rig) -> Vec<String> {
    let cols = rig.app.grid().labels.unwrap_or(ok_ui::grid::Cols { first: 0, last: -1 });
    let screen = rig.screen().clone();
    (1..screen.rows()).map(|row| screen.span(cols, row)).filter(|row| !row.is_empty()).collect()
}

/// The row of a pane the bar is on: the one row that is inverse across the
/// whole pane. `None` when there is no bar.
pub fn bar_in(rig: &mut Rig, cols: ok_ui::grid::Cols) -> Option<String> {
    let screen = rig.screen().clone();
    (0..screen.rows()).find(|&row| (cols.first..=cols.last).all(|col| screen.at(col, row).is_some_and(|cell| cell.inverse))).map(|row| screen.span(cols, row).trim().to_string())
}

/// The row of the line's list the bar is on: in the contents, or, on a
/// screen too narrow for them, among the rows risen over the foot of the
/// sheet.
pub fn bar(rig: &mut Rig) -> Option<String> {
    let cols = rig.app.grid().contents.unwrap_or(rig.app.grid().text);
    bar_in(rig, cols)
}

/// The row of the table of options that starts with a name, wherever the
/// table is drawn.
pub fn option_row(rig: &mut Rig, name: &str) -> Option<String> {
    let cols = rig.app.grid().contents.unwrap_or(rig.app.grid().text);
    let screen = rig.screen().clone();
    (0..screen.rows()).map(|row| screen.span(cols, row).trim().to_string()).find(|row| row.starts_with(&format!("{name} .")))
}

/// What is typed on the machine line, after `ok` and its space.
pub fn typed(rig: &mut Rig) -> String {
    let grid = rig.app.grid().clone();
    let screen = rig.screen();
    let text: String = (grid.typing_col()..=grid.text.last).filter_map(|col| screen.at(col, grid.machine.unwrap_or(-1))).filter(|cell| cell.strength == ok_ui::look::Strength::INK).map(|cell| cell.ch).collect();
    text.trim_end().to_string()
}

/// Where the cursor is in the text, as (line, byte).
pub fn head(rig: &Rig) -> (usize, usize) {
    let head = rig.app.page().cursor.head;
    (head.line, head.col)
}

/// Everything Esc must put back: the text, the cursor, the paper, the
/// look, and what every cell of the screen reads.
pub fn picture(rig: &mut Rig) -> (String, (usize, usize), i64, ok_ui::look::Look, Vec<String>) {
    let lines = rig.screen().lines();
    (rig.text(), head(rig), rig.app.view().top(), rig.app.look().clone(), lines)
}

/// Opens the line and types on it.
pub fn line_types(rig: &mut Rig, text: &str) {
    rig.press("esc");
    rig.type_text(text);
}

/// The row a piece of text is first written on, looking down from the top.
pub fn row_of(rig: &mut Rig, needle: &str) -> i32 {
    rig.screen().find(needle).map(|(_, row)| row).unwrap_or_else(|| panic!("{needle:?} is not on the screen"))
}

/// The big title on the page: the first row of the text column with
/// anything on it, read a letter in every two cells.
pub fn title(rig: &mut Rig) -> String {
    let text = rig.app.grid().text;
    let screen = rig.screen().clone();
    (0..screen.rows()).find(|&row| !screen.span(text, row).is_empty()).map(|row| screen.big(text, row)).unwrap_or_default()
}
