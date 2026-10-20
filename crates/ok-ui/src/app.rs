//! The program: one text on one screen, driven by the shell's events.
//!
//! [`App`] owns the page, the paper's position, the grid, the look and the
//! three seams (`session`, `line`, `flow`), and is the only thing that
//! joins them. An event comes in, a key becomes a [`Command`], the command
//! acts on the page, the seams are told, and the next frame is drawn from
//! what is then true. Nothing here opens a window or touches a file.

use std::path::Path;

use ok_gfx::{gallant, spleen, terminus, Face, FaceSet, Frame, OutlineFace, Rect, Rgb};
use ok_host::{Event, Host, Key, Mods, MouseKind, Program, Wants};
use ok_text::history::Cursor;
use ok_text::markup::LineKind;
use ok_text::{Pos, Span};

use crate::canvas::{Canvas, Screen};
use crate::contents::Entry;
use crate::flow::{self, Act, Flow, Said};
use crate::grid::{Grid, MEASURE};
use crate::keys::{self, Command};
use crate::line::{self, settings, Closed, Line};
use crate::look::{Look, Surface};
use crate::map;
use crate::page::{Clock, Day, Page, Unit, Way};
use crate::platen::View;
use crate::scene::{self, Scene};
use crate::session::{self, Opened, Options, Session};

/// The settings the interface reads that are plain numbers and switches.
/// `set` on the line changes them (see `line/settings.rs`, which also
/// keeps them in the settings text); the look, the face and the window
/// are settings too and are the [`App`]'s own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// Cells of text to a row, 40 to 100.
    pub measure: usize,
    /// 1 to 3.
    pub scale: u32,
    /// Hold the typing row at the platen.
    pub platen: bool,
    /// Let the paper take four frames to move; otherwise it jumps.
    pub smooth: bool,
    /// Draw the cursor as an underscore under its letter, not a block.
    pub underline: bool,
    /// Let the screen fade round the paragraph being written.
    pub focus: bool,
    /// Let the machine say, after a run, how many words went by.
    pub echo: bool,
    /// Enter is one line break and Shift Enter ends the paragraph, where
    /// otherwise it is the other way round.
    pub enter_line: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { measure: MEASURE, scale: 1, platen: true, smooth: true, underline: false, focus: true, echo: true, enter_line: false }
    }
}

/// The pixel height a face of the system's is asked for at scale 1. A
/// fixed-pitch face of that height has a cell close to Gallant's 12 by 22,
/// so changing face changes the letters and little else.
const OUTLINE_HEIGHT: u32 = 20;

/// How long Ctrl or Alt is held alone before the say row explains it.
const HOLD_MS: u64 = 500;

/// Milliseconds from one frame to the next while the paper rolls or a fade
/// runs, for whoever stands in for the shell.
pub const FRAME_MS: u64 = 16;

/// Ctrl or Alt held with nothing else pressed.
#[derive(Clone, Copy, Debug)]
struct Hold {
    since_ms: u64,
    /// Another key was pressed while it was down, so it was a chord and
    /// not a question.
    used: bool,
}

/// What was on the page before the line began trying things.
struct Held {
    /// The text itself, when another is being shown in its place.
    page: Option<Page>,
    cursor: Cursor,
    top: i64,
    /// What the text shown in its place is called, for the facts.
    name: String,
    /// The contents with no row lit, since the text they would light is
    /// not the one on the page.
    rows: Vec<Entry>,
    /// The highest the text shown may be scrolled: the row of it that was
    /// first put at the top of the sheet.
    floor: i64,
}

/// The text itself: the page, or the one held while another is shown in
/// its place. Everything handed to the session is read from this, so that
/// a text which is only being looked at can never be taken for his.
fn text_of<'a>(held: &'a Option<Held>, page: &'a Page) -> &'a Page {
    match held {
        Some(Held { page: Some(own), .. }) => own,
        _ => page,
    }
}

/// The session as the painters are to see it while some other text is
/// shown on the page in place of the open one. The facts then count the
/// words on the page, so they must name what is on the page too; and the
/// contents and the links, which are about the open text, stand back.
struct Shown<'a> {
    session: &'a dyn Session,
    held: &'a Held,
}

impl Session for Shown<'_> {
    fn on_disk(&self) -> bool {
        self.session.on_disk()
    }

    fn failing(&self) -> bool {
        self.session.failing()
    }

    fn name(&self) -> &str {
        &self.held.name
    }

    fn contents(&self) -> &[Entry] {
        &self.held.rows
    }

    fn exists(&self, target: &str) -> bool {
        self.session.exists(target)
    }

    fn today(&self, host: &dyn Host) -> Day {
        self.session.today(host)
    }

    fn words_today(&self) -> usize {
        self.session.words_today()
    }
}

