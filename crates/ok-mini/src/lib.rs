//! The window shell for macOS and Linux: one minifb window, the framebuffer
//! shown in it, and the machine behind [`ok_host::Host`].
//!
//! It does what `ok-win` does on Windows with less ceremony. minifb opens
//! the window and puts the picture on the glass; this crate turns its keys,
//! characters, pointer and size into [`ok_host::Event`]s, asks the program
//! what it [`ok_host::Wants`], and draws when it says so. The interface is
//! the same program on every system.
//!
//! On the Mac, Cmd does what Ctrl does elsewhere, Option with an arrow moves
//! by word, Cmd with an arrow goes to the end of the line or the text, and
//! Option with a letter types the letter the keyboard gives it. The picture
//! is drawn at the screen's own pixel density, so Gallant stays sharp on a
//! Retina screen.
#![cfg(not(windows))]

mod host;
mod keys;
#[cfg(target_os = "macos")]
mod mac;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use minifb::{InputCallback, MouseButton, MouseMode, ScaleMode, Window, WindowOptions};
use ok_gfx::Frame;
use ok_host::{Event, Host, MouseKind, Program, Wants};

use host::MiniHost;
use keys::{Held, Translated};

/// What the shell needs to know before the program exists.
#[derive(Clone, Debug)]
pub struct Options {
    /// The application's folder name under the system's data folders.
    pub name: String,
    /// The file named on the command line, sent as [`Event::Open`] after
    /// the first [`Event::Resize`].
    pub open: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options { name: String::from("ok"), open: None }
    }
}

/// The window's size when it first opens, in points.
const FIRST_SIZE: (usize, usize) = (1280, 800);
/// How long to wait between looks at the window while nothing is moving.
/// Short enough that a key is drawn within it; long enough to cost nothing.
const IDLE_POLL: Duration = Duration::from_millis(4);
/// One frame while something is moving.
const FRAME: Duration = Duration::from_millis(16);

/// What arrives from minifb's input callback, in the order it came.
enum Input {
    Key(minifb::Key, bool),
    Char(u32),
}

struct Catcher(Rc<RefCell<Vec<Input>>>);

impl InputCallback for Catcher {
    fn add_char(&mut self, ch: u32) {
        self.0.borrow_mut().push(Input::Char(ch));
    }

    fn set_key_state(&mut self, key: minifb::Key, down: bool) {
        self.0.borrow_mut().push(Input::Key(key, down));
    }
}

/// Opens the window and runs `make_program`'s program in it until the
/// program asks to quit or the window is closed. Returns the process exit
/// code: 0, or 1 if the window could not be made.
pub fn run<F>(options: Options, make_program: F) -> i32
where
    F: FnOnce(&mut dyn Host) -> Box<dyn Program>,
{
    let window_options = WindowOptions { resize: true, scale_mode: ScaleMode::Stretch, ..WindowOptions::default() };
    let Ok(mut window) = Window::new("ok", FIRST_SIZE.0, FIRST_SIZE.1, window_options) else {
        eprintln!("ok: the window could not be made");
        return 1;
    };
    #[cfg(target_os = "macos")]
    mac::quit_by_closing();
    let inputs = Rc::new(RefCell::new(Vec::new()));
    window.set_input_callback(Box::new(Catcher(inputs.clone())));

    let woken = Arc::new(AtomicBool::new(false));
    let mut host = MiniHost::new(&options.name, woken.clone());
    let mut program = make_program(&mut host);
    let mut shell = Shell::new(window, inputs, woken);
    shell.go(&mut *program, &mut host, options.open);
    0
}

struct Shell {
    window: Window,
    inputs: Rc<RefCell<Vec<Input>>>,
    woken: Arc<AtomicBool>,
    frame: Frame,
    /// Pixels to a point: 2 on a Retina screen, 1 elsewhere.
    density: f64,
    size: (usize, usize),
    held: Held,
    pointer: Option<(i32, i32)>,
    pressed: bool,
    wheel: f32,
    active: bool,
    wants: Wants,
    wake_at: Option<Instant>,
    next_frame: Instant,
    drawn: bool,
    fullscreen: bool,
    title: String,
}

impl Shell {
    fn new(window: Window, inputs: Rc<RefCell<Vec<Input>>>, woken: Arc<AtomicBool>) -> Shell {
        Shell {
            window,
            inputs,
            woken,
            frame: Frame::new(1, 1),
            density: 1.0,
            size: (0, 0),
            held: Held::default(),
            pointer: None,
            pressed: false,
            wheel: 0.0,
            active: true,
            wants: Wants { fullscreen: false, ..Wants::default() },
            wake_at: None,
            next_frame: Instant::now(),
            drawn: false,
            fullscreen: false,
            title: String::new(),
        }
    }

