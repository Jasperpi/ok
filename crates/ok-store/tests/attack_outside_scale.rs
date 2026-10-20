//! Attack, from outside the crate: a writing folder that holds a few
//! thousand texts. The store is opened on it with the tuning the program
//! will run with, and from the first instant someone types into a new text,
//! saves it every forty words, and now and then opens another text, while
//! the store lists the folder and reads every text in it.
//!
//! Measured: how long the folder takes to open, to be listed and to be read;
//! what one `record` costs meanwhile; how long a save takes to be reported;
//! and the longest the saver thread kept a request waiting, which is how
//! long `open_text` (called from the thread that handles keystrokes) would
//! have frozen the window.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ok_store::testing::Rig;
use ok_store::{News, Store, StoreError, Tuning};
use ok_text::{Doc, Span};

const EACH: usize = 100;

/// How many folders of a hundred texts to make: forty unless
/// `OK_ATTACK_FOLDERS` says otherwise.
fn folders() -> usize {
    std::env::var("OK_ATTACK_FOLDERS").ok().and_then(|n| n.parse().ok()).unwrap_or(40)
}

fn rel_of(n: usize) -> PathBuf {
    PathBuf::from(format!("Folder {:03}\\Text {:05}.md", n / EACH, n))
}

/// Asks the saver thread something it can answer without touching a disk
/// much, and returns how long the answer took: how long it was busy.
fn stall(store: &Store, id: ok_store::DocId, timeouts: &mut u32) -> Duration {
    let asked = Instant::now();
    if store.versions(id).err() == Some(StoreError::Timeout) {
        *timeouts += 1;
    }
    asked.elapsed()
}

fn longest(times: &[Duration]) -> Duration {
    times.iter().copied().max().unwrap_or_default()
}

