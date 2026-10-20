//! The kill test, widened. A helper process (`examples\attack_kill_actor.rs`)
//! plays the interface doing everything at once on three texts: typing,
//! saving, keeping versions, closing a text and opening it again, renaming a
//! text while it is open, writing a state file, and shutting the store down
//! and opening it again. This test stops it with TerminateProcess, either at
//! a random instant up to a second and a half in, or aimed: a few hundred
//! microseconds after the helper has said it is about to rename, close,
//! reopen, shut down or start again. In some rounds recovery itself is then
//! killed once or twice before it is allowed to finish.
//!
//! After every kill:
//!
//! * each text's file, as the kill left it, is one whole state that text was in;
//! * after recovery each text is in exactly one file, under the name it had
//!   or the name it was being given, holding every edit acknowledged more
//!   than 100 ms before the kill and nothing older than the file held;
//! * nothing else is in the folder, no journal is left, the state file is
//!   whole, and the helper never saw anything it should not have.
//!
//! `OK_ATTACK_KILLS` sets the number of kills (240 if unset).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ok_store::testing::{Rig, Script};
use ok_store::Recovery;
use ok_text::Doc;

const SLOTS: usize = 3;
const WORKERS: usize = 3;
const WINDOW_NS: u128 = 100_000_000;

fn wall_ns() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

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

fn actor_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if matches!(dir.file_name().and_then(|name| name.to_str()), Some("deps" | "examples")) {
        dir.pop();
    }
    dir.join("examples").join(format!("attack_kill_actor{}", std::env::consts::EXE_SUFFIX))
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

/// What the helper said before it was stopped.
#[derive(Default)]
struct Heard {
    /// Name, length and fingerprint each slot started from.
    started: [Option<(String, usize, u64)>; SLOTS],
    /// When each edit of each slot was acknowledged.
    acks: [Vec<u128>; SLOTS],
    /// The name each slot has, and the name it was being given when the helper stopped.
    names: [(String, Option<String>); SLOTS],
    wrong: Vec<String>,
    worth_counting: usize,
}

/// What the reader thread tells the test while the helper runs.
enum Event {
    Go,
    Marker(String),
    CouldNotStart(String),
}

fn listen(stdout: std::process::ChildStdout, events: mpsc::Sender<Event>) -> Heard {
    let mut heard = Heard::default();
    let mut going = false;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["B", slot, name, len, hash] => {
                let (Ok(slot), Ok(len), Ok(hash)) = (slot.parse::<usize>(), len.parse(), hash.parse()) else { break };
                if slot < SLOTS {
                    heard.started[slot] = Some((name.to_string(), len, hash));
                    heard.names[slot] = (name.to_string(), None);
                }
            }
            ["G"] => {
                going = true;
                let _ = events.send(Event::Go);
            }
            ["E", slot, n, at] => {
                // A line the kill cut short does not parse or is out of step: it was not acknowledged.
                match (slot.parse::<usize>(), n.parse::<usize>(), at.parse::<u128>()) {
                    (Ok(slot), Ok(n), Ok(at)) if slot < SLOTS && n == heard.acks[slot].len() + 1 && at > 1_000_000_000_000_000_000 => heard.acks[slot].push(at),
                    _ => break,
                }
            }
            ["N", slot, name, what, ..] => {
                let Ok(slot) = slot.parse::<usize>() else { break };
                if slot >= SLOTS {
                    break;
                }
                match *what {
                    "b" => {
                        heard.names[slot].1 = Some(name.to_string());
                        let _ = events.send(Event::Marker("N".to_string()));
                    }
                    "ok" => heard.names[slot] = (name.to_string(), None),
                    "err" => heard.names[slot].1 = None,
                    _ => break,
                }
            }
            [kind @ ("C" | "O"), _, "b"] => {
                let _ = events.send(Event::Marker(kind.to_string()));
            }
            ["S", what @ ("b" | "down")] => {
                let _ = events.send(Event::Marker(format!("S{what}")));
            }
            ["C" | "O" | "S", ..] => {}
            ["W", ..] => heard.worth_counting += 1,
            ["X", ..] => {
                if !going {
                    let _ = events.send(Event::CouldNotStart(line.clone()));
                }
                heard.wrong.push(line);
            }
            _ => break,
        }
    }
    heard
}

