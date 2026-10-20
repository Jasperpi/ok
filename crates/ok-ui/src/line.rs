//! The line: the one place besides the page where the cursor can be.
//!
//! Esc puts the cursor after `ok` on the machine line, and everything
//! DESIGN.md says under "The line" happens from there: finding a text by
//! its title, `#` to a heading, `/` to find and change, `rename`, `set`,
//! `earlier`, `discard`, `copy`, `keys`, `notices`, `quit`, and the `[[`
//! offer on the page.
//!
//! The interface is the [`Line`] trait, which is all the rest of the
//! program knows of the line. Every method has a body that does nothing,
//! and [`Closed`] is the line that takes them all as they are. The line
//! the program runs is [`prompt::Prompt`], handed out by [`start`]; what
//! it shows on the page for `keys` and `notices` is in [`pages`], and the
//! options behind `set` are in [`settings`]. `tests/seams.rs` drives the
//! trait with a small stand-in, which is the shortest worked example of
//! what each method is for.
//!
//! What a line receives:
//! - [`Line::open`] when Esc is pressed on the page (with `""`) and when
//!   Ctrl F is (with `"/"`).
//! - While [`Line::is_open`] is true, every key and every typed character
//!   comes to [`Line::key`] and [`Line::text`] first, Esc and Enter
//!   included. A key you hand back with `false` is dropped, except the
//!   chords that leave or resize the window (Ctrl Q, Alt F4, F11, Ctrl =
//!   and Ctrl -), which the interface then runs itself.
//! - [`Line::step`] for F3 and Shift F3 and [`Line::keys`] for F1, when the
//!   line is closed (when it is open they arrive as keys like any other).
//! - On the page: [`Line::typed_on_page`] after every typed character
//!   there, [`Line::tab`] when Tab is pressed outside a list, and
//!   [`Line::moved_on_page`] after anything else that may have moved the
//!   cursor or the paper.
//! - [`Line::tick`] on every tick of the shell, after the delay you asked
//!   for through [`Line::wake_in_ms`]: the 250 ms a bar rests before the
//!   page shows what is under it, the 1200 ms before an empty line shows
//!   what it understands.
//!
//! Every call that may act is handed the [`App`]. Through it a line can:
//! - move in the text: [`App::go`] puts the cursor somewhere and brings
//!   that row to the platen, [`App::select`] does the same for a stretch;
//! - try and put back: [`App::hold_place`] remembers the text, the cursor
//!   and the row as they are, [`App::preview`] shows some other text on the
//!   page without opening it, [`App::restore`] puts back whatever was held
//!   and [`App::keep_place`] lets go of it ("Arrows try, Enter keeps, Esc
//!   puts back");
//! - show a text for reading under a name of its own with [`App::show`],
//!   and move the paper under it with [`App::scroll_page`];
//! - open for good: [`App::open_named`], and the other acts on the folder
//!   that end with another text on the page ([`App::discard`],
//!   [`App::bring_back`], [`App::use_folder`]) or with this one renamed
//!   ([`App::rename`]). Each asks the session and passes its sentence on
//!   when it cannot be done;
//! - change the text through [`App::edit`], which is all `/old/new` needs,
//!   and Ctrl Z takes it back like any other edit;
//! - change how the screen looks: [`App::use_look`], [`App::use_face`],
//!   [`App::set_scale`], [`App::set_measure`], [`App::prefs_mut`], and
//!   keep what was changed with [`App::keep_settings`];
//! - speak after it has closed: [`App::say`]; and leave: [`App::leave`].
//!
//! While one of a line's methods is running, the `App` holds a [`Closed`]
//! line in its place, so a line does not find itself there.
//!
//! What a line must answer, cheaply, because the painters ask on every
//! frame:
//! - [`Line::typed`] and [`Line::caret`]: what is on the machine line after
//!   `ok`, and where its cursor is. [`Line::hint`] for the faint list of
//!   what the line understands.
//! - [`Line::say`]: the one sentence, starting with Enter, that says what
//!   Enter will do now; or a mistake's `?` and its sentence.
//! - [`Line::answer`]: which pane answers and with what rows, and which
//!   row the bar is on. The pane draws your rows in place of its own, the
//!   row under the bar in inverse video. If that pane is not on screen the
//!   interface raises the rows from the say row over the foot of the
//!   sheet, eight at most; a line need do nothing for that.
//! - [`Line::hits`]: stretches of the open text to show in inverse video
//!   and to flag beside the map.
//! - [`Line::offer`]: the rest of the name offered after `[[` on the page,
//!   drawn at half strength after the cursor.
//!
//! What a line may not touch: files (it asks the session), strengths of
//! ink (that is `flow.rs`), and the layout of the screen. The line never
//! draws; it answers.

