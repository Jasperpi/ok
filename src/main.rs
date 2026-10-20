//! ok: the program. Everything it does lives in the crates; this file only
//! starts the window with the interface in it: the Win32 shell on Windows,
//! the minifb one on macOS and Linux.
#![windows_subsystem = "windows"]

#[cfg(windows)]
use std::path::PathBuf;

use ok_gfx::{gallant, FaceSet, Frame, Pen, Rect, Rgb};
use ok_ui::{App, Options};
#[cfg(windows)]
use ok_win::{Icon, SelfTest, Step};

/// The icon at `size` pixels square: the word on its sheet, as the machine
/// prints it. It is drawn at 32 pixels; smaller sizes are that picture
/// averaged down, larger ones the same pixels made bigger.
fn icon_frame(size: u32) -> Frame {
    const PAPER: Rgb = Rgb(0xEE, 0xE3, 0xC8);
    const INK: Rgb = Rgb(0x22, 0x1F, 0x1B);
    let faces = FaceSet::new(gallant());
    let (cell_w, cell_h) = faces.cell();
    let mut frame = Frame::new(32, 32);
    frame.clear(PAPER);
    let (x, y) = ((32 - 2 * cell_w as i32) / 2, (32 - cell_h as i32) / 2);
    faces.draw_text(&mut frame, "ok", x, y, Pen::new(INK));
    let size = size.max(1) as i32;
    let mut out = Frame::new(size as usize, size as usize);
    if size >= 32 {
        for sy in 0..size {
            for sx in 0..size {
                let colour = frame.get(sx * 32 / size, sy * 32 / size).unwrap_or(PAPER);
                out.fill(Rect::new(sx, sy, 1, 1), colour);
            }
        }
        return out;
    }
    let step = (32 / size).max(1);
    for sy in 0..size {
        for sx in 0..size {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..step {
                for dx in 0..step {
                    if let Some(Rgb(pr, pg, pb)) = frame.get(sx * step + dx, sy * step + dy) {
                        r += pr as u32;
                        g += pg as u32;
                        b += pb as u32;
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.fill(Rect::new(sx, sy, 1, 1), Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8));
        }
    }
    out
}

/// The icon as Windows takes it.
#[cfg(windows)]
fn icon(size: u32) -> Icon {
    let frame = icon_frame(size);
    Icon { width: size, height: size, argb: frame.pixels().iter().map(|px| 0xFF00_0000 | px).collect() }
}

/// `ok --icons dir`: the icon as the PNG files of a macOS `.iconset`, for
/// `iconutil` to make the app's `.icns` from.
#[cfg(not(windows))]
fn write_icons(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for size in [16u32, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let frame = icon_frame(size * scale);
            let png = ok_gfx::encode_png(frame.width(), frame.height(), frame.pixels());
            std::fs::write(dir.join(format!("icon_{size}x{size}{suffix}.png")), png)?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice() {
        if flag == "--icons" {
            std::process::exit(if write_icons(std::path::Path::new(dir)).is_ok() { 0 } else { 1 });
        }
    }
    let file = std::env::args().nth(1).filter(|arg| !arg.starts_with('-')).and_then(|arg| std::path::absolute(arg).ok());
    let window = ok_mini::Options { open: file.clone(), ..ok_mini::Options::default() };
    let code = ok_mini::run(window, move |host| Box::new(App::new(host, Options { file, store: true, ..Options::default() })));
    std::process::exit(code);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let icons = vec![icon(32), icon(16)];

    // ok --stamp out.exe: a copy of this program carrying its manifest and
    // icon as resources, for Explorer and the taskbar.
    if let [flag, out] = args.as_slice() {
        if flag == "--stamp" {
            let done = std::env::current_exe().and_then(|exe| ok_win::stamp(&exe, &PathBuf::from(out), &icons, ok_win::MANIFEST));
            std::process::exit(if done.is_ok() { 0 } else { 1 });
        }
    }

    // ok --selftest dir out.png [look]: the real window procedure and
    // message loop in a window nobody sees, typing a few words into a
    // writing folder made under `dir`, then the frame written as a picture.
    if let [flag, dir, png, rest @ ..] = args.as_slice() {
        if flag == "--selftest" {
            let script = vec![
                Step::Client(1920, 1080),
                Step::Type("The first words typed into ok.".to_string()),
                Step::Wait(3200),
                Step::Shot(PathBuf::from(png)),
                // Then to the line, to try a look: nothing is kept.
                Step::Press(ok_host::Key::Escape),
                Step::Type("set look amber".to_string()),
                Step::Wait(400),
                Step::Shot(PathBuf::from(png).with_extension("line.png")),
                Step::Press(ok_host::Key::Escape),
                Step::Press(ok_host::Key::Escape),
                Step::Close,
            ];
            let look = rest.first().cloned();
            let report = ok_win::selftest(ok_win::Options::default(), SelfTest::new(dir, script), move |host| {
                Box::new(App::new(host, Options { store: true, look, ..Options::default() }))
            });
            std::process::exit(if report.crashed.is_some() || report.timed_out { 1 } else { 0 });
        }
    }

    let file = args.first().filter(|arg| !arg.starts_with("--")).and_then(|arg| std::path::absolute(arg).ok());
    let window = ok_win::Options { icons, open: file.clone(), ..ok_win::Options::default() };
    let code = ok_win::run(window, move |host| Box::new(App::new(host, Options { file, store: true, ..Options::default() })));
    std::process::exit(code);
}
