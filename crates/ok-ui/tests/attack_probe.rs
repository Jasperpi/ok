//! Scratch probes; not kept.
use std::time::Instant;
use ok_host::{Event, Key, Mods};
use ok_ui::harness::Rig;

fn time<R>(what: &str, times: u32, mut act: impl FnMut() -> R) {
    let start = Instant::now();
    for _ in 0..times {
        act();
    }
    println!("{what}: {:?} each", start.elapsed() / times);
}

#[test]
fn probe_big_texts() {
    let para = "The orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm. ".repeat(3);
    for (name, text) in [
        ("2,000 paragraphs", format!("# Title\n\n{}", format!("{para}\n\n").repeat(2000))),
        ("60,000 short lines", "line\n".repeat(60_000)),
        ("one line of 300 KB", "word ".repeat(60_000)),
        ("one word of 300 KB", "x".repeat(300_000)),
    ] {
        println!("== {name}: {} bytes", text.len());
        let start = Instant::now();
        let mut rig = Rig::with_text(&text, 1920, 1080);
        println!("open: {:?}", start.elapsed());
        time("frame at end", 5, || { rig.draw(); });
        time("type at end", 20, || rig.send(Event::Text('a')));
        time("enter at end", 5, || rig.key(Key::Enter, Mods::NONE));
        time("backspace at end", 5, || rig.key(Key::Backspace, Mods::NONE));
        rig.key(Key::Home, Mods::CTRL);
        time("frame at start", 5, || { rig.draw(); });
        time("type at start", 20, || rig.send(Event::Text('a')));
        time("enter at start", 5, || rig.key(Key::Enter, Mods::NONE));
        time("undo", 5, || rig.key(Key::Char('z'), Mods::CTRL));
        time("down", 20, || rig.key(Key::Down, Mods::NONE));
        time("page down", 20, || rig.key(Key::PageDown, Mods::NONE));
        time("ctrl+end", 3, || rig.key(Key::End, Mods::CTRL));
        time("ctrl+a", 3, || rig.key(Key::Char('a'), Mods::CTRL));
        time("frame all selected", 3, || { rig.draw(); });
        time("ctrl+c", 2, || rig.key(Key::Char('c'), Mods::CTRL));
        time("alt+up", 3, || rig.key(Key::Up, Mods::ALT));
        time("ctrl+t", 3, || rig.key(Key::Char('t'), Mods::CTRL));
        time("resize", 3, || { rig.size(1000, 700); rig.size(1920, 1080); });
    }
}
