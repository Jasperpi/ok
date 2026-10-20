//! The line under a hand that presses anything: long runs of keys, text,
//! waits, wheel and window sizes chosen by a seeded generator. Nothing may
//! bring the program down, the text the session holds must always be the
//! text itself, and when the line is closed again nothing that was only
//! tried may be left behind.

mod line_support;

use line_support::*;
use ok_host::Event;
use ok_ui::line::settings;

/// A small generator, the same on every run.
struct Dice(u64);

impl Dice {
    fn roll(&mut self, sides: usize) -> usize {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % sides.max(1)
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.roll(from.len())]
    }
}

const KEYS: [&str; 34] = [
    "esc", "esc", "enter", "enter", "up", "down", "down", "left", "right", "home", "end", "pageup", "pagedown", "backspace", "backspace", "delete", "tab", "f1", "f3", "shift+f3", "ctrl+f", "ctrl+z", "ctrl+y", "ctrl+shift+z", "ctrl+v",
    "ctrl+backspace", "ctrl+=", "ctrl+-", "f11", "ctrl+s", "ctrl+enter", "ctrl+home", "ctrl+end", "shift+enter",
];

const WORDS: [&str; 48] = [
    "set", "set ", " ", "look ", "face ", "measure ", "scale ", "paper ", "half ", "cursor ", "limit ", "folder ", "window ", "default", "amber", "spleen", "underline", "framed", "off", "#", "/", "/", "g", "a", "the", "orch", "grand", "apricots",
    "rename ", "earlier", "discard", "copy", "keys", "notices", "[[", "]]", "\u{e9}", "\\", "7", "40", "fff", "000000", "D:\\Essays", "C:\\nowhere", "x", "New text", "\u{4e00}", "TK",
];

const SIZES: [(usize, usize); 6] = [(1920, 1080), (1280, 720), (960, 528), (640, 400), (300, 100), (0, 0)];

/// One run of the dice.
fn run(seed: u64, steps: usize) {
    let (mut rig, folder) = rig();
    folder.borrow_mut().versions = vec![(String::from("Today 13:40, kept"), String::from("The second draft.")), (String::from("Yesterday 22:10"), String::from("# First\n\nThe first draft of it."))];
    rig.host.clipboard = Some(String::from("pasted words\nand a second line"));
    let mut dice = Dice(seed);
    let mut trail: Vec<String> = Vec::new();
    for _ in 0..steps {
        let did = match dice.roll(20) {
            0..=8 => {
                let key = dice.pick(&KEYS);
                rig.press(key);
                format!("key {key}")
            }
            9..=15 => {
                let word = dice.pick(&WORDS);
                rig.type_text(word);
                format!("type {word:?}")
            }
            16 => {
                let ms = [1, 249, 250, 1200, 5000][dice.roll(5)];
                rig.wait(ms);
                format!("wait {ms}")
            }
            17 => {
                let rows = dice.roll(21) as i32 - 10;
                rig.wheel(rows);
                format!("wheel {rows}")
            }
            18 => {
                let (width, height) = SIZES[dice.roll(SIZES.len())];
                rig.size(width, height);
                format!("size {width} {height}")
            }
            _ => {
                rig.send(Event::Tick);
                String::from("tick")
            }
        };
        trail.push(did);
        let told = || format!("seed {seed} after:\n  {}", trail.join("\n  "));
        // It draws, and what it says is on the row it says it on: one
        // character to a cell, none past the measure.
        let measure = rig.app.grid().measure.max(0) as usize;
        let sentence = said(&mut rig);
        assert!(sentence.chars().count() <= measure.max(1), "{sentence}\n{}", told());
        // The cursor is in what is on the page.
        let page = rig.app.page();
        let cursor = page.cursor;
        assert_eq!((page.doc().clamp(cursor.head), page.doc().clamp(cursor.anchor)), (cursor.head, cursor.anchor), "{}", told());
        // The session holds the text itself, whatever is being shown.
        let state = folder.borrow();
        let own = rig.app.text().doc().to_text("\n");
        assert_eq!(state.text(state.open_name()), Some(own.as_str()), "{}", told());
        assert!(!state.names().is_empty());
    }
    // Close the line if it is open, and nothing tried is left behind: the
    // settings in force are the settings that were kept.
    rig.size(1920, 1080);
    let machine = rig.app.grid().machine;
    if rig.screen().cursor.is_some_and(|cursor| Some(cursor.row) == machine) {
        rig.press("esc");
    }
    assert!(!rig.app.showing(), "seed {seed}:\n  {}", trail.join("\n  "));
    assert_eq!(rig.screen().kept, None, "seed {seed}:\n  {}", trail.join("\n  "));
    let kept = folder.borrow().settings.clone();
    if let Some(kept) = kept {
        assert_eq!(settings::written(&rig.app), kept, "seed {seed}:\n  {}", trail.join("\n  "));
    } else {
        let (fresh, _) = line_support::rig();
        assert_eq!(settings::written(&rig.app), settings::written(&fresh.app), "seed {seed}:\n  {}", trail.join("\n  "));
    }
    // And the page takes the next letter.
    let before = rig.app.text().doc().len_bytes();
    if !rig.app.page().read_only {
        rig.type_text("z");
        assert_eq!(rig.app.text().doc().len_bytes(), before + 1, "seed {seed}:\n  {}", trail.join("\n  "));
    }
}

#[test]
fn nothing_typed_on_the_line_brings_it_down() {
    for seed in 1..=60 {
        run(seed, 140);
    }
}

#[test]
fn nor_does_a_long_sitting() {
    for seed in 1001..=1004 {
        run(seed, 1200);
    }
}
