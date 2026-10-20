//! The kill test. A helper process runs the store with no window, applies a
//! stream of random edits and acknowledges each one on its standard output
//! only after `record` has returned for it. This test stops the helper with
//! TerminateProcess at a random instant (in the middle of saves and of
//! journal rotations, since the helper is told to do both constantly), then
//! starts the store again, which runs recovery, and checks:
//!
//! * the file in the folder, as the kill left it, is byte for byte one whole
//!   state the text really was in, never a mixture and never a part;
//! * after recovery the file holds every edit acknowledged more than 100 ms
//!   before the kill, and is again one whole state, not older than the file was;
//! * nothing but the text is left in the folder, and no journal is left.
//!
//! Every fifth round a second helper is killed in the middle of recovery
//! itself before the real recovery runs.
//!
//! The number of kills is `OK_STORE_KILLS`, 200 if unset.

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ok_store::testing::{headless_exe, Rig, Script};
use ok_store::Recovery;
use ok_text::Doc;

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

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

#[derive(Default)]
struct Tally {
    kills: usize,
    edits_acknowledged: u64,
    /// Rounds in which the journal was ahead of the file when the kill came.
    file_behind: usize,
    /// Rounds in which a save's temp file was lying in the folder: the kill landed inside a save.
    mid_save: usize,
    /// Rounds in which a half-made journal generation, or two generations at
    /// once, were lying there: the kill landed inside a rotation.
    mid_rotation: usize,
    /// Rounds in which an acknowledged edit (younger than the window) did not come back.
    lost_young: usize,
    /// The oldest acknowledged edit that did not come back, in milliseconds before the kill.
    oldest_lost_ms: f64,
    /// Rounds whose oldest lost edit was more than 10, 25 and 50 ms old.
    lost_older_than: [usize; 3],
    killed_in_recovery: usize,
}

/// Replays the script from `base` and finds where two texts fall in it: the
/// first step after which the text was `before`, and the last step (at or
/// after that one) after which it was `after`. Step 0 is `base` itself.
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