/// Every file under the writing folder, as (path inside it, bytes).
fn snapshot(folder: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for sub in ["", "Sub"] {
        let Ok(entries) = fs::read_dir(folder.join(sub)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                assert!(name == "Sub" && sub.is_empty(), "a folder nobody made: {name}");
                continue;
            }
            let rel = if sub.is_empty() { name } else { format!("{sub}\\{name}") };
            // A save's temp file may vanish between the listing and the read.
            if let Ok(bytes) = fs::read(entry.path()) {
                out.push((rel, bytes));
            }
        }
    }
    out.sort();
    out
}

fn is_temp(rel: &str) -> bool {
    let name = rel.rsplit('\\').next().unwrap_or(rel);
    name.starts_with('~') && name.ends_with(".tmp")
}

/// Which slot a file belongs to, by its name.
fn slot_of(rel: &str) -> Option<usize> {
    let name = rel.rsplit('\\').next().unwrap_or(rel);
    let rest = name.strip_prefix('t')?;
    let (slot, tail) = rest.split_once('-')?;
    tail.strip_suffix(".md")?.parse::<u32>().ok()?;
    slot.parse().ok().filter(|&slot| slot < SLOTS)
}

/// Replays a slot's script from `base` and finds where two texts fall in it:
/// the first step after which the text was `before`, and the last step, at
/// or after that one, after which it was `after`. Step 0 is `base` itself.
fn locate(base: &str, seed: u64, steps: usize, before: Option<&str>, after: &str) -> (Option<usize>, Option<usize>) {
    let mut doc = Doc::from_text(base);
    let mut script = Script::new(seed);
    let (mut first, mut last) = (None, None);
    for step in 0..=steps {
        if step > 0 {
            script.step(&mut doc);
        }
        let len = doc.len_bytes();
        let mut text = None;
        if first.is_none() && before.is_some_and(|b| b.len() == len) {
            let now = doc.to_text("\n");
            if Some(now.as_str()) == before {
                first = Some(step);
            }
            text = Some(now);
        }
        if after.len() == len && (before.is_none() || first.is_some()) && text.unwrap_or_else(|| doc.to_text("\n")) == after {
            last = Some(step);
        }
    }
    (first, last)
}

#[derive(Default)]
struct Tally {
    kills: usize,
    edits: u64,
    aimed: HashMap<String, usize>,
    mid_save: usize,
    mid_rotation: usize,
    recovery_kills: usize,
    legacy_rename: usize,
    lost_young: usize,
    oldest_lost_ms: f64,
    worth_counting: usize,
    renames_seen: usize,
}

fn text_of(bytes: &[u8], what: &str) -> String {
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| panic!("{what} is not whole UTF-8"))
}