/// What the say row reads when the line is not supplying it: a held key's
/// explanation once it has been held long enough, or the last thing said.
fn saying(hold: Option<Hold>, mods: Mods, said: &Option<(String, Option<u64>)>, now_ms: u64) -> Option<&str> {
    let asking = hold.filter(|hold| !hold.used && now_ms.saturating_sub(hold.since_ms) >= HOLD_MS);
    asking.and_then(|_| keys::held(mods)).or(said.as_ref().map(|(sentence, _)| sentence.as_str()))
}

/// The interface of ok.
pub struct App {
    faces: FaceSet,
    /// The face in use, by the word the settings take for it.
    face: String,
    look: Look,
    /// The looks he has kept under names of his own.
    looks: Vec<Look>,
    prefs: Prefs,
    /// The settings text as the session last had it, so that it is only
    /// written again when it would read differently.
    settings_written: String,
    grid: Grid,
    size: (usize, usize),
    page: Page,
    view: View,
    session: Box<dyn Session>,
    line: Box<dyn Line>,
    flow: Box<dyn Flow>,
    screen: Screen,
    /// The sentence on the say row and, when it is to be taken down by the
    /// clock, the moment for that.
    said: Option<(String, Option<u64>)>,
    held: Option<Held>,
    /// The cursor's row is to be brought to the platen as soon as the
    /// window has room for a grid.
    seat_due: bool,
    mods: Mods,
    hold: Option<Hold>,
    dragging: bool,
    focused: bool,
    fullscreen: bool,
    quit: bool,
    pointer_hidden: bool,
    redraw: bool,
}

impl App {
    /// Starts the interface with the session, the line and the flow their
    /// modules hand out. The session is asked for its first text and that
    /// text is put on the page. The screen has no size until the shell
    /// sends [`Event::Resize`].
    pub fn new(host: &mut dyn Host, options: Options) -> App {
        let session = session::start(&options, host);
        App::with_seams(host, options, session, line::start(), flow::start())
    }

    /// Starts the interface with a session, a line and a flow of the
    /// caller's choosing: how each seam is tested on its own, with
    /// stand-ins for the other two.
    pub fn with_seams(host: &mut dyn Host, options: Options, mut session: Box<dyn Session>, line: Box<dyn Line>, flow: Box<dyn Flow>) -> App {
        let faces = FaceSet::new(gallant());
        let prefs = Prefs::default();
        let opened = session.first(host);
        let grid = Grid::new(0, 0, faces.cell(), prefs.scale, prefs.measure);
        let mut app = App {
            faces,
            face: String::from(settings::FACES[0]),
            look: Look::manila(),
            looks: Vec::new(),
            settings_written: String::new(),
            page: Page::new("", prefs.measure),
            prefs,
            grid,
            size: (0, 0),
            view: View::default(),
            session,
            line,
            flow,
            screen: Screen::default(),
            said: None,
            held: None,
            seat_due: true,
            mods: Mods::NONE,
            hold: None,
            dragging: false,
            focused: true,
            fullscreen: true,
            quit: false,
            pointer_hidden: false,
            redraw: true,
        };
        app.open_text(opened);
        // The settings are read once, here. What the options ask for is
        // for this launch alone and comes on top of them.
        if let Some(text) = app.session.settings() {
            settings::load(&mut app, &text, host);
        }
        if let Some(look) = options.look.as_deref().and_then(Look::named) {
            app.look = look;
        }
        app.settings_written = settings::written(&app);
        app
    }

    /// What is on the page: the text, or another being shown in its place.
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// The text itself, even while another is being shown in its place.
    pub fn text(&self) -> &Page {
        text_of(&self.held, &self.page)
    }

    /// True while the cursor is on the machine line and not on the page.
    pub fn line_open(&self) -> bool {
        self.line.is_open()
    }

    /// True while some other text is shown on the page in place of his.
    pub fn showing(&self) -> bool {
        self.held.as_ref().is_some_and(|held| held.page.is_some())
    }

