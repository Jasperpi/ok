//! The one window: its procedure, its message loop and everything it keeps.
//!
//! How a key press reaches the glass. The message loop hands the key to the
//! window procedure, which turns it into an [`Event`] and gives it to the
//! program. The program is then asked what it [`Wants`]; if the picture
//! changed it is drawn into the frame and copied to the window before the
//! procedure returns. `WM_PAINT` only ever copies a frame that is already
//! drawn.
//!
//! Why everything is behind `Cell` and `RefCell`. Windows calls the window
//! procedure from inside calls the procedure itself makes (resizing the
//! window sends `WM_SIZE` before `SetWindowPos` returns), so the state is
//! only ever reached through a shared reference.
//!
//! Every event goes through one queue. An event that arrives while the
//! shell is in the middle of anything that involves the program (another
//! event, a frame being drawn, the window being placed) waits there and is
//! handled right after, in order; none is dropped. The end of the session
//! is the one thing that does not wait for the shell to finish what it is
//! doing, because Windows goes on as soon as that message is answered: the
//! queue is emptied into the program there and then. See
//! [`Shell::close_now`].
//!
//! Where ticks come from. A frame tick comes from the loop itself, timed by
//! a waitable timer, while the program animates. A one-shot wake comes from
//! a Windows timer. A waker's tick starts as a message posted by another
//! thread and is sent once the queue is empty. A key never waits behind more
//! than the one tick already being handled, and a queue that is never empty
//! cannot keep a frame or a wake waiting for long. While Windows runs a loop
//! of its own (a window being dragged, a menu) plain timers stand in for the
//! frame clock and for the waker.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::Arc;

use ok_gfx::{encode_png, Frame, Rect, Rgb};
use ok_host::{Event, Host, Key, Mods, MouseKind, Program, Wants};

use crate::crash;
use crate::host::{now_ms, Folders, WakeLine, WinHost, WM_WAKE};
use crate::keys::{carries_text, classify, classify_char, is_modifier, CharIn, KeyDown};
use crate::placement::{self, Placement};
use crate::selftest::{expand, Micro, Probe, Report, SelfTest, Tick, TickSource, Watchdog};
use crate::win32::*;
use crate::{Icon, Options};

/// The program asked for a one-shot wake.
const TIMER_WAKE: usize = 1;
/// Stands in for the frame clock while Windows runs its own loop.
const TIMER_MOVING: usize = 2;
/// Ends a pause in a self-test script.
const TIMER_SCRIPT: usize = 3;
/// Sends a waker's tick while Windows runs its own loop.
const TIMER_WAKER: usize = 4;

/// A self-test's watchdog posts this when the test's time has run out. It is
/// a posted message and not a timer because Windows hands out timers last:
/// a queue that is never empty would keep a timer waiting for ever.
pub(crate) const WM_WATCHDOG: u32 = WM_APP + 2;

/// A wake whose deadline moved by no more than this is the same wake.
const WAKE_SLACK_MS: u64 = 2;
/// A wake this far past its moment is being held back by other messages
/// (Windows hands out a timer only when the queue is empty) and is sent
/// without waiting for the timer.
const WAKE_PATIENCE_MS: u64 = 30;

/// Marks a `WM_COPYDATA` as a second launch's file to open ("okOP").
const COPYDATA_OPEN: usize = 0x6F6B_4F50;

/// No frame is made wider or taller than this, or bigger in all than
/// [`FRAME_AREA`]. Real windows never come near; it keeps a nonsense size
/// message from asking for gigabytes.
const FRAME_SIDE: usize = 16_384;
const FRAME_AREA: usize = 1 << 26;

pub(crate) type MakeProgram<'a> = Box<dyn FnOnce(&mut dyn Host) -> Box<dyn Program> + 'a>;

pub(crate) struct Outcome {
    pub code: i32,
    pub report: Report,
}

/// A self-test's script while it runs.
struct Script {
    steps: VecDeque<Micro>,
    /// The key state the window's thread is told about, so that held
    /// modifiers and typed capitals behave as they would from a keyboard.
    keys: [u8; 256],
    /// True from Alt going down until another key does: only then does
    /// letting go of Alt count as "Alt alone".
    alt_alone: bool,
    paused: bool,
    finished: bool,
}

struct Shell {
    hwnd: Cell<HWND>,
    /// A self-test: nothing may reach the screen, the focus, the pointer, the
    /// clipboard or the browser.
    quiet: bool,
    /// Where the window's placement and any crash report are kept.
    data_dir: PathBuf,

    program: RefCell<Option<Box<dyn Program>>>,
    host: RefCell<WinHost>,
    /// The host's wakers post here. Held apart from the host so a wake can
    /// be answered while the program has the host.
    wake: Arc<WakeLine>,
    frame: RefCell<Frame>,
    /// Events the program has not been given yet, oldest first.
    waiting: RefCell<VecDeque<Event>>,
    /// The shell is in the middle of something that involves the program:
    /// an event, a frame, the first placing of the window. Whatever arrives
    /// meanwhile stays in `waiting` until that is done.
    busy: Cell<bool>,
    /// The program has been given an event since it was last asked what it
    /// wants.
    unsettled: Cell<bool>,
    /// True once the program exists and until it panics or the window goes.
    live: Cell<bool>,
    dead: Cell<bool>,
    /// What the program was last asked to do, for the crash report.
    doing: Cell<&'static str>,
    last_event: RefCell<Option<Event>>,
    exit_code: Cell<i32>,
    /// A close was asked for: the window goes as soon as the program has
    /// handled [`Event::Close`].
    closing: Cell<bool>,
    leaving: Cell<bool>,

    /// The size the program has been told, which is the frame's size.
    client: Cell<(usize, usize)>,
    sized: Cell<bool>,
    shown: Cell<bool>,
    /// The program asked for a redraw that has not been drawn yet.
    need_present: Cell<bool>,

    title: RefCell<String>,
    fullscreen: Cell<bool>,
    /// While full screen: the placement to go back to.
    restore: Cell<Option<WINDOWPLACEMENT>>,
    open_maximized: Cell<bool>,
    cell: Cell<(i32, i32)>,
    min_cells: Cell<(i32, i32)>,
    caption: Cell<i32>,
    ground: Cell<Rgb>,
    dark: Cell<Option<bool>>,
    pointer_hidden: Cell<bool>,
    caret: Cell<Option<Rect>>,
    /// The size of the system caret that exists now, if one does.
    caret_made: Cell<Option<(i32, i32)>>,
    caret_at: Cell<Option<(i32, i32)>>,
    focused: Cell<bool>,

    animating: Cell<bool>,
    /// When the next frame is due, in seconds on the performance counter.
    next_frame: Cell<f64>,
    period: Cell<f64>,
    timer: FrameTimer,
    /// When the one-shot wake that is armed now is due, on the host's clock.
    wake_due: Cell<Option<u64>>,
    /// How many of Windows' own loops (drag, resize, menu) are running.
    moving: Cell<u32>,
    moving_timer: Cell<bool>,
    /// A waker's message has come in and its tick has not been sent yet.
    wake_arrived: Cell<bool>,
    waker_timer: Cell<bool>,
    /// When the last frame tick had been fully handled, in seconds.
    frame_done: Cell<f64>,

    decoder: RefCell<CharDecoder>,
    /// True when the last key message was a real key going down: not a
    /// modifier, and not one of the keys that only carry text (injected
    /// text, an input method). Characters that arrive then are that key's
    /// own, and the key itself has already been reported.
    key_owns_chars: Cell<bool>,
    /// The last thing the keyboard did was a carriage return with no key
    /// behind it, so a line feed that comes next is the same line break.
    bare_return: Cell<bool>,
    wheel: RefCell<WheelAccumulator>,
    mods: Cell<Mods>,
    pointer_at: Cell<Option<(i32, i32)>>,
    button: Cell<bool>,

    started: f64,
    script: RefCell<Option<Script>>,
    png: Option<PathBuf>,
    report: RefCell<Report>,
}

fn current_mods() -> Mods {
    let m = mods();
    Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift }
}

/// The frame size for a client area, kept within what can be allocated.
fn frame_size(width: usize, height: usize) -> (usize, usize) {
    let w = width.min(FRAME_SIDE);
    let mut h = height.min(FRAME_SIDE);
    if w > 0 && w * h > FRAME_AREA {
        h = FRAME_AREA / w;
    }
    (w, h)
}

/// A path as the bytes a second launch sends: its UTF-16 units, so a name
/// that is not valid Unicode still arrives whole.
fn open_payload(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().flat_map(|u| u.to_le_bytes()).collect()
}

/// A path named on a command line, made whole against this launch's own
/// current directory, which the instance that opens it does not share.
fn whole_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn open_path(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) || bytes.len() > 1 << 16 {
        return None;
    }
    let units: Vec<u16> = bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
    if units.contains(&0) {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

/// Processor time this process has used so far, in milliseconds.
fn cpu_ms() -> f64 {
    let (mut made, mut ended, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut made, &mut ended, &mut kernel, &mut user) };
    let ms = |t: FILETIME| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) as f64 / 10_000.0;
    ms(kernel) + ms(user)
}

/// Copies what a window's client area really holds into pixels of
/// `0x00RRGGBB`, top row first. Reads the window's own surface, so it works
/// for a cloaked window and touches nothing else on the screen.
fn read_glass(hwnd: HWND, width: i32, height: i32) -> Option<Vec<u32>> {
    if width <= 0 || height <= 0 {
        return None;
    }
    unsafe {
        let window = GetDC(hwnd);
        if window == 0 {
            return None;
        }
        let memory = CreateCompatibleDC(window);
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(memory, &dib_header(width, height), DIB_RGB_COLORS, &mut bits, 0, 0);
        let mut pixels = None;
        if memory != 0 && bitmap != 0 && !bits.is_null() {
            let old = SelectObject(memory, bitmap);
            if BitBlt(memory, 0, 0, width, height, window, 0, 0, SRCCOPY) != 0 {
                GdiFlush();
                let copied = std::slice::from_raw_parts(bits as *const u32, width as usize * height as usize);
                pixels = Some(copied.iter().map(|p| p & 0x00FF_FFFF).collect());
            }
            SelectObject(memory, old);
        }
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(hwnd, window);
        pixels
    }
}

