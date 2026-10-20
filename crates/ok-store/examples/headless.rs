//! The store with no window: a helper the kill tests start and then stop
//! with TerminateProcess.
//!
//! ```text
//! headless run <folder> <data> <seed> <pace us> <save every ms> <rotate at>
//! headless recover <folder> <data>
//! ```
//!
//! `run` opens the store on the two folders, opens (or starts) `essay.md`,
//! and applies an endless stream of random edits to it. It prints `B <len>
//! <hash>` for the text it starts from, and after each edit, only once
//! `record` has returned for it, `E <edit number> <wall-clock ns>`. Saves
//! are asked for every few milliseconds. It never exits by itself.
//!
//! `recover` only opens the store, which runs recovery, and shuts it down.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ok_store::testing::{quick, Script};
use ok_store::{Setup, Store, StoreError};
use ok_text::Doc;

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    let (Some(mode), Some(folder), Some(data)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("usage: headless run|recover <folder> <data> [seed pace-us save-ms rotate-at]");
        std::process::exit(2);
    };
    let mut tuning = quick();
    tuning.rotate_at = number(6).max(1);
    let mut setup = Setup::new(PathBuf::from(folder), PathBuf::from(data));
    setup.tuning = tuning;
    let (mut store, _) = match Store::open(setup) {
        Ok(opened) => opened,
        Err(error) => {
            println!("X {error:?}");
            std::process::exit(3);
        }
    };
    if mode == "recover" {
        println!("R");
        store.shutdown(Duration::from_secs(5));
        return;
    }
    let opened = match store.open_text("essay.md") {
        Ok(opened) => opened,
        Err(StoreError::Missing) => match store.create_text("essay.md") {
            Ok(opened) => opened,
            Err(error) => {
                println!("X {error:?}");
                std::process::exit(4);
            }
        },
        Err(error) => {
            println!("X {error:?}");
            std::process::exit(4);
        }
    };
    let mut doc = Doc::from_text(&opened.text);
    let mut script = Script::new(number(3));
    let pace = Duration::from_micros(number(4));
    let save_every = Duration::from_millis(number(5).max(1));
    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = writeln!(out, "B {} {}", opened.text.len(), fnv(opened.text.as_bytes()));
    let mut last_save = Instant::now();
    loop {
        let change = script.step(&mut doc);
        let edit = store.record(opened.doc, &change, doc.len_bytes());
        // Acknowledged only now that record has returned.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let _ = writeln!(out, "E {edit} {now}");
        if last_save.elapsed() >= save_every {
            store.save(opened.doc, doc.to_text("\n"), edit);
            last_save = Instant::now();
        }
        while store.poll().is_some() {}
        let wait = Instant::now();
        while wait.elapsed() < pace {
            std::hint::spin_loop();
        }
    }
}