    /// The page, for changing the text. Changes made through it reach the
    /// session and the undo history like any typed by hand.
    pub fn page_mut(&mut self) -> &mut Page {
        &mut self.page
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn look(&self) -> &Look {
        &self.look
    }

    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// The settings, for changing. Call [`App::relayout`] afterwards if the
    /// measure or the scale changed, and [`App::keep_settings`] to have a
    /// change outlast the sitting.
    pub fn prefs_mut(&mut self) -> &mut Prefs {
        &mut self.prefs
    }

    pub fn session(&self) -> &dyn Session {
        self.session.as_ref()
    }

    pub fn session_mut(&mut self) -> &mut dyn Session {
        self.session.as_mut()
    }

    /// The screen as text, as the last frame drew it.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// The moment, for an edit made now.
    pub fn clock(&self, host: &dyn Host) -> Clock {
        Clock { now_ms: host.now_ms(), today: self.session.today(host) }
    }

    /// Changes the look to a built-in one. False when there is none of that
    /// name.
    pub fn set_look(&mut self, name: &str) -> bool {
        match Look::named(name) {
            Some(look) => {
                self.use_look(look);
                true
            }
            None => false,
        }
    }

    /// Changes the look to any seven values.
    pub fn use_look(&mut self, look: Look) {
        self.look = look;
        self.redraw = true;
    }

    /// The looks kept under names of his own, oldest first.
    pub fn looks(&self) -> &[Look] {
        &self.looks
    }

    /// Replaces the looks kept under names of his own.
    pub fn set_looks(&mut self, looks: Vec<Look>) {
        self.looks = looks;
    }

    /// Keeps the present colours under a name, and that is what the look is
    /// called from now on. A look already kept under that name is replaced.
    pub fn keep_look(&mut self, name: &str) {
        self.look.name = name.trim().to_string();
        let kept = self.look.clone();
        match self.looks.iter_mut().find(|look| look.name.eq_ignore_ascii_case(&kept.name)) {
            Some(look) => *look = kept,
            None => self.looks.push(kept),
        }
        self.redraw = true;
    }

    /// Changes the face the screen is drawn in. The cell comes from the
    /// face, so the whole screen is laid out again. An outline face is
    /// filled from the host as its characters are first drawn.
    pub fn set_face(&mut self, face: impl Into<Face>) {
        let face = face.into();
        let built_in = settings::FACES.iter().find(|name| name.eq_ignore_ascii_case(face.name()));
        self.face = built_in.map_or_else(|| face.name().to_string(), |name| name.to_string());
        self.faces.set_active(face);
        self.relayout();
    }

    /// The face in use, by the word the settings take for it: `gallant`,
    /// `spleen`, `terminus`, or the family name of a face of the system's.
    pub fn face_name(&self) -> &str {
        &self.face
    }

    /// Changes the face by name: one of the three inside the program, or a
    /// fixed-pitch face installed in the system, which the host measures
    /// and will rasterize into the grid. False, and nothing changed, when
    /// there is no such face.
    pub fn use_face(&mut self, name: &str, host: &mut dyn Host) -> bool {
        let name = name.trim();
        match name.to_ascii_lowercase().as_str() {
            "gallant" => self.set_face(gallant()),
            "spleen" => self.set_face(spleen()),
            "terminus" => self.set_face(terminus()),
            _ => {
                let Some(family) = host.system_faces().into_iter().find(|family| family.eq_ignore_ascii_case(name)) else { return false };
                let Some(cell) = host.face_cell(&family, OUTLINE_HEIGHT) else { return false };
                self.set_face(OutlineFace::new(&family, OUTLINE_HEIGHT, (cell.width, cell.height), cell.ascent));
            }
        }
        true
    }

    /// Whether the window is borderless and covers the screen.
    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Asks the shell for a borderless full screen, or for a window.
    pub fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
    }

    /// Has the next frame drawn, after something it shows was changed from
    /// outside.
    pub fn redraw(&mut self) {
        self.redraw = true;
    }

    /// Writes the settings as they now stand through the session, if they
    /// read any differently from what it last had.
    pub fn keep_settings(&mut self) {
        let text = settings::written(self);
        if text != self.settings_written {
            self.session.write_settings(&text);
            self.settings_written = text;
        }
    }

    /// Sets the scale, 1 to 3.
    pub fn set_scale(&mut self, scale: u32) {
        self.prefs.scale = scale.clamp(1, 3);
        self.relayout();
    }

    /// Sets the measure, 40 to 100.
    pub fn set_measure(&mut self, measure: usize) {
        self.prefs.measure = measure.clamp(40, 100);
        self.relayout();
    }

    /// The measure the page is wrapped at: the grid's, or the setting while
    /// the window has no room for a grid.
    fn wrap_at(&self) -> usize {
        if self.grid.is_blank() {
            self.prefs.measure
        } else {
            self.grid.measure.max(1) as usize
        }
    }

