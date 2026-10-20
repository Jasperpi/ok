//! Invariant 6 of the corrected design, by experiment: no handle to a file in
//! his folder outlives one save or one read, and with the window inactive not
//! even the folder is held. Plus one case of a save left waiting for good.

// Holds files open the way Windows programs do; there is no such thing elsewhere.
#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig, Scratch};
use ok_store::{News, Setup, Store};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

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

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Opens every file under `folder` with no sharing at all, which Windows
/// allows only if nobody else has the file open in any way. The saver may be
/// in the middle of one read, so each file gets a few tries over 300 ms.
fn nobody_holds_anything(folder: &Path, when: &str) {
    let mut all = Vec::new();
    files_under(folder, &mut all);
    assert!(!all.is_empty());
    for path in all {
        let end = Instant::now() + Duration::from_millis(300);
        loop {
            match OpenOptions::new().read(true).write(true).share_mode(0).open(&path) {
                Ok(_) => break,
                Err(e) if Instant::now() >= end => panic!("{when}: {} is still held open ({e})", path.display()),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}

#[test]
fn no_handle_on_his_files_outlives_the_operation_that_needed_it() {
    let rig = Rig::new("inv-handles");
    fs::create_dir_all(rig.folder.join("College")).unwrap();
    fs::write(rig.folder.join("essay.md"), "An essay.").unwrap();
    fs::write(rig.folder.join("College").join("notes.txt"), "Some notes with a [[essay]] link.").unwrap();
    fs::write(rig.folder.join("plan.markdown"), "See [[essay]].").unwrap();
    let mut tuning = quick();
    tuning.scan_texts = true;
    let (mut store, _) = rig.open_with(tuning);

    // The first scan reads every text once.
    let mut texts = 0;
    while texts < 3 {
        expect(&mut store, "FileText", |news| matches!(news, News::FileText { .. }));
        texts += 1;
    }
    nobody_holds_anything(&rig.folder, "after the first scan");

    let opened = store.open_text("essay.md").unwrap();
    nobody_holds_anything(&rig.folder, "after opening a text");
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " More of it.");
    save_wait(&mut store, opened.doc, &doc, edit);
    nobody_holds_anything(&rig.folder, "after a save");

    store.keep(opened.doc, doc.to_text("\n"), edit, "kept");
    expect(&mut store, "Kept", |news| matches!(news, News::Kept { .. }));
    let versions = store.versions(opened.doc).unwrap();
    assert!(!versions.is_empty());
    store.read_version(opened.doc, &versions[0].name).unwrap();
    nobody_holds_anything(&rig.folder, "after keeping and reading versions");

    // An outside change is noticed and its text held in memory, not by a handle.
    fs::write(rig.folder.join("essay.md"), "Changed by another program.").unwrap();
    let News::ChangedOutside { text, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    nobody_holds_anything(&rig.folder, "while an outside change waits to be taken");
    let edit = replace_all(&mut store, opened.doc, &mut doc, &text);
    save_wait(&mut store, opened.doc, &doc, edit);

    store.rename("essay.md", r"College\essay.md").unwrap();
    nobody_holds_anything(&rig.folder, "after a rename");
    store.rewrite("plan.markdown", "See [[essay]].", r"See [[College\essay]].").unwrap();
    nobody_holds_anything(&rig.folder, "after a rewrite");
    store.close(opened.doc, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    let gone = store.discard("plan.markdown", None).unwrap();
    assert!(rig.folder.join(&gone).exists());
    let _ = listen(&mut store, Duration::from_millis(200));
    nobody_holds_anything(&rig.folder, "after a close and a discard");
    assert!(store.shutdown(SOON));
    nobody_holds_anything(&rig.folder, "after shutdown");
}

#[test]
fn with_the_window_inactive_a_folder_above_the_writing_folder_can_be_renamed() {
    let scratch = Scratch::new("inv-parent");
    let outer = scratch.path().join("Documents");
    let folder = outer.join("ok");
    let data = scratch.path().join("data");
    fs::create_dir_all(&folder).unwrap();
    let mut setup = Setup::new(folder.clone(), data);
    setup.tuning = quick();
    let (mut store, _) = Store::open(setup).unwrap();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "In the folder.");
    save_wait(&mut store, opened.doc, &doc, edit);

    // Active: the watch is held, and Windows then refuses to rename a parent.
    let moved = scratch.path().join("Documents renamed");
    let while_active = fs::rename(&outer, &moved);
    println!("renaming the parent while active: {:?}", while_active.as_ref().map_err(|e| e.raw_os_error()));
    if while_active.is_ok() {
        fs::rename(&moved, &outer).unwrap();
        let _ = listen(&mut store, Duration::from_millis(500));
    }

    store.set_active(false);
    let mut renamed = Err(None);
    let asked = Instant::now();
    while asked.elapsed() < Duration::from_secs(2) {
        renamed = fs::rename(&outer, &moved).map_err(|e| e.raw_os_error());
        if renamed.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("renaming the parent while inactive: {renamed:?} after {:?}", asked.elapsed());
    assert!(renamed.is_ok(), "something of the store's still holds the folder with the window inactive: {renamed:?}");

    // And the text is still saved, in the folder's new place, when he comes back.
    store.set_active(true);
    let edit = type_end(&mut store, opened.doc, &mut doc, " Typed after the parent was renamed.");
    store.save(opened.doc, doc.to_text("\n"), edit);
    if store.volume().open_by_id {
        let mut seen = Vec::new();
        let saved = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
        assert!(saved.is_some(), "{seen:?}");
        assert_eq!(fs::read_to_string(moved.join("ok").join("essay.md")).unwrap(), doc.to_text("\n"));
    }
    assert!(store.shutdown(SOON));
}

/// `Store::save`: "The answer is `News::Saved`, or `News::Held` with the
/// reason." A save that finds the file gone waits to learn whether it was
/// removed or renamed. When it turns out to have been renamed and is
/// followed, the waiting save is not tried again and nothing is said: `ok`
/// stays away until the interface happens to ask once more.
#[test]
fn a_save_waiting_while_an_outside_rename_is_followed_is_made_afterwards() {
    let rig = Rig::new("inv-rename-pending");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Named one thing.");
    save_wait(&mut store, opened.doc, &doc, edit);
    let _ = listen(&mut store, Duration::from_millis(200));

    // Renamed in Explorer while he is typing; the pause that follows asks for a save.
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Renamed in Explorer.md")).unwrap();
    let edit = type_end(&mut store, opened.doc, &mut doc, " Typed as it was being renamed.");
    store.save(opened.doc, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let moved = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Moved { .. }));
    assert!(moved.is_some(), "{seen:?}");
    let answered = wait_news(&mut store, &mut seen, Duration::from_secs(3), |news| matches!(news, News::Saved { edit: e, .. } if *e == edit) || matches!(news, News::Held { .. }));
    println!("news after the rename: {seen:?}");
    println!("the renamed file reads {:?}", fs::read_to_string(rig.folder.join("Renamed in Explorer.md")).unwrap_or_default());
    assert!(answered.is_some(), "three seconds after the rename was followed the waiting save is neither made nor reported held");
    assert!(store.shutdown(SOON));
}