fn save_png(path: &Path, frame: &Frame) {
    if frame.width() == 0 || frame.height() == 0 {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, encode_png(frame.width(), frame.height(), frame.pixels()));
}

impl Shell {
    fn new(options: &Options, test: Option<&SelfTest>) -> Shell {
        let quiet = test.is_some();
        let folders = match test {
            Some(t) => Folders::under(&t.data_dir),
            None => Folders::of_user(&options.name),
        };
        let data_dir = folders.local.clone();
        let host = WinHost::new(folders, quiet);
        Shell {
            hwnd: Cell::new(0),
            quiet,
            data_dir,
            program: RefCell::new(None),
            wake: Arc::clone(&host.wake),
            host: RefCell::new(host),
            frame: RefCell::new(Frame::new(0, 0)),
            waiting: RefCell::new(VecDeque::new()),
            busy: Cell::new(false),
            unsettled: Cell::new(false),
            live: Cell::new(false),
            dead: Cell::new(false),
            doing: Cell::new("starting"),
            last_event: RefCell::new(None),
            exit_code: Cell::new(0),
            closing: Cell::new(false),
            leaving: Cell::new(false),
            client: Cell::new((0, 0)),
            sized: Cell::new(false),
            shown: Cell::new(false),
            need_present: Cell::new(false),
            title: RefCell::new(String::from("ok")),
            fullscreen: Cell::new(false),
            restore: Cell::new(None),
            open_maximized: Cell::new(false),
            cell: Cell::new((12, 22)),
            min_cells: Cell::new((1, 1)),
            caption: Cell::new(0),
            ground: Cell::new(Rgb(0, 0, 0)),
            dark: Cell::new(None),
            pointer_hidden: Cell::new(false),
            caret: Cell::new(None),
            caret_made: Cell::new(None),
            caret_at: Cell::new(None),
            focused: Cell::new(false),
            animating: Cell::new(false),
            next_frame: Cell::new(0.0),
            period: Cell::new(even_frame_period(60.0)),
            timer: FrameTimer::new(true),
            wake_due: Cell::new(None),
            moving: Cell::new(0),
            moving_timer: Cell::new(false),
            wake_arrived: Cell::new(false),
            waker_timer: Cell::new(false),
            frame_done: Cell::new(0.0),
            decoder: RefCell::new(CharDecoder::default()),
            key_owns_chars: Cell::new(false),
            bare_return: Cell::new(false),
            wheel: RefCell::new(WheelAccumulator::default()),
            mods: Cell::new(Mods::NONE),
            pointer_at: Cell::new(None),
            button: Cell::new(false),
            started: now(),
            script: RefCell::new(test.map(|t| Script {
                steps: expand(&t.script),
                keys: [0; 256],
                alt_alone: false,
                paused: false,
                finished: false,
            })),
            png: test.and_then(|t| t.png.clone()),
            report: RefCell::new(Report::default()),
        }
    }
}

/// Calling into the program. Every call goes through [`Shell::call`], so a
/// panic in the program never crosses into Windows.
impl Shell {
    /// Calls into the program with a panic turned into a clean stop instead
    /// of a dead process. Returns `None` when the program could not be
    /// called or panicked.
    fn call<R>(&self, doing: &'static str, f: impl FnOnce(&mut dyn Program, &mut WinHost) -> R) -> Option<R> {
        if self.dead.get() {
            return None;
        }
        let outcome = {
            let mut program = self.program.try_borrow_mut().ok()?;
            let mut host = self.host.try_borrow_mut().ok()?;
            let program = program.as_mut()?;
            self.doing.set(doing);
            catch_unwind(AssertUnwindSafe(|| f(program.as_mut(), &mut host)))
        };
        match outcome {
            Ok(value) => Some(value),
            Err(payload) => {
                self.crashed(payload);
                None
            }
        }
    }

    /// The program (or the shell itself) panicked. Writes the message down,
    /// gives the program one last chance to put everything on disk, tells
    /// the person once, and ends the run with a failing exit code.
    fn crashed(&self, payload: Box<dyn Any + Send>) {
        let message = crash::describe(payload.as_ref());
        let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
        if self.dead.replace(true) {
            return;
        }
        let doing = match (self.doing.get(), self.last_event.try_borrow().ok().and_then(|e| e.clone())) {
            ("handling", Some(event)) => format!("handling {event:?}"),
            (doing, _) => doing.to_string(),
        };
        crash::write_report(&self.data_dir, &doing, &message);
        // The dialog belongs to the window only when the window can be seen.
        let owned = self.on_screen();
        if let Ok(mut report) = self.report.try_borrow_mut() {
            report.crashed = Some(message);
            report.crash_dialog_owned = Some(owned);
        }
        if let (Ok(mut program), Ok(mut host)) = (self.program.try_borrow_mut(), self.host.try_borrow_mut()) {
            if let Some(program) = program.as_mut() {
                let _ = catch_unwind(AssertUnwindSafe(|| program.event(Event::Close, &mut *host)));
            }
        }
        self.exit_code.set(crash::EXIT_CRASH);
        if !self.quiet {
            // A window that is on screen stays up behind the dialog, showing
            // the last frame that was drawn, until the dialog is answered.
            crash::tell(if owned { self.hwnd.get() } else { 0 });
        }
        self.leave();
    }

    /// True when the window is in front of the person: shown, not minimized
    /// and not cloaked. During the first show it is shown but still cloaked,
    /// and Windows cloaks a window together with the window that owns it, so
    /// a dialog given to it then would have the keyboard and never be seen.
    fn on_screen(&self) -> bool {
        let hwnd = self.hwnd.get();
        self.shown.get() && unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 } && !self.is_cloaked()
    }

    fn accepting(&self) -> bool {
        self.live.get() && !self.dead.get() && !self.leaving.get()
    }

    /// True when the program is not in the middle of a call, so that it can
    /// be called now.
    fn program_free(&self) -> bool {
        self.program.try_borrow_mut().is_ok() && self.host.try_borrow_mut().is_ok()
    }

    /// Gives the program an event and then does what it wants. An event that
    /// arrives while the shell is busy joins the queue and is handled when
    /// its turn comes.
    fn deliver(&self, event: Event) {
        if !self.accepting() {
            return;
        }
        self.waiting.borrow_mut().push_back(event);
        self.work();
    }

    /// Gives the program every event that is waiting, oldest first, and
    /// after each does what the program then wants. Does nothing when the
    /// shell is already busy: whatever it is busy with ends by coming here.
    fn work(&self) {
        if self.busy.replace(true) {
            return;
        }
        loop {
            let next = self.waiting.borrow_mut().pop_front();
            match next {
                Some(event) if self.accepting() => {
                    if self.hand(event) {
                        self.leave();
                        break;
                    }
                    // When the window's last Close is next, what the program
                    // wants in between no longer matters; it hears Close
                    // without that delay.
                    let closing = self.closing.get() && matches!(self.waiting.borrow().front(), Some(Event::Close));
                    if !closing {
                        self.settle();
                    }
                }
                // An event was handed over out of turn (see `close_now`) and
                // the program has not been asked what it wants since.
                None if self.unsettled.get() && self.accepting() => self.settle(),
                _ => break,
            }
        }
        self.waiting.borrow_mut().clear();
        self.busy.set(false);
    }

    /// Gives the program one event. True when it was the Close the window
    /// was waiting for before going.
    fn hand(&self, event: Event) -> bool {
        if let Event::Resize { width, height } = event {
            self.resize_frame(width, height);
        }
        let last = matches!(event, Event::Close) && self.closing.get();
        if let Ok(mut note) = self.last_event.try_borrow_mut() {
            *note = Some(event.clone());
        }
        self.unsettled.set(true);
        self.call("handling", |program, host| program.event(event, host));
        last
    }

    /// Makes the frame the new size. What it held is gone, so a new picture
    /// is owed whether or not the program thinks to ask for one.
    fn resize_frame(&self, width: usize, height: usize) {
        self.sized.set(true);
        if let Ok(mut frame) = self.frame.try_borrow_mut() {
            frame.resize(width, height);
            frame.clear(self.ground.get());
        }
        self.need_present.set(true);
    }

    /// Asks the program what it wants and makes it so.
    fn settle(&self) {
        self.unsettled.set(false);
        if let Some(wants) = self.call("being asked what it wants", |program, host| program.wants(&*host)) {
            self.apply(wants);
        }
    }

    /// Takes the sizes and colours from what the program wants. They feed
    /// hit-testing, snapping and the first placement of the window.
    fn adopt(&self, wants: &Wants) {
        self.cell.set((wants.cell.0.clamp(1, 512) as i32, wants.cell.1.clamp(1, 512) as i32));
        self.min_cells.set((wants.min_cells.0.clamp(1, 1000) as i32, wants.min_cells.1.clamp(1, 1000) as i32));
        self.caption.set(wants.caption_height.min(4096) as i32);
        self.ground.set(wants.ground);
        // The thin edge Windows draws round a frameless window follows the
        // ground: dark round a dark page, light round a light one.
        let dark = wants.ground.luminance() < 0.2;
        if self.dark.replace(Some(dark)) != Some(dark) {
            dynapi().set_dark_title_bar(self.hwnd.get(), dark);
        }
    }

    fn apply(&self, wants: Wants) {
        let hwnd = self.hwnd.get();
        self.adopt(&wants);
        if *self.title.borrow() != wants.title {
            unsafe { SetWindowTextW(hwnd, wide(&wants.title).as_ptr()) };
            *self.title.borrow_mut() = wants.title.clone();
        }
        if wants.fullscreen != self.fullscreen.get() {
            self.set_fullscreen(wants.fullscreen);
        }
        self.set_pointer(wants.pointer_hidden);
        self.set_caret(wants.caret);
        self.set_wake(wants.wake_in_ms);
        self.set_animating(wants.animating);
        if wants.redraw {
            self.need_present.set(true);
        }
        if wants.quit {
            self.leave();
            return;
        }
        self.present_if_due();
    }
}