    /// Lays the screen out again after the size, the scale, the measure or
    /// the face changed, and keeps the typing row where it was: at the
    /// platen if it was there, in sight otherwise. A window with no room
    /// for any text (minimized, or still without a size) changes nothing
    /// about the page or the paper, so that it comes back as it left.
    pub fn relayout(&mut self) {
        let seated = if self.grid.is_blank() { self.seat_due } else { self.view.cursor_row(&self.page) == self.grid.platen as i64 };
        self.grid = Grid::new(self.size.0, self.size.1, self.faces.cell(), self.prefs.scale, self.prefs.measure);
        self.redraw = true;
        self.seat_due = seated;
        if self.grid.is_blank() {
            return;
        }
        let measure = self.wrap_at();
        self.page.set_measure(measure);
        if let Some(Held { page: Some(page), .. }) = &mut self.held {
            page.set_measure(measure);
        }
        if self.showing() {
            // A text that is only being read has no typing row to keep in
            // sight; the paper stays where the reading had got to.
            self.view.settle(&self.page, &self.grid);
            self.keep_floor();
        } else if seated {
            self.view.seat(&self.page, &self.grid);
        } else {
            self.view.settle(&self.page, &self.grid);
            self.view.reveal(&self.page, &self.grid);
        }
        self.seat_due = false;
    }

    /// Keeps a text that is only being shown from being scrolled above the
    /// row it was shown from.
    fn keep_floor(&mut self) {
        if let Some(Held { page: Some(_), floor, .. }) = &self.held {
            if self.view.top() < *floor {
                self.view.set_top(*floor, &self.page, &self.grid);
            }
        }
    }

    /// Puts a sentence on the say row until the next key.
    pub fn say(&mut self, sentence: impl Into<String>) {
        self.said = Some((sentence.into(), None));
        self.redraw = true;
    }

    /// Puts a text on the page for good: the page, its history and the
    /// paper's position all start again from what was handed over.
    pub fn open_text(&mut self, opened: Opened) {
        self.held = None;
        self.page = Page::new(&opened.text, self.wrap_at());
        self.page.set_limit(self.session.limit());
        self.page.set_stamps(&opened.stamps);
        self.page.read_only = opened.read_only;
        if let Some(cursor) = opened.cursor {
            self.page.place(cursor, false);
        }
        match opened.top {
            Some(top) => self.view.put_back(top as i64, &self.page, &self.grid),
            None => self.view.seat(&self.page, &self.grid),
        }
        // With no grid yet, the row is brought to the platen when there is one.
        self.seat_due = self.grid.is_blank() && opened.top.is_none();
        if let Some(sentence) = opened.say {
            self.say(sentence);
        }
        self.redraw = true;
    }

    /// Puts the cursor at `pos` and brings its row to the platen.
    pub fn go(&mut self, pos: Pos) {
        self.page.place(pos, false);
        self.view.seat(&self.page, &self.grid);
        self.keep_floor();
        self.redraw = true;
    }

    /// Selects a stretch and brings its row to the platen.
    pub fn select(&mut self, span: Span) {
        self.page.select(span);
        self.view.seat(&self.page, &self.grid);
        self.redraw = true;
    }

    /// Remembers the text, the cursor and the paper's position as they are
    /// now, so that [`App::restore`] can put them back. Holding again
    /// before a restore keeps the first place.
    pub fn hold_place(&mut self) {
        if self.held.is_none() {
            self.held = Some(Held { page: None, cursor: self.page.cursor, top: self.view.top(), name: String::new(), rows: Vec::new(), floor: -1 });
        }
    }

    /// Shows some other text on the page without opening it: read only,
    /// from its top. The text that was there is held until
    /// [`App::restore`] or [`App::open_text`].
    pub fn preview(&mut self, text: &str) {
        self.show("", text, -1);
    }

    /// Shows some other text on the page without opening it, as
    /// [`App::preview`] does, under the name the facts are to give it.
    /// Row `top` of it goes at the top of the sheet and it cannot be
    /// scrolled above that; -1 leaves the row of bare paper a text at rest
    /// has above its first row.
    pub fn show(&mut self, name: &str, text: &str, top: i64) {
        self.hold_place();
        let mut shown = Page::new(text, self.wrap_at());
        shown.read_only = true;
        shown.place(Pos::default(), false);
        let before = std::mem::replace(&mut self.page, shown);
        if let Some(held) = &mut self.held {
            held.page.get_or_insert(before);
            held.name = name.to_string();
            held.floor = top;
            held.rows = self.session.contents().iter().map(|entry| Entry { lit: false, ..entry.clone() }).collect();
        }
        self.view.set_top(top, &self.page, &self.grid);
        self.redraw = true;
    }

    /// Moves the paper by whole rows and leaves the cursor where it is in
    /// the text: how a text that is only being shown is read. Positive
    /// rows bring later text into view.
    pub fn scroll_page(&mut self, rows: i64) {
        self.view.scroll(rows, &self.page, &self.grid);
        self.keep_floor();
        self.redraw = true;
    }

