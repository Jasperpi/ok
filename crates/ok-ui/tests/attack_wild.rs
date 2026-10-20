//! Nothing the shell can send brings the interface down: keys that do not
//! exist, characters from every corner of Unicode, a pointer and a wheel at
//! the ends of their ranges, a clock at the end of time, a frame that is
//! not the size the window said it was, events after the window has closed.
//! After each, the text is what the keys that can change it made it, and
//! the cursor is a place the text has.
//!
//! A run that panics prints the events that led to it.

mod attack_support;

use std::path::PathBuf;

use attack_support::{on_cluster_boundary, pasted, Rng};
use ok_gfx::Frame;
use ok_host::{Event, Key, Mods, MouseKind, Program};
use ok_ui::harness::Rig;

/// The last events sent, printed if the test goes down.
struct Trail {
    seed: u64,
    events: Vec<String>,
}

impl Trail {
    fn note(&mut self, what: String) {
        if self.events.len() >= 40 {
            self.events.remove(0);
        }
        self.events.push(what);
    }
}

impl Drop for Trail {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("seed {} went down after:\n  {}", self.seed, self.events.join("\n  "));
        }
    }
}

const WILD: [i32; 12] = [0, 1, -1, 7, 500, 1079, 1920, 5000, -5000, i32::MAX, i32::MIN, i32::MAX - 1];

/// Characters of every sort a shell could hand over as typed.
const ODD: [char; 30] = [
    '\u{0}', '\u{8}', '\t', '\n', '\r', '\u{1B}', '\u{7F}', '\u{85}', '\u{A0}', '\u{AD}', '\u{300}', '\u{34F}', '\u{200B}', '\u{200D}', '\u{202E}', '\u{2028}', '\u{2029}', '\u{2060}', '\u{D7FF}', '\u{E000}', '\u{FE0F}', '\u{FEFF}', '\u{FFFC}', '\u{FFFD}', '\u{FFFE}', '\u{FFFF}', '\u{1F469}', '\u{E0001}', '\u{E01EF}',
    '\u{10FFFF}',
];

fn any_key(rng: &mut Rng) -> Key {
    match rng.below(20) {
        0 => Key::F(rng.below(256) as u8),
        1 => Key::Char(rng.pick(&ODD)),
        2 => Key::Char(char::from_u32(rng.below(0x250) as u32).unwrap_or('a')),
        3 => Key::Char(rng.pick(&['Z', 'Y', 'X', 'C', 'V', 'A', 'Q', 'S', 'T', 'F', '+', '_'])),
        4 => Key::Char(rng.pick(&['z', 'y', 'x', 'c', 'v', 'a', 's', 't', 'f', '=', '-'])),
        n => [Key::Left, Key::Right, Key::Up, Key::Down, Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Enter, Key::Tab, Key::Backspace, Key::Delete, Key::Escape, Key::Left, Key::Enter][n - 5],
    }
}

fn any_mods(rng: &mut Rng) -> Mods {
    Mods { ctrl: rng.chance(3), alt: rng.chance(4), shift: rng.chance(3) }
}

