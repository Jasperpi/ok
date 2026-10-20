//! Kills aimed at the journal's rotation. A helper process
//! (`examples\attack_kill_rotor.rs`) keeps one very small text and saves it
//! constantly, so that its journal is started afresh at nearly every save;
//! this test stops it with TerminateProcess at a random instant, a few
//! milliseconds to a tenth of a second in. In a third of the rounds recovery
//! is then itself killed once or twice before it is allowed to finish.
//!
//! After every kill:
//!
//! * the file, as the kill left it, is one whole state the text was in;
//! * after recovery the file holds every edit acknowledged more than 100 ms
//!   before the kill, and nothing older than it held;
//! * nothing else is in the folder and no journal is left.
//!
//! The test also counts how often the kill found half a generation (a
//! `.part` file) or two whole generations side by side, and fails if it never
//! did: then it would not be testing what it says.
//!
//! `OK_ATTACK_ROTATIONS` sets the number of kills (150 if unset).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ok_store::testing::Rig;
use ok_store::{Change, Pos, Recovery};
use ok_text::{Doc, Span};

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

fn helper(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    if matches!(dir.file_name().and_then(|name| name.to_str()), Some("deps" | "examples")) {
        dir.pop();
    }
    dir.join("examples").join(name)
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

/// Edit `n` of the helper's script. The helper has the same function.
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

/// Replays the script from `base` and finds where two texts fall in it: the
/// first step after which the text was `before`, and the last step, at or
/// after that one, after which it was `after`. Step 0 is `base` itself.
fn locate(base: &str, cap: usize, steps: usize, before: Option<&str>, after: &str) -> (Option<usize>, Option<usize>) {
    let mut doc = Doc::from_text(base);
    let (mut first, mut last) = (None, None);
    for n in 0..=steps {
        if n > 0 {
            step(&mut doc, n as u64, cap);
        }
        let now = doc.to_text("\n");
        if first.is_none() && before == Some(now.as_str()) {
            first = Some(n);
        }
        if now == after && (before.is_none() || first.is_some()) {
            last = Some(n);
        }
    }
    (first, last)
}

#[derive(Default)]
struct Tally {
    kills: usize,
    edits: u64,
    half_generation: usize,
    two_generations: usize,
    highest_generation: u32,
    mid_save: usize,
    recovery_kills: usize,
    lost_young: usize,
    oldest_lost_ms: f64,
    worth_counting: usize,
}

fn worker(index: usize, rounds: usize) -> Tally {
    let mut tally = Tally::default();
    let mut rng = Rng(0x2074_A7E5 + index as u64 * 15_485_863 + (wall_ns() as u64 | 1));
    let rotor = helper("attack_kill_rotor.exe");
    let actor = helper("attack_kill_actor.exe");
    assert!(rotor.exists() && actor.exists(), "the helpers {} and {} have not been built", rotor.display(), actor.display());
    let mut rig = Rig::new(&format!("rotation-{index}"));
    let mut base = String::new();
    let mut on_disk = false;
    for round in 0..rounds {
        if round % 10 == 0 && round > 0 {
            rig = Rig::new(&format!("rotation-{index}"));
            base.clear();
            on_disk = false;
        }
        let path = rig.folder.join("rotor.md");
        let pace = [0u64, 30, 150, 600][rng.below(4) as usize];
        let every = [1u64, 2, 4, 16][rng.below(4) as usize];
        let cap = [24usize, 96, 400][rng.below(3) as usize];
        let flags = rng.below(8);
        let setting = format!("worker {index} round {round}: pace {pace} us, a save every {every} edits, cap {cap} bytes, flags {flags}");
        let mut child = Command::new(&rotor)
            .arg(&rig.folder)
            .arg(&rig.data)
            .args([pace.to_string(), every.to_string(), cap.to_string(), flags.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the helper starts");
        let stdout = child.stdout.take().expect("the helper's output is piped");
        let (started_tx, started_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut start = None;
            let mut acks: Vec<u128> = Vec::new();
            let mut wrong: Vec<String> = Vec::new();
            let mut worth = 0usize;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let parts: Vec<&str> = line.split_whitespace().collect();
                match parts.as_slice() {
                    ["B", len, hash] => {
                        start = Some((len.parse::<usize>().unwrap_or(usize::MAX), hash.parse::<u64>().unwrap_or(0)));
                        let _ = started_tx.send(());
                    }
                    // A line the kill cut short does not parse or is out of step: it was not acknowledged.
                    ["E", n, at] => match (n.parse::<usize>(), at.parse::<u128>()) {
                        (Ok(n), Ok(at)) if n == acks.len() + 1 && at > 1_000_000_000_000_000_000 => acks.push(at),
                        _ => break,
                    },
                    ["W", ..] => worth += 1,
                    ["X", ..] => {
                        wrong.push(line.clone());
                        let _ = started_tx.send(());
                    }
                    _ => break,
                }
            }
            (start, acks, wrong, worth)
        });
        if started_rx.recv_timeout(Duration::from_secs(30)).is_err() {
            let _ = child.kill();
            panic!("{setting}: the helper never got going");
        }
        std::thread::sleep(Duration::from_micros(2_000 + rng.below(120_000)));
        let killed_at = wall_ns();
        child.kill().expect("the helper can be killed");
        child.wait().expect("the helper ends");
        let (start, acks, wrong, worth) = reader.join().expect("the reader thread ends");
        assert!(wrong.is_empty(), "{setting}: the helper saw what must never happen: {wrong:?}");
        assert_eq!(start, Some((base.len(), fnv(base.as_bytes()))), "{setting}: the helper started from a text this test does not expect");

        // What the kill left behind.
        let before = fs::read(&path).ok().map(|bytes| String::from_utf8(bytes).unwrap_or_else(|_| panic!("{setting}: the file is not whole UTF-8")));
        assert!(before.is_some() || !on_disk, "{setting}: the file was there and is gone: {:?}", rig.listing());
        if rig.listing().iter().any(|name| name.ends_with(".tmp")) {
            tally.mid_save += 1;
        }
        let journals = rig.journals();
        let whole: Vec<u32> = journals.iter().filter(|name| name.ends_with(".wal")).filter_map(|name| name.split('.').nth(1)?.parse().ok()).collect();
        if journals.iter().any(|name| name.ends_with(".part")) {
            tally.half_generation += 1;
        }
        if whole.len() > 1 {
            tally.two_generations += 1;
        }
        tally.highest_generation = tally.highest_generation.max(whole.iter().copied().max().unwrap_or(0));

        if rng.below(3) == 0 {
            for _ in 0..1 + rng.below(2) {
                let mut second = Command::new(&actor).arg("recover").arg(&rig.folder).arg(&rig.data).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the helper starts");
                std::thread::sleep(Duration::from_micros(rng.below(35_000)));
                let _ = second.kill();
                let _ = second.wait();
                tally.recovery_kills += 1;
            }
        }
        let (store, report) = rig.open();
        assert!(store.shutdown(Duration::from_secs(10)), "{setting}: shutdown after recovery timed out");
        for told in &report {
            let fine = matches!(told.how, Recovery::BroughtBack) || (!on_disk && matches!(&told.how, Recovery::WroteBack(rel) if rel.as_os_str() == "rotor.md"));
            assert!(fine, "{setting}: recovery misjudged its own file (journals at the kill {journals:?}): {report:?}");
        }
        let required = acks.iter().filter(|&&at| at + WINDOW_NS <= killed_at).count();
        let here = format!("{setting} ({} edits acknowledged, {required} of them more than 100 ms before the kill; journals at the kill {journals:?}; report {report:?})", acks.len());
        let listing = rig.listing();
        assert!(listing == ["rotor.md"] || (listing.is_empty() && !on_disk && required == 0), "{here}: the folder holds {listing:?}");
        assert!(rig.journals().is_empty(), "{here}: a journal outlived recovery: {:?}", rig.journals());
        let after = String::from_utf8(fs::read(&path).unwrap_or_default()).unwrap_or_else(|_| panic!("{here}: the file is not whole UTF-8"));

        let (first, last) = locate(&base, cap, acks.len() + 2, before.as_deref(), &after);
        if let Some(before) = &before {
            assert!(first.is_some(), "{here}: the file the kill left is not any state the text was in: {before:?}");
        }
        let last = last.unwrap_or_else(|| panic!("{here}: the recovered text is not any state the text was in at or after the file's: {after:?}"));
        assert!(last >= required, "{here}: only {last} edits came back");
        if last < acks.len() {
            tally.lost_young += 1;
            tally.oldest_lost_ms = tally.oldest_lost_ms.max(killed_at.saturating_sub(acks[last]) as f64 / 1e6);
        }
        tally.kills += 1;
        tally.edits += acks.len() as u64;
        tally.worth_counting += worth;
        base = after;
        on_disk = path.exists();
    }
    tally
}

/// What one run of the helper said before it was stopped: the text it
/// started from, when each edit was acknowledged, and anything wrong.
type Heard = (Option<(usize, u64)>, Vec<u128>, Vec<String>);

/// Starts the helper, lets it run for `run`, kills it, and returns what it
/// said and the wall-clock time of the kill.
fn run_and_kill(rotor: &Path, rig: &Rig, args: [String; 4], run: Duration) -> (Heard, u128) {
    let mut child = Command::new(rotor).arg(&rig.folder).arg(&rig.data).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("the helper starts");
    let stdout = child.stdout.take().expect("the helper's output is piped");
    let (started_tx, started_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut heard: Heard = (None, Vec::new(), Vec::new());
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                ["B", len, hash] => {
                    heard.0 = Some((len.parse::<usize>().unwrap_or(usize::MAX), hash.parse::<u64>().unwrap_or(0)));
                    let _ = started_tx.send(());
                }
                ["E", n, at] => match (n.parse::<usize>(), at.parse::<u128>()) {
                    (Ok(n), Ok(at)) if n == heard.1.len() + 1 && at > 1_000_000_000_000_000_000 => heard.1.push(at),
                    _ => break,
                },
                ["W", ..] => {}
                ["X", ..] => {
                    heard.2.push(line.clone());
                    let _ = started_tx.send(());
                }
                _ => break,
            }
        }
        heard
    });
    if started_rx.recv_timeout(Duration::from_secs(30)).is_err() {
        let _ = child.kill();
        panic!("the helper never got going");
    }
    std::thread::sleep(run);
    let killed_at = wall_ns();
    child.kill().expect("the helper can be killed");
    child.wait().expect("the helper ends");
    (reader.join().expect("the reader thread ends"), killed_at)
}