/// Getting the picture to the glass.
impl Shell {
    /// True when more typing is already queued. A burst of characters (a
    /// paste through the keyboard, dictation) is then drawn once, at its
    /// end, instead of once per character.
    fn keys_waiting(&self) -> bool {
        let mut msg = MSG::default();
        unsafe { PeekMessageW(&mut msg, self.hwnd.get(), WM_KEYFIRST, WM_KEYLAST, PM_NOREMOVE | PM_NOYIELD) != 0 }
    }

    fn present_if_due(&self) {
        if !self.need_present.get() {
            return;
        }
        let more_events = !self.waiting.borrow().is_empty();
        if more_events || self.keys_waiting() {
            return;
        }
        // Looking at the queue lets Windows call the window procedure, and
        // what it did there may have drawn the frame or closed the window.
        if self.need_present.get() {
            self.present();
        }
    }

    /// Draws anything still owed. Called from outside event handling: after
    /// a key message that led to no event, and whenever the loop is about
    /// to wait. Whatever arrives while the frame is being drawn waits in the
    /// queue and is handled as soon as the frame is on the glass.
    fn catch_up(&self) {
        if self.busy.replace(true) {
            return;
        }
        self.present_if_due();
        self.busy.set(false);
        self.work();
    }

    /// Has the program draw the frame and copies it to the window.
    fn present(&self) {
        if self.dead.get() || self.leaving.get() {
            return;
        }
        // Nothing can be seen while minimized: the redraw stays owed until
        // the window comes back.
        if unsafe { IsIconic(self.hwnd.get()) } != 0 {
            return;
        }
        let Ok(mut frame) = self.frame.try_borrow_mut() else { return };
        self.need_present.set(false);
        if frame.width() == 0 || frame.height() == 0 {
            return;
        }
        frame.reset_clip();
        let drawn = self.call("drawing", |program, host| program.render(&mut frame, host)).is_some();
        frame.reset_clip();
        if !drawn {
            return;
        }
        self.report.borrow_mut().renders += 1;
        if self.shown.get() {
            let hwnd = self.hwnd.get();
            unsafe {
                let hdc = GetDC(hwnd);
                if hdc != 0 {
                    let mut all = RECT::default();
                    GetClientRect(hwnd, &mut all);
                    self.put(hdc, &frame, all);
                    ReleaseDC(hwnd, hdc);
                    ValidateRect(hwnd, null());
                }
            }
        }
    }

    /// Copies `area` of the frame to a device context. Where the frame does
    /// not reach (the window has just grown and the program has not drawn
    /// that size yet) the ground colour goes down instead.
    fn put(&self, hdc: HDC, frame: &Frame, area: RECT) {
        let (fw, fh) = (frame.width() as i32, frame.height() as i32);
        blit_rect(hdc, frame.pixels(), fw, fh, area);
        if area.right <= fw && area.bottom <= fh {
            return;
        }
        let g = self.ground.get();
        unsafe {
            let brush = CreateSolidBrush(rgb(g.0, g.1, g.2));
            if area.right > fw {
                FillRect(hdc, &RECT { left: fw.max(area.left), ..area }, brush);
            }
            if area.bottom > fh {
                FillRect(hdc, &RECT { top: fh.max(area.top), right: area.right.min(fw), ..area }, brush);
            }
            DeleteObject(brush);
        }
    }

    fn on_paint(&self) -> LRESULT {
        let hwnd = self.hwnd.get();
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc != 0 {
                // If the frame is being drawn this instant, that draw copies
                // the whole frame when it finishes; nothing to do here.
                if let Ok(frame) = self.frame.try_borrow() {
                    self.put(hdc, &frame, ps.rcPaint);
                }
            }
            EndPaint(hwnd, &ps);
        }
        self.report.borrow_mut().paints += 1;
        0
    }

    fn on_size(&self, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if wparam == SIZE_MINIMIZED {
            return 0;
        }
        let size = frame_size(loword(lparam as usize) as usize, hiword(lparam as usize) as usize);
        if self.client.replace(size) != size || !self.sized.get() {
            self.deliver(Event::Resize { width: size.0, height: size.1 });
        } else {
            self.catch_up();
        }
        0
    }
}

/// Full screen, and where the window sits when it is an ordinary window.
impl Shell {
    /// Sizes the window to the monitor it is on. Exactly the monitor, and
    /// not topmost: that is what makes Windows let it cover the taskbar.
    /// Returns false when Windows will not say where the monitor is.
    fn cover_monitor(&self) -> bool {
        let hwnd = self.hwnd.get();
        let Some(monitor) = monitor_info(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }) else { return false };
        let m = monitor.rcMonitor;
        unsafe { SetWindowPos(hwnd, 0, m.left, m.top, m.w(), m.h(), SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) != 0 }
    }

    /// Goes borderless over the whole monitor, remembering the window's
    /// place so that leaving full screen puts it back exactly.
    fn set_fullscreen(&self, on: bool) {
        let hwnd = self.hwnd.get();
        unsafe {
            if on {
                if self.restore.get().is_some() {
                    return;
                }
                let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                if GetWindowPlacement(hwnd, &mut wp) == 0 {
                    return;
                }
                if self.open_maximized.get() && IsWindowVisible(hwnd) == 0 {
                    wp.showCmd = SW_SHOWMAXIMIZED as u32;
                }
                // Resizing sends a size message before it returns, so the
                // state is already the new one when that message arrives.
                self.restore.set(Some(wp));
                self.fullscreen.set(true);
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW as isize));
                if !self.cover_monitor() {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style);
                    self.restore.set(None);
                    self.fullscreen.set(false);
                }
            } else {
                let Some(mut wp) = self.restore.take() else { return };
                self.fullscreen.set(false);
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_OVERLAPPEDWINDOW as isize);
                // Back to where it was, without taking the focus from
                // whatever has it. Only a maximized window has no way to be
                // shown without being activated.
                let maximized = wp.showCmd == SW_SHOWMAXIMIZED as u32;
                let how = if IsWindowVisible(hwnd) == 0 {
                    self.open_maximized.set(maximized);
                    SW_HIDE
                } else if maximized && !self.quiet {
                    SW_SHOWMAXIMIZED
                } else if wp.showCmd == SW_SHOWMINIMIZED as u32 {
                    SW_SHOWMINNOACTIVE
                } else {
                    SW_SHOWNOACTIVATE
                };
                wp.showCmd = how as u32;
                SetWindowPlacement(hwnd, &wp);
                SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            }
        }
    }

    /// The window's place as an ordinary window, whatever it is right now.
    fn placement_now(&self) -> Option<Placement> {
        let wp = match self.restore.get() {
            Some(wp) => wp,
            None => {
                let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                if unsafe { GetWindowPlacement(self.hwnd.get(), &mut wp) } == 0 {
                    return None;
                }
                wp
            }
        };
        let r = wp.rcNormalPosition;
        let maximized = wp.showCmd == SW_SHOWMAXIMIZED as u32
            || (wp.showCmd == SW_SHOWMINIMIZED as u32 && wp.flags & WPF_RESTORETOMAXIMIZED != 0);
        Some(Placement { left: r.left, top: r.top, right: r.right, bottom: r.bottom, maximized })
    }

    fn save_placement(&self) {
        if let Some(p) = self.placement_now() {
            placement::save(&self.data_dir, &p);
        }
    }

    /// How much bigger the window is than its client area.
    fn chrome(&self) -> (i32, i32) {
        let (mut window, mut client) = (RECT::default(), RECT::default());
        unsafe {
            GetWindowRect(self.hwnd.get(), &mut window);
            GetClientRect(self.hwnd.get(), &mut client);
        }
        (window.w() - client.w(), window.h() - client.h())
    }

    /// Where a window that has never been placed goes: four fifths of the
    /// work area in whole cells, centred. `client` asks for an exact client
    /// size instead.
    fn first_rect(&self, client: Option<(u32, u32)>) -> RECT {
        let hwnd = self.hwnd.get();
        let mut work = RECT { left: 0, top: 0, right: 1280, bottom: 720 };
        unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as *mut c_void, 0) };
        let border = resize_border(hwnd);
        let (chrome_w, chrome_h) = (2 * border, border);
        let (cell_w, cell_h) = self.cell.get();
        let (min_cols, min_rows) = self.min_cells.get();
        let (w, h) = match client {
            Some((w, h)) => (w.min(FRAME_SIDE as u32) as i32, h.min(FRAME_SIDE as u32) as i32),
            None => {
                let cols = ((work.w() * 4 / 5 - chrome_w) / cell_w).max(min_cols);
                let rows = ((work.h() * 4 / 5 - chrome_h) / cell_h).max(min_rows);
                (cols * cell_w, rows * cell_h)
            }
        };
        let (w, h) = (w + chrome_w, h + chrome_h);
        // A placement counts from the work area's corner, except for the
        // self-test's tool window, which counts from the screen's.
        let (ox, oy) = if self.quiet { (work.left, work.top) } else { (0, 0) };
        let (x, y) = (ox + ((work.w() - w) / 2).max(0), oy + ((work.h() - h) / 2).max(0));
        RECT { left: x, top: y, right: x + w, bottom: y + h }
    }

    /// Puts the still hidden window where it was last time.
    fn place(&self, client: Option<(u32, u32)>) {
        let saved = if client.is_none() { placement::load(&self.data_dir) } else { None };
        let (rect, maximized) = match saved {
            Some(p) => (RECT { left: p.left, top: p.top, right: p.right, bottom: p.bottom }, p.maximized),
            None => (self.first_rect(client), false),
        };
        self.open_maximized.set(maximized);
        let hwnd = self.hwnd.get();
        unsafe {
            let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            GetWindowPlacement(hwnd, &mut wp);
            wp.flags = 0;
            wp.showCmd = SW_HIDE as u32;
            wp.rcNormalPosition = rect;
            // A rectangle that is no longer on any monitor is moved back onto one.
            SetWindowPlacement(hwnd, &wp);
        }
    }
}