pub mod pages;
pub mod prompt;
pub mod settings;

use ok_host::{Host, Key, Mods};
use ok_text::Span;

use crate::app::App;
use crate::contents::Entry;
use crate::page::Way;

/// The lists already on the screen that the line can drive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    /// The contents, on the left.
    Contents,
    /// The labels beside the map, on the right.
    Labels,
    /// The page itself: hits in the text.
    Page,
}

/// What the line shows in the pane that answers it.
#[derive(Clone, Copy, Debug)]
pub struct Answer<'a> {
    pub pane: Pane,
    /// The rows to draw in the pane in place of its own. Ignored for
    /// [`Pane::Page`].
    pub rows: &'a [Entry],
    /// The row Enter would act on, drawn in inverse video.
    pub bar: Option<usize>,
}

/// The ok line, as the interface sees it.
pub trait Line {
    /// True while the cursor is on the machine line. The page then shows a
    /// hollow box where the cursor will return.
    fn is_open(&self) -> bool {
        false
    }

    /// Esc on the page (`with` is empty) or Ctrl F (`with` is `/`): take
    /// the cursor, starting with `with` already typed.
    fn open(&mut self, _with: &str, _app: &mut App, _host: &mut dyn Host) {}

    /// A key pressed while the line is open. True when the line took it.
    fn key(&mut self, _key: Key, _mods: Mods, _app: &mut App, _host: &mut dyn Host) -> bool {
        false
    }

    /// A character typed while the line is open.
    fn text(&mut self, _ch: char, _app: &mut App, _host: &mut dyn Host) {}

    /// F3 (`Way::On`) and Shift F3 (`Way::Back`) on the page: the next or
    /// the last hit.
    fn step(&mut self, _way: Way, _app: &mut App, _host: &mut dyn Host) {}

    /// F1 on the page: the keys, on the page.
    fn keys(&mut self, _app: &mut App, _host: &mut dyn Host) {}

    /// A character has just been typed on the page, at the cursor. This is
    /// where typing `[[` wakes the contents and an offer is made.
    fn typed_on_page(&mut self, _app: &mut App, _host: &mut dyn Host) {}

    /// Tab on the page, outside a list. True when an offer was taken.
    fn tab(&mut self, _app: &mut App, _host: &mut dyn Host) -> bool {
        false
    }

    /// The cursor or the paper may have moved by something other than
    /// typing: a key that travels or erases, a click, the wheel. An offer
    /// made where the cursor was is stale now.
    fn moved_on_page(&mut self, _app: &mut App, _host: &mut dyn Host) {}

    /// The shell ticked.
    fn tick(&mut self, _app: &mut App, _host: &mut dyn Host) {}

    /// How long from `now_ms` the line wants a tick, if it wants one.
    fn wake_in_ms(&self, _now_ms: u64) -> Option<u64> {
        None
    }

    /// What has been typed on the line, shown after `ok` and a space.
    fn typed(&self) -> &str {
        ""
    }

    /// Where the line's cursor is, in cells from the start of
    /// [`Line::typed`].
    fn caret(&self) -> usize {
        0
    }

    /// What the empty line shows, faint, when it has been left alone.
    fn hint(&self) -> Option<&str> {
        None
    }

    /// The sentence for the say row while the line is open.
    fn say(&self) -> Option<&str> {
        None
    }

    /// Which pane answers the line, and with what.
    fn answer(&self) -> Option<Answer<'_>> {
        None
    }

    /// Stretches of the open text to show in inverse video.
    fn hits(&self) -> &[Span] {
        &[]
    }

    /// The rest of a name offered after `[[` on the page.
    fn offer(&self) -> Option<&str> {
        None
    }
}

/// The do-nothing line: closed, and it stays closed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Closed;

impl Line for Closed {}

/// The line the program starts with.
pub fn start() -> Box<dyn Line> {
    Box::new(prompt::Prompt::default())
}
