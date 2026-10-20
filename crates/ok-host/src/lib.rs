//! The seam between the window shell and the interface.
//!
//! The shell owns the window, the message loop and everything Win32. The
//! interface is a [`Program`]: it is handed [`Event`]s, says what it
//! [`Wants`], and draws into a frame. It never sees a window handle, so the
//! whole interface can be driven and rendered with no window at all, which is
//! how it is tested and how still frames are made.

use std::path::{Path, PathBuf};

use ok_gfx::{AlphaGlyph, Frame, Rect, Rgb};

/// Which of Ctrl, Alt and Shift are down.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, alt: false, shift: false };
    pub const CTRL: Mods = Mods { ctrl: true, alt: false, shift: false };
    pub const ALT: Mods = Mods { ctrl: false, alt: true, shift: false };
    pub const SHIFT: Mods = Mods { ctrl: false, alt: false, shift: true };

    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    /// F1 to F12.
    F(u8),
    /// A printing key pressed as part of a chord (Ctrl or Alt held): the
    /// unshifted character the key carries on a US keyboard, lower case, such
    /// as `'s'`, `'='`, `'-'`, `'/'`. Plain typing never arrives this way.
    Char(char),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseKind {
    Down,
    Up,
    Move,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    /// A movement or command key. With Ctrl and Alt both down nothing is sent
    /// (that is AltGr on other keyboards and belongs to typing). A line break
    /// that arrives as a character with no Enter key behind it (dictation,
    /// some input methods) is delivered as `Key::Enter`.
    Key { key: Key, mods: Mods, repeat: bool },
    /// One character to put in at the cursor. Surrogate pairs arrive joined;
    /// control characters never arrive.
    Text(char),
    /// Ctrl, Alt or Shift went down or came up; carries what is held now.
    Mods(Mods),
    /// The left button or a move, in client pixels.
    Mouse { kind: MouseKind, x: i32, y: i32, mods: Mods },
    /// The wheel, in rows: positive is away from the user (toward the top).
    Wheel { x: i32, y: i32, rows: i32 },
    /// The client area's size in pixels. The next frame will be this size.
    Resize { width: usize, height: usize },
    Focus(bool),
    /// A reason to look at the clock: the moment asked for in
    /// [`Wants::wake_in_ms`] has come, a frame is due because
    /// [`Wants::animating`] is set, or a [`Waker`] was called. It is never
    /// proof that a particular wake moment has arrived.
    Tick,
    /// The window is being closed (Alt F4, shutdown, sign-out). Everything
    /// must be on disk before `event` returns. It can come more than once
    /// and is not always final: a sign-out the user cancels leaves the window
    /// working, so the program must carry on correctly afterwards.
    Close,
    /// Open this file. Sent when a second launch hands its file over, and
    /// also for this launch's own file argument, after the first `Resize`
    /// and before the first frame is shown. The path is absolute.
    Open(PathBuf),
}

/// The local wall clock, as the person at the keyboard would read it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 is Monday.
    pub weekday: u8,
    /// Seconds since 1970 in UTC, for ordering and arithmetic.
    pub unix: i64,
}

/// The cell an outline face needs at a given pixel height.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FaceCell {
    pub width: u32,
    pub height: u32,
    /// Rows from the top of the cell to the baseline.
    pub ascent: u32,
}

/// Something any thread may call to have the shell deliver an [`Event::Tick`]
/// soon. Calling it many times before the tick arrives costs one tick.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// What the interface may ask of the machine it runs on.
pub trait Host {
    /// Milliseconds on a clock that never goes backwards.
    fn now_ms(&self) -> u64;
    fn local_time(&self) -> LocalTime;
    /// A waker for this window. The save layer's worker threads use it to
    /// say there is news (a save finished, a file changed outside) without
    /// the interface having to poll while idle.
    fn waker(&self) -> Waker;

    fn clipboard_get(&mut self) -> Option<String>;
    fn clipboard_set(&mut self, text: &str);

    /// The user's Documents folder.
    fn documents_dir(&self) -> PathBuf;
    /// Where the journal, versions and state go: `%LOCALAPPDATA%\ok`.
    fn local_data_dir(&self) -> PathBuf;
    /// Where settings and looks go: `%APPDATA%\ok`.
    fn roaming_data_dir(&self) -> PathBuf;

    /// Family names of the fixed-pitch faces installed in Windows.
    fn system_faces(&mut self) -> Vec<String>;
    /// Makes a `.ttf` or `.otf` file usable by this process only and returns
    /// its family name.
    fn load_face_file(&mut self, path: &Path) -> Option<String>;
    /// The cell `family` needs when drawn `height` pixels tall. `None` when
    /// the face is missing or is not fixed-pitch (its M and i differ in
    /// width). Raster faces such as Terminal work as well as outline ones.
    fn face_cell(&mut self, family: &str, height: u32) -> Option<FaceCell>;
    /// One character of an outline face as coverage, placed in its cell.
    /// `None` when the face has no glyph for it.
    fn rasterize(&mut self, family: &str, height: u32, ch: char) -> Option<AlphaGlyph>;

    /// Opens an `http://` or `https://` address in the default browser.
    fn open_in_browser(&mut self, url: &str);
    /// True when Windows has animations turned off.
    fn reduced_motion(&self) -> bool;
}

/// What the interface needs from the shell right now. Asked after every event.
#[derive(Clone, PartialEq, Debug)]
pub struct Wants {
    /// The picture changed: render and show it before waiting again.
    pub redraw: bool,
    /// Send [`Event::Tick`] every frame until this is false.
    pub animating: bool,
    /// Send one [`Event::Tick`] after this many milliseconds. It is the time
    /// still to go, restated after every event; the shell never ticks early.
    pub wake_in_ms: Option<u64>,
    /// Close the window. Everything is already saved.
    pub quit: bool,
    /// Borderless and covering the monitor, or an ordinary window.
    pub fullscreen: bool,
    pub pointer_hidden: bool,
    /// Where the text cursor is, in client pixels, so the emoji panel and
    /// input methods open beside it.
    pub caret: Option<Rect>,
    pub title: String,
    /// The cell in pixels, so a window is sized to whole cells.
    pub cell: (u32, u32),
    /// The smallest useful window, in cells.
    pub min_cells: (u32, u32),
    /// Height in pixels of the strip along the top that drags the window
    /// when it is not full screen. Presses in that strip never reach the
    /// program while windowed (Windows drags, maximizes on a double click and
    /// opens the system menu on a right click); moves over it do.
    pub caption_height: u32,
    /// The colour to show wherever no frame has been drawn yet.
    pub ground: Rgb,
}

impl Default for Wants {
    fn default() -> Wants {
        Wants {
            redraw: false,
            animating: false,
            wake_in_ms: None,
            quit: false,
            fullscreen: true,
            pointer_hidden: false,
            caret: None,
            title: String::from("ok"),
            cell: (12, 22),
            min_cells: (60, 12),
            caption_height: 22,
            ground: Rgb(0, 0, 0),
        }
    }
}

/// The interface, as the shell sees it.
pub trait Program {
    /// Takes one event. Must return quickly: a key press is drawn in the
    /// same window message that delivered it.
    fn event(&mut self, event: Event, host: &mut dyn Host);
    fn wants(&mut self, host: &dyn Host) -> Wants;
    /// Draws the whole picture. The frame is already the size last given in
    /// [`Event::Resize`].
    fn render(&mut self, frame: &mut Frame, host: &mut dyn Host);
}