#[test]
fn a_folder_of_a_few_thousand_texts_keeps_nothing_waiting() {
    let rig = Rig::new("ao-scale");
    let (folders, texts) = (folders(), folders() * EACH);
    let body = "A paragraph of an essay that links to [[Text 00001]] and to [[Text 00002]] and goes on a while.\n\n".repeat(16);
    for folder in 0..folders {
        fs::create_dir_all(rig.folder.join(format!("Folder {folder:03}"))).unwrap();
    }
    for n in 0..texts {
        fs::write(rig.folder.join(rel_of(n)), format!("# Text {n:05}\n\n{body}")).unwrap();
    }

    let started = Instant::now();
    let (mut store, _) = rig.open_with(Tuning::default());
    let open_took = started.elapsed();
    let create = Instant::now();
    let opened = store.create_text("Today.md").expect("a new text can be started at once");
    let create_took = create.elapsed();
    let id = opened.doc;
    let mut doc = Doc::new();

    let mut read: HashSet<PathBuf> = HashSet::new();
    let (mut first_tree, mut all_listed, mut all_read) = (None, None, None);
    let mut trees = 0u32;
    let mut records: Vec<Duration> = Vec::new();
    let mut stalls: Vec<Duration> = Vec::new();
    let mut opens: Vec<Duration> = Vec::new();
    let mut saves: Vec<Duration> = Vec::new();
    let mut asked: Option<(u64, Instant)> = None;
    let (mut timeouts, mut open_failures) = (0u32, Vec::new());
    let mut words = 0usize;
    let mut latest;
    let limit = Duration::from_secs(240);
    while started.elapsed() < limit {
        // One keystroke.
        let change = doc.replace(Span::at(doc.end()), "word ");
        let len = doc.len_bytes();
        let at = Instant::now();
        latest = store.record(id, &change, len);
        records.push(at.elapsed());
        words += 1;
        if words % 40 == 0 && asked.is_none() {
            store.save(id, doc.to_text("\n"), latest);
            asked = Some((latest, Instant::now()));
        }
        if words % 3 == 0 {
            stalls.push(stall(&store, id, &mut timeouts));
        }
        if words % 150 == 0 {
            // He goes to another text for a moment.
            let rel = rel_of((words * 7) % texts);
            let at = Instant::now();
            match store.open_text(&rel) {
                Ok(other) => {
                    opens.push(at.elapsed());
                    store.close(other.doc, None, None);
                }
                Err(error) => {
                    opens.push(at.elapsed());
                    open_failures.push(format!("{rel:?}: {error:?} after {:?}", at.elapsed()));
                }
            }
        }
        while let Some(news) = store.poll() {
            match news {
                News::Tree(tree) => {
                    trees += 1;
                    first_tree.get_or_insert(started.elapsed());
                    if tree.texts().len() >= texts {
                        all_listed.get_or_insert(started.elapsed());
                    }
                }
                News::FileText { rel, .. } => {
                    read.insert(rel);
                    if read.len() >= texts {
                        all_read.get_or_insert(started.elapsed());
                    }
                }
                News::Saved { edit, .. } => {
                    if let Some((wanted, at)) = asked {
                        if edit >= wanted {
                            saves.push(at.elapsed());
                            asked = None;
                        }
                    }
                }
                _ => {}
            }
        }
        if all_read.is_some() && asked.is_none() && words > 600 {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let scanning = (records.len(), stalls.len(), opens.len(), saves.len());

    // Afterwards, with everything read: what one change in the folder costs.
    // Another program rewrites one text; then we save ours.
    let mut after_outside = Vec::new();
    fs::write(rig.folder.join(rel_of(17)), "rewritten by another program").unwrap();
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        after_outside.push(stall(&store, id, &mut timeouts));
        while store.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut after_save = Vec::new();
    let change = doc.replace(Span::at(doc.end()), "last ");
    latest = store.record(id, &change, doc.len_bytes());
    store.save(id, doc.to_text("\n"), latest);
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        after_save.push(stall(&store, id, &mut timeouts));
        while store.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(1));
    }

    let mean_record = records.iter().sum::<Duration>() / records.len().max(1) as u32;
    let mut sorted = stalls.clone();
    sorted.sort();
    let over = |ms: u64| stalls.iter().filter(|stall| **stall > Duration::from_millis(ms)).count();
    println!("{texts} texts in {folders} folders, {} bytes each", body.len() + 15);
    println!("Store::open returned after          {open_took:?}");
    println!("create_text right after took        {create_took:?}");
    println!("first listing arrived after         {first_tree:?}");
    println!("a listing with every text after     {all_listed:?}");
    println!("every text had been read after      {all_read:?} ({} of {texts} read; {trees} listings sent)", read.len());
    println!("record(): {} calls, mean {mean_record:?}, longest {:?}", scanning.0, longest(&records));
    println!("saver busy, sampled {} times: median {:?}, longest {:?}; over 50 ms {} times, over 250 ms {} times, over 1 s {} times", scanning.1, sorted.get(sorted.len() / 2).copied().unwrap_or_default(), longest(&stalls), over(50), over(250), over(1000));
    println!("open_text of another text, {} times: longest {:?}", scanning.2, longest(&opens));
    println!("save asked to Saved, {} saves: longest {:?}", scanning.3, longest(&saves));
    println!("afterwards, another program rewrites one text: saver busy for at most {:?}", longest(&after_outside));
    println!("afterwards, one save of ours: saver busy for at most {:?}", longest(&after_save));
    println!("requests that ran out of time: {timeouts}; texts that would not open: {open_failures:?}");
    assert!(store.shutdown(Duration::from_secs(10)));

    assert!(mean_record < Duration::from_micros(50), "record() cost {mean_record:?} on average");
    assert!(all_read.is_some(), "after {limit:?} only {} of {texts} texts had been read", read.len());
    assert_eq!(timeouts, 0, "requests were kept waiting past the store's own bound");
    assert!(open_failures.is_empty(), "{open_failures:?}");
    // A quarter of a second is the pause after which the page is to show the
    // text under the bar (DESIGN.md, the line); a request kept waiting longer
    // than that freezes the window he is typing in.
    let worst = longest(&stalls).max(longest(&opens)).max(longest(&after_outside)).max(longest(&after_save));
    assert!(worst < Duration::from_millis(250), "the saver thread kept a request from the keystroke thread waiting {worst:?}");
}