/// Another program keeps the file open in a way that lets nothing be renamed
/// over it, for as long as the helper runs and through the first start after
/// the kill. Every save is held; everything typed is in the journal alone.
/// The held file is never touched, the first start says the text is waiting,
/// and once the other program lets go the next start brings everything back.
#[test]
fn killed_while_every_save_is_held_by_another_program_everything_typed_comes_back() {
    use std::os::windows::fs::OpenOptionsExt;
    const SHARE_READ_WRITE: u32 = 3;
    let rounds: usize = std::env::var("OK_ATTACK_HELD").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let rotor = helper("attack_kill_rotor.exe");
    assert!(rotor.exists(), "the helper {} has not been built", rotor.display());
    let mut rng = Rng(0x4E1D_4E1D ^ (wall_ns() as u64 | 1));
    let rig = Rig::new("rotation-held");
    let path = rig.folder.join("rotor.md");
    let mut base = String::new();
    let (mut waited, mut edits, mut lost_young, mut oldest_lost_ms) = (0usize, 0u64, 0usize, 0f64);
    for round in 0..rounds {
        // The first round puts a file there; from then on it is held.
        let holder = path.exists().then(|| fs::OpenOptions::new().read(true).share_mode(SHARE_READ_WRITE).open(&path).expect("the file can be opened for reading"));
        let cap = [24usize, 400][rng.below(2) as usize];
        let args = [[0u64, 100, 800][rng.below(3) as usize].to_string(), [1u64, 3, 20][rng.below(3) as usize].to_string(), cap.to_string(), rng.below(4).to_string()];
        let setting = format!("round {round}: arguments {args:?}, the file held: {}", holder.is_some());
        let ((start, acks, wrong), killed_at) = run_and_kill(&rotor, &rig, args, Duration::from_micros(4_000 + rng.below(150_000)));
        assert!(wrong.is_empty(), "{setting}: the helper saw what must never happen: {wrong:?}");
        assert_eq!(start, Some((base.len(), fnv(base.as_bytes()))), "{setting}: the helper started from a text this test does not expect");
        let left = fs::read(&path).ok().map(|bytes| String::from_utf8(bytes).unwrap_or_else(|_| panic!("{setting}: the file is not whole UTF-8")));
        if let Some(holder) = holder {
            assert_eq!(left.as_deref(), Some(base.as_str()), "{setting}: the held file was touched");
            let (store, report) = rig.open();
            assert!(store.shutdown(Duration::from_secs(10)), "{setting}: shutdown timed out");
            assert_eq!(fs::read_to_string(&path).ok().as_deref(), Some(base.as_str()), "{setting}: the held file was touched by recovery: {report:?}");
            assert!(report.iter().all(|told| told.how == Recovery::Waiting), "{setting}: with the file still held, recovery said {report:?}");
            assert_eq!(report.is_empty(), rig.journals().is_empty(), "{setting}: a journal and the report disagree: {:?} against {report:?}", rig.journals());
            waited += report.len();
            drop(holder);
        }
        let (store, report) = rig.open();
        assert!(store.shutdown(Duration::from_secs(10)), "{setting}: shutdown timed out");
        assert!(report.iter().all(|told| matches!(told.how, Recovery::BroughtBack | Recovery::WroteBack(_))), "{setting}: {report:?}");
        let required = acks.iter().filter(|&&at| at + WINDOW_NS <= killed_at).count();
        let here = format!("{setting} ({} edits acknowledged, {required} of them more than 100 ms before the kill; report {report:?})", acks.len());
        let listing = rig.listing();
        assert!(listing == ["rotor.md"] || (listing.is_empty() && base.is_empty() && required == 0), "{here}: the folder holds {listing:?}");
        assert!(rig.journals().is_empty(), "{here}: a journal outlived recovery: {:?}", rig.journals());
        let after = fs::read_to_string(&path).unwrap_or_default();
        let (first, last) = locate(&base, cap, acks.len() + 2, left.as_deref(), &after);
        assert!(left.is_none() || first.is_some(), "{here}: the file the kill left is not any state the text was in");
        let last = last.unwrap_or_else(|| panic!("{here}: the recovered text is not any state the text was in: {after:?}"));
        assert!(last >= required, "{here}: only {last} edits came back");
        if last < acks.len() {
            lost_young += 1;
            oldest_lost_ms = oldest_lost_ms.max(killed_at.saturating_sub(acks[last]) as f64 / 1e6);
        }
        edits += acks.len() as u64;
        base = after;
    }
    println!("held-save kill test: {rounds} kills, {edits} edits acknowledged; the first start after the kill reported a text waiting {waited} times; rounds that lost an acknowledged edit {lost_young} (oldest {oldest_lost_ms:.2} ms before the kill; the pass line is 100 ms)");
    assert!(rounds < 10 || waited > 0, "no round left a journal behind a held file: the test is not testing what it says");
}

