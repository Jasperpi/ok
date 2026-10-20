//! Review: does a window give back everything it took?
//!
//! Windows allows a process ten thousand drawing objects and ten thousand
//! window-manager objects. A window that kept one per frame would stop
//! being able to draw after a few minutes of animation. This counts the
//! process's objects (`GetGuiResources`) and kernel handles around runs
//! that do a little of everything: frames, the ground filled in beside a
//! frame smaller than the window, faces through both drawing routes, icons
//! from pixels, full screen and back, focus and the hidden caret.
//!
//! One test, alone in its file, so that nothing else in the process makes
//! or frees objects while it counts.

#[path = "review_ffi.rs"]
mod ffi;

use std::path::PathBuf;

use ffi::*;
use ok_gfx::{draw_alpha_glyph, Frame, Rect, Rgb};
use ok_host::{Event, Host, Key, Mods, Program, Wants};
use ok_win::{Icon, Options, SelfTest, Step};

#[link(name = "user32")]
unsafe extern "system" {
    fn GetGuiResources(process: isize, which: u32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcessHandleCount(process: isize, count: *mut u32) -> i32;
}

/// Drawing objects, window-manager objects and kernel handles this process holds now.
fn held() -> (u32, u32, u32) {
    let mut handles = 0u32;
    unsafe {
        let process = GetCurrentProcess();
        GetProcessHandleCount(process, &mut handles);
        (GetGuiResources(process, 0), GetGuiResources(process, 1), handles)
    }
}

/// Uses a bit of everything the shell hands out, and animates throughout.
struct Everything {
    frames: u32,
    fullscreen: bool,
    typed: usize,
    dark: bool,
}

impl Program for Everything {
    fn event(&mut self, event: Event, host: &mut dyn Host) {
        match event {
            Event::Key { key: Key::F(11), .. } => self.fullscreen = !self.fullscreen,
            Event::Key { key: Key::F(2), .. } => self.dark = !self.dark,
            Event::Key { key: Key::Char('c'), mods, .. } if mods.ctrl => host.clipboard_set("held by the stand-in"),
            Event::Text(_) => self.typed += 1,
            Event::Tick => self.frames += 1,
            _ => {}
        }
    }
    fn wants(&mut self, _: &dyn Host) -> Wants {
        Wants {
            redraw: true,
            animating: true,
            fullscreen: self.fullscreen,
            caret: Some(Rect::new(48 + 12 * self.typed as i32, 44, 12, 22 + (self.typed as i32 % 2))),
            title: format!("review, {} typed", self.typed),
            ground: if self.dark { Rgb(0x10, 0x19, 0x13) } else { Rgb(0xEE, 0xE3, 0xC8) },
            ..Wants::default()
        }
    }
    fn render(&mut self, frame: &mut Frame, host: &mut dyn Host) {
        frame.clear(Rgb(0xEE, 0xE3, 0xC8));
        let ink = Rgb(0x22, 0x1F, 0x1B);
        let mut x = 12;
        // An outline face, a raster face, a character above U+FFFF, one no
        // face has, a face that does not exist, and a size not seen before.
        let asked = [("Consolas", 22, 'g'), ("Terminal", 12, 'g'), ("SimSun-ExtB", 22, '\u{20000}'), ("Consolas", 22, '\u{2713}'), ("No Such Face", 22, 'g')];
        for (family, height, ch) in asked {
            if let Some(glyph) = host.rasterize(family, height, ch) {
                draw_alpha_glyph(frame, &glyph, x, 12, ink, 255);
            }
            x += 30;
        }
        let _ = host.face_cell("Courier New", 14 + self.frames % 6);
    }
}

fn icon(side: u32) -> Icon {
    Icon { width: side, height: side, argb: vec![0xFF35_251B; (side * side) as usize] }
}

fn one_run(name: &str) {
    let script = vec![
        Step::Focus(true),
        Step::Type(String::from("abc")),
        Step::Chord(Key::Char('c'), Mods::CTRL),
        Step::MouseDown(40, 40),
        Step::MouseUp(44, 48),
        Step::Wheel { x: 10, y: 10, delta: 120 },
        // A frame smaller than the window: the rest is filled with a brush.
        Step::Size(300, 200),
        Step::Wait(120),
        Step::Size(960, 528),
        Step::Client(720, 440),
        Step::Press(Key::F(2)),
        Step::Press(Key::F(11)),
        Step::Wait(120),
        Step::Press(Key::F(11)),
        Step::Moving(true),
        Step::Wait(80),
        Step::Moving(false),
        Step::Open(PathBuf::from("C:\\notes\\essay.md")),
        Step::EndSession(false),
        Step::Focus(false),
        Step::Wait(120),
    ];
    let options = Options { icons: vec![icon(16), icon(32)], ..options(name) };
    let report = ok_win::selftest(options, SelfTest::new(folder(name), script), |_| {
        Box::new(Everything { frames: 0, fullscreen: false, typed: 0, dark: false })
    });
    assert!(report.cloaked && !report.was_foreground && !report.timed_out && report.has_icons);
    assert_eq!(report.crashed, None);
    assert!(report.renders > 20, "the run drew {} frames", report.renders);
}

#[test]
fn a_window_gives_back_every_object_it_took() {
    // The first runs load what stays loaded for the life of the process.
    one_run("leaks-warm-1");
    one_run("leaks-warm-2");
    let before = held();
    const RUNS: u32 = 12;
    for _ in 0..RUNS {
        one_run("leaks");
    }
    let after = held();
    println!("drawing objects, window objects, kernel handles: {before:?} before {RUNS} runs, {after:?} after");
    assert_eq!(after.0, before.0, "drawing objects were kept: {} to {} over {RUNS} runs", before.0, after.0);
    assert_eq!(after.1, before.1, "window-manager objects were kept: {} to {} over {RUNS} runs", before.1, after.1);
    assert!(after.2 <= before.2 + 4, "kernel handles were kept: {} to {} over {RUNS} runs", before.2, after.2);
}