fn storm(seed: u64, steps: usize, start: &str) {
    let mut rng = Rng::new(seed);
    let mut trail = Trail { seed, events: Vec::new() };
    let mut rig = Rig::with_text(start, 960, 528);
    let mut frame = Frame::new(0, 0);
    for _ in 0..steps {
        let before = rig.own_text();
        // With the cursor on the machine line a typed character goes there.
        let on_line = rig.app.line_open();
        let (head, anchor) = {
            let cursor = rig.app.page().cursor;
            let doc = rig.app.page().doc();
            (doc.offset_of(cursor.head), doc.offset_of(cursor.anchor))
        };
        let mut typed = None;
        let mut may_edit = false;
        match rng.below(24) {
            0..=4 => {
                let ch = if rng.chance(2) { rng.pick(&ODD) } else { char::from_u32(rng.below(0x3000) as u32).unwrap_or('x') };
                trail.note(format!("Text({ch:?})"));
                typed = Some(ch);
                rig.send(Event::Text(ch));
            }
            5..=10 => {
                let (key, mods, repeat) = (any_key(&mut rng), any_mods(&mut rng), rng.chance(2));
                trail.note(format!("Key {key:?} {mods:?} repeat {repeat}"));
                may_edit = true;
                rig.send(Event::Key { key, mods, repeat });
            }
            11 => {
                let mods = any_mods(&mut rng);
                trail.note(format!("Mods {mods:?}"));
                rig.send(Event::Mods(mods));
            }
            12 | 13 => {
                let kind = rng.pick(&[MouseKind::Down, MouseKind::Up, MouseKind::Move]);
                let (x, y) = (rng.pick(&WILD), rng.pick(&WILD));
                trail.note(format!("Mouse {kind:?} {x} {y}"));
                rig.send(Event::Mouse { kind, x, y, mods: any_mods(&mut rng) });
            }
            14 => {
                let (x, y, rows) = (rng.pick(&WILD), rng.pick(&WILD), rng.pick(&WILD));
                trail.note(format!("Wheel {x} {y} {rows}"));
                rig.send(Event::Wheel { x, y, rows });
            }
            15 => {
                let (width, height) = (rng.pick(&[0usize, 1, 11, 12, 13, 600, 960, 1920, 2560, 5000]), rng.pick(&[0usize, 1, 21, 22, 23, 44, 400, 528, 1080, 3000]));
                trail.note(format!("Resize {width} {height}"));
                rig.size(width, height);
            }
            16 => {
                // The shell draws into a frame that is not the size it last
                // announced: a resize is on its way.
                let (width, height) = (rng.pick(&[0usize, 1, 12, 300, 961, 1500]), rng.pick(&[0usize, 1, 22, 300, 529, 900]));
                trail.note(format!("render into {width} by {height}"));
                frame.resize(width, height);
                rig.app.render(&mut frame, &mut rig.host);
            }
            17 => {
                let to = rng.pick(&[0u64, 1, 15, 16, 499, 500, 1000, 60_000, u64::MAX / 2, u64::MAX - 1, u64::MAX]);
                trail.note(format!("the clock reads {to}"));
                rig.host.now = rig.host.now.max(to);
                rig.send(Event::Tick);
            }
            18 => {
                let (year, month, day, hour, minute) = (rng.pick(&[0, 1, 1601, 1970, 2026, 9999, 30000]), rng.below(14) as u8, rng.below(33) as u8, rng.below(26) as u8, rng.below(62) as u8);
                trail.note(format!("the date is {year}-{month}-{day} {hour}:{minute}"));
                rig.host.set_time(year, month, day, hour, minute);
                rig.send(Event::Tick);
            }
            19 => {
                let clip = rng.pick(&[None, Some(String::new()), Some(String::from("\u{0}")), Some("\r".repeat(50)), Some("a\tb\r\n".repeat(200)), Some(ODD.iter().collect::<String>()), Some("x".repeat(20_000))]);
                trail.note(format!("paste {:?}", clip.as_ref().map(|clip| clip.chars().take(12).collect::<String>())));
                rig.host.clipboard = clip.clone();
                let (from, to) = (head.min(anchor), head.max(anchor));
                rig.send(Event::Key { key: Key::Char('v'), mods: Mods::CTRL, repeat: false });
                let put = pasted(clip.as_deref().unwrap_or(""));
                let want = if put.is_empty() || on_line { before.clone() } else { format!("{}{}{}", &before[..from], put, &before[to..]) };
                assert_eq!(rig.own_text(), want, "seed {seed}: a paste");
            }
            20 => {
                let what = rng.below(6);
                trail.note(format!("setting {what}"));
                match what {
                    0 => rig.app.set_scale(rng.pick(&[0, 1, 2, 3, 4, u32::MAX])),
                    1 => rig.app.set_measure(rng.pick(&[0, 1, 39, 40, 66, 100, 101, usize::MAX])),
                    2 => rig.app.set_face(ok_gfx::spleen()),
                    3 => rig.app.set_face(ok_gfx::terminus()),
                    4 => rig.app.set_face(ok_gfx::gallant()),
                    _ => {
                        rig.app.set_look(rng.pick(&["manila", "sun", "acme", "register", "register night", "amber", "vt100", "5151", "wordperfect", "", "nothing of the kind"]));
                    }
                }
            }
            21 => {
                let focused = rng.chance(2);
                trail.note(format!("Focus {focused}"));
                rig.send(Event::Focus(focused));
            }
            22 => {
                let path = rng.pick(&["", ".", "C:\\", "\\\\?\\nowhere\\at all.md", "con", "a\u{0}b", "target", "no such file.md"]);
                trail.note(format!("Open {path:?}"));
                rig.send(Event::Open(PathBuf::from(path)));
            }
            _ => {
                trail.note(String::from("draw"));
                rig.draw();
            }
        }
        let now = rig.own_text();
        let cursor = rig.app.page().cursor;
        let doc = rig.app.page().doc();
        assert_eq!((doc.clamp(cursor.head), doc.clamp(cursor.anchor)), (cursor.head, cursor.anchor), "seed {seed}: the cursor left the text");
        assert!(doc.lines().iter().all(|line| !line.contains(['\n', '\r'])), "seed {seed}: a line holds a line break");
        if let Some(ch) = typed {
            // A typed character goes in exactly, or not at all if it is a
            // control character.
            let (from, to) = (head.min(anchor), head.max(anchor));
            let want = if ch.is_control() || on_line { before.clone() } else { format!("{}{}{}", &before[..from], ch, &before[to..]) };
            assert_eq!(now, want, "seed {seed}: typing {ch:?}");
        } else if !may_edit && trail.events.last().is_some_and(|last| !last.starts_with("paste")) {
            assert_eq!(now, before, "seed {seed}: the text changed and no key was pressed");
        }
        let _ = on_cluster_boundary(&now, doc.offset_of(cursor.head).min(now.len()));
    }
    // The window is closed, and the shell goes on sending for a moment.
    trail.note(String::from("Close, then more"));
    let last = rig.own_text();
    rig.send(Event::Close);
    rig.send(Event::Key { key: Key::Char('q'), mods: Mods::CTRL, repeat: false });
    assert!(rig.wants().quit, "seed {seed}: Ctrl Q did not ask to leave");
    rig.send(Event::Tick);
    rig.send(Event::Resize { width: 0, height: 0 });
    rig.send(Event::Focus(false));
    rig.send(Event::Close);
    rig.draw();
    assert_eq!(rig.own_text(), last, "seed {seed}: closing changed the text");
}