/// The pointer, the hidden caret, and the ticks.
impl Shell {
    fn set_pointer(&self, hidden: bool) {
        if self.pointer_hidden.replace(hidden) == hidden || self.quiet {
            return;
        }
        // Windows asks which pointer to show only when the pointer moves, so
        // if it is resting on the window the change is made here. It hides
        // anywhere on the window's own pixels, the strip that drags the
        // window included. It is only brought back over the page: over the
        // strip Windows has already put up the pointer that belongs there.
        let hwnd = self.hwnd.get();
        unsafe {
            let mut at = POINT::default();
            if GetCursorPos(&mut at) == 0 || WindowFromPoint(at) != hwnd {
                return;
            }
            let part = SendMessageW(hwnd, WM_NCHITTEST, 0, make_lparam(at.x, at.y));
            if hidden && matches!(part, HTCLIENT | HTCAPTION | HTTOP | HTTOPLEFT | HTTOPRIGHT) {
                SetCursor(0);
            } else if !hidden && part == HTCLIENT {
                SetCursor(LoadCursorW(0, IDC_ARROW));
            }
        }
    }

    /// Keeps the system caret, which is never shown, where the text cursor
    /// is. Input methods, the emoji panel and Magnifier all look for it.
    fn set_caret(&self, want: Option<Rect>) {
        self.caret.set(want);
        if !self.focused.get() {
            return;
        }
        let hwnd = self.hwnd.get();
        match want.filter(|r| !r.is_empty()) {
            Some(r) => unsafe {
                if self.caret_made.get() != Some((r.w, r.h)) {
                    if self.caret_made.get().is_some() {
                        DestroyCaret();
                    }
                    let made = CreateCaret(hwnd, 0, r.w, r.h) != 0;
                    self.caret_made.set(made.then_some((r.w, r.h)));
                    self.caret_at.set(None);
                }
                if self.caret_made.get().is_some() && self.caret_at.get() != Some((r.x, r.y)) {
                    SetCaretPos(r.x, r.y);
                    dynapi().ime_set_caret(hwnd, r.x, r.y, r.h);
                    self.caret_at.set(Some((r.x, r.y)));
                }
            },
            None => self.drop_caret(),
        }
    }

    fn drop_caret(&self) {
        if self.caret_made.take().is_some() {
            unsafe { DestroyCaret() };
        }
        self.caret_at.set(None);
    }

    /// Arms, moves or cancels the one-shot wake. The program restates its
    /// wake after every event as the time still to go, so the timer is only
    /// restarted when the moment it names has really moved; restarting it
    /// each time would let a stream of events (the frames of an animation)
    /// push it back for ever. For the same reason a wake whose moment has
    /// already come is left alone: its tick is on its way, and the program
    /// says what it wants next when that arrives.
    fn set_wake(&self, wake: Option<u64>) {
        let hwnd = self.hwnd.get();
        match wake {
            Some(ms) => {
                let now = now_ms();
                let due = now.saturating_add(ms);
                let stands = self.wake_due.get().is_some_and(|armed| armed <= now || armed.abs_diff(due) <= WAKE_SLACK_MS);
                if !stands {
                    unsafe { SetTimer(hwnd, TIMER_WAKE, ms.min(USER_TIMER_MAXIMUM as u64) as u32, None) };
                    self.wake_due.set(Some(due));
                }
            }
            None => {
                if self.wake_due.take().is_some() {
                    unsafe { KillTimer(hwnd, TIMER_WAKE) };
                }
            }
        }
    }

    fn set_animating(&self, on: bool) {
        if on && !self.animating.replace(on) {
            self.next_frame.set(now() + self.period.get());
        }
        self.animating.set(on);
        self.sync_moving_timer();
    }

    /// The message loop does not run while Windows drags or resizes the
    /// window or shows its menu, so the frame clock cannot either. A plain
    /// timer, which Windows does keep delivering, takes over until then.
    fn sync_moving_timer(&self) {
        let want = self.animating.get() && self.moving.get() > 0 && self.accepting();
        if self.moving_timer.replace(want) != want {
            let hwnd = self.hwnd.get();
            unsafe {
                if want {
                    SetTimer(hwnd, TIMER_MOVING, USER_TIMER_MINIMUM, None);
                } else {
                    KillTimer(hwnd, TIMER_MOVING);
                }
            }
        }
    }

    /// A waker's tick is sent by the message loop once the queue is empty
    /// (see [`Shell::run`]). While Windows runs a loop of its own that never
    /// happens, so a timer, which waits for an empty queue too, sends it.
    fn sync_waker_timer(&self) {
        let want = self.wake_arrived.get() && self.moving.get() > 0 && self.accepting();
        if self.waker_timer.replace(want) != want {
            let hwnd = self.hwnd.get();
            unsafe {
                if want {
                    SetTimer(hwnd, TIMER_WAKER, USER_TIMER_MINIMUM, None);
                } else {
                    KillTimer(hwnd, TIMER_WAKER);
                }
            }
        }
    }

    /// Sends the tick a waker asked for, if one is waiting. The waker's
    /// flag is cleared first, so news that comes while the program handles
    /// this tick asks for another.
    fn waker_tick(&self) {
        if self.wake_arrived.replace(false) {
            self.sync_waker_timer();
            if self.wake.take() {
                self.tick(TickSource::Waker);
            }
        }
    }

    fn tick(&self, source: TickSource) {
        if self.quiet {
            self.report.borrow_mut().ticks.push(Tick { at_ms: (now() - self.started) * 1000.0, source });
        }
        self.deliver(Event::Tick);
    }

    fn on_timer(&self, id: usize) {
        let hwnd = self.hwnd.get();
        match id {
            TIMER_WAKE => {
                let Some(due) = self.wake_due.get() else {
                    // The wake was called off after this message was made.
                    unsafe { KillTimer(hwnd, TIMER_WAKE) };
                    return;
                };
                // Windows counts a timer from its last clock tick, not from
                // the moment it was set, so it can come most of a tick
                // early. The program asked not to be woken before its time.
                let now = now_ms();
                if now < due {
                    unsafe { SetTimer(hwnd, TIMER_WAKE, (due - now).min(USER_TIMER_MAXIMUM as u64) as u32, None) };
                    return;
                }
                unsafe { KillTimer(hwnd, TIMER_WAKE) };
                self.wake_due.set(None);
                self.tick(TickSource::Wake);
            }
            TIMER_MOVING => {
                if self.animating.get() && self.moving.get() > 0 {
                    self.tick(TickSource::Moving);
                } else {
                    self.sync_moving_timer();
                }
            }
            TIMER_WAKER => self.waker_tick(),
            TIMER_SCRIPT => {
                unsafe { KillTimer(hwnd, TIMER_SCRIPT) };
                if let Some(script) = self.script.borrow_mut().as_mut() {
                    script.paused = false;
                }
            }
            _ => {}
        }
    }
}

/// Turning keyboard and mouse messages into events.
impl Shell {
    /// Tells the program when Ctrl, Alt or Shift changed since it last heard.
    fn sync_mods(&self, now: Mods) {
        if self.mods.replace(now) != now {
            self.deliver(Event::Mods(now));
        }
    }

    fn on_key_down(&self, wparam: WPARAM, lparam: LPARAM) {
        let vk = (wparam & 0xFFFF) as i32;
        // Windows has already turned this key into its characters, and they
        // are the next messages in the queue. They are this key's own unless
        // the key is only a carrier for text. A modifier makes none.
        let real = !carries_text(vk) && !is_modifier(vk);
        self.key_owns_chars.set(real);
        if real {
            // A line feed is the second half of a line break only straight
            // after its carriage return. A key pressed in between ends that.
            self.bare_return.set(false);
        }
        let held = current_mods();
        self.sync_mods(held);
        if let KeyDown::Command(key) = classify(vk, held) {
            self.deliver(Event::Key { key, mods: held, repeat: key_was_down(lparam) });
        }
        self.catch_up();
    }

    fn on_key_up(&self) {
        self.key_owns_chars.set(false);
        self.sync_mods(current_mods());
        self.catch_up();
    }

    fn on_char(&self, ch: Option<char>) {
        if let Some(ch) = ch {
            let held = current_mods();
            let from_key = self.key_owns_chars.get();
            let after_return = self.bare_return.replace(ch == '\r' && !from_key);
            match classify_char(ch, held, from_key, after_return) {
                CharIn::Text(ch) => self.deliver(Event::Text(ch)),
                CharIn::Enter => self.deliver(Event::Key { key: Key::Enter, mods: held, repeat: false }),
                CharIn::Nothing => {}
            }
        }
        self.catch_up();
    }

