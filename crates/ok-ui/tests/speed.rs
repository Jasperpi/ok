//! Speed is the absence of delay: a key press changes the text and paints
//! in the same message. This measures both halves on a 20,000-word text and
//! prints what it finds:
//!
//! ```text
//! cargo test -p ok-ui --offline --release --test speed -- --nocapture
//! ```
//!
//! The targets are under 1 ms to handle one typed character and under 4 ms
//! to draw one frame at 1920 by 1080. They are held to in a release build;
//! a debug build only prints.

use std::time::{Duration, Instant};

use ok_host::Event;
use ok_text::Pos;
use ok_ui::harness::Rig;

/// Two hundred paragraphs of a hundred words, a heading every tenth, with
/// some of every kind of markup so no painter has an easy time of it.
fn twenty_thousand_words() -> String {
    let words = ["the", "orchard", "was", "eleven", "acres", "when", "my", "grandfather", "bought", "it", "and", "thinning", "*waste*", "**harder**", "[[interview]]", "TK"];
    let mut text = String::new();
    let mut n = 0usize;
    for paragraph in 0..200 {
        if paragraph % 10 == 0 {
            text.push_str(&format!("## Part {}\n\n", paragraph / 10 + 1));
        }
        for word in 0..100 {
            n = n.wrapping_mul(31).wrapping_add(7);
            text.push_str(words[if word % 17 == 16 { 12 + n % 4 } else { n % 12 }]);
            text.push(if word == 99 { '.' } else { ' ' });
        }
        text.push_str("\n\n");
    }
    text
}

fn average(times: u32, mut act: impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..times {
        act();
    }
    start.elapsed() / times
}

#[test]
fn a_twenty_thousand_word_text_stays_fast() {
    let text = twenty_thousand_words();
    let mut r = Rig::with_text(&text, 1920, 1080);
    let lines = r.app.page().doc().line_count();
    assert!(r.app.page().words() >= 20_000, "{} words", r.app.page().words());
    r.draw();

    // One typed character, handled as the shell would hand it over: the
    // event, and the question of what the program wants next. At the end
    // of the text, in the middle of a paragraph in the middle of it, and at
    // its very start, where every line below has to make room.
    let mut slowest_key = Duration::ZERO;
    for (place, at) in [("at the end", r.app.page().doc().end()), ("in the middle", Pos::new(lines / 2, 300)), ("at the start", Pos::new(2, 0))] {
        r.app.page_mut().place(at, false);
        let per_key = average(400, || {
            for ch in ['w', 'o', 'r', 'd', ' '] {
                r.send(Event::Text(ch));
            }
        }) / 5;
        println!("one typed character {place}: {per_key:?}");
        slowest_key = slowest_key.max(per_key);
    }
    // Enter and Backspace, which change the number of lines.
    let per_break = average(200, || {
        r.press("enter");
        r.press("backspace");
    }) / 2;
    println!("one Enter or Backspace at the start: {per_break:?}");

    // One frame at 1920 by 1080: the sheet full of text, the contents
    // empty, the map at its smallest and scrolling.
    r.app.page_mut().place(Pos::new(lines / 2, 300), false);
    r.app.go(Pos::new(lines / 2, 300));
    let per_frame = average(60, || {
        r.draw();
    });
    println!("one frame at 1920 by 1080: {per_frame:?}");
    // And with the paper rolling, which draws the rows it uncovers too.
    r.press("ctrl+end");
    r.press("shift+enter");
    assert!(r.wants().animating);
    let rolling = average(20, || {
        r.draw();
    });
    println!("one frame while the paper rolls: {rolling:?}");

    if cfg!(debug_assertions) {
        println!("a debug build: nothing is held to the targets");
    } else {
        assert!(slowest_key < Duration::from_millis(1), "a typed character took {slowest_key:?}");
        assert!(per_break < Duration::from_millis(1), "Enter took {per_break:?}");
        assert!(per_frame < Duration::from_millis(4), "a frame took {per_frame:?}");
        assert!(rolling < Duration::from_millis(4), "a rolling frame took {rolling:?}");
    }
}