#[test]
fn nothing_the_shell_can_send_brings_it_down() {
    for seed in 1..=40 * attack_support::scale() {
        storm(seed, 500, "");
    }
}

#[test]
fn nothing_the_shell_can_send_brings_it_down_with_a_text_already_there() {
    let start = format!("# T\n\n- a\n- \n\n> q\n```\n\u{0301}\n```\n{}\n\n\t\u{4E00} TK [[x]] {}", "x".repeat(3000), "word ".repeat(400));
    for seed in 1..=16 * attack_support::scale() {
        storm(1000 + seed, 400, &start);
    }
}

/// A frame of every small size, drawn into without a resize having been
/// announced, and a window that announces a size and is then drawn at
/// another.
#[test]
fn drawing_into_a_frame_of_any_size_is_safe() {
    let mut rig = Rig::with_text("# Title\n\nSome words here, enough of them to wrap at a narrow measure and fill a few rows.\n\n- item", 1920, 1080);
    let mut frame = Frame::new(0, 0);
    for width in (0..=200).step_by(7) {
        for height in (0..=130).step_by(9) {
            frame.resize(width, height);
            rig.app.render(&mut frame, &mut rig.host);
            rig.send(Event::Text('a'));
            rig.send(Event::Resize { width: height * 3, height: width });
            rig.app.render(&mut frame, &mut rig.host);
            rig.send(Event::Key { key: Key::Backspace, mods: Mods::NONE, repeat: false });
        }
    }
    assert!(rig.own_text().starts_with("# Title\n\nSome words here"));
    rig.size(1920, 1080);
    assert!(rig.screen().cursor.is_some(), "back at a full screen the cursor is on the sheet");
    // The title is on the sheet with it, on whichever row the paper has it.
    let text = ok_ui::grid::Cols { first: 47, last: 112 };
    assert!((0..45).any(|row| rig.screen().big(text, row) == "Title"));
}
