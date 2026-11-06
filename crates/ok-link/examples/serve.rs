//! Serves the phone page with a made-up text, for looking at the page in a
//! browser: prints the address, then every op a phone sends.
//! `cargo run -p ok-link --example serve -- [look-paper look-ink desk desk-ink]`
use std::sync::Arc;
use std::time::Duration;

use ok_link::{Link, Status};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let look: [String; 4] = if args.len() == 4 { [args[0].clone(), args[1].clone(), args[2].clone(), args[3].clone()] } else { ["EEE3C8".into(), "221F1B".into(), "35251A".into(), "E6D9BA".into()] };
    let dir = std::env::temp_dir().join("ok-link-serve");
    let icon = ok_gfx::icon(192);
    let png = ok_gfx::encode_png(icon.width(), icon.height(), icon.pixels());
    let relay = match (std::env::var("OK_RELAY_PAGE"), std::env::var("OK_RELAY_DESK")) {
        (Ok(page), Ok(desk)) => Some(ok_link::Relay { page, desk }),
        _ => None,
    };
    let link = Link::start(&dir, relay, png, Arc::new(|| {}));
    println!("{}", link.direct_url().unwrap_or("no address"));
    println!("{}", link.relay_url().unwrap_or_default());
    let mut before = String::from("# The orchard\n\nThe orchard was eleven acres when my grandfather bought it.\n\nYou take off two of every three apricots, TK how many that is. The apricots that stay have room.\n\n");
    loop {
        for op in link.take() {
            println!("{op:?}");
            match op {
                ok_link::Op::Text(t) => before.push_str(&t),
                ok_link::Op::Back(n) => {
                    for _ in 0..n {
                        before.pop();
                    }
                }
                ok_link::Op::Key(ok_link::Key::Enter) => before.push_str("\n\n"),
                _ => {}
            }
        }
        let words = before.split_whitespace().count();
        link.publish(&Status { name: "The orchard".into(), words, today: 48, saved: true, before: before.clone(), after: String::new(), look: look.clone() });
        std::thread::sleep(Duration::from_millis(30));
    }
}