    /// Puts back what [`App::hold_place`] remembered. Nothing happens when
    /// nothing is held.
    pub fn restore(&mut self) {
        let Some(held) = self.held.take() else { return };
        if let Some(page) = held.page {
            self.page = page;
        }
        // The text may have been changed from the line since it was held.
        let doc = self.page.doc();
        self.page.cursor = Cursor { head: doc.clamp(held.cursor.head), anchor: doc.clamp(held.cursor.anchor) };
        self.view.put_back(held.top, &self.page, &self.grid);
        self.redraw = true;
    }

    /// Lets go of what was held without putting it back: what was tried is
    /// kept. A text only being previewed stays a preview; open it with
    /// [`App::open_text`] to keep it.
    pub fn keep_place(&mut self) {
        if self.held.as_ref().is_some_and(|held| held.page.is_none()) {
            self.held = None;
        }
    }

    /// Leaves: everything is handed to the session and the shell is asked
    /// to close the window. Nothing is asked of the writer because nothing
    /// can be lost.
    pub fn leave(&mut self, host: &mut dyn Host) {
        self.deliver(host);
        self.session.close(text_of(&self.held, &self.page).doc(), host);
        self.quit = true;
    }

    /// Changes the text on behalf of the line: `change` is handed the page
    /// and the moment and says whether anything changed. The paper follows
    /// as it follows an edit made by hand, and Ctrl Z takes it back like
    /// one.
    pub fn edit(&mut self, host: &mut dyn Host, change: impl FnOnce(&mut Page, Clock) -> bool) -> bool {
        self.view.finish();
        let was = self.view.cursor_row(&self.page);
        let clock = self.clock(host);
        let changed = change(&mut self.page, clock);
        if changed {
            self.after_edit(was, host);
            self.redraw = true;
        }
        changed
    }

    /// Takes back the last step, as Ctrl Z does. False when there was
    /// nothing to take back.
    pub fn undo(&mut self, host: &mut dyn Host) -> bool {
        self.edit(host, |page, clock| page.undo(clock))
    }

    /// Makes the step undone last again, as Ctrl Y does.
    pub fn redo(&mut self, host: &mut dyn Host) -> bool {
        self.edit(host, |page, clock| page.redo(clock))
    }

    /// Puts the text called `name` on the page for good. With `start` set,
    /// a name that leads nowhere starts a new text by that title. The
    /// error is the sentence to say, and nothing has changed.
    pub fn open_named(&mut self, name: &str, start: bool, host: &mut dyn Host) -> Result<(), String> {
        self.deliver(host);
        let opened = self.session.open_named(name, start, text_of(&self.held, &self.page).doc(), host)?;
        self.open_text(opened);
        Ok(())
    }

    /// Renames the open text and has every link to it mended. Hands back
    /// how many links were.
    pub fn rename(&mut self, title: &str, host: &mut dyn Host) -> Result<usize, String> {
        self.deliver(host);
        self.session.rename(title, text_of(&self.held, &self.page).doc(), host)
    }

    /// Moves the open text into `.discarded` and puts on the page whatever
    /// the session hands over in its place.
    pub fn discard(&mut self, host: &mut dyn Host) -> Result<(), String> {
        self.deliver(host);
        let opened = self.session.discard(text_of(&self.held, &self.page).doc(), host)?;
        self.open_text(opened);
        Ok(())
    }

    /// Makes kept version `n` the text again. The session keeps the
    /// present text as a version first.
    pub fn bring_back(&mut self, n: usize, host: &mut dyn Host) -> Result<(), String> {
        self.deliver(host);
        let opened = self.session.bring_back(n, text_of(&self.held, &self.page).doc(), host)?;
        self.open_text(opened);
        Ok(())
    }

    /// Writes in another folder from now on, and puts on the page the text
    /// the session opens there.
    pub fn use_folder(&mut self, folder: &Path, host: &mut dyn Host) -> Result<(), String> {
        self.deliver(host);
        let opened = self.session.use_folder(folder, text_of(&self.held, &self.page).doc(), host)?;
        self.open_text(opened);
        Ok(())
    }

    /// Runs something on the line with the rest of the interface at hand.
    /// The line is lifted out while it runs, which is why it finds a closed
    /// stand-in if it looks for itself.
    fn with_line<R>(&mut self, host: &mut dyn Host, act: impl FnOnce(&mut dyn Line, &mut App, &mut dyn Host) -> R) -> R {
        let mut line = std::mem::replace(&mut self.line, Box::new(Closed));
        let result = act(line.as_mut(), self, host);
        self.line = line;
        result
    }

    /// Hands every change made to the text since the last time to the
    /// session.
    fn deliver(&mut self, host: &mut dyn Host) {
        let text = match &mut self.held {
            Some(Held { page: Some(own), .. }) => own,
            _ => &mut self.page,
        };
        for change in text.take_changes() {
            self.session.changed(text_of(&self.held, &self.page).doc(), &change, host);
        }
    }