    fn on_mouse(&self, kind: MouseKind, wparam: WPARAM, lparam: LPARAM) {
        let (x, y) = (get_x_lparam(lparam), get_y_lparam(lparam));
        let keys = get_keystate_wparam(wparam);
        let moved = self.pointer_at.replace(Some((x, y))) != Some((x, y));
        let hwnd = self.hwnd.get();
        match kind {
            // Windows also sends a move when nothing moved (a window opening
            // under the pointer is enough); only a real move is passed on,
            // because a move is what brings a hidden pointer back.
            MouseKind::Move if !moved => return,
            MouseKind::Down => {
                self.button.set(true);
                if !self.quiet {
                    unsafe { SetCapture(hwnd) };
                }
            }
            MouseKind::Up => {
                if self.button.replace(false) && !self.quiet {
                    unsafe { ReleaseCapture() };
                }
            }
            MouseKind::Move => {}
        }
        let mods = Mods { ctrl: keys & MK_CONTROL != 0, shift: keys & MK_SHIFT != 0, alt: unsafe { GetKeyState(VK_MENU) } < 0 };
        self.deliver(Event::Mouse { kind, x, y, mods });
    }

    /// A move over the top of the window, where the strip that drags it and
    /// the top resize edge lie. Those pixels are the program's own, so the
    /// move is reported like any other; without it a pointer hidden while
    /// typing could be moved there and the program would never hear of it.
    fn on_strip_move(&self, lparam: LPARAM) {
        let hwnd = self.hwnd.get();
        let mut at = POINT { x: get_x_lparam(lparam), y: get_y_lparam(lparam) };
        let mut client = RECT::default();
        unsafe {
            ScreenToClient(hwnd, &mut at);
            GetClientRect(hwnd, &mut client);
        }
        let inside = at.x >= 0 && at.y >= 0 && at.x < client.right && at.y < client.bottom;
        if inside && self.pointer_at.replace(Some((at.x, at.y))) != Some((at.x, at.y)) {
            self.deliver(Event::Mouse { kind: MouseKind::Move, x: at.x, y: at.y, mods: current_mods() });
        }
    }

    fn on_wheel(&self, wparam: WPARAM, lparam: LPARAM) {
        let (lines, _) = wheel_settings();
        let per_notch = if lines == WHEEL_PAGESCROLL {
            (self.client.get().1 as i32 / self.cell.get().1 - 1).max(1)
        } else {
            lines.min(100) as i32
        };
        let rows = self.wheel.borrow_mut().feed(get_wheel_delta(wparam), per_notch);
        if rows != 0 {
            // The wheel's position is in screen pixels, unlike every other mouse message.
            let mut at = POINT { x: get_x_lparam(lparam), y: get_y_lparam(lparam) };
            unsafe { ScreenToClient(self.hwnd.get(), &mut at) };
            self.deliver(Event::Wheel { x: at.x, y: at.y, rows });
        }
    }

    fn on_focus(&self, gained: bool) {
        self.focused.set(gained);
        // A key that went down before the focus moved will not be seen to
        // come up, so it can no longer claim the characters that follow.
        self.key_owns_chars.set(false);
        if gained {
            self.set_caret(self.caret.get());
            self.sync_mods(current_mods());
        } else {
            self.drop_caret();
            self.end_drag(true);
            // Whatever was held is no longer ours to see come up.
            self.sync_mods(Mods::NONE);
        }
        self.deliver(Event::Focus(gained));
    }

    /// A drag that loses the mouse (focus went elsewhere, another window
    /// took the capture) is ended where it was. `release` gives the capture
    /// back when it is still ours.
    fn end_drag(&self, release: bool) {
        if self.button.replace(false) {
            if release && !self.quiet {
                unsafe { ReleaseCapture() };
            }
            let (x, y) = self.pointer_at.get().unwrap_or((0, 0));
            self.deliver(Event::Mouse { kind: MouseKind::Up, x, y, mods: current_mods() });
        }
    }

    fn on_copydata(&self, lparam: LPARAM) -> LRESULT {
        let Some(data) = (unsafe { (lparam as *const COPYDATASTRUCT).as_ref() }) else { return 0 };
        if data.dwData != COPYDATA_OPEN {
            return 0;
        }
        if !data.lpData.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
            if let Some(path) = open_path(bytes) {
                self.deliver(Event::Open(path));
            }
        }
        1
    }

    /// Alt Space asks for the system menu. True when the request should go
    /// on to Windows, which opens it. Full screen the window has no system
    /// menu, and Windows answers a request for a menu that is not there
    /// with its beep, so there the request is refused. A self-test never
    /// passes it on either, because the menu would be a real one on the
    /// screen; it only notes that a real run would have.
    fn system_menu_asked(&self) -> bool {
        let style = unsafe { GetWindowLongPtrW(self.hwnd.get(), GWL_STYLE) } as u32;
        if style & WS_SYSMENU == 0 {
            self.report.borrow_mut().menus_refused += 1;
            return false;
        }
        if self.quiet {
            self.report.borrow_mut().system_menus += 1;
            return false;
        }
        true
    }
}

/// Closing, and what a self-test reports at the end.
impl Shell {
    /// The window was asked to close. The program hears of it first and the
    /// window goes when the program has finished.
    fn close(&self) {
        if self.accepting() {
            self.closing.set(true);
            self.deliver(Event::Close);
        } else {
            self.leave();
        }
    }

    /// Windows is ending the session. It takes the answer to this message
    /// as leave to go on, and after the last one may end the process at any
    /// moment, so the program must have heard Close before this returns,
    /// whatever the shell was in the middle of. `for_good` is the session
    /// really ending: the window goes too.
    ///
    /// Idle, that is an ordinary delivery. But Windows hands over a message
    /// like this whenever the thread looks at its queue, and the shell looks
    /// part way through an event, to see whether more typing is waiting
    /// before it draws. The shell is busy then and the program is not: it
    /// is told at once, first whatever was waiting its turn, so that nothing
    /// is heard out of order or left unheard, and then Close.
    ///
    /// The one case that cannot be met is the program itself being in the
    /// middle of a call that let the message in. It cannot be called again
    /// from inside its own call, so Close takes its turn in the queue and
    /// is heard as soon as that call returns, after anything that was
    /// already waiting.
    fn close_now(&self, for_good: bool) {
        if for_good {
            self.closing.set(true);
        }
        if !self.accepting() {
            if for_good {
                self.leave();
            }
            return;
        }
        self.waiting.borrow_mut().push_back(Event::Close);
        if !self.busy.get() {
            self.work();
            return;
        }
        if !self.program_free() {
            return;
        }
        loop {
            let next = self.waiting.borrow_mut().pop_front();
            let Some(event) = next else { break };
            if !self.accepting() {
                break;
            }
            if self.hand(event) {
                self.leave();
                break;
            }
        }
    }

    /// Remembers the window's place and destroys it, which ends the loop.
    fn leave(&self) {
        if self.leaving.replace(true) {
            return;
        }
        let hwnd = self.hwnd.get();
        if hwnd == 0 {
            return;
        }
        self.save_placement();
        if self.quiet {
            self.finish_report();
        }
        unsafe { DestroyWindow(hwnd) };
    }

    /// Fills in what a self-test reports about the window's last moment.
    fn finish_report(&self) {
        let hwnd = self.hwnd.get();
        let has_icons = unsafe { SendMessageW(hwnd, WM_GETICON, ICON_BIG, 0) != 0 && SendMessageW(hwnd, WM_GETICON, ICON_SMALL, 0) != 0 };
        let mut report = self.report.borrow_mut();
        report.has_icons = has_icons;
        if let Ok(frame) = self.frame.try_borrow() {
            report.width = frame.width();
            report.height = frame.height();
            report.pixels = frame.pixels().to_vec();
            if let Some(path) = &self.png {
                save_png(path, &frame);
            }
        }
        let mut client = RECT::default();
        unsafe { GetClientRect(hwnd, &mut client) };
        report.client = (client.w(), client.h());
        if let (true, Ok(frame)) = (self.shown.get(), self.frame.try_borrow()) {
            let ground = self.ground.get();
            report.on_glass = read_glass(hwnd, client.w(), client.h()).map(|glass| {
                let width = client.w() as usize;
                glass.iter().enumerate().all(|(i, seen)| {
                    let (x, y) = ((i % width) as i32, (i / width) as i32);
                    *seen == frame.get(x, y).unwrap_or(ground).px()
                })
            });
        }
        report.fullscreen = self.fullscreen.get();
        report.title = self.title.borrow().clone();
        report.frame_period_ms = self.period.get() * 1000.0;
        if self.caret_made.get().is_some() {
            let mut at = POINT::default();
            if unsafe { GetCaretPos(&mut at) } != 0 {
                report.caret = Some((at.x, at.y));
            }
        }
        if let Ok(host) = self.host.try_borrow() {
            if let Some(stand_ins) = &host.stand_ins {
                report.clipboard = stand_ins.clipboard.clone();
                report.opened = stand_ins.opened.clone();
            }
        }
        report.cloaked &= self.is_cloaked();
        report.was_foreground |= unsafe { GetForegroundWindow() } == hwnd;
    }

    fn is_cloaked(&self) -> bool {
        let mut cloaked: u32 = 0;
        dynapi().get_window_attr(self.hwnd.get(), DWMWA_CLOAKED, &mut cloaked) >= 0 && cloaked != 0
    }
}

