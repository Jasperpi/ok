//! The keystroke path while a worker thread is stuck or gone.
//!
//! The store has no fault switch that makes a worker stall or die, so these
//! tests use the one piece of outside code a worker runs: the wake callback.
//! A callback that sleeps on the journal thread stands in for a journal disk
//! that has stopped answering; one that sleeps on the saver thread stands in
//! for a network folder that hangs; one that panics stands in for any bug in
//! a worker. What is measured and asserted is what the thread that owns the
//! `Store` sees meanwhile.

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_store::testing::{quick, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, Hold, News, Store, StoreError, Tuning};
use ok_text::{Doc, Span};

const SOON: Duration = Duration::from_secs(10);

/// Switches a test flips to hold a worker inside the wake callback, or to
/// make the callback panic there once.
#[derive(Default)]
struct Gate {
    hold_saver: AtomicBool,
    hold_journal: AtomicBool,
    die_saver: AtomicBool,
    die_journal: AtomicBool,
    saver_in: AtomicBool,
    journal_in: AtomicBool,
    saver_died: AtomicBool,
    journal_died: AtomicBool,
}

impl Gate {
    fn wake(self: &Arc<Gate>) -> Arc<dyn Fn() + Send + Sync> {
        let gate = self.clone();
        Arc::new(move || {
            let thread = std::thread::current();
            let (hold, inside, die, died) = match thread.name() {
                Some("ok-saver") => (&gate.hold_saver, &gate.saver_in, &gate.die_saver, &gate.saver_died),
                Some("ok-journal") => (&gate.hold_journal, &gate.journal_in, &gate.die_journal, &gate.journal_died),
                _ => return,
            };
            if die.swap(false, Ordering::AcqRel) {
                died.store(true, Ordering::Release);
                panic!("a test makes the wake callback panic here, standing in for a bug in this worker");
            }
            if hold.load(Ordering::Acquire) {
                inside.store(true, Ordering::Release);
                while hold.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                inside.store(false, Ordering::Release);
            }
        })
    }
}

fn open(rig: &Rig, tuning: Tuning, gate: &Arc<Gate>) -> Store {
    let mut setup = rig.setup(tuning);
    setup.wake = gate.wake();
    Store::open(setup).expect("the store opens").0
}

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

fn listen(store: &mut Store, time: Duration) -> Vec<News> {
    let mut seen = Vec::new();
    wait_news(store, &mut seen, time, |_| false);
    seen
}

fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

