//! A stand-in for the interface that keeps one very small text and saves it
//! constantly. A journal is started afresh once it has grown past four times
//! the text, so with a text of a few dozen bytes that happens at nearly every
//! save: this helper spends much of its life inside a journal rotation, and a
//! kill test that stops it at a random instant lands there often.
//!
//! ```text
//! attack_kill_rotor <folder> <data> <pace us> <save every n edits> <cap bytes> <flags>
//! ```
//!
//! The text is `rotor.md`. Edit `n` types `n` and a space at the end of the
//! text (a line break first, every eleventh), unless the text has grown past
//! `cap` bytes: then the edit cuts away its first half. The test that started
//! this process makes the same edits and so knows the text after each one.
//!
//! ```text
//! B <bytes> <fnv>          the text it starts from
//! E <n> <wall-clock ns>    edit n has been recorded
//! W <anything>             something worth counting that is not wrong
//! X <anything>             something that must never happen
//! ```
//!
//! `flags` adds up: 1 the save path of volumes without the one-step rename,
//! 2 a caret note with every edit, 4 every fifth save also keeps a version.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ok_store::testing::quick;
use ok_store::{Change, News, Pos, Setup, Store, StoreError};
use ok_text::{Doc, Span};

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// Edit `n` of the script. The test has the same function.
fn step(doc: &mut Doc, n: u64, cap: usize) -> Change {
    if doc.len_bytes() > cap {
        let half = doc.pos_at(doc.len_bytes() / 2);
        doc.replace(Span::new(Pos::default(), half), "")
    } else if n.is_multiple_of(11) {
        doc.replace(Span::at(doc.end()), &format!("\n{n} "))
    } else {
        doc.replace(Span::at(doc.end()), &format!("{n} "))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    let (Some(folder), Some(data)) = (args.first(), args.get(1)) else {
        eprintln!("usage: attack_kill_rotor <folder> <data> <pace us> <save every n edits> <cap bytes> <flags>");
        std::process::exit(2);
    };
    let pace = Duration::from_micros(number(2));
    let save_every = number(3).max(1);
    let cap = number(4).max(16) as usize;
    let flags = number(5);
    let mut tuning = quick();
    tuning.rotate_at = 1;
    if flags & 1 != 0 {
        tuning.faults.no_one_step.store(1, Ordering::Relaxed);
    }
    let mut setup = Setup::new(PathBuf::from(folder), PathBuf::from(data));
    setup.tuning = tuning;
    let out = std::io::stdout();
    let mut out = out.lock();
    let (mut store, _) = match Store::open(setup) {
        Ok(opened) => opened,
        Err(error) => {
            let _ = writeln!(out, "X the store did not open: {error:?}");
            std::process::exit(3);
        }
    };
    let opened = match store.open_text("rotor.md") {
        Ok(opened) => opened,
        Err(StoreError::Missing) => match store.create_text("rotor.md") {
            Ok(opened) => opened,
            Err(error) => {
                let _ = writeln!(out, "X the text could not be started: {error:?}");
                std::process::exit(4);
            }
        },
        Err(error) => {
            let _ = writeln!(out, "X the text could not be opened: {error:?}");
            std::process::exit(4);
        }
    };
    let mut doc = Doc::from_text(&opened.text);
    let _ = writeln!(out, "B {} {}", opened.text.len(), fnv(opened.text.as_bytes()));
    let mut saves = 0u64;
    for n in 1u64.. {
        let change = step(&mut doc, n, cap);
        let edit = store.record(opened.doc, &change, doc.len_bytes());
        // Acknowledged only now that record has returned.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let _ = writeln!(out, "E {n} {now}");
        if edit != n {
            let _ = writeln!(out, "X edit {n} was numbered {edit}");
        }
        if flags & 2 != 0 {
            store.caret(opened.doc, doc.end());
        }
        if n.is_multiple_of(save_every) {
            saves += 1;
            if flags & 4 != 0 && saves.is_multiple_of(5) {
                store.keep(opened.doc, doc.to_text("\n"), edit, "kept");
            } else {
                store.save(opened.doc, doc.to_text("\n"), edit);
            }
        }
        while let Some(news) = store.poll() {
            match news {
                News::Saved { .. } | News::Kept { .. } | News::Tree(_) | News::FileText { .. } | News::Released { .. } => {}
                News::Held { why, .. } => {
                    let _ = writeln!(out, "W held {why:?}");
                }
                News::JournalAlarm(up) => {
                    let _ = writeln!(out, "W alarm {up}");
                }
                other => {
                    let _ = writeln!(out, "X news nobody here could cause: {other:?}");
                }
            }
        }
        let wait = Instant::now();
        while wait.elapsed() < pace {
            std::hint::spin_loop();
        }
    }
}