    /// Follows an edit with the paper. `was` is the row of the sheet the
    /// cursor was on before it.
    fn after_edit(&mut self, was: i64, host: &dyn Host) {
        let smooth = self.prefs.smooth && !host.reduced_motion();
        self.view.follow_edit(&self.page, &self.grid, was, self.prefs.platen, smooth);
    }

    fn typed(&mut self, ch: char, host: &mut dyn Host) {
        self.pointer_hidden = true;
        self.said = None;
        if let Some(hold) = &mut self.hold {
            hold.used = true;
        }
        if self.line.is_open() {
            return self.with_line(host, |line, app, host| line.text(ch, app, host));
        }
        if ch.is_control() {
            return;
        }
        self.view.finish();
        let was = self.view.cursor_row(&self.page);
        let mut buffer = [0u8; 4];
        if self.page.insert(ch.encode_utf8(&mut buffer), self.clock(host)) {
            self.flow.note(Act::Typed(ch), host);
            self.after_edit(was, host);
            self.with_line(host, |line, app, host| line.typed_on_page(app, host));
        }
    }

    fn key(&mut self, key: Key, mods: Mods, host: &mut dyn Host) {
        self.pointer_hidden = true;
        self.said = None;
        if let Some(hold) = &mut self.hold {
            hold.used = true;
        }
        let command = keys::command(key, mods);
        if self.line.is_open() {
            let taken = self.with_line(host, |line, app, host| line.key(key, mods, app, host));
            // The chords that leave or resize the window work from the line too.
            if let (false, Some(command @ (Command::Quit | Command::Fullscreen | Command::Scale(_)))) = (taken, command) {
                self.run(command, host);
            }
            return;
        }
        // Any chord takes the wheel, whether or not it is bound to anything.
        if mods.ctrl || mods.alt {
            self.flow.note(Act::Chord, host);
        }
        if let Some(command) = command {
            self.view.finish();
            self.run(command, host);
            self.with_line(host, |line, app, host| line.moved_on_page(app, host));
        }
    }

