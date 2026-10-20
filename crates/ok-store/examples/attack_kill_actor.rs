//! A stand-in for the interface that does everything the interface may do,
//! all mixed together, so that a kill test can stop it at any of the narrow
//! moments: three texts typed in turn, saves, kept versions, closing a text
//! and opening it again, renaming a text while it is open, state files, and
//! shutting the whole store down and opening it again inside one process.
//!
//! ```text
//! attack_kill_actor run <folder> <data> <seed> <pace us> <save every ms> <rotate at> <flags>
//! attack_kill_actor recover <folder> <data>
//! ```
//!
//! Text `i` lives in a file called `t<i>-<n>.md`, at the top of the folder or
//! under `Sub`; `n` goes up by one each time the text is renamed. Its edits
//! come from `Script::new(seed_of(seed, i))`, so the test that started this
//! process can work out what the text was after every one of them.
//!
//! What is printed, one line each, only after the call it reports returned:
//!
//! ```text
//! B <i> <name> <bytes> <fnv>     the text slot i starts from
//! G                              everything is open; edits follow
//! E <i> <n> <wall-clock ns>      edit n of slot i has been recorded
//! N <i> <name> b|ok|err          a rename to <name> is about to be asked for, was made, was refused
//! C <i> b|ok                     slot i is about to be closed, has been closed
//! O <i> b|ok                     slot i is about to be opened again, is open
//! S b|down|up                    the store is about to shut down, is down, is open again
//! W <anything>                   something worth counting that is not wrong
//! X <anything>                   something that must never happen
//! ```
//!
//! `flags` adds up: 1 the save path of volumes without the one-step rename,
//! 2 renames, 4 closing and reopening, 8 shutting down and opening again,
//! 16 kept versions, 32 state files.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ok_store::testing::{quick, Script};
use ok_store::{DocId, News, Pos, Recovery, Setup, Store, StoreError, Tuning};
use ok_text::Doc;