fn worker(index: usize, rounds: usize) -> Tally {
    let mut tally = Tally::default();
    let mut rng = Rng(0x5EED_0000 + index as u64 * 7919 + (wall_ns() as u64 | 1));
    let exe = headless_exe();
    assert!(exe.exists(), "the helper {} has not been built", exe.display());
    let mut rig = Rig::new(&format!("kill-{index}"));
    let mut base = String::new();
    let mut on_disk = false;
    for round in 0..rounds {
        if round % 8 == 0 && round > 0 {
            rig = Rig::new(&format!("kill-{index}"));
            base.clear();
            on_disk = false;
        }
        let path = rig.folder.join("essay.md");
        let seed = rng.below(u64::MAX) | 1;
        let pace = [20u64, 100, 400, 2000][rng.below(4) as usize];
        let save_ms = [1u64, 3, 10, 40][rng.below(4) as usize];
        let rotate = [1u64, 3000, 40_000][rng.below(3) as usize];
        let mut child = Command::new(&exe)
            .arg("run")
            .arg(&rig.folder)
            .arg(&rig.data)
            .args([seed.to_string(), pace.to_string(), save_ms.to_string(), rotate.to_string()])
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
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let mut parts = line.split_whitespace();
                match (parts.next(), parts.next(), parts.next()) {
                    (Some("B"), Some(len), Some(hash)) => {
                        start = Some((len.parse::<usize>().unwrap_or(usize::MAX), hash.parse::<u64>().unwrap_or(0)));
                        let _ = started_tx.send(true);
                    }
                    (Some("E"), Some(edit), Some(at)) => {
                        // A line cut short by the kill does not parse, or is out of step: stop there.
                        match (edit.parse::<usize>(), at.parse::<u128>()) {
                            (Ok(edit), Ok(at)) if edit == acks.len() + 1 && at > 1_000_000_000_000_000_000 => acks.push(at),
                            _ => break,
                        }
                    }
                    (Some("X"), ..) => {
                        let _ = started_tx.send(false);
                        panic!("the helper could not start: {line}");
                    }
                    _ => break,
                }
            }
            (start, acks)
        });
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(30)), Ok(true), "the helper never got going");
        std::thread::sleep(Duration::from_micros(2_000 + rng.below(300_000)));
        let killed_at = wall_ns();
        child.kill().expect("the helper can be killed");
        child.wait().expect("the helper ends");
        let (start, acks) = reader.join().expect("the reader thread ends");
        assert_eq!(start, Some((base.len(), fnv(base.as_bytes()))), "the helper started from a text the test does not expect");

        // What the kill left behind.
        let before = fs::read(&path).ok().map(|bytes| String::from_utf8(bytes).expect("the file is whole UTF-8"));
        let listing = rig.listing();
        if listing.iter().any(|name| name.ends_with(".tmp")) {
            tally.mid_save += 1;
        }
        let journals = rig.journals();
        if journals.len() > 1 || journals.iter().any(|name| name.ends_with(".part")) {
            tally.mid_rotation += 1;
        }
        assert!(before.is_some() || !on_disk, "round {round}: the file was there and is gone: {listing:?}");

        if round % 5 == 3 {
            // Kill a recovery part-way, then let the real one run.
            let mut second = Command::new(&exe).arg("recover").arg(&rig.folder).arg(&rig.data).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the helper starts");
            std::thread::sleep(Duration::from_micros(rng.below(40_000)));
            let _ = second.kill();
            let _ = second.wait();
            tally.killed_in_recovery += 1;
        }

        let (store, report) = rig.open();
        assert!(store.shutdown(Duration::from_secs(10)), "round {round}: shutdown after recovery timed out");
        for told in &report {
            let fine = matches!(told.how, Recovery::BroughtBack) || (!on_disk && matches!(&told.how, Recovery::WroteBack(rel) if rel.as_os_str() == "essay.md"));
            assert!(fine, "round {round}: recovery misjudged its own file: {report:?}");
        }
        let after = String::from_utf8(fs::read(&path).unwrap_or_default()).expect("the file is whole UTF-8");
        let listing = rig.listing();
        assert!(listing == ["essay.md"] || (listing.is_empty() && !on_disk && acks.is_empty()), "round {round}: something other than the text is in the folder: {listing:?}");
        assert!(rig.journals().is_empty(), "round {round}: a journal outlived recovery: {:?}", rig.journals());

        let (first, last) = locate(&base, seed, acks.len() + 2, before.as_deref(), &after);
        if let Some(before) = &before {
            assert!(first.is_some(), "round {round}: the file the kill left ({} bytes) is not any state the text was in (seed {seed})", before.len());
        }
        let last = last.unwrap_or_else(|| panic!("round {round}: the recovered text ({} bytes) is not any state the text was in at or after the file's (seed {seed}, {} acknowledged)", after.len(), acks.len()));
        let required = acks.iter().filter(|&&at| at + WINDOW_NS <= killed_at).count();
        assert!(last >= required, "round {round}: edit {required} was acknowledged more than 100 ms before the kill, but only {last} came back (seed {seed})");
        if first.is_some_and(|first| first < last) {
            tally.file_behind += 1;
        }
        if last < acks.len() {
            tally.lost_young += 1;
            let age_ms = killed_at.saturating_sub(acks[last]) as f64 / 1e6;
            tally.oldest_lost_ms = tally.oldest_lost_ms.max(age_ms);
            for (slot, limit) in tally.lost_older_than.iter_mut().zip([10.0, 25.0, 50.0]) {
                if age_ms > limit {
                    *slot += 1;
                }
            }
        }
        tally.kills += 1;
        tally.edits_acknowledged += acks.len() as u64;
        base = after;
        on_disk = path.exists();
    }
    tally
}

#[test]
fn killed_at_random_instants_nothing_older_than_100_ms_is_lost() {
    let kills: usize = std::env::var("OK_STORE_KILLS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let per_worker = kills.div_ceil(WORKERS);
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..WORKERS).map(|index| scope.spawn(move || worker(index, per_worker))).collect();
        handles.into_iter().map(|handle| handle.join().expect("a kill worker failed")).collect()
    });
    let mut total = Tally::default();
    for tally in tallies {
        total.kills += tally.kills;
        total.edits_acknowledged += tally.edits_acknowledged;
        total.file_behind += tally.file_behind;
        total.mid_save += tally.mid_save;
        total.mid_rotation += tally.mid_rotation;
        total.lost_young += tally.lost_young;
        total.killed_in_recovery += tally.killed_in_recovery;
        total.oldest_lost_ms = total.oldest_lost_ms.max(tally.oldest_lost_ms);
        for (slot, more) in total.lost_older_than.iter_mut().zip(tally.lost_older_than) {
            *slot += more;
        }
    }
    println!(
        "kill test: {} kills, {} edits acknowledged, journal ahead of the file in {}, a save's temp file present in {}, a journal rotation under way in {}, recovery itself killed {} times; rounds that lost any acknowledged edit: {} (older than 10 ms in {}, than 25 ms in {}, than 50 ms in {}; oldest {:.2} ms before the kill; the pass line is 100 ms)",
        total.kills,
        total.edits_acknowledged,
        total.file_behind,
        total.mid_save,
        total.mid_rotation,
        total.killed_in_recovery,
        total.lost_young,
        total.lost_older_than[0],
        total.lost_older_than[1],
        total.lost_older_than[2],
        total.oldest_lost_ms
    );
    assert!(total.kills >= kills);
    if kills >= 100 {
        assert!(total.mid_save > 0 && total.file_behind > 0, "no kill landed inside a save: the test is not testing what it says");
    }
}