/// The window procedure, once the shell has been found for the window.
impl Shell {
    fn message(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_GETMINMAXINFO => {
                    let result = DefWindowProcW(hwnd, msg, wparam, lparam);
                    if let Some(info) = (lparam as *mut MINMAXINFO).as_mut() {
                        let border = resize_border(hwnd);
                        let (cell_w, cell_h) = self.cell.get();
                        let (cols, rows) = self.min_cells.get();
                        let mut least = POINT { x: cols * cell_w + 2 * border, y: rows * cell_h + border };
                        // Never so large that the window cannot fit its monitor.
                        if let Some(monitor) = monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)) {
                            least.x = least.x.min(monitor.rcWork.w());
                            least.y = least.y.min(monitor.rcWork.h());
                        }
                        info.ptMinTrackSize = least;
                    }
                    result
                }
                WM_NCCALCSIZE => frameless_nccalcsize(hwnd, wparam, lparam).unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam)),
                WM_NCHITTEST => frameless_hittest(hwnd, lparam, self.caption.get()),
                WM_ERASEBKGND => 1,
                WM_PAINT => self.on_paint(),
                WM_SIZE => self.on_size(wparam, lparam),
                WM_SIZING => {
                    let (chrome_w, chrome_h) = self.chrome();
                    let (cell_w, cell_h) = self.cell.get();
                    let (cols, rows) = self.min_cells.get();
                    if lparam != 0 {
                        snap_sizing_rect(wparam, lparam as *mut RECT, cell_w, cell_h, chrome_w, chrome_h, cols, rows);
                    }
                    1
                }
                WM_WINDOWPOSCHANGING => {
                    // While an edge is dragged, Windows would first slide the
                    // old picture to the new corner and only then let it be
                    // redrawn, which shows as a shudder when the left or top
                    // edge moves. The frame is redrawn whole anyway.
                    if self.moving.get() > 0 {
                        if let Some(pos) = (lparam as *mut WINDOWPOS).as_mut() {
                            if pos.flags & SWP_NOSIZE == 0 {
                                pos.flags |= SWP_NOCOPYBITS;
                            }
                        }
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
                    self.moving.set(self.moving.get() + 1);
                    self.sync_moving_timer();
                    self.sync_waker_timer();
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
                    self.moving.set(self.moving.get().saturating_sub(1));
                    self.sync_moving_timer();
                    self.sync_waker_timer();
                    self.next_frame.set(now() + self.period.get());
                    if msg == WM_EXITSIZEMOVE && !self.leaving.get() {
                        self.save_placement();
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_TIMER => {
                    self.on_timer(wparam);
                    0
                }
                // Another thread has news. Its tick is not sent from here:
                // a posted message is served before the keyboard, so a
                // thread that woke the window in a tight loop could keep
                // typing out. The tick goes when the queue is empty.
                WM_WAKE => {
                    self.wake_arrived.set(true);
                    self.sync_waker_timer();
                    0
                }
                WM_KEYDOWN => {
                    self.on_key_down(wparam, lparam);
                    0
                }
                WM_KEYUP => {
                    self.on_key_up();
                    0
                }
                // Alt chords and F10 arrive in the system forms. They are
                // read the same way and then also given to Windows, which is
                // what keeps Alt Space working.
                WM_SYSKEYDOWN => {
                    self.on_key_down(wparam, lparam);
                    // Alt F4. Windows would turn it into the close command
                    // too, but only for a window it counts as active; sent
                    // from here it is the same command with no condition.
                    if (wparam & 0xFFFF) as i32 == VK_F4 && key_alt_down(lparam) {
                        PostMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
                        return 0;
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_SYSKEYUP => {
                    self.on_key_up();
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_CHAR => {
                    let ch = self.decoder.borrow_mut().push(wparam as u16);
                    self.on_char(ch);
                    0
                }
                WM_UNICHAR => {
                    if wparam == UNICODE_NOCHAR {
                        return 1;
                    }
                    let ch = self.decoder.borrow_mut().push_utf32(wparam as u32);
                    self.on_char(ch);
                    0
                }
                // The character half of an Alt chord. Windows would beep for
                // one it has no menu item for, so only Alt Space is passed
                // on, and only while there is a system menu for it to open.
                WM_SYSCHAR => {
                    if wparam == b' ' as usize && self.system_menu_asked() {
                        DefWindowProcW(hwnd, msg, wparam, lparam)
                    } else {
                        0
                    }
                }
                WM_DEADCHAR | WM_SYSDEADCHAR => 0,
                WM_SYSCOMMAND => {
                    if wparam & 0xFFF0 == SC_KEYMENU {
                        // With a space this is Alt Space, on its way from
                        // the character above to the system menu.
                        if lparam == b' ' as isize {
                            if !self.system_menu_asked() {
                                return 0;
                            }
                        } else {
                            // A bare Alt, or F10, asks to enter the menu
                            // bar. There is none, and going there would
                            // swallow the next key typed.
                            self.report.borrow_mut().menus_refused += 1;
                            return 0;
                        }
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_LBUTTONDOWN => {
                    self.on_mouse(MouseKind::Down, wparam, lparam);
                    0
                }
                WM_LBUTTONUP => {
                    self.on_mouse(MouseKind::Up, wparam, lparam);
                    0
                }
                WM_MOUSEMOVE => {
                    self.on_mouse(MouseKind::Move, wparam, lparam);
                    0
                }
                WM_NCMOUSEMOVE => {
                    self.on_strip_move(lparam);
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_MOUSEWHEEL => {
                    self.on_wheel(wparam, lparam);
                    0
                }
                WM_CAPTURECHANGED => {
                    if lparam != hwnd {
                        self.end_drag(false);
                    }
                    0
                }
                WM_SETCURSOR => {
                    if loword(lparam as usize) as isize == HTCLIENT {
                        if !self.quiet {
                            SetCursor(if self.pointer_hidden.get() { 0 } else { LoadCursorW(0, IDC_ARROW) });
                        }
                        return 1;
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_MOUSEACTIVATE if self.quiet => MA_NOACTIVATE,
                WM_SETFOCUS => {
                    self.on_focus(true);
                    0
                }
                WM_KILLFOCUS => {
                    self.on_focus(false);
                    0
                }
                // The window has come onto a monitor with another scaling.
                // Windows suggests a rectangle for an ordinary window; a
                // full-screen one takes its new monitor whole instead.
                WM_DPICHANGED => {
                    if self.fullscreen.get() {
                        self.cover_monitor();
                    } else if let Some(r) = (lparam as *const RECT).as_ref() {
                        SetWindowPos(hwnd, 0, r.left, r.top, r.w(), r.h(), SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                    0
                }
                // The monitor's size or refresh rate changed.
                WM_DISPLAYCHANGE => {
                    self.period.set(even_frame_period(60.0));
                    if self.fullscreen.get() {
                        self.cover_monitor();
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_COPYDATA => self.on_copydata(lparam),
                WM_CLOSE => {
                    self.close();
                    0
                }
                // Shutdown or sign-out. The program saves now, while it is
                // certain to get the time, and again when the session really
                // ends, in case anything reached it in between; then the
                // window goes too.
                WM_QUERYENDSESSION => {
                    self.close_now(false);
                    1
                }
                WM_ENDSESSION => {
                    if wparam != 0 {
                        self.close_now(true);
                    }
                    0
                }
                WM_WATCHDOG if self.quiet => {
                    self.report.borrow_mut().timed_out = true;
                    self.leave();
                    0
                }
                WM_DESTROY => {
                    for id in [TIMER_WAKE, TIMER_MOVING, TIMER_SCRIPT, TIMER_WAKER] {
                        KillTimer(hwnd, id);
                    }
                    self.drop_caret();
                    if self.button.replace(false) && !self.quiet {
                        ReleaseCapture();
                    }
                    self.leaving.set(true);
                    PostQuitMessage(self.exit_code.get());
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }
}

/// Performing a self-test's script.
impl Shell {
    /// Does the next step of the script, if there is one and it is not
    /// pausing. Returns true when something was done.
    fn script_turn(&self) -> bool {
        let step = {
            let mut guard = self.script.borrow_mut();
            let Some(script) = guard.as_mut() else { return false };
            if script.paused || script.finished || self.leaving.get() {
                return false;
            }
            match script.steps.pop_front() {
                Some(step) => step,
                None => {
                    script.finished = true;
                    Micro::End
                }
            }
        };
        self.perform(step);
        true
    }

    fn perform(&self, step: Micro) {
        let hwnd = self.hwnd.get();
        // Every step starts from the keys the script holds and no others.
        // Windows can bring a thread's key state into line with the real
        // keyboard, and a Ctrl really held at that moment would otherwise
        // turn the step's characters into a chord.
        if let Some(script) = self.script.borrow().as_ref() {
            unsafe { SetKeyboardState(script.keys.as_ptr()) };
        }
        unsafe {
            match step {
                Micro::Key { vk, down, repeat } => {
                    if let Some((message, lparam)) = self.script_key(vk, down, repeat) {
                        PostMessageW(hwnd, message, (vk & 0xFF) as usize, lparam);
                    }
                }
                Micro::Chars(units) => {
                    for unit in units {
                        PostMessageW(hwnd, WM_CHAR, unit as usize, 1);
                    }
                }
                Micro::Mouse { message, x, y } => {
                    let pressed = message == WM_LBUTTONDOWN || (message == WM_MOUSEMOVE && self.button.get());
                    let keys = self.script_mouse_keys() | if pressed { MK_LBUTTON } else { 0 };
                    PostMessageW(hwnd, message, keys as usize, make_lparam(x, y));
                }
                Micro::Trail(points) => {
                    let keys = self.script_mouse_keys() | if self.button.get() { MK_LBUTTON } else { 0 };
                    for (x, y) in points {
                        PostMessageW(hwnd, WM_MOUSEMOVE, keys as usize, make_lparam(x, y));
                    }
                }
                Micro::Wheel { x, y, delta } => {
                    let mut at = POINT { x, y };
                    ClientToScreen(hwnd, &mut at);
                    let wparam = ((delta as i16 as u16 as usize) << 16) | self.script_mouse_keys() as usize;
                    PostMessageW(hwnd, WM_MOUSEWHEEL, wparam, make_lparam(at.x, at.y));
                }
                Micro::Size(w, h) => {
                    PostMessageW(hwnd, WM_SIZE, SIZE_RESTORED, make_lparam(w.min(0xFFFF) as i32, h.min(0xFFFF) as i32));
                }
                Micro::Client(w, h) => {
                    if !self.fullscreen.get() {
                        let (chrome_w, chrome_h) = self.chrome();
                        let (w, h) = (w.min(FRAME_SIDE as u32) as i32 + chrome_w, h.min(FRAME_SIDE as u32) as i32 + chrome_h);
                        SetWindowPos(hwnd, 0, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE);
                    }
                }
                Micro::Drag(w, h) => {
                    let (chrome_w, chrome_h) = self.chrome();
                    let mut window = RECT::default();
                    GetWindowRect(hwnd, &mut window);
                    let mut dragged = RECT {
                        right: window.left + w.min(FRAME_SIDE as u32) as i32 + chrome_w,
                        bottom: window.top + h.min(FRAME_SIDE as u32) as i32 + chrome_h,
                        ..window
                    };
                    SendMessageW(hwnd, WM_SIZING, WMSZ_BOTTOMRIGHT, &mut dragged as *mut RECT as LPARAM);
                    self.report.borrow_mut().snaps.push((dragged.w() - chrome_w, dragged.h() - chrome_h));
                }
                Micro::Focus(on) => {
                    PostMessageW(hwnd, if on { WM_SETFOCUS } else { WM_KILLFOCUS }, 0, 0);
                }
                Micro::Wait(ms) => {
                    if let Some(script) = self.script.borrow_mut().as_mut() {
                        script.paused = true;
                    }
                    SetTimer(hwnd, TIMER_SCRIPT, ms.min(USER_TIMER_MAXIMUM as u64) as u32, None);
                }
                Micro::Probe => {
                    let mut report = self.report.borrow_mut();
                    let probe = Probe {
                        at_ms: (now() - self.started) * 1000.0,
                        cpu_ms: cpu_ms(),
                        ticks: report.ticks.len(),
                        renders: report.renders,
                    };
                    report.probes.push(probe);
                    report.was_foreground |= GetForegroundWindow() == hwnd;
                }
                Micro::Open(path) => {
                    send_copydata(hwnd, hwnd, COPYDATA_OPEN, &open_payload(&path));
                }
                Micro::Moving(on) => {
                    PostMessageW(hwnd, if on { WM_ENTERSIZEMOVE } else { WM_EXITSIZEMOVE }, 0, 0);
                }
                Micro::EndSession(really) => {
                    SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0);
                    SendMessageW(hwnd, WM_ENDSESSION, really as usize, 0);
                }
                Micro::Shot(path) => {
                    if let Ok(frame) = self.frame.try_borrow() {
                        save_png(&path, &frame);
                    }
                }
                Micro::Close => {
                    PostMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE, 0);
                }
                Micro::End => {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
        }
    }

    /// Records a scripted key in the thread's key state and works out the
    /// message a keyboard would have produced for it: the system form while
    /// Alt is held without Ctrl, and for F10.
    fn script_key(&self, vk: i32, down: bool, repeat: bool) -> Option<(u32, LPARAM)> {
        let mut guard = self.script.borrow_mut();
        let script = guard.as_mut()?;
        let vk = vk & 0xFF;
        let state = if down { 0x80 } else { 0 };
        script.keys[vk as usize] = state;
        let left = match vk {
            VK_CONTROL => Some(VK_LCONTROL),
            VK_SHIFT => Some(VK_LSHIFT),
            VK_MENU => Some(VK_LMENU),
            _ => None,
        };
        if let Some(left) = left {
            script.keys[left as usize] = state;
        }
        unsafe { SetKeyboardState(script.keys.as_ptr()) };

        let alt = script.keys[VK_MENU as usize] != 0;
        let ctrl = script.keys[VK_CONTROL as usize] != 0;
        let system = if vk == VK_MENU {
            !ctrl && (down || script.alt_alone)
        } else {
            vk == VK_F10 || (alt && !ctrl)
        };
        if down {
            script.alt_alone = vk == VK_MENU;
        }
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as isize & 0xFF;
        let mut lparam: isize = 1 | (scan << 16);
        if system && alt {
            lparam |= 1 << 29;
        }
        if repeat || !down {
            lparam |= 1 << 30;
        }
        if !down {
            lparam |= 1 << 31;
        }
        let message = match (system, down) {
            (true, true) => WM_SYSKEYDOWN,
            (true, false) => WM_SYSKEYUP,
            (false, true) => WM_KEYDOWN,
            (false, false) => WM_KEYUP,
        };
        Some((message, lparam))
    }

    /// The modifier bits a mouse message carries for what the script holds.
    fn script_mouse_keys(&self) -> u32 {
        let guard = self.script.borrow();
        let Some(script) = guard.as_ref() else { return 0 };
        let mut keys = 0;
        if script.keys[VK_SHIFT as usize] != 0 {
            keys |= MK_SHIFT;
        }
        if script.keys[VK_CONTROL as usize] != 0 {
            keys |= MK_CONTROL;
        }
        keys
    }
}

/// The message loop.
impl Shell {
    /// Runs until the window is gone. Idle, it is blocked in `GetMessageW`
    /// and uses no processor at all. While the program animates it waits on
    /// a high-resolution timer and the message queue together, so a frame is
    /// never late and a key is never kept waiting behind one.
    ///
    /// Each turn handles every message that is waiting and only then what
    /// must not come before the keyboard: a waker's tick, and a script's
    /// next step. A frame or a wake that is overdue cuts the handling short
    /// for that one tick, and the next turn takes it up again.
    fn run(&self) -> i32 {
        let mut msg = MSG::default();
        loop {
            let mut emptied = true;
            unsafe {
                while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        return msg.wParam as i32;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                    if self.frame_cannot_wait() || self.wake_cannot_wait() {
                        emptied = false;
                        break;
                    }
                }
            }
            if self.wake_cannot_wait() {
                self.on_timer(TIMER_WAKE);
            }
            if emptied {
                self.waker_tick();
            }
            self.catch_up();
            // A script's next step waits for an empty queue, so that each
            // step meets the window as the one before it left it.
            if emptied && self.script_turn() {
                continue;
            }
            if self.animating.get() && self.moving.get() == 0 && self.accepting() {
                self.wait_for_frame();
            } else {
                unsafe {
                    let got = GetMessageW(&mut msg, 0, 0, 0);
                    if got <= 0 {
                        return if got == 0 { msg.wParam as i32 } else { 1 };
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    /// True when a frame is due and messages have had their share of the
    /// time since the last one. A stream of messages that never lets the
    /// queue run empty (a fast mouse dragging a selection) would otherwise
    /// hold every frame back until it stopped. The share, half a frame
    /// period, is what keeps the reverse from happening: frames that are
    /// slow to draw never crowd out the keyboard.
    fn frame_cannot_wait(&self) -> bool {
        if !self.animating.get() || self.moving.get() > 0 || !self.accepting() {
            return false;
        }
        let t = now();
        t >= self.next_frame.get() && t >= self.frame_done.get() + self.period.get() / 2.0
    }

    /// True when the one-shot wake is well past its moment, which means
    /// the queue has not been empty for its timer to be handed out.
    fn wake_cannot_wait(&self) -> bool {
        self.wake_due.get().is_some_and(|due| now_ms() >= due.saturating_add(WAKE_PATIENCE_MS)) && self.accepting()
    }

    /// Sleeps until the next frame is due or a message arrives, and sends
    /// the tick if it was the frame.
    fn wait_for_frame(&self) {
        let wait = self.next_frame.get() - now();
        if wait > 0.0 {
            let ms = (wait * 1000.0).ceil() as u32;
            let woke = unsafe {
                if self.timer.handle != 0 && self.timer.arm_in(wait) {
                    // The timeout is only a net under the timer.
                    MsgWaitForMultipleObjectsEx(1, &self.timer.handle, ms.saturating_add(100), QS_ALLINPUT, MWMO_INPUTAVAILABLE)
                } else {
                    match MsgWaitForMultipleObjectsEx(0, null(), ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE) {
                        WAIT_OBJECT_0 => WAIT_OBJECT_0 + 1,
                        other => other,
                    }
                }
            };
            match woke {
                WAIT_OBJECT_0 | WAIT_TIMEOUT => {}
                // A message: go round and handle it before any frame.
                other if other == WAIT_OBJECT_0 + 1 => return,
                // The wait itself failed. Never spin on that.
                _ => {
                    unsafe { Sleep(1) };
                    return;
                }
            }
        }
        // If frames were missed, skip them; do not send a burst to catch up.
        let (period, t) = (self.period.get(), now());
        let next = self.next_frame.get() + period;
        self.next_frame.set(if next > t { next } else { t + period });
        self.tick(TickSource::Frame);
        self.frame_done.set(now());
    }
}

/// Starting: the program is made, the window placed and shown.
impl Shell {
    /// Makes the program, places the window and shows it. `open` is the file
    /// this launch was given, which the program is told of before the first
    /// frame is seen.
    fn start(&self, make: MakeProgram, client: Option<(u32, u32)>, open: Option<PathBuf>) {
        let made = {
            let mut host = self.host.borrow_mut();
            catch_unwind(AssertUnwindSafe(|| make(&mut *host)))
        };
        match made {
            Ok(program) => {
                *self.program.borrow_mut() = Some(program);
                self.live.set(true);
            }
            Err(payload) => {
                self.crashed(payload);
                return;
            }
        }
        // The program is asked what it wants once before it has been told
        // any size, because the cell, the smallest window and whether to
        // open full screen decide what that size will be. Placing the
        // window, and taking it full screen, each send a size message. All
        // of it happens with the queue held back, so the program is told
        // the size once: the size it will first be seen at.
        self.busy.set(true);
        let wants = self.call("being asked what it wants", |program, host| program.wants(&*host));
        if let Some(wants) = &wants {
            self.adopt(wants);
            self.place(client);
            if wants.fullscreen {
                self.set_fullscreen(true);
            }
        }
        self.busy.set(false);
        if wants.is_none() {
            return;
        }
        let mut area = RECT::default();
        unsafe { GetClientRect(self.hwnd.get(), &mut area) };
        let (width, height) = frame_size(area.w().max(0) as usize, area.h().max(0) as usize);
        self.client.set((width, height));
        {
            // The sizes passed through on the way are dropped. Anything
            // else that came in meanwhile (a second launch's file) keeps
            // its place behind the first size and this launch's own file.
            let mut waiting = self.waiting.borrow_mut();
            waiting.retain(|event| !matches!(event, Event::Resize { .. }));
            if let Some(path) = open {
                waiting.push_front(Event::Open(path));
            }
            waiting.push_front(Event::Resize { width, height });
        }
        self.work();
        self.show();
    }

    /// The first show, with no white flash: the window is cloaked, shown,
    /// painted, and only then uncovered. A self-test stays cloaked for good,
    /// and is never shown at all unless Windows confirms the cloak.
    fn show(&self) {
        if self.leaving.get() {
            return;
        }
        let hwnd = self.hwnd.get();
        if self.quiet {
            if self.report.borrow().cloaked {
                self.shown.set(true);
                unsafe {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    UpdateWindow(hwnd);
                }
                self.report.borrow_mut().was_foreground |= unsafe { GetForegroundWindow() } == hwnd;
            }
            return;
        }
        self.shown.set(true);
        let how = if self.fullscreen.get() {
            SW_SHOW
        } else if self.open_maximized.get() {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        };
        show_without_flash(hwnd, how);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            if let Some(create) = (lparam as *const CREATESTRUCTW).as_ref() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
        }
        // The very first message, WM_GETMINMAXINFO, comes before the pointer is there.
        let Some(shell) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shell).as_ref() else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        if msg == WM_NCCREATE {
            shell.hwnd.set(hwnd);
        }
        if msg == WM_NCDESTROY {
            shell.wake.detach();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // A panic must not cross into Windows. One in the shell's own code
        // is treated like one in the program: written down, and the program
        // given its last chance to save.
        match catch_unwind(AssertUnwindSafe(|| shell.message(hwnd, msg, wparam, lparam))) {
            Ok(result) => result,
            Err(payload) => {
                let _ = catch_unwind(AssertUnwindSafe(|| shell.crashed(payload)));
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }
}

/// Finds the window of the instance that is already running, hands it the
/// file this launch was given, and brings it forward.
fn hand_over(class: &[u16], open: Option<&Path>) {
    unsafe {
        let mut hwnd = 0;
        // The first instance may still be making its window.
        for _ in 0..40 {
            hwnd = FindWindowW(class.as_ptr(), null());
            if hwnd != 0 {
                break;
            }
            Sleep(50);
        }
        if hwnd == 0 {
            return;
        }
        let mut process = 0u32;
        GetWindowThreadProcessId(hwnd, &mut process);
        AllowSetForegroundWindow(process);
        if let Some(path) = open {
            send_copydata(hwnd, 0, COPYDATA_OPEN, &open_payload(&whole_path(path)));
        }
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}

/// Builds the window's large and small icons from the pixels given, choosing
/// for each the smallest image that is big enough. With no pixels, an icon
/// stamped into the exe is used if there is one.
fn make_icons(hwnd: HWND, icons: &[Icon]) -> Vec<HICON> {
    unsafe {
        let pick = |want: i32| -> HICON {
            let best = icons.iter().filter(|i| i.is_well_formed()).min_by_key(|i| {
                let w = i.width as i32;
                if w >= want {
                    (0, w - want)
                } else {
                    (1, want - w)
                }
            });
            match best {
                Some(i) => icon_from_argb(i.width as i32, i.height as i32, &i.argb),
                None => LoadImageW(GetModuleHandleW(null()), make_int_resource(1), IMAGE_ICON, want, want, LR_DEFAULTCOLOR),
            }
        };
        let big = pick(GetSystemMetrics(SM_CXICON));
        let small = pick(GetSystemMetrics(SM_CXSMICON));
        if big != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG, big);
        }
        if small != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small);
        }
        vec![big, small]
    }
}

/// Everything from process setup to the last message. `test` is `Some` for a
/// self-test.
pub(crate) fn run_shell(options: Options, test: Option<SelfTest>, make: MakeProgram) -> Outcome {
    // Before any window exists and before anything measures the screen.
    init_dpi_awareness();
    let quiet = test.is_some();
    let class = wide(&format!("{}.{}", options.app_id, if quiet { "selftest" } else { "window" }));
    let failed = |code: i32| Outcome { code, report: Report { exit_code: code, ..Report::default() } };

    let mut instance = 0;
    if !quiet {
        unsafe { SetCurrentProcessExplicitAppUserModelID(wide(&options.app_id).as_ptr()) };
        let lock = instance_lock(&format!("Local\\{}.instance", options.app_id));
        if lock.already_running {
            hand_over(&class, options.open.as_deref());
            if lock.handle != 0 {
                unsafe { CloseHandle(lock.handle) };
            }
            return failed(0);
        }
        instance = lock.handle;
    }

    crash::watch(true);
    let shell = Box::new(Shell::new(&options, test.as_ref()));
    let hwnd = unsafe {
        let module = GetModuleHandleW(null());
        let window_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            // No redraw-on-resize styles and no background brush: the frame
            // is the only thing ever painted, so there is nothing to flash.
            style: 0,
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: 0,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if RegisterClassExW(&window_class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            0
        } else {
            let extra = if quiet { WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW } else { 0 };
            CreateWindowExW(
                extra,
                class.as_ptr(),
                wide("ok").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                module,
                &*shell as *const Shell as *mut c_void,
            )
        }
    };
    if hwnd == 0 {
        crash::watch(false);
        if instance != 0 {
            unsafe { CloseHandle(instance) };
        }
        return failed(1);
    }
    shell.hwnd.set(hwnd);
    shell.host.borrow_mut().hwnd = hwnd;
    shell.wake.attach(hwnd);

    let mut saved_keys = [0u8; 256];
    let mut watchdog = None;
    if let Some(test) = &test {
        let on: BOOL = TRUE;
        dynapi().set_window_attr(hwnd, DWMWA_CLOAK, &on);
        shell.report.borrow_mut().cloaked = shell.is_cloaked();
        unsafe { GetKeyboardState(saved_keys.as_mut_ptr()) };
        watchdog = Some(Watchdog::start(hwnd, WM_WATCHDOG, test.timeout_ms));
    }
    let icons = make_icons(hwnd, &options.icons);

    let open = options.open.as_deref().map(whole_path);
    shell.start(make, test.as_ref().and_then(|t| t.client), open);
    let code = shell.run();

    unsafe {
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        for icon in icons {
            if icon != 0 {
                DestroyIcon(icon);
            }
        }
        if quiet {
            SetKeyboardState(saved_keys.as_ptr());
        }
        if instance != 0 {
            CloseHandle(instance);
        }
    }
    // The program's own destructors run here, still under guard.
    let program = shell.program.borrow_mut().take();
    let _ = catch_unwind(AssertUnwindSafe(move || drop(program)));
    crash::watch(false);
    // Only now is the run over as the watchdog counts it: a program whose
    // destructor never returned would be as stuck as any other.
    drop(watchdog);

    let mut report = shell.report.take();
    report.exit_code = code;
    Outcome { code, report }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_survives_the_trip_between_instances() {
        for text in [r"C:\notes\essay.md", r"C:\Users\jaspe\Documents\ok\The orchard.md", "C:\\caf\u{00E9}\\\u{1F600}.txt"] {
            let path = PathBuf::from(text);
            assert_eq!(open_path(&open_payload(&path)), Some(path));
        }
    }

    #[test]
    fn a_launch_names_its_file_in_full() {
        let whole = whole_path(Path::new("notes.txt"));
        assert!(whole.is_absolute() && whole.ends_with("notes.txt"), "{whole:?}");
        assert_eq!(whole_path(Path::new(r"C:\notes\essay.md")), PathBuf::from(r"C:\notes\essay.md"));
    }

    #[test]
    fn only_the_first_holder_of_the_lock_is_first() {
        let name = format!("Local\\JasperMeller.ok.test.instance.{}", std::process::id());
        let first = instance_lock(&name);
        assert!(first.handle != 0 && !first.already_running);
        let second = instance_lock(&name);
        assert!(second.already_running, "a second launch sees the first");
        unsafe {
            CloseHandle(second.handle);
            CloseHandle(first.handle);
        }
        let later = instance_lock(&name);
        assert!(!later.already_running, "and the lock goes with the instance that held it");
        unsafe { CloseHandle(later.handle) };
    }

    #[test]
    fn a_damaged_hand_over_is_refused() {
        assert_eq!(open_path(&[]), None);
        assert_eq!(open_path(&[0x41]), None, "half a code unit");
        assert_eq!(open_path(&[0x41, 0, 0, 0, 0x42, 0]), None, "a NUL inside");
        assert_eq!(open_path(&vec![0x41; (1 << 16) + 2]), None, "absurdly long");
    }

    #[test]
    fn frames_stay_within_what_can_be_allocated() {
        assert_eq!(frame_size(1920, 1080), (1920, 1080));
        assert_eq!(frame_size(0, 500), (0, 500));
        assert_eq!(frame_size(7680, 4320), (7680, 4320));
        let (w, h) = frame_size(65_535, 65_535);
        assert!(w <= FRAME_SIDE && w * h <= FRAME_AREA, "{w} by {h}");
    }
}