    /// Does what a key asked for.
    fn run(&mut self, command: Command, host: &mut dyn Host) {
        let clock = self.clock(host);
        let was = self.view.cursor_row(&self.page);
        let own = self.page.own_paragraph();
        match command {
            Command::Travel { unit, way, select } => {
                self.page.travel(unit, way, select);
                self.view.reveal(&self.page, &self.grid);
                self.flow.note(Act::Moved { left: self.page.own_paragraph() != own }, host);
            }
            Command::Screen { way, select } => {
                // One screen less a row, the cursor keeping its row of the
                // sheet where the text allows.
                let rows = (self.grid.text_rows - 1).max(1);
                self.page.travel(Unit::Rows(rows as usize), way, select);
                self.view.scroll(if way == Way::Back { -(rows as i64) } else { rows as i64 }, &self.page, &self.grid);
                self.view.reveal(&self.page, &self.grid);
                self.flow.note(Act::Moved { left: self.page.own_paragraph() != own }, host);
            }
            Command::Erase { unit, way } => {
                if self.page.erase(unit, way, clock) {
                    self.flow.note(Act::Erased, host);
                    self.after_edit(was, host);
                }
            }
            Command::Carry(way) => {
                if self.page.carry(way, clock) {
                    self.after_edit(was, host);
                }
            }
            Command::Enter { soft } => {
                // With `enter line` the two trade places, except in a list,
                // where Enter goes on starting the next item.
                let in_list = self.page.kind(self.page.cursor.span().start.line) == LineKind::Item;
                let soft = if self.prefs.enter_line && !in_list { !soft } else { soft };
                if self.page.enter(soft, clock) {
                    self.flow.note(Act::Entered { first: own.0, last: own.1 }, host);
                    self.after_edit(was, host);
                }
            }
            Command::Nest(way) => {
                if self.page.nest(way, clock) {
                    self.after_edit(was, host);
                } else if way == Way::On && self.with_line(host, |line, app, host| line.tab(app, host)) {
                    self.after_edit(was, host);
                }
            }
            Command::Undo => {
                self.undo(host);
            }
            Command::Redo => {
                self.redo(host);
            }
            Command::Cut | Command::Copy => match self.page.selection() {
                // Line breaks go to the clipboard the way Windows writes them.
                Some(text) => {
                    host.clipboard_set(&text.replace('\n', "\r\n"));
                    if command == Command::Cut && self.page.erase(Unit::Letter, Way::Back, clock) {
                        self.flow.note(Act::Erased, host);
                        self.after_edit(was, host);
                    }
                }
                None => self.say("Nothing selected. Shift with an arrow selects."),
            },
            Command::Paste => {
                // Plain text: whatever the clipboard holds, less the control
                // characters no key could have typed.
                let pasted: String = host.clipboard_get().unwrap_or_default().chars().filter(|&ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t')).collect();
                if !pasted.is_empty() && self.page.insert(&pasted, clock) {
                    self.after_edit(was, host);
                }
            }
            Command::SelectAll => self.page.select_all(),
            Command::NextTk => {
                if self.page.next_tk() {
                    self.view.reveal(&self.page, &self.grid);
                } else {
                    self.say("Nothing still to come.");
                }
            }
            // The chords that change a setting keep it, as `set` does.
            Command::Scale(by) => {
                self.set_scale(self.prefs.scale.saturating_add_signed(by));
                self.keep_settings();
            }
            Command::Fullscreen => {
                self.fullscreen = !self.fullscreen;
                self.keep_settings();
            }
            Command::Quit => self.leave(host),
            Command::ToLine => {
                self.flow.note(Act::Escape, host);
                self.with_line(host, |line, app, host| line.open("", app, host));
            }
            Command::Find => self.with_line(host, |line, app, host| line.open("/", app, host)),
            Command::Step(way) => self.with_line(host, |line, app, host| line.step(way, app, host)),
            Command::Keys => self.with_line(host, |line, app, host| line.keys(app, host)),
            Command::Keep => {
                self.deliver(host);
                if let Some(sentence) = self.session.keep(text_of(&self.held, &self.page).doc(), host) {
                    self.say(sentence);
                }
            }
            // A link is followed from the text itself, never from one that
            // is only being shown.
            Command::Follow if self.showing() => {}
            Command::Follow => {
                self.deliver(host);
                if let Some(opened) = self.session.follow(self.page.doc(), self.page.cursor.head, host) {
                    self.open_text(opened);
                }
            }
        }
    }

    /// The position in the text under a pixel of the sheet's rows of text.
    fn pos_under(&self, x: i32, y: i32) -> Option<Pos> {
        let (col, row) = self.grid.cell_at(x, y);
        if !self.grid.sheet.contains(col) || row < 0 || row >= self.grid.text_rows {
            return None;
        }
        let at = self.view.top() + row as i64;
        if at >= self.page.paper_rows() as i64 {
            return Some(self.page.doc().end());
        }
        Some(self.page.pos_at(at.max(0) as usize, col - self.grid.text.first))
    }

    fn mouse(&mut self, kind: MouseKind, x: i32, y: i32, mods: Mods, host: &mut dyn Host) {
        self.pointer_hidden = false;
        self.flow.note(Act::Mouse, host);
        match kind {
            MouseKind::Down => {
                self.view.finish();
                if let Some(pos) = map::click(&self.grid, &self.page, x, y) {
                    self.go(pos);
                } else if let Some(pos) = self.pos_under(x, y) {
                    self.page.place(pos, mods.shift);
                    self.dragging = true;
                }
                self.with_line(host, |line, app, host| line.moved_on_page(app, host));
            }
            MouseKind::Move if self.dragging => {
                if let Some(pos) = self.pos_under(x, y) {
                    self.page.place(pos, true);
                }
            }
            MouseKind::Move => {}
            MouseKind::Up => self.dragging = false,
        }
    }

    fn tick(&mut self, host: &mut dyn Host) {
        self.view.step();
        self.deliver(host);
        let news = self.session.tick(text_of(&self.held, &self.page).doc(), host);
        if let Some(opened) = news.replaced {
            self.open_text(opened);
        }
        if let Some(sentence) = news.say {
            self.say(sentence);
        }
        // With `echo off` the flow still counts, and is not heard.
        if let Some(Said { text, for_ms }) = self.flow.tick(host).filter(|_| self.prefs.echo) {
            self.said = Some((text, for_ms.map(|ms| host.now_ms().saturating_add(ms))));
        }
        self.with_line(host, |line, app, host| line.tick(app, host));
        if self.said.as_ref().is_some_and(|(_, until)| until.is_some_and(|until| host.now_ms() >= until)) {
            self.said = None;
        }
    }

    /// Where the text cursor is in pixels, for the emoji panel and input
    /// methods.
    fn caret_rect(&self) -> Option<Rect> {
        let caret = self.page.caret();
        let row = self.view.cursor_row(&self.page);
        if self.line.is_open() {
            let machine = self.grid.machine?;
            return Some(self.grid.cells(self.grid.typing_col() + self.line.caret().min(1 << 16) as i32, machine, 1, 1));
        }
        (0 <= row && row < self.grid.text_rows as i64).then(|| self.grid.cells(self.grid.text.first + caret.cell, row as i32, caret.size as i32, caret.size as i32))
    }
}

impl Program for App {
    fn event(&mut self, event: Event, host: &mut dyn Host) {
        match event {
            Event::Text(ch) => self.typed(ch, host),
            Event::Key { key, mods, .. } => self.key(key, mods, host),
            Event::Mods(mods) => {
                // One of Ctrl and Alt down, alone, starts the wait for its
                // explanation; anything else ends it.
                let alone = (mods.ctrl != mods.alt) && !mods.shift;
                let same = alone && (mods.ctrl, mods.alt) == (self.mods.ctrl, self.mods.alt);
                self.hold = match self.hold {
                    Some(hold) if same => Some(hold),
                    _ if alone => Some(Hold { since_ms: host.now_ms(), used: false }),
                    _ => None,
                };
                self.mods = mods;
            }
            Event::Mouse { kind, x, y, mods } => self.mouse(kind, x, y, mods, host),
            Event::Wheel { rows, .. } => {
                self.pointer_hidden = false;
                self.flow.note(Act::Mouse, host);
                self.view.wheel(rows, &self.page, &self.grid);
                self.keep_floor();
                self.with_line(host, |line, app, host| line.moved_on_page(app, host));
            }
            Event::Resize { width, height } => {
                self.size = (width, height);
                self.relayout();
            }
            Event::Focus(focused) => {
                self.focused = focused;
                if !focused {
                    self.hold = None;
                    self.mods = Mods::NONE;
                    self.dragging = false;
                }
            }
            Event::Tick => self.tick(host),
            Event::Close => {
                self.deliver(host);
                self.session.close(text_of(&self.held, &self.page).doc(), host);
            }
            Event::Open(path) => {
                self.deliver(host);
                if let Some(opened) = self.session.open(&path, text_of(&self.held, &self.page).doc(), host) {
                    self.open_text(opened);
                }
            }
        }
        self.deliver(host);
        self.page.set_limit(self.session.limit());
        self.redraw = true;
    }