const SLOTS: usize = 3;

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn seed_of(seed: u64, slot: usize) -> u64 {
    seed ^ (slot as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn wall_ns() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Prints one line and hands it to the pipe at once.
macro_rules! say {
    ($($arg:tt)*) => {{
        let out = std::io::stdout();
        let mut out = out.lock();
        let _ = writeln!(out, $($arg)*);
        let _ = out.flush();
    }};
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

struct Slot {
    rel: String,
    serial: u32,
    id: DocId,
    doc: Doc,
    script: Script,
    /// Script steps applied so far.
    steps: u64,
    /// The store's own number for the newest edit of the present handle.
    edit: u64,
    last_save: Instant,
}

/// The file slot `i`'s text is in, if there is one: `t<i>-<n>.md` at the top
/// of the folder or under `Sub`. Two of them is something that must never be.
fn find(folder: &Path, slot: usize) -> Option<(String, u32)> {
    let mut found: Vec<(String, u32)> = Vec::new();
    for sub in ["", "Sub"] {
        let Ok(entries) = std::fs::read_dir(folder.join(sub)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(serial) = name.strip_prefix(&format!("t{slot}-")).and_then(|rest| rest.strip_suffix(".md")).and_then(|n| n.parse().ok()) else { continue };
            let rel = if sub.is_empty() { name } else { format!("{sub}\\{name}") };
            found.push((rel, serial));
        }
    }
    if found.len() > 1 {
        say!("X slot {slot} has two files: {found:?}");
    }
    found.into_iter().max_by_key(|(_, serial)| *serial)
}

fn open_slot(store: &mut Store, folder: &Path, slot: usize) -> Option<(String, u32, DocId, String)> {
    let (rel, serial) = find(folder, slot).unwrap_or((format!("t{slot}-0.md"), 0));
    let opened = match store.open_text(&rel) {
        Ok(opened) => opened,
        Err(StoreError::Missing) => match store.create_text(&rel) {
            Ok(opened) => opened,
            Err(error) => {
                say!("X slot {slot} could not be started: {error:?}");
                return None;
            }
        },
        Err(error) => {
            say!("X slot {slot} could not be opened: {error:?}");
            return None;
        }
    };
    if opened.read_only.is_some() {
        say!("X slot {slot} opened for reading only: {:?}", opened.read_only);
    }
    Some((rel, serial, opened.doc, opened.text))
}

fn open_store(folder: &Path, data: &Path, tuning: &Tuning) -> Option<(Store, Vec<ok_store::Recovered>)> {
    let mut setup = Setup::new(folder.to_path_buf(), data.to_path_buf());
    setup.tuning = tuning.clone();
    match Store::open(setup) {
        Ok(opened) => Some(opened),
        Err(error) => {
            say!("X the store did not open: {error:?}");
            None
        }
    }
}

/// Reads all the news there is. Anything that speaks of another program
/// touching the folder is wrong here: nobody else does.
fn drain(store: &mut Store, closed: Option<DocId>) -> Option<(bool, bool)> {
    let mut answer = None;
    while let Some(news) = store.poll() {
        match news {
            News::Saved { .. } | News::Kept { .. } | News::Tree(_) | News::FileText { .. } | News::Released { .. } => {}
            News::Closed { doc, beside, in_journal } => {
                if closed == Some(doc) {
                    answer = Some((beside.is_some(), in_journal));
                }
            }
            News::Held { why, .. } => say!("W held {why:?}"),
            News::JournalAlarm(up) => say!("W alarm {up}"),
            other => say!("X news that only another program could cause: {other:?}"),
        }
    }
    answer
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    let (Some(mode), Some(folder), Some(data)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("usage: attack_kill_actor run|recover <folder> <data> [seed pace-us save-ms rotate-at flags]");
        std::process::exit(2);
    };
    let (folder, data) = (PathBuf::from(folder), PathBuf::from(data));
    let flags = number(7);
    let mut tuning = quick();
    tuning.rotate_at = number(6).max(1);
    if flags & 1 != 0 {
        tuning.faults.no_one_step.store(1, Ordering::Relaxed);
    }
    let Some((mut store, _)) = open_store(&folder, &data, &tuning) else { std::process::exit(3) };
    if mode == "recover" {
        say!("R");
        store.shutdown(Duration::from_secs(5));
        return;
    }

    let seed = number(3);
    let pace = Duration::from_micros(number(4));
    let save_every = Duration::from_millis(number(5).max(1));
    let mut rng = Rng(seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
    let mut slots: Vec<Slot> = Vec::new();
    for slot in 0..SLOTS {
        let Some((rel, serial, id, text)) = open_slot(&mut store, &folder, slot) else { std::process::exit(4) };
        say!("B {slot} {rel} {} {}", text.len(), fnv(text.as_bytes()));
        slots.push(Slot { rel, serial, id, doc: Doc::from_text(&text), script: Script::new(seed_of(seed, slot)), steps: 0, edit: 0, last_save: Instant::now() });
    }
    say!("G");

    let mut state_counter = 0u64;
    loop {
        let roll = rng.below(4000);
        let pick = match rng.below(10) {
            0..=5 => 0,
            6..=8 => 1,
            _ => 2,
        };
        if roll < 6 && flags & 2 != 0 {
            // Rename the text while it is open, into or out of the sub-folder.
            let slot = &mut slots[pick];
            let serial = slot.serial + 1;
            let name = format!("t{pick}-{serial}.md");
            let to = if rng.below(2) == 0 { name } else { format!("Sub\\{name}") };
            say!("N {pick} {to} b");
            match store.rename(&slot.rel, &to) {
                Ok(()) => {
                    slot.rel = to.clone();
                    slot.serial = serial;
                    say!("N {pick} {to} ok");
                }
                Err(error) => say!("N {pick} {to} err {error:?}"),
            }
        } else if roll < 12 && flags & 4 != 0 {
            // Close the text with its last words, wait for that, open it again.
            let text = slots[pick].doc.to_text("\n");
            let last = if rng.below(4) == 0 { None } else { Some((text.clone(), slots[pick].edit)) };
            let id = slots[pick].id;
            say!("C {pick} b");
            store.close(id, last, Some(Pos { line: 0, col: 0 }));
            let started = Instant::now();
            let answer = loop {
                if let Some(answer) = drain(&mut store, Some(id)) {
                    break Some(answer);
                }
                if started.elapsed() > Duration::from_secs(20) {
                    break None;
                }
                std::thread::sleep(Duration::from_micros(200));
            };
            match answer {
                Some((false, _)) => say!("C {pick} ok"),
                Some((true, _)) => say!("X slot {pick} was written beside its own file at close"),
                None => say!("X slot {pick} never reported closed"),
            }
            say!("O {pick} b");
            match store.open_text(&slots[pick].rel) {
                Ok(opened) => {
                    if opened.text != text {
                        say!("X slot {pick} came back from close as {} bytes, not the {} it was closed with (restored {:?})", opened.text.len(), text.len(), opened.restored);
                    }
                    slots[pick].id = opened.doc;
                    slots[pick].edit = 0;
                    slots[pick].doc = Doc::from_text(&text);
                    say!("O {pick} ok");
                }
                Err(StoreError::Missing) if slots[pick].steps == 0 && text.is_empty() => match store.create_text(&slots[pick].rel) {
                    Ok(opened) => {
                        slots[pick].id = opened.doc;
                        slots[pick].edit = 0;
                        say!("O {pick} ok");
                    }
                    Err(error) => say!("X slot {pick} could not be started again: {error:?}"),
                },
                Err(error) => say!("X slot {pick} could not be opened again: {error:?}"),
            }
        } else if roll < 15 && flags & 8 != 0 {
            // Leave and come back, sometimes without closing anything first.
            let tidy = rng.below(2) == 0;
            say!("S b");
            if tidy {
                for slot in &slots {
                    store.close(slot.id, Some((slot.doc.to_text("\n"), slot.edit)), None);
                }
            }
            let down = store.shutdown(Duration::from_secs(10));
            say!("S down");
            if !down {
                say!("W shutdown ran out of time");
            }
            let Some((again, report)) = open_store(&folder, &data, &tuning) else { std::process::exit(3) };
            store = again;
            if tidy && down && !report.is_empty() {
                say!("X recovery had work after a tidy shutdown: {report:?}");
            }
            if report.iter().any(|told| !matches!(told.how, Recovery::BroughtBack | Recovery::WroteBack(_))) {
                say!("X recovery misjudged a text after a shutdown: {report:?}");
            }
            for (i, slot) in slots.iter_mut().enumerate() {
                let Some((rel, serial, id, text)) = open_slot(&mut store, &folder, i) else { std::process::exit(4) };
                let mine = slot.doc.to_text("\n");
                if mine.is_empty() && text.is_empty() {
                    // Never typed in, so never a file: only the store knew the name it had.
                    slot.rel = rel;
                    slot.serial = serial;
                } else if text != mine || rel != slot.rel || serial != slot.serial {
                    say!("X slot {i} came back after a shutdown as {rel} with {} bytes, not {} with {} (tidy {tidy}, down {down}, report {report:?})", text.len(), slot.rel, mine.len());
                }
                slot.id = id;
                slot.edit = 0;
            }
            say!("S up");
        } else if roll < 22 && flags & 16 != 0 {
            let slot = &slots[pick];
            store.keep(slot.id, slot.doc.to_text("\n"), slot.edit, "kept");
        } else if roll < 40 && flags & 32 != 0 {
            // A state file is one number repeated: a mixture of two shows.
            state_counter += 1;
            let block = format!("{state_counter:020}\n").repeat(64);
            store.write_state("window", block.as_bytes());
        } else {
            let slot = &mut slots[pick];
            let change = slot.script.step(&mut slot.doc);
            slot.edit = store.record(slot.id, &change, slot.doc.len_bytes());
            slot.steps += 1;
            // Acknowledged only now that record has returned.
            say!("E {pick} {} {}", slot.steps, wall_ns());
            if slot.edit == 0 {
                say!("X slot {pick} recorded into a handle the store does not know");
            }
        }
        for slot in &mut slots {
            if slot.last_save.elapsed() >= save_every && slot.edit > 0 {
                store.save(slot.id, slot.doc.to_text("\n"), slot.edit);
                slot.last_save = Instant::now();
            }
        }
        drain(&mut store, None);
        let wait = Instant::now();
        while wait.elapsed() < pace {
            std::hint::spin_loop();
        }
    }
}
