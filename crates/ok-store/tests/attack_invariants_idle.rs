//! Do the worker threads ever spin? One test, alone in its process, so that
//! the processor time measured is the store's own: with a text open and
//! nothing happening, and in each state in which a save is being held back
//! and retried on a timer.

use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::time::Duration;

use ok_store::testing::{quick, save_wait, type_end, wait_news, Rig};
use ok_store::{Hold, News, Store, Tuning};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn GetProcessTimes(process: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
}

/// Processor time this process has used so far, kernel and user together.
fn cpu() -> Duration {
    let (mut created, mut exited, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: the four out pointers are valid FILETIME-sized values and the pseudo-handle is always valid.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) };
    assert!(ok != 0, "GetProcessTimes failed");
    Duration::from_nanos((kernel + user) * 100)
}

/// Sleeps for `time` without touching the store and returns the processor
/// time the whole process used meanwhile.
fn idle(time: Duration) -> Duration {
    let before = cpu();
    std::thread::sleep(time);
    cpu() - before
}

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

fn settle(store: &mut Store) {
    let mut seen = Vec::new();
    wait_news(store, &mut seen, Duration::from_millis(400), |_| false);
}

#[test]
fn the_worker_threads_use_no_processor_time_to_speak_of_while_they_wait() {
    const WINDOW: Duration = Duration::from_millis(1500);
    // A thread that spins would use the whole window; allow a fifth of it.
    const CEILING: Duration = Duration::from_millis(300);
    let mut worst = (Duration::ZERO, "");
    let mut check = |what: &'static str, used: Duration| {
        println!("{what}: {used:?} of processor time in {WINDOW:?}");
        if used > worst.0 {
            worst = (used, what);
        }
    };

    // The real waits, not the shortened ones: a text open, saved, nothing happening.
    {
        let rig = Rig::new("inv-idle-clean");
        let tuning = Tuning { scan_texts: true, ..Tuning::default() };
        let (mut store, _) = rig.open_with(tuning);
        let opened = store.create_text("essay.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved and left alone.");
        save_wait(&mut store, opened.doc, &doc, edit);
        settle(&mut store);
        check("default tuning, a clean text open", idle(WINDOW));
        store.set_active(false);
        settle(&mut store);
        check("default tuning, window inactive", idle(WINDOW));
        assert!(store.shutdown(SOON));
    }

    // Held because another program will not let the file be replaced: retried every 300 ms here.
    {
        let rig = Rig::new("inv-idle-busy");
        let (mut store, _) = rig.open();
        let opened = store.create_text("essay.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved.");
        save_wait(&mut store, opened.doc, &doc, edit);
        let holder = OpenOptions::new().read(true).share_mode(3).open(rig.folder.join("essay.md")).unwrap();
        let edit = type_end(&mut store, opened.doc, &mut doc, " More.");
        store.save(opened.doc, doc.to_text("\n"), edit);
        assert_eq!(expect(&mut store, "Held", |news| matches!(news, News::Held { .. })), News::Held { doc: opened.doc, why: Hold::Busy });
        settle(&mut store);
        check("held busy, a save retried every 300 ms", idle(WINDOW));
        drop(holder);
        expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
        assert!(store.shutdown(SOON));
    }

    // Held for an outside change nobody has answered.
    {
        let rig = Rig::new("inv-idle-outside");
        let (mut store, _) = rig.open();
        let opened = store.create_text("essay.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved.");
        save_wait(&mut store, opened.doc, &doc, edit);
        let edit = type_end(&mut store, opened.doc, &mut doc, " Mine.");
        fs::write(rig.folder.join("essay.md"), "Theirs.").unwrap();
        store.save(opened.doc, doc.to_text("\n"), edit);
        expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. }));
        store.save(opened.doc, doc.to_text("\n"), edit);
        settle(&mut store);
        check("held for an outside change, a save waiting", idle(WINDOW));
        assert!(store.shutdown(SOON));
    }

    // The file removed by another program, a save waiting.
    {
        let rig = Rig::new("inv-idle-removed");
        let (mut store, _) = rig.open();
        let opened = store.create_text("essay.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved.");
        save_wait(&mut store, opened.doc, &doc, edit);
        fs::remove_file(rig.folder.join("essay.md")).unwrap();
        let edit = type_end(&mut store, opened.doc, &mut doc, " More.");
        store.save(opened.doc, doc.to_text("\n"), edit);
        expect(&mut store, "Removed", |news| matches!(news, News::Removed { .. }));
        settle(&mut store);
        check("file removed outside, a save waiting", idle(WINDOW));
        assert!(store.shutdown(SOON));
    }

    // Opened while a writer had it: looked at again twice a second.
    {
        let rig = Rig::new("inv-idle-shown-busy");
        fs::write(rig.folder.join("essay.md"), "Being written by another program.").unwrap();
        let (mut store, _) = rig.open();
        let writer = OpenOptions::new().write(true).share_mode(7).open(rig.folder.join("essay.md")).unwrap();
        let opened = store.open_text("essay.md").unwrap();
        assert!(opened.read_only.is_some());
        settle(&mut store);
        check("opened while another program writes it", idle(WINDOW));
        drop(writer);
        assert!(store.shutdown(SOON));
    }

    // The journal cannot be written: retried once a second.
    {
        let rig = Rig::new("inv-idle-journal");
        let tuning = quick();
        let (mut store, _) = rig.open_with(tuning.clone());
        let opened = store.create_text("essay.md").unwrap();
        let mut doc = Doc::new();
        let edit = type_end(&mut store, opened.doc, &mut doc, "Saved.");
        save_wait(&mut store, opened.doc, &doc, edit);
        tuning.faults.journal_write.store(112, std::sync::atomic::Ordering::Relaxed);
        type_end(&mut store, opened.doc, &mut doc, " Typed onto a full disk.");
        expect(&mut store, "the alarm", |news| matches!(news, News::JournalAlarm(true)));
        settle(&mut store);
        check("journal disk full, retried once a second", idle(WINDOW));
        tuning.faults.journal_write.store(0, std::sync::atomic::Ordering::Relaxed);
        expect(&mut store, "the alarm going down", |news| matches!(news, News::JournalAlarm(false)));
        assert!(store.shutdown(SOON));
    }

    println!("most used: {:?} ({})", worst.0, worst.1);
    assert!(worst.0 < CEILING, "{}: {:?} of processor time in {WINDOW:?}", worst.1, worst.0);
}