fn until(what: &str, flag: &AtomicBool, store: &mut Store) {
    let end = Instant::now() + SOON;
    while !flag.load(Ordering::Acquire) {
        // Polling also clears the "already woken" mark, so the next news wakes again.
        let _ = store.poll();
        assert!(Instant::now() < end, "waited for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// How long calls took, in nanoseconds.
struct Stats {
    n: usize,
    mean: f64,
    p50: u64,
    p99: u64,
    p999: u64,
    max: u64,
}

fn stats(mut times: Vec<u64>) -> Stats {
    times.sort_unstable();
    let n = times.len().max(1);
    let at = |q: f64| times.get(((n as f64 * q) as usize).min(n - 1)).copied().unwrap_or(0);
    Stats { n: times.len(), mean: times.iter().sum::<u64>() as f64 / n as f64, p50: at(0.5), p99: at(0.99), p999: at(0.999), max: times.last().copied().unwrap_or(0) }
}

impl Stats {
    fn say(&self, what: &str) {
        println!(
            "{what}: {} calls, mean {:.2} us, p50 {:.2} us, p99 {:.2} us, p99.9 {:.2} us, worst {:.2} us",
            self.n,
            self.mean / 1e3,
            self.p50 as f64 / 1e3,
            self.p99 as f64 / 1e3,
            self.p999 as f64 / 1e3,
            self.max as f64 / 1e3
        );
    }

    /// The keystroke path's budget: microseconds on average, and no single
    /// call anywhere near the hundreds of milliseconds a wait on the stuck
    /// worker would cost.
    fn check(&self, what: &str) {
        assert!(self.mean < 50_000.0, "{what}: record() averaged {:.2} us", self.mean / 1e3);
        assert!(self.max < 100_000_000, "{what}: one record() took {:.2} ms", self.max as f64 / 1e6);
    }
}

/// Types single letters for `time`, timing every `record` on its own. Every
/// `save_every`th edit also asks for a save, as an interface under nonstop
/// typing would now and then.
fn typing(store: &mut Store, id: DocId, doc: &mut Doc, time: Duration, save_every: usize) -> (Stats, u64) {
    let mut times = Vec::with_capacity(400_000);
    let end = Instant::now() + time;
    let mut last = 0;
    let mut n = 0usize;
    while Instant::now() < end && times.len() < 400_000 {
        let piece = if n % 60 == 59 { "\n" } else if n % 6 == 5 { " " } else { "e" };
        let change = doc.replace(Span::at(doc.end()), piece);
        let len = doc.len_bytes();
        let one = Instant::now();
        last = store.record(id, &change, len);
        times.push(one.elapsed().as_nanos() as u64);
        n += 1;
        if save_every != 0 && n.is_multiple_of(save_every) {
            store.save(id, doc.to_text("\n"), last);
        }
        if n.is_multiple_of(64) {
            // Leave the workers some of the processor, as a typist would.
            let pause = Instant::now();
            while pause.elapsed() < Duration::from_micros(200) {
                std::hint::spin_loop();
            }
            while store.poll().is_some() {}
        }
    }
    (stats(times), last)
}

/// Holds the journal thread inside the wake callback with nothing reported
/// as failed: to the other threads it is a journal disk that has stopped
/// answering. Returns once it is held.
fn stall_journal(store: &mut Store, tuning: &Tuning, gate: &Arc<Gate>, id: DocId, doc: &mut Doc) {
    // The journal thread calls the wake itself only when its failed flag
    // changes. Make a write fail, then let the retry succeed and catch it there.
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    type_end(store, id, doc, "f");
    assert_eq!(expect(store, "the alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    gate.hold_journal.store(true, Ordering::Release);
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    until("the journal thread to be held", &gate.journal_in, store);
    // It published "caught up" just before it was held.
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = listen(store, Duration::from_millis(20));
}

#[test]
fn record_costs_microseconds_with_nothing_in_the_way() {
    let rig = Rig::new("inv-idle");
    let gate = Arc::new(Gate::default());
    let mut store = open(&rig, quick(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let (s, _) = typing(&mut store, id, &mut doc, Duration::from_millis(1500), 2000);
    s.say("record(), idle disk, a save every 2000 edits");
    s.check("idle");
    assert!(store.shutdown(SOON));
}

#[test]
fn record_costs_microseconds_while_the_saver_retries_a_file_another_program_holds() {
    let rig = Rig::new("inv-held");
    let gate = Arc::new(Gate::default());
    let mut store = open(&rig, quick(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let path = rig.folder.join("essay.md");
    // A reader that does not share delete: every save is refused at the rename.
    let stubborn = OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
    let (s, last) = typing(&mut store, id, &mut doc, Duration::from_millis(1500), 200);
    s.say("record(), saver retrying a held file, a save every 200 edits");
    assert_eq!(fs::read_to_string(&path).unwrap(), "Start.", "nothing may be written while it is held");
    drop(stubborn);
    store.save(id, doc.to_text("\n"), last);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit, .. } if *edit == last));
    assert_eq!(fs::read_to_string(&path).unwrap(), doc.to_text("\n"));
    s.check("held file");
    assert!(store.shutdown(SOON));
}

#[test]
fn a_stuck_saver_thread_costs_the_keystroke_thread_nothing_but_the_bound() {
    let rig = Rig::new("inv-stuck-saver");
    let gate = Arc::new(Gate::default());
    let mut tuning = quick();
    tuning.bound = Duration::from_millis(300);
    let mut store = open(&rig, tuning, &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let _ = listen(&mut store, Duration::from_millis(300));

    gate.hold_saver.store(true, Ordering::Release);
    let edit = type_end(&mut store, id, &mut doc, " One more.");
    store.save(id, doc.to_text("\n"), edit);
    until("the saver thread to be held", &gate.saver_in, &mut store);

    let (s, last) = typing(&mut store, id, &mut doc, Duration::from_millis(1000), 500);
    s.say("record(), saver thread stuck");

    // A call that needs the saver waits for the bound and no longer.
    let asked = Instant::now();
    let answer = store.open_text("other.md").err();
    let waited = asked.elapsed();
    println!("open_text with the saver stuck: {answer:?} after {waited:?} (bound 300 ms)");
    let asked = Instant::now();
    store.save(id, doc.to_text("\n"), last);
    let save_took = asked.elapsed();
    let asked = Instant::now();
    let _ = store.poll();
    let poll_took = asked.elapsed();
    println!("save() returned in {save_took:?}, poll() in {poll_took:?}");

    let asked = Instant::now();
    let finished = store.shutdown(Duration::from_millis(500));
    let shutdown_took = asked.elapsed();
    println!("shutdown(500 ms) with the saver stuck: {finished} after {shutdown_took:?}");
    gate.hold_saver.store(false, Ordering::Release);
    // In the program the process would end here. In a test the thread lives on: let it finish.
    std::thread::sleep(Duration::from_millis(800));

    s.check("stuck saver");
    assert_eq!(answer, Some(StoreError::Timeout));
    assert!(waited < Duration::from_millis(450), "open_text waited {waited:?}");
    assert!(save_took < Duration::from_millis(20) && poll_took < Duration::from_millis(20));
    assert!(!finished, "the saver never answered, so shutdown cannot have finished");
    assert!(shutdown_took < Duration::from_millis(700), "shutdown took {shutdown_took:?}");

    // Everything typed went to the journal all the while.
    let (store, recovered) = rig.open();
    println!("next start: {recovered:?}");
    assert_eq!(fs::read_to_string(rig.folder.join("essay.md")).unwrap(), doc.to_text("\n"));
    assert!(store.shutdown(SOON));
}

#[test]
fn a_stuck_journal_thread_raises_the_alarm_and_costs_the_keystroke_thread_nothing() {
    let rig = Rig::new("inv-stuck-journal");
    let gate = Arc::new(Gate::default());
    let mut tuning = quick();
    tuning.barrier = Duration::from_millis(400);
    let mut store = open(&rig, tuning.clone(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let path = rig.folder.join("essay.md");
    stall_journal(&mut store, &tuning, &gate, id, &mut doc);

    // Typing goes on. The alarm is the stall alarm: nothing has failed, the
    // journal has only stopped keeping up.
    let stalled = Instant::now();
    type_end(&mut store, id, &mut doc, " Typed into a stalled journal.");
    assert!(store.unwritten());
    let early = listen(&mut store, Duration::from_millis(120));
    assert!(!early.iter().any(|news| matches!(news, News::JournalAlarm(true))), "alarm before a quarter of a second: {early:?}");
    assert_eq!(expect(&mut store, "the stall alarm", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(true));
    println!("stall alarm raised {:?} after the first unwritten edit", stalled.elapsed());

    let (s, last) = typing(&mut store, id, &mut doc, Duration::from_millis(1000), 0);
    s.say("record(), journal thread stuck");

    // A save goes ahead once the barrier has waited its 400 ms. All that
    // time the saver holds its deny-write handle on his file.
    let asked = Instant::now();
    store.save(id, doc.to_text("\n"), last);
    let mut shut_out = Duration::ZERO;
    let mut seen = Vec::new();
    let saved = loop {
        if OpenOptions::new().write(true).share_mode(7).open(&path).is_err() {
            shut_out = asked.elapsed();
        }
        if let Some(news) = wait_news(&mut store, &mut seen, Duration::from_millis(2), |news| matches!(news, News::Saved { edit, .. } if *edit == last)) {
            break news;
        }
        assert!(asked.elapsed() < SOON, "no Saved with the journal stuck: {seen:?}");
    };
    println!("save with the journal stuck: {saved:?} after {:?}; another program's writes were refused for {shut_out:?} of that", asked.elapsed());
    assert_eq!(fs::read_to_string(&path).unwrap(), doc.to_text("\n"));

    gate.hold_journal.store(false, Ordering::Release);
    assert_eq!(expect(&mut store, "the alarm going down", |news| matches!(news, News::JournalAlarm(_))), News::JournalAlarm(false));
    // The alarm goes down at the first progress past the edit it was timing,
    // which can be a few milliseconds before the whole backlog is written.
    println!("when the alarm went down, unwritten() was {}", store.unwritten());
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up");
        std::thread::sleep(Duration::from_millis(1));
    }
    s.check("stuck journal");
    store.abandon();
    let (store, _) = rig.open();
    assert_eq!(fs::read_to_string(&path).unwrap(), doc.to_text("\n"));
    assert!(store.shutdown(SOON));
}

#[test]
fn shutdown_keeps_its_bound_with_the_journal_stuck_and_the_closed_text_still_reaches_its_file() {
    let rig = Rig::new("inv-stuck-exit");
    let gate = Arc::new(Gate::default());
    let mut tuning = quick();
    tuning.barrier = Duration::from_millis(200);
    let mut store = open(&rig, tuning.clone(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let path = rig.folder.join("essay.md");
    stall_journal(&mut store, &tuning, &gate, id, &mut doc);

    let edit = type_end(&mut store, id, &mut doc, " Typed just before leaving.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let asked = Instant::now();
    let finished = store.shutdown(Duration::from_millis(700));
    let took = asked.elapsed();
    let on_disk = fs::read_to_string(&path).unwrap();
    println!("shutdown(700 ms) with the journal stuck and a 200 ms barrier: {finished} after {took:?}; the file reads {on_disk:?}");
    gate.hold_journal.store(false, Ordering::Release);
    std::thread::sleep(Duration::from_millis(500));
    assert!(took < Duration::from_millis(900), "shutdown took {took:?}");
    assert!(!finished, "the journal never answered");
    assert_eq!(on_disk, doc.to_text("\n"), "with the journal stuck the file is the only place left, and the saver had the time");
}

/// `Store::save`: "The answer is `News::Saved`, or `News::Held` with the
/// reason." Once the saver thread is gone no request is ever answered and
/// nothing says so: the interface would simply never print `ok` again.
#[test]
fn a_saver_thread_that_dies_is_reported_to_the_interface() {
    let rig = Rig::new("inv-dead-saver");
    let gate = Arc::new(Gate::default());
    let mut store = open(&rig, quick(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let _ = listen(&mut store, Duration::from_millis(300));

    gate.die_saver.store(true, Ordering::Release);
    let edit = type_end(&mut store, id, &mut doc, " The save that the thread dies after.");
    store.save(id, doc.to_text("\n"), edit);
    until("the saver thread to die", &gate.saver_died, &mut store);
    let at_death = listen(&mut store, Duration::from_millis(200));
    println!("news around the death: {at_death:?}");

    let edit = type_end(&mut store, id, &mut doc, " Typed after the saver thread died.");
    store.save(id, doc.to_text("\n"), edit);
    let after = listen(&mut store, Duration::from_millis(1500));
    println!("news in the 1.5 s after asking a dead saver to save: {after:?}");
    let opening = store.open_text("other.md").err();
    println!("open_text answers {opening:?}; unwritten() is {}", store.unwritten());
    let asked = Instant::now();
    let finished = store.shutdown(Duration::from_secs(2));
    println!("shutdown(2 s) returned {finished} after {:?}", asked.elapsed());

    // Nothing is lost: the journal thread was never touched.
    let (store, recovered) = rig.open();
    println!("next start: {recovered:?}");
    assert_eq!(fs::read_to_string(rig.folder.join("essay.md")).unwrap(), doc.to_text("\n"));
    assert!(store.shutdown(SOON));

    assert_eq!(opening, Some(StoreError::Stopped));
    let told = after.iter().any(|news| matches!(news, News::Saved { .. } | News::Held { .. }));
    assert!(told, "a save asked of a dead saver thread was answered with nothing at all: {after:?}");
}

/// `News::Closed { in_journal: true }` after the journal thread has died: the
/// text is said to wait in a journal that nothing has written to since.
#[test]
fn text_closed_after_the_journal_thread_died_is_not_reported_as_safe_in_the_journal() {
    let rig = Rig::new("inv-dead-journal");
    let gate = Arc::new(Gate::default());
    let tuning = quick();
    let mut store = open(&rig, tuning.clone(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let path = rig.folder.join("essay.md");
    let _ = listen(&mut store, Duration::from_millis(300));

    // One failed journal write, and the thread dies telling the interface so.
    gate.die_journal.store(true, Ordering::Release);
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    type_end(&mut store, id, &mut doc, " a");
    until("the journal thread to die", &gate.journal_died, &mut store);
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(100));

    // What works: the alarm goes up at the next keystroke, and saves go on.
    let edit = type_end(&mut store, id, &mut doc, " Typed with no journal thread.");
    let alarm = expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(_)));
    assert_eq!(alarm, News::JournalAlarm(true));
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(fs::read_to_string(&path).unwrap(), doc.to_text("\n"));

    // Another program takes the file; he types on and switches texts.
    let holder = OpenOptions::new().write(true).share_mode(7).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " NO-JOURNAL words the file could not take.");
    store.save(id, doc.to_text("\n"), edit);
    let held = expect(&mut store, "Held", |news| matches!(news, News::Held { .. }));
    assert_eq!(held, News::Held { doc: id, why: Hold::Busy });
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    let closed = expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    println!("closing was answered with {closed:?}");
    drop(holder);

    let opened = store.open_text("essay.md");
    println!("opened again as {:?}", opened.as_ref().map(|o| (o.text.clone(), o.restored)));
    let finished = store.shutdown(Duration::from_secs(2));
    println!("shutdown returned {finished}");
    let (store, recovered) = rig.open();
    println!("next start: {recovered:?}; the file reads {:?}; the folder holds {:?}", fs::read_to_string(&path).unwrap_or_default(), rig.listing());
    assert!(store.shutdown(SOON));

    let said_in_journal = matches!(closed, News::Closed { in_journal: true, .. });
    let came_back = opened.is_ok_and(|o| o.text.contains("NO-JOURNAL"));
    assert!(!said_in_journal || came_back, "Closed said the text waits in the journal; no journal held it and it did not come back");
}

/// `StoreError::Timeout`: "The saver thread did not answer within
/// `Tuning::bound`. Nothing is lost; ask again." An open that times out is
/// taken back. A rename or a discard that times out is not: it stays queued
/// and is carried out whenever the saver gets to it, after the interface has
/// told him it did not happen.
#[test]
fn a_request_that_timed_out_is_not_carried_out_behind_the_callers_back() {
    let rig = Rig::new("inv-timeout");
    let gate = Arc::new(Gate::default());
    let mut tuning = quick();
    tuning.bound = Duration::from_millis(200);
    let mut store = open(&rig, tuning, &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    fs::write(rig.folder.join("other.md"), "Another text.").unwrap();
    fs::write(rig.folder.join("third.md"), "A third text.").unwrap();
    let _ = listen(&mut store, Duration::from_millis(300));

    gate.hold_saver.store(true, Ordering::Release);
    let edit = type_end(&mut store, id, &mut doc, " One more.");
    store.save(id, doc.to_text("
"), edit);
    until("the saver thread to be held", &gate.saver_in, &mut store);

    let renamed = store.rename("other.md", "renamed.md");
    let discarded = store.discard("third.md", None);
    println!("with the saver stuck: rename answered {renamed:?}, discard answered {discarded:?}");
    assert_eq!(renamed, Err(StoreError::Timeout));
    assert_eq!(discarded.as_ref().err(), Some(&StoreError::Timeout));

    // The saver comes back a moment later.
    gate.hold_saver.store(false, Ordering::Release);
    let _ = listen(&mut store, Duration::from_millis(600));
    let listing = rig.listing();
    println!("the folder afterwards: {listing:?}");
    assert!(store.shutdown(SOON));
    assert!(listing.contains(&"other.md".to_string()), "the rename that was reported as not done was done afterwards: {listing:?}");
    assert!(listing.contains(&"third.md".to_string()), "the discard that was reported as not done was done afterwards: {listing:?}");
}

/// A real stand-in for trouble on the journal disk: another program opens the
/// text's journal file and locks it, so every write the journal thread makes
/// to it is refused (error 33). The journal thread must start a new file
/// from what it holds in memory, typing must not feel it, and nothing typed
/// may be missing after a kill.
#[test]
fn a_journal_file_locked_by_another_program_costs_no_keystroke_and_no_text() {
    let rig = Rig::new("inv-locked-journal");
    let gate = Arc::new(Gate::default());
    let mut store = open(&rig, quick(), &gate);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    type_end(&mut store, id, &mut doc, " A little more, so the journal has a tail.");
    let _ = listen(&mut store, Duration::from_millis(200));
    let before = rig.journals();
    assert_eq!(before.len(), 1, "{before:?}");

    let locker = File::open(rig.data.join("journal").join(&before[0])).expect("another program can open the journal for reading");
    locker.lock().expect("and lock it");
    let (s, _) = typing(&mut store, id, &mut doc, Duration::from_millis(800), 0);
    s.say("record(), journal file locked by another program");
    let seen = listen(&mut store, Duration::from_millis(300));
    let alarms: Vec<&News> = seen.iter().filter(|news| matches!(news, News::JournalAlarm(_))).collect();
    println!("alarms while it was locked: {alarms:?}; journals now {:?}; unwritten() {}", rig.journals(), store.unwritten());
    drop(locker);
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up after its file was locked");
        std::thread::sleep(Duration::from_millis(1));
    }
    s.check("locked journal file");
    store.abandon();
    let (store, recovered) = rig.open();
    println!("next start: {} recovered, journals {:?}", recovered.len(), rig.journals());
    assert_eq!(fs::read_to_string(rig.folder.join("essay.md")).unwrap(), doc.to_text("
"));
    assert!(store.shutdown(SOON));
}