fn worker(index: usize, rounds: usize) -> Tally {
    let mut tally = Tally::default();
    let mut rng = Rng(0xC4A0_5000 + index as u64 * 104_729 + (wall_ns() as u64 | 1));
    let exe = actor_exe();
    assert!(exe.exists(), "the helper {} has not been built", exe.display());
    let mut rig = Rig::new(&format!("chaos-{index}"));
    // What each slot's file holds, and under which name, as far as this test knows.
    let mut expect: [(Option<String>, String); SLOTS] = Default::default();
    for round in 0..rounds {
        if round % 10 == 0 && round > 0 {
            rig = Rig::new(&format!("chaos-{index}"));
            expect = Default::default();
        }
        let seed = rng.below(u64::MAX) | 1;
        let pace = [20u64, 100, 400, 2000][rng.below(4) as usize];
        let save_ms = [1u64, 3, 10, 40, 200][rng.below(5) as usize];
        let rotate = [1u64, 3000, 40_000, 262_144][rng.below(4) as usize];
        // What to aim the kill at, and the things the helper must be doing for that.
        let aim = ["", "", "N", "C", "O", "Sb", "Sdown"][rng.below(7) as usize];
        let mut flags = rng.below(64);
        flags |= match aim {
            "N" => 2,
            "C" | "O" => 4,
            "Sb" | "Sdown" => 8,
            _ => 0,
        };
        let setting = format!("worker {index} round {round}: seed {seed}, pace {pace} us, save every {save_ms} ms, rotate at {rotate}, flags {flags}, aim {aim:?}");
        let mut child = Command::new(&exe)
            .arg("run")
            .arg(&rig.folder)
            .arg(&rig.data)
            .args([seed.to_string(), pace.to_string(), save_ms.to_string(), rotate.to_string(), flags.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the helper starts");
        let stdout = child.stdout.take().expect("the helper's output is piped");
        let (events_tx, events) = mpsc::channel();
        let reader = std::thread::spawn(move || listen(stdout, events_tx));
        match events.recv_timeout(Duration::from_secs(30)) {
            Ok(Event::Go) => {}
            Ok(Event::CouldNotStart(line)) => {
                let _ = child.kill();
                panic!("{setting}: the helper could not start: {line}");
            }
            _ => {
                let _ = child.kill();
                panic!("{setting}: the helper never got going");
            }
        }
        if aim.is_empty() {
            std::thread::sleep(Duration::from_micros(3_000 + rng.below(1_500_000)));
        } else {
            // Let it run a little, then wait for the helper to say it is
            // about to do the thing, and stop it a moment into it.
            std::thread::sleep(Duration::from_micros(rng.below(150_000)));
            while events.try_recv().is_ok() {}
            let end = Instant::now() + Duration::from_secs(3);
            let mut hit = false;
            while let Ok(event) = events.recv_timeout(end.saturating_duration_since(Instant::now())) {
                if matches!(&event, Event::Marker(kind) if kind == aim) {
                    hit = true;
                    break;
                }
            }
            if hit {
                *tally.aimed.entry(aim.to_string()).or_default() += 1;
                let wait = Instant::now();
                let spin = Duration::from_micros(rng.below(4_000));
                while wait.elapsed() < spin {
                    std::hint::spin_loop();
                }
            }
        }
        let killed_at = wall_ns();
        child.kill().expect("the helper can be killed");
        child.wait().expect("the helper ends");
        let heard = reader.join().expect("the reader thread ends");
        assert!(heard.wrong.is_empty(), "{setting}: the helper saw what must never happen: {:?}", heard.wrong);
        for (slot, (name, text)) in expect.iter().enumerate() {
            let (said_name, said_len, said_hash) = heard.started[slot].clone().unwrap_or_else(|| panic!("{setting}: slot {slot} never started"));
            assert_eq!((said_len, said_hash), (text.len(), fnv(text.as_bytes())), "{setting}: slot {slot} started from a text this test does not expect");
            if let Some(name) = name {
                assert_eq!(&said_name, name, "{setting}: slot {slot} started under another name");
            }
        }

        // What the kill left behind.
        let left = snapshot(&rig.folder);
        if left.iter().any(|(rel, _)| is_temp(rel)) {
            tally.mid_save += 1;
        }
        let journals = rig.journals();
        if journals.iter().any(|name| name.ends_with(".part")) {
            tally.mid_rotation += 1;
        }
        let mut before: [Option<String>; SLOTS] = Default::default();
        for (rel, bytes) in &left {
            if is_temp(rel) {
                continue;
            }
            let slot = slot_of(rel).unwrap_or_else(|| panic!("{setting}: the kill left a file nobody made: {rel}"));
            assert!(before[slot].is_none(), "{setting}: the kill left slot {slot} under two names: {:?}", left.iter().map(|(rel, _)| rel).collect::<Vec<_>>());
            before[slot] = Some(text_of(bytes, rel));
        }

        // Recovery, now and then itself killed first, once or twice.
        if rng.below(3) == 0 {
            for _ in 0..1 + rng.below(2) {
                let mut second = Command::new(&exe).arg("recover").arg(&rig.folder).arg(&rig.data).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the helper starts");
                std::thread::sleep(Duration::from_micros(rng.below(45_000)));
                let _ = second.kill();
                let _ = second.wait();
                tally.recovery_kills += 1;
            }
        }
        let (store, report) = rig.open();
        assert!(store.shutdown(Duration::from_secs(10)), "{setting}: shutdown after recovery timed out");
        for told in &report {
            let fine = matches!(told.how, Recovery::BroughtBack) || matches!(&told.how, Recovery::WroteBack(rel) if *rel == told.rel);
            assert!(fine, "{setting}: recovery misjudged its own file: {report:?}");
        }

        let now = snapshot(&rig.folder);
        let names: Vec<&String> = now.iter().map(|(rel, _)| rel).collect();
        assert!(rig.journals().is_empty(), "{setting}: a journal outlived recovery: {:?}", rig.journals());
        let mut after: [Option<(String, String)>; SLOTS] = Default::default();
        for (rel, bytes) in &now {
            let slot = slot_of(rel).unwrap_or_else(|| panic!("{setting}: after recovery the folder holds something that is not a text of his: {names:?} (report {report:?})"));
            assert!(after[slot].is_none(), "{setting}: after recovery slot {slot} is in two files: {names:?} (report {report:?}, renames heard {:?})", heard.names[slot]);
            after[slot] = Some((rel.clone(), text_of(bytes, rel)));
        }
        for slot in 0..SLOTS {
            let (_, base) = &expect[slot];
            let acks = &heard.acks[slot];
            let required = acks.iter().filter(|&&at| at + WINDOW_NS <= killed_at).count();
            let here = format!("{setting}, slot {slot} ({} edits acknowledged, {required} of them more than 100 ms before the kill, names {:?}, report {report:?})", acks.len(), heard.names[slot]);
            let Some((rel, text)) = &after[slot] else {
                assert!(before[slot].is_none(), "{here}: the file was there after the kill and is gone after recovery");
                assert!(base.is_empty() && required == 0, "{here}: the text is in no file at all");
                // Not typed in long enough ago to be owed, or not at all.
                expect[slot] = (None, String::new());
                continue;
            };
            let (current, coming) = &heard.names[slot];
            assert!(rel == current || Some(rel) == coming.as_ref(), "{here}: the text is under {rel}, a name it never had");
            let (first, last) = locate(base, seed_of(seed, slot), acks.len() + 2, before[slot].as_deref(), text);
            if let Some(before) = &before[slot] {
                assert!(first.is_some(), "{here}: the file the kill left ({} bytes) is not any state the text was in", before.len());
            }
            let last = last.unwrap_or_else(|| panic!("{here}: the recovered text ({} bytes) is not any state the text was in at or after the file's", text.len()));
            assert!(last >= required, "{here}: only {last} edits came back");
            if last < acks.len() {
                tally.lost_young += 1;
                tally.oldest_lost_ms = tally.oldest_lost_ms.max(killed_at.saturating_sub(acks[last]) as f64 / 1e6);
            }
            tally.edits += acks.len() as u64;
            if coming.is_some() || current != &heard.started[slot].as_ref().map(|s| s.0.clone()).unwrap_or_default() {
                tally.renames_seen += 1;
            }
            expect[slot] = (Some(rel.clone()), text.clone());
        }
        // A state file is one number repeated sixty-four times, or it is torn.
        if let Ok(state) = fs::read_to_string(rig.data.join("state").join("window")) {
            let lines: Vec<&str> = state.lines().collect();
            assert!(lines.len() == 64 && lines.iter().all(|line| line.len() == 20 && *line == lines[0]), "{setting}: the state file is a mixture: {} lines", lines.len());
        }
        tally.kills += 1;
        tally.worth_counting += heard.worth_counting;
        tally.legacy_rename += (flags & 1) as usize;
    }
    tally
}

/// Every file under `dir`, folders walked, as (path inside it, bytes).
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned(), bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Puts `dir` back to exactly the files in `files`.
fn restore(dir: &Path, files: &[(String, Vec<u8>)]) {
    for _ in 0..200 {
        match fs::remove_dir_all(dir) {
            Ok(()) => break,
            Err(_) if !dir.exists() => break,
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(!dir.exists(), "{} could not be cleared", dir.display());
    fs::create_dir_all(dir).unwrap();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
    }
}

/// A copy kept beside a text carries the time of the journal it came from,
/// which differs from one restored copy of the same journal to the next.
fn without_stamp(rel: &str) -> String {
    match (rel.find("(recovered "), rel.rfind(')')) {
        (Some(from), Some(to)) if to > from => format!("{}(recovered){}", &rel[..from], &rel[to + 1..]),
        _ => rel.to_string(),
    }
}

/// A crashed session with every kind of work for recovery to do, recovered
/// once without interruption and then, from the same starting point, by a
/// process that is killed part-way (up to three times) before one that is
/// allowed to finish. Both must end with the same files holding the same text.
#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn a_recovery_killed_part_way_and_run_again_ends_where_an_unbroken_one_does() {
    use ok_store::testing::{quick, save_wait, type_end};
    use ok_store::{Setup, Store};

    let exe = actor_exe();
    assert!(exe.exists(), "the helper {} has not been built", exe.display());
    let rig = Rig::new("recover-twice");
    let other = rig.folder.with_file_name("other writing");
    let vanished = rig.folder.with_file_name("vanished writing");
    let open_on = |folder: &Path| {
        let mut setup = Setup::new(folder.to_path_buf(), rig.data.clone());
        setup.tuning = quick();
        Store::open(setup).expect("the store opens").0
    };
    let settle = |store: &mut Store| {
        let end = Instant::now() + Duration::from_secs(10);
        while store.unwritten() {
            assert!(Instant::now() < end, "the journal never caught up");
            while store.poll().is_some() {}
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    // In the folder being opened: texts whose file is merely behind, texts
    // whose file another program changed, texts whose file is gone, texts
    // never saved (one in a sub-folder), and one closed cleanly.
    let mut store = open_on(&rig.folder);
    let mut changed_outside = Vec::new();
    let mut removed = Vec::new();
    for n in 0..16 {
        let kind = n % 4;
        let rel = if n == 15 { "Sub\\never-15.md".to_string() } else { format!("{}-{n}.md", ["behind", "changed", "gone", "never"][kind]) };
        let opened = store.create_text(&rel).unwrap();
        let mut doc = Doc::new();
        let mut edit = type_end(&mut store, opened.doc, &mut doc, &format!("Text {n}, first paragraph, saved.\n\n{}", "A longer run of words so the files have some weight. ".repeat(40 + n)));
        if kind != 3 {
            save_wait(&mut store, opened.doc, &doc, edit);
        }
        if n == 12 {
            store.close(opened.doc, None, None);
            continue;
        }
        for word in 0..6 {
            edit = type_end(&mut store, opened.doc, &mut doc, &format!(" unsaved{word}"));
        }
        let _ = edit;
        match kind {
            1 => changed_outside.push(rel),
            2 => removed.push(rel),
            _ => {}
        }
    }
    settle(&mut store);
    // Two more writing folders with journals in the same data directory: one
    // still there afterwards, one deleted. All three stores run at once, so
    // that none of them takes the others' journals for a crashed session's.
    let mut elsewhere = Vec::new();
    for folder in [&other, &vanished] {
        let mut store = open_on(folder);
        let opened = store.create_text("elsewhere.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved in another folder.");
        save_wait(&mut store, opened.doc, &doc, edit);
        type_end(&mut store, opened.doc, &mut doc, " And typed there, unsaved.");
        settle(&mut store);
        elsewhere.push(store);
    }
    store.abandon();
    for store in elsewhere {
        store.abandon();
    }
    for rel in &changed_outside {
        fs::write(rig.folder.join(rel), format!("Rewritten by another program while {rel} was down.")).unwrap();
    }
    for rel in &removed {
        fs::remove_file(rig.folder.join(rel)).unwrap();
    }
    fs::remove_dir_all(&vanished).unwrap();

    let start = [tree(&rig.folder), tree(&rig.data), tree(&other)];
    let journals_at_start = rig.journals().len();
    assert_eq!(journals_at_start, 17, "{:?}", rig.journals());
    let put_back = || {
        restore(&rig.folder, &start[0]);
        restore(&rig.data, &start[1]);
        restore(&other, &start[2]);
        let _ = fs::remove_dir_all(&vanished);
    };
    // What the three places hold, with temp files under the data directory
    // counted apart: nothing clears those, and they are not his.
    let outcome = || {
        let mut files = Vec::new();
        let mut stray = 0;
        for (label, dir) in [("folder", rig.folder.clone()), ("other", other.clone()), ("recovered", rig.data.join("recovered"))] {
            for (rel, bytes) in tree(&dir) {
                if label == "recovered" && is_temp(&rel) {
                    stray += 1;
                } else {
                    files.push((format!("{label}\\{}", without_stamp(&rel)), bytes));
                }
            }
        }
        files.sort();
        (files, stray)
    };
    let finish = || {
        let (store, report) = rig.open();
        assert!(store.shutdown(Duration::from_secs(10)));
        report
    };

    put_back();
    let report = finish();
    let (reference, _) = outcome();
    assert!(rig.journals().is_empty(), "an unbroken recovery left journals: {:?}", rig.journals());
    assert!(!vanished.exists());
    assert_eq!(report.len(), 4 + 4 + 4 + 4 - 1 + 2, "{report:?}");
    assert!(reference.iter().all(|(rel, _)| !is_temp(rel)), "an unbroken recovery left temp files: {:?}", reference.iter().map(|(rel, _)| rel).collect::<Vec<_>>());
    assert!(finish().is_empty(), "a second recovery found work the first had left");
    assert_eq!(outcome().0, reference);

    let rounds: usize = std::env::var("OK_ATTACK_RECOVERIES").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let mut rng = Rng(0x7EC0_7EC0 ^ (wall_ns() as u64 | 1));
    let (mut part_way, mut kills, mut strays) = (0, 0, 0);
    for round in 0..rounds {
        put_back();
        let mut delays = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let mut child = Command::new(&exe).arg("recover").arg(&rig.folder).arg(&rig.data).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the helper starts");
            let delay = rng.below(70_000);
            std::thread::sleep(Duration::from_micros(delay));
            let _ = child.kill();
            let _ = child.wait();
            delays.push(delay);
            kills += 1;
            let left = rig.journals().len();
            if left > 0 && left < journals_at_start {
                part_way += 1;
            }
        }
        let report = finish();
        let (now, stray) = outcome();
        strays += stray;
        let names = |files: &[(String, Vec<u8>)]| files.iter().map(|(rel, bytes)| format!("{rel} ({} bytes)", bytes.len())).collect::<Vec<_>>();
        assert_eq!(names(&now), names(&reference), "round {round}, recovery killed after {delays:?} us and then finished ({report:?}): the files differ from an unbroken recovery's");
        assert!(now == reference, "round {round}, recovery killed after {delays:?} us: same files, different text");
        assert!(rig.journals().is_empty(), "round {round}: a journal outlived recovery: {:?}", rig.journals());
        assert!(finish().is_empty(), "round {round}: a further recovery still found work");
    }
    println!("recovery kill test: {rounds} rounds, {kills} recoveries killed, {part_way} of them with some journals dealt with and some not; temp files left under the data directory's recovered folder: {strays}");
    assert!(rounds < 50 || part_way > 0, "no kill landed inside recovery: the test is not testing what it says");
}

#[test]
fn killed_while_doing_everything_at_once_each_text_is_whole_and_in_one_place() {
    let kills: usize = std::env::var("OK_ATTACK_KILLS").ok().and_then(|v| v.parse().ok()).unwrap_or(240);
    let per_worker = kills.div_ceil(WORKERS);
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..WORKERS).map(|index| scope.spawn(move || worker(index, per_worker))).collect();
        handles.into_iter().map(|handle| handle.join().expect("a kill worker failed")).collect()
    });
    let mut total = Tally::default();
    for tally in tallies {
        total.kills += tally.kills;
        total.edits += tally.edits;
        total.mid_save += tally.mid_save;
        total.mid_rotation += tally.mid_rotation;
        total.recovery_kills += tally.recovery_kills;
        total.legacy_rename += tally.legacy_rename;
        total.lost_young += tally.lost_young;
        total.worth_counting += tally.worth_counting;
        total.renames_seen += tally.renames_seen;
        total.oldest_lost_ms = total.oldest_lost_ms.max(tally.oldest_lost_ms);
        for (kind, n) in tally.aimed {
            *total.aimed.entry(kind).or_default() += n;
        }
    }
    let mut aimed: Vec<(String, usize)> = total.aimed.into_iter().collect();
    aimed.sort();
    println!(
        "chaos kill test: {} kills, {} edits acknowledged; aimed kills that found their moment {:?} (N rename, C close, O reopen, Sb shutdown, Sdown start); a save's temp file present in {}, half a journal generation in {}, recovery itself killed {} times, the old-rename save path in {} rounds, texts renamed or being renamed in {}; texts that lost an acknowledged edit {} (oldest {:.2} ms before the kill; the pass line is 100 ms); holds and alarms the helper counted {}",
        total.kills, total.edits, aimed, total.mid_save, total.mid_rotation, total.recovery_kills, total.legacy_rename, total.renames_seen, total.lost_young, total.oldest_lost_ms, total.worth_counting
    );
    assert!(total.kills >= kills);
}
