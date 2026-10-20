//! Nothing a keyboard, a mouse, a file or a window size can do may bring
//! the interface down. These tests throw everything at it, in a debug
//! build where an overflow is a panic, and check after every step that the
//! screen still draws and the cursor is still inside the text.

use std::path::PathBuf;

use ok_host::{Event, Key, Mods, MouseKind};
use ok_ui::harness::Rig;
use ok_ui::session::Opened;
use ok_ui::Options;

/// A fixed-seed generator, so a failure can be run again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() >> 16) as usize % n.max(1)
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }
}

const KEYS: [Key; 22] = [
    Key::Left,
    Key::Right,
    Key::Up,
    Key::Down,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::Enter,
    Key::Tab,
    Key::Backspace,
    Key::Delete,
    Key::Escape,
    Key::F(1),
    Key::F(3),
    Key::F(11),
    Key::Char('z'),
    Key::Char('y'),
    Key::Char('a'),
    Key::Char('x'),
    Key::Char('v'),
    Key::Char('t'),
];

const TYPED: [&str; 24] = [
    "a", "word ", " ", "# ", "## ", "> ", "- ", "1. ", "- [ ] ", "*", "**", "_", "`", "```", "[[", "]]", "TK", "\u{0301}", "\u{4E00}", "\u{1F600}", "e\u{0301}\u{0308}\u{0313}\u{0323}\u{0330}", "\u{200B}", "\u{202E}", "---",
];

const PASTED: [&str; 8] = ["\t", "a\tb\r\nc\rd\n\n", "\u{0}\u{1}\u{7f}\u{1b}[31m", "# H\n\n> q\n- i\n```\nTK\n```\n", "\u{FEFF}", "\u{FFFD}\u{FFFF}\u{10FFFF}", "[[a|b#c]] [x](http://y) https://z.example/", "- [x] \u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{200F}"];

/// The widths and heights a window is dragged through.
const SIZES: [(usize, usize); 14] = [(1920, 1080), (1280, 720), (960, 528), (640, 480), (333, 777), (180, 44), (100, 100), (13, 23), (12, 22), (1, 1), (0, 0), (0, 600), (800, 0), (3840, 2160)];

fn one_event(rng: &mut Rng, rig: &mut Rig) {
    match rng.below(40) {
        0..=17 => {
            for ch in rng.pick(&TYPED).chars() {
                rig.send(Event::Text(ch));
            }
        }
        18..=29 => {
            let mods = Mods { ctrl: rng.below(4) == 0, alt: rng.below(5) == 0, shift: rng.below(3) == 0 };
            rig.send(Event::Key { key: rng.pick(&KEYS), mods, repeat: rng.below(2) == 0 });
        }
        30 => {
            rig.host.clipboard = Some(rng.pick(&PASTED).repeat(1 + rng.below(3)));
            rig.press("ctrl+v");
        }
        31 | 32 => {
            let wild = |rng: &mut Rng| [0, 1, 500, 1000, 5000, -1, -500, i32::MAX, i32::MIN][rng.below(9)].saturating_add(rng.below(3) as i32 - 1);
            let kind = rng.pick(&[MouseKind::Down, MouseKind::Up, MouseKind::Move]);
            let (x, y) = (wild(rng), wild(rng));
            rig.send(Event::Mouse { kind, x, y, mods: Mods { ctrl: false, alt: false, shift: rng.below(2) == 0 } });
        }
        33 => rig.wheel([1, -1, 3, -3, 1000, -1000, i32::MAX, i32::MIN][rng.below(8)]),
        34 => {
            let (width, height) = rng.pick(&SIZES);
            rig.size(width, height);
        }
        35 => rig.send(Event::Mods(Mods { ctrl: rng.below(2) == 0, alt: rng.below(2) == 0, shift: rng.below(2) == 0 })),
        36 => rig.send(Event::Focus(rng.below(2) == 0)),
        37 => rig.wait([0, 1, 16, 500, 5000][rng.below(5)]),
        38 => {
            let look = ["manila", "sun", "amber", "wordperfect", "5151"][rng.below(5)];
            rig.app.set_look(look);
            rig.app.set_scale(1 + rng.below(3) as u32);
            rig.app.set_measure(30 + rng.below(90));
        }
        _ => rig.send(Event::Tick),
    }
}

fn storm(seed: u64, steps: usize, start: &str) {
    let mut rng = Rng(seed | 1);
    let mut rig = Rig::with_text(start, 1920, 1080);
    for step in 0..steps {
        one_event(&mut rng, &mut rig);
        // The cursor and the selection are always places the text has.
        let cursor = rig.app.page().cursor;
        let doc = rig.app.page().doc();
        assert_eq!((doc.clamp(cursor.head), doc.clamp(cursor.anchor)), (cursor.head, cursor.anchor), "seed {seed} step {step}");
        // The layout is the one a fresh page would have for this text.
        if step % 64 == 0 {
            // A text read afresh loses one byte-order mark from its head, so
            // one is put there to be lost.
            let fresh = ok_ui::page::Page::new(&format!("\u{FEFF}{}", rig.text()), rig.app.page().measure());
            let page = rig.app.page();
            for line in 0..page.doc().line_count() {
                let (kept, made) = ((page.kind(line), page.layout().shape(line), page.layout().rows(line)), (fresh.kind(line), fresh.layout().shape(line), fresh.layout().rows(line)));
                assert_eq!(kept, made, "seed {seed} step {step}: line {line} {:?} has drifted from the text", page.doc().line(line));
            }
            assert!(fresh.layout() == page.layout(), "seed {seed} step {step}: the rows have drifted from the text");
            assert_eq!(fresh.words(), rig.app.page().words(), "seed {seed} step {step}");
        }
        if step % 8 == 0 {
            rig.draw();
            let (screen, grid) = (rig.app.screen(), rig.app.grid());
            assert_eq!((screen.cols(), screen.rows()), (grid.cols, grid.rows));
        }
    }
    rig.size(1920, 1080);
    rig.draw();
}