    fn wants(&mut self, host: &dyn Host) -> Wants {
        let now = host.now_ms();
        let hold = self.hold.filter(|hold| !hold.used).map(|hold| hold.since_ms.saturating_add(HOLD_MS).saturating_sub(now)).filter(|&left| left > 0);
        let said = self.said.as_ref().and_then(|(_, until)| *until).map(|until| until.saturating_sub(now));
        // A flow that is neither seen nor heard needs no ticks.
        let flow = if self.prefs.focus || self.prefs.echo { self.flow.wake_in_ms(now) } else { None };
        let wake = [hold, said, self.session.wake_in_ms(host), self.line.wake_in_ms(now), flow].into_iter().flatten().min();
        let ground: Rgb = self.look.ground(Surface::Desk);
        Wants {
            redraw: self.redraw,
            animating: self.view.rolling() || (self.prefs.focus && self.flow.animating(now)),
            wake_in_ms: wake,
            quit: self.quit,
            fullscreen: self.fullscreen,
            pointer_hidden: self.pointer_hidden,
            caret: self.caret_rect(),
            title: if self.session.name().is_empty() { String::from("ok") } else { self.session.name().to_string() },
            cell: (self.grid.cell_w as u32, self.grid.cell_h as u32),
            min_cells: (60, 12),
            caption_height: self.grid.cell_h as u32,
            ground,
        }
    }

    fn render(&mut self, frame: &mut Frame, host: &mut dyn Host) {
        if (frame.width(), frame.height()) != self.size {
            self.size = (frame.width(), frame.height());
            self.relayout();
        }
        let now_ms = host.now_ms();
        let today = self.session.today(host);
        // While another text is shown in place of his, the painters are
        // told about that one; with `focus off` nothing ever fades.
        let shown = match &self.held {
            Some(held) if held.page.is_some() => Some(Shown { session: self.session.as_ref(), held }),
            _ => None,
        };
        let scene = Scene {
            page: &self.page,
            view: &self.view,
            session: match &shown {
                Some(shown) => shown,
                None => self.session.as_ref(),
            },
            line: self.line.as_ref(),
            flow: if self.prefs.focus { self.flow.as_ref() } else { &flow::Rest },
            now_ms,
            today,
            focused: self.focused,
            said: saying(self.hold, self.mods, &self.said, now_ms),
        };
        let mut canvas = Canvas::new(frame, &mut self.faces, host, &self.grid, &self.look, &mut self.screen);
        canvas.underline_cursor(self.prefs.underline);
        canvas.hide_place(shown.is_some());
        scene::paint(&mut canvas, &scene);
        self.redraw = false;
    }
}