    fn go(&mut self, program: &mut dyn Program, host: &mut MiniHost, open: Option<PathBuf>) {
        self.window.update();
        self.resize(program, host);
        if let Some(path) = open {
            self.deliver(program, host, Event::Open(path));
        }
        loop {
            if !self.window.is_open() {
                self.close(program, host);
                return;
            }
            if self.wants.quit {
                return;
            }
            self.resize(program, host);
            self.take_input(program, host);
            self.take_pointer(program, host);
            let active = self.window.is_active();
            if active != self.active {
                self.active = active;
                if !active {
                    // Keys let go of while another window had the keyboard
                    // are never heard of: start again with none held.
                    self.held = Held::default();
                    self.deliver(program, host, Event::Mods(self.held.mods()));
                }
                self.deliver(program, host, Event::Focus(active));
            }
            let now = Instant::now();
            let woken = self.woken.swap(false, Ordering::AcqRel);
            let due = self.wake_at.is_some_and(|at| now >= at);
            let frame_due = self.wants.animating && now >= self.next_frame;
            if woken || due || frame_due {
                if due {
                    self.wake_at = None;
                }
                if frame_due {
                    self.next_frame = now + FRAME;
                }
                self.deliver(program, host, Event::Tick);
            }
            if self.wants.redraw || !self.drawn {
                self.draw(program, host);
            } else {
                self.window.update();
            }
            std::thread::sleep(self.pause());
        }
    }

    /// How long to sleep before looking at the window again.
    fn pause(&self) -> Duration {
        let now = Instant::now();
        let mut pause = IDLE_POLL;
        if self.wants.animating {
            pause = pause.min(self.next_frame.saturating_duration_since(now));
        }
        if let Some(at) = self.wake_at {
            pause = pause.min(at.saturating_duration_since(now));
        }
        pause
    }

    /// Hands one event to the program and does what it then wants.
    fn deliver(&mut self, program: &mut dyn Program, host: &mut MiniHost, event: Event) {
        program.event(event, host);
        let wants = program.wants(host);
        self.apply(wants);
    }

    fn apply(&mut self, wants: Wants) {
        if wants.title != self.title {
            self.window.set_title(&wants.title);
            self.title = wants.title.clone();
        }
        self.window.set_cursor_visibility(!wants.pointer_hidden);
        if wants.fullscreen != self.fullscreen {
            self.fullscreen = wants.fullscreen;
            #[cfg(target_os = "macos")]
            mac::set_fullscreen(self.window.get_window_handle(), wants.fullscreen);
        }
        self.wake_at = wants.wake_in_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
        if wants.animating && !self.wants.animating {
            self.next_frame = Instant::now();
        }
        let redraw = wants.redraw || self.wants.redraw;
        self.wants = Wants { redraw, ..wants };
    }

    fn draw(&mut self, program: &mut dyn Program, host: &mut MiniHost) {
        program.render(&mut self.frame, host);
        self.wants.redraw = false;
        self.drawn = true;
        let (width, height) = (self.frame.width(), self.frame.height());
        if self.window.update_with_buffer(self.frame.pixels(), width, height).is_err() {
            self.window.update();
        }
    }

    /// The window's size in pixels, and a Resize when it has changed.
    fn resize(&mut self, program: &mut dyn Program, host: &mut MiniHost) {
        #[cfg(target_os = "macos")]
        {
            self.density = mac::density(self.window.get_window_handle());
        }
        let (points_w, points_h) = self.window.get_size();
        let size = (((points_w as f64) * self.density).round() as usize, ((points_h as f64) * self.density).round() as usize);
        if size == self.size || size.0 == 0 || size.1 == 0 {
            return;
        }
        self.size = size;
        self.frame = Frame::new(size.0, size.1);
        self.drawn = false;
        self.deliver(program, host, Event::Resize { width: size.0, height: size.1 });
    }

    fn take_input(&mut self, program: &mut dyn Program, host: &mut MiniHost) {
        let inputs: Vec<Input> = self.inputs.borrow_mut().drain(..).collect();
        for input in inputs {
            let translated = match input {
                Input::Key(key, down) => self.held.key(key, down),
                Input::Char(ch) => self.held.text(ch),
            };
            for out in translated {
                match out {
                    Translated::Event(event) => self.deliver(program, host, event),
                    Translated::Quit => {
                        self.close(program, host);
                        return;
                    }
                }
            }
        }
    }

    fn take_pointer(&mut self, program: &mut dyn Program, host: &mut MiniHost) {
        let mods = self.held.mods();
        let at = self.window.get_mouse_pos(MouseMode::Discard).map(|(x, y)| ((x as f64 * self.density) as i32, (y as f64 * self.density) as i32));
        if at != self.pointer {
            self.pointer = at;
            if let Some((x, y)) = at {
                self.deliver(program, host, Event::Mouse { kind: MouseKind::Move, x, y, mods });
            }
        }
        let pressed = self.window.get_mouse_down(MouseButton::Left);
        if pressed != self.pressed {
            self.pressed = pressed;
            if let Some((x, y)) = self.pointer {
                let kind = if pressed { MouseKind::Down } else { MouseKind::Up };
                self.deliver(program, host, Event::Mouse { kind, x, y, mods });
            }
        }
        if let Some((_, dy)) = self.window.get_scroll_wheel() {
            // A notch of the wheel (a line, on a Mac) is three rows.
            self.wheel += dy * 3.0;
            let rows = self.wheel.trunc();
            if rows != 0.0 {
                self.wheel -= rows;
                let (x, y) = self.pointer.unwrap_or((0, 0));
                self.deliver(program, host, Event::Wheel { x, y, rows: rows as i32 });
            }
        }
    }

    /// The window was closed, or Cmd Q was pressed. The program hears Close
    /// and puts everything on disk before its call returns; then the shell
    /// is done.
    fn close(&mut self, program: &mut dyn Program, host: &mut MiniHost) {
        program.event(Event::Close, host);
        self.wants.quit = true;
    }
}