/// How many times over to run the storms: `OK_ADVERSARIAL_SCALE=20 cargo
/// test -p ok-ui --offline --test hostile` for a longer campaign.
fn scale() -> u64 {
    std::env::var("OK_ADVERSARIAL_SCALE").ok().and_then(|scale| scale.parse().ok()).unwrap_or(1).max(1)
}

#[test]
fn a_storm_of_keys_mice_pastes_and_resizes_from_an_empty_text() {
    for seed in 1..=6 * scale() {
        storm(seed * 7919, 1500, "");
    }
}

#[test]
fn a_storm_over_a_text_that_is_already_hostile() {
    let start = format!(
        "# {}\n\n> {}\n- {}\n```\n{}\n\n\u{0301}\u{0301}\n{}\n\t\t\u{4E00}\u{4E00}\n{}",
        "Heading ".repeat(40),
        "quoted ".repeat(60),
        "x".repeat(700),
        "TK ".repeat(50),
        " ".repeat(5000),
        "word\u{0301} ".repeat(300)
    );
    for seed in 1..=3 * scale() {
        storm(seed * 104_729, 1200, &start);
    }
}

#[test]
fn files_that_are_not_text_open_without_complaint() {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/harness").join(format!("hostile-{}", std::process::id()));
    std::fs::create_dir_all(folder.join("deep/deeper")).unwrap();
    let files: [(&str, Vec<u8>); 7] = [
        ("empty.md", Vec::new()),
        ("not utf8.md", vec![0xFF, 0xFE, b'a', 0x80, b'\n', 0xC3, 0x28, 0, 0, 0xED, 0xA0, 0x80]),
        ("bom and crlf.txt", b"\xEF\xBB\xBF# Title\r\n\r\nBody\rmore".to_vec()),
        ("one long line.md", "word ".repeat(40_000).into_bytes()),
        ("only marks.md", "\u{0301}".repeat(500).into_bytes()),
        ("deep/deeper/links.markdown", b"[[empty]] [[nowhere]] [[../empty]] [[]] [[[[x]]".to_vec()),
        (".hidden.md", b"not listed".to_vec()),
    ];
    for (name, bytes) in &files {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    for (name, _) in &files {
        let mut rig = Rig::new(Options { file: Some(folder.join(name)), folder: Some(folder.clone()), look: None, store: false });
        rig.size(1920, 1080);
        rig.draw();
        rig.type_text("x\n");
        for chord in ["ctrl+home", "shift+end", "ctrl+end", "pageup", "ctrl+a", "ctrl+c", "ctrl+z"] {
            rig.press(chord);
        }
        rig.draw();
        // A name starting with a dot is never listed in the contents.
        let contents = rig.app.grid().contents.unwrap();
        assert!((0..49).all(|row| !rig.screen().span(contents, row).contains(".hidden")), "{name}");
        assert!((0..49).any(|row| rig.screen().span(contents, row).contains("only marks")), "{name}");
    }
    // A file that is not there, a folder that is not there, a folder that is a file.
    for (file, folder) in [("no such.md", "no such folder"), ("empty.md", "empty.md"), ("deep", "deep")] {
        let mut rig = Rig::new(Options { file: Some(folder_path(&folder_of(folder), file)), folder: Some(folder_of(folder)), look: Some(String::from("no such look")), store: false });
        rig.size(800, 600);
        rig.draw();
        assert_eq!(rig.app.look().name, "manila");
    }
    std::fs::remove_dir_all(&folder).unwrap();

    fn folder_of(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/harness").join(format!("hostile-{}", std::process::id())).join(name)
    }
    fn folder_path(folder: &std::path::Path, file: &str) -> PathBuf {
        folder.join(file)
    }
}

#[test]
fn a_megabyte_on_one_row_is_drawn_no_slower_than_a_row_of_words() {
    let mut rig = Rig::sized(1920, 1080);
    rig.app.open_text(Opened { text: format!("word{}\n\n{}", " ".repeat(1 << 20), "x".repeat(1 << 16)), ..Opened::default() });
    let start = std::time::Instant::now();
    for _ in 0..5 {
        rig.draw();
    }
    assert!(start.elapsed().as_secs() < 5, "five frames took {:?}", start.elapsed());
    rig.press("ctrl+home");
    rig.press("end");
    rig.type_text("y");
    rig.draw();
    assert!(rig.screen().cursor.is_some());
}