#[test]
fn killed_while_the_journal_is_being_started_afresh_nothing_older_than_100_ms_is_lost() {
    let kills: usize = std::env::var("OK_ATTACK_ROTATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let per_worker = kills.div_ceil(WORKERS);
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..WORKERS).map(|index| scope.spawn(move || worker(index, per_worker))).collect();
        handles.into_iter().map(|handle| handle.join().expect("a kill worker failed")).collect()
    });
    let mut total = Tally::default();
    for tally in tallies {
        total.kills += tally.kills;
        total.edits += tally.edits;
        total.half_generation += tally.half_generation;
        total.two_generations += tally.two_generations;
        total.highest_generation = total.highest_generation.max(tally.highest_generation);
        total.mid_save += tally.mid_save;
        total.recovery_kills += tally.recovery_kills;
        total.lost_young += tally.lost_young;
        total.worth_counting += tally.worth_counting;
        total.oldest_lost_ms = total.oldest_lost_ms.max(tally.oldest_lost_ms);
    }
    println!(
        "rotation kill test: {} kills, {} edits acknowledged; the kill found half a journal generation in {}, two whole generations in {}, a save's temp file in {}; highest generation reached {}; recovery itself killed {} times; rounds that lost an acknowledged edit {} (oldest {:.2} ms before the kill; the pass line is 100 ms); holds and alarms the helper counted {}",
        total.kills, total.edits, total.half_generation, total.two_generations, total.mid_save, total.highest_generation, total.recovery_kills, total.lost_young, total.oldest_lost_ms, total.worth_counting
    );
    assert!(total.kills >= kills);
    if kills >= 90 {
        assert!(total.half_generation + total.two_generations > 0, "no kill landed inside a rotation: the test is not testing what it says");
    }
}
