//! The wake callback as the interface will use it: it sleeps in its message
//! loop and calls `poll` when a worker has woken it (and on a timer only
//! while `unwritten()` is true, as `poll` asks).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ok_store::testing::{quick, save_wait, type_end, Rig};
use ok_store::{DocId, News, Store, Tuning};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

/// Counts the wake calls by the thread that made them, and can hold the
/// journal thread inside one, which to everything else is a journal disk
/// that has stopped answering.
#[derive(Default)]
struct Bell {
    rings: AtomicUsize,
    from_saver: AtomicUsize,
    from_journal: AtomicUsize,
    from_elsewhere: AtomicUsize,
    hold_journal: AtomicBool,
    journal_in: AtomicBool,
}

impl Bell {
    fn wake(self: &Arc<Bell>) -> Arc<dyn Fn() + Send + Sync> {
        let bell = self.clone();
        Arc::new(move || {
            bell.rings.fetch_add(1, Ordering::AcqRel);
            match std::thread::current().name() {
                Some("ok-saver") => {
                    bell.from_saver.fetch_add(1, Ordering::AcqRel);
                }
                Some("ok-journal") => {
                    bell.from_journal.fetch_add(1, Ordering::AcqRel);
                    if bell.hold_journal.load(Ordering::Acquire) {
                        bell.journal_in.store(true, Ordering::Release);
                        while bell.hold_journal.load(Ordering::Acquire) {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        bell.journal_in.store(false, Ordering::Release);
                    }
                }
                _ => {
                    bell.from_elsewhere.fetch_add(1, Ordering::AcqRel);
                }
            }
        })
    }
}

fn open(rig: &Rig, tuning: Tuning, bell: &Arc<Bell>) -> Store {
    let mut setup = rig.setup(tuning);
    setup.wake = bell.wake();
    Store::open(setup).expect("the store opens").0
}

/// Polls until `poll` answers `None`, as an interface does when woken.
fn drain(store: &mut Store, into: &mut Vec<News>) {
    while let Some(news) = store.poll() {
        into.push(news);
    }
}

fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

/// An interface that polls only when woken must still see every `Saved`,
/// and the callback must come from the two worker threads and no other.
#[test]
fn polling_only_when_woken_sees_every_save() {
    let rig = Rig::new("inv-wake-only");
    let bell = Arc::new(Bell::default());
    let mut store = open(&rig, quick(), &bell);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let mut news = Vec::new();
    drain(&mut store, &mut news);

    let mut heard = bell.rings.load(Ordering::Acquire);
    let mut slowest = Duration::ZERO;
    for round in 0..300 {
        let edit = type_end(&mut store, id, &mut doc, "x");
        store.save(id, doc.to_text("\n"), edit);
        let asked = Instant::now();
        let mut got = false;
        while !got {
            // Asleep in the message loop until the bell rings.
            let rung = bell.rings.load(Ordering::Acquire);
            if rung == heard {
                assert!(asked.elapsed() < Duration::from_secs(3), "round {round}: Saved for edit {edit} was never announced by a wake");
                std::thread::sleep(Duration::from_micros(200));
                continue;
            }
            heard = rung;
            let mut batch = Vec::new();
            drain(&mut store, &mut batch);
            got = batch.iter().any(|n| matches!(n, News::Saved { edit: e, .. } if *e == edit));
        }
        slowest = slowest.max(asked.elapsed());
    }
    println!(
        "300 saves heard by wake alone; slowest {slowest:?}; wakes: {} from the saver, {} from the journal thread, {} from any other thread",
        bell.from_saver.load(Ordering::Acquire),
        bell.from_journal.load(Ordering::Acquire),
        bell.from_elsewhere.load(Ordering::Acquire)
    );
    assert_eq!(bell.from_elsewhere.load(Ordering::Acquire), 0, "the wake callback ran on a thread that is not a worker");
    assert!(store.shutdown(SOON));
}

/// What the interface needs: a wake "when there is news: ... the journal
/// alarm going up or down". The alarm for a journal that has stopped keeping
/// up exists only inside `poll`. `poll` asks to be called every quarter of a
/// second while `unwritten()` is true, which is how the alarm goes up. Once
/// the journal has caught up `unwritten()` is false, nothing asks for a poll
/// any more, and no worker wakes the interface: the alarm stays on the
/// screen until some unrelated news happens to arrive.
#[test]
fn the_stall_alarm_going_down_is_announced_by_a_wake() {
    let rig = Rig::new("inv-wake-alarm");
    let bell = Arc::new(Bell::default());
    let tuning = quick();
    let mut store = open(&rig, tuning.clone(), &bell);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let mut news = Vec::new();

    // Get the journal thread held inside a wake: fail one write, let the
    // retry succeed, and catch the call that announces the recovery.
    tuning.faults.journal_write.store(112, Ordering::Relaxed);
    type_end(&mut store, id, &mut doc, "f");
    let end = Instant::now() + SOON;
    while !news.contains(&News::JournalAlarm(true)) {
        drain(&mut store, &mut news);
        assert!(Instant::now() < end, "no alarm for a failed journal write: {news:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
    bell.hold_journal.store(true, Ordering::Release);
    tuning.faults.journal_write.store(0, Ordering::Relaxed);
    while !bell.journal_in.load(Ordering::Acquire) {
        drain(&mut store, &mut news);
        assert!(Instant::now() < end + SOON, "the journal thread never came back to be held");
        std::thread::sleep(Duration::from_millis(1));
    }
    while store.unwritten() {
        std::thread::sleep(Duration::from_millis(1));
    }
    drain(&mut store, &mut news);
    assert_eq!(news.iter().rev().find(|n| matches!(n, News::JournalAlarm(_))), Some(&News::JournalAlarm(false)), "{news:?}");

    // The journal disk stops answering while he types. The interface polls
    // on its quarter-second timer because unwritten() is true: alarm up.
    type_end(&mut store, id, &mut doc, " Typed into a stalled journal.");
    let mut up = Vec::new();
    let end = Instant::now() + SOON;
    while !up.contains(&News::JournalAlarm(true)) {
        assert!(store.unwritten());
        drain(&mut store, &mut up);
        assert!(Instant::now() < end, "no stall alarm: {up:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    drain(&mut store, &mut up);
    let rings_before = bell.rings.load(Ordering::Acquire);

    // The disk answers again. The interface is asleep: it polls when woken,
    // and on its timer only while unwritten() is true.
    bell.hold_journal.store(false, Ordering::Release);
    let end = Instant::now() + SOON;
    while store.unwritten() {
        assert!(Instant::now() < end, "the journal never caught up");
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(600));
    let rings_after = bell.rings.load(Ordering::Acquire);
    let mut down = Vec::new();
    drain(&mut store, &mut down);
    println!("the journal caught up; wakes since: {}; a poll made by hand then answered {down:?}", rings_after - rings_before);
    assert!(store.shutdown(SOON));
    assert!(down.contains(&News::JournalAlarm(false)), "{down:?}");
    assert!(rings_after > rings_before, "the alarm could go down 600 ms ago and no worker has woken the interface to say so");
}
