//! Where the window was, kept between runs in `window.txt`.
//!
//! The file is the shell's own and is written the same way the settings are:
//! one `name value` per line. What it holds is the rectangle of the window as
//! an ordinary window and whether it was maximized. Full screen is not in it,
//! because that is the program's setting, not the window's.

use std::path::Path;

const FILE: &str = "window.txt";

/// Coordinates beyond this are not a real screen position.
const FAR: i32 = 100_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Placement {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub maximized: bool,
}

impl Placement {
    fn is_sane(&self) -> bool {
        let near = |v: i32| (-FAR..=FAR).contains(&v);
        near(self.left) && near(self.top) && near(self.right) && near(self.bottom) && self.right > self.left && self.bottom > self.top
    }
}

pub(crate) fn parse(text: &str) -> Option<Placement> {
    let (mut left, mut top, mut right, mut bottom, mut maximized) = (None, None, None, None, false);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(name), Some(value)) = (words.next(), words.next()) else { continue };
        match name {
            "left" => left = value.parse().ok(),
            "top" => top = value.parse().ok(),
            "right" => right = value.parse().ok(),
            "bottom" => bottom = value.parse().ok(),
            "maximized" => maximized = value == "yes",
            _ => {}
        }
    }
    let p = Placement { left: left?, top: top?, right: right?, bottom: bottom?, maximized };
    p.is_sane().then_some(p)
}

pub(crate) fn format(p: &Placement) -> String {
    format!(
        "left {}\r\ntop {}\r\nright {}\r\nbottom {}\r\nmaximized {}\r\n",
        p.left,
        p.top,
        p.right,
        p.bottom,
        if p.maximized { "yes" } else { "no" }
    )
}

pub(crate) fn load(dir: &Path) -> Option<Placement> {
    parse(&std::fs::read_to_string(dir.join(FILE)).ok()?)
}

/// Writes the file whole beside the old one and swaps it in, so a crash
/// mid-write leaves the old placement rather than half of a new one. A
/// failure is ignored: losing the window's position costs nothing that
/// matters.
pub(crate) fn save(dir: &Path, p: &Placement) {
    if !p.is_sane() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let fresh = dir.join("window.txt.new");
    if std::fs::write(&fresh, format(p)).is_ok() && std::fs::rename(&fresh, dir.join(FILE)).is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Placement = Placement { left: -8, top: 40, right: 1000, bottom: 700, maximized: true };

    #[test]
    fn what_is_written_reads_back() {
        assert_eq!(parse(&format(&P)), Some(P));
        let plain = Placement { maximized: false, ..P };
        assert_eq!(parse(&format(&plain)), Some(plain));
    }

    #[test]
    fn nonsense_is_refused() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("left 0\ntop 0\nright 10\n"), None, "a missing edge");
        assert_eq!(parse("left 10\ntop 0\nright 10\nbottom 5\n"), None, "no width");
        assert_eq!(parse("left 0\ntop 0\nright 999999999\nbottom 5\n"), None, "off any screen");
        assert_eq!(parse("left x\ntop 0\nright 10\nbottom 5\n"), None);
        assert_eq!(parse("left 0\ntop 0\nright 99999999999999999999\nbottom 5\n"), None);
    }

    #[test]
    fn unknown_lines_and_odd_spacing_are_tolerated() {
        let text = "  left   1 \r\ncolour blue\r\n\r\ntop 2\r\nright 30\r\nbottom 40\r\nmaximized yes\r\nleft\r\n";
        assert_eq!(parse(text), Some(Placement { left: 1, top: 2, right: 30, bottom: 40, maximized: true }));
    }

    #[test]
    fn saves_and_loads_from_a_folder_that_did_not_exist() {
        let dir = crate::scratch("placement").join("deeper");
        assert_eq!(load(&dir), None);
        save(&dir, &P);
        assert_eq!(load(&dir), Some(P));
        let moved = Placement { left: 5, ..P };
        save(&dir, &moved);
        assert_eq!(load(&dir), Some(moved));
        assert!(!dir.join("window.txt.new").exists());
    }
}
