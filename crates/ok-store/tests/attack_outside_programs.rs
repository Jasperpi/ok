//! Attack, from outside the crate: other programs and Explorer at his
//! folder and at the store's own files. A text that opens read-only; a
//! folder marked read-only; a link; a text opened while another program is
//! half way through rewriting it; an editor that deletes and makes the file
//! again; the folder renamed in Explorer and then a kill; a second copy of
//! the program reaching the folder by another spelling; a scanner that has
//! a journal open at the wrong moment; a file whose bytes are not on this
//! disk; and a sync client updating a text nobody is typing in.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ok_store::testing::{quick, replace_all, save_wait, type_end, wait_news, Rig};
use ok_store::{DocId, Hold, News, OpenError, ReadOnly, Recovery, Resolve, Setup, Store, StoreError};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);
const SHARE_ALL: u32 = 7;
const ATTR_OFFLINE: u32 = 0x1000;

#[link(name = "kernel32")]
extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;
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

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).expect("the file can be read")).expect("the file is UTF-8")
}

/// A new text holding `text`, saved.
fn written(store: &mut Store, rel: &str, text: &str) -> (DocId, Doc, u64) {
    let opened = store.create_text(rel).expect("the text can be started");
    let mut doc = Doc::new();
    let edit = type_end(store, opened.doc, &mut doc, text);
    save_wait(store, opened.doc, &doc, edit);
    (opened.doc, doc, edit)
}

fn closed(store: &mut Store, id: DocId) -> News {
    expect(store, "Closed", |news| matches!(news, News::Closed { doc, .. } if *doc == id))
}

fn set_read_only(path: &Path, on: bool) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(on);
    fs::set_permissions(path, perms).unwrap();
}

/// Adds attribute bits to a file, as `attrib` would.
fn add_attributes(path: &Path, bits: u32) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is nul-terminated and outlives both calls.
    unsafe {
        let now = GetFileAttributesW(wide.as_ptr());
        assert_ne!(now, u32::MAX, "the attributes of {path:?} can be read");
        assert_ne!(SetFileAttributesW(wide.as_ptr(), now | bits), 0, "the attributes of {path:?} can be set");
    }
}

/// True if `needle` is in any file under `dir`, whatever kind of file it is.
fn anywhere(dir: &Path, needle: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            if anywhere(&path, needle) {
                return true;
            }
        } else if let Ok(bytes) = fs::read(&path) {
            if bytes.windows(needle.len()).any(|window| window == needle.as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// Opens every journal file under the rig's data directory the way a
/// scanner, an indexer or a backup tool would: for reading, sharing
/// everything, in nobody's way as far as it knows.
fn scan_journals(rig: &Rig) -> Vec<File> {
    let dir = rig.data.join("journal");
    let scanners: Vec<File> = rig.journals().iter().map(|name| OpenOptions::new().read(true).share_mode(SHARE_ALL).open(dir.join(name)).expect("a journal can be opened for reading")).collect();
    assert!(!scanners.is_empty(), "there is a journal to scan");
    scanners
}

// ------------------------------------------- a text that opens read-only (fail)

/// `Store::save`: "The answer is `News::Saved`, or `News::Held` with the
/// reason." `Store::keep`: "If the save cannot be made the answer is
/// `News::Held` instead." A text whose file carries the read-only attribute,
/// or has a second name, opens saying so; he types anyway and Ctrl S is
/// pressed. The request must be answered, as it is for a file that is not
/// UTF-8. It is met with silence, so the interface cannot print the one
/// sentence it has for this.
#[test]
fn a_save_asked_of_a_text_that_opened_read_only_is_answered() {
    let rig = Rig::new("ao-ro-answer");
    let attribute = rig.folder.join("attribute.md");
    let linked = rig.folder.join("linked.md");
    fs::write(&attribute, "Read-only by attribute.").unwrap();
    fs::write(&linked, "One file, two names.").unwrap();
    set_read_only(&attribute, true);
    let has_links = fs::hard_link(&linked, rig.folder.join("its other name.md")).is_ok();
    let (mut store, _) = rig.open();
    let mut unanswered = Vec::new();
    for (name, why, reason) in [("attribute.md", ReadOnly::Attribute, Hold::ReadOnly), ("linked.md", ReadOnly::Linked, Hold::Linked)] {
        if why == ReadOnly::Linked && !has_links {
            continue;
        }
        let opened = store.open_text(name).unwrap();
        assert_eq!(opened.read_only, Some(why));
        let mut doc = Doc::from_text(&opened.text);
        let edit = type_end(&mut store, opened.doc, &mut doc, " Typed anyway.");
        for (call, keep) in [("save", false), ("keep", true)] {
            if keep {
                store.keep(opened.doc, doc.to_text("\n"), edit, "kept");
            } else {
                store.save(opened.doc, doc.to_text("\n"), edit);
            }
            let mut seen = Vec::new();
            let answer = wait_news(&mut store, &mut seen, Duration::from_secs(2), |news| matches!(news, News::Held { doc, .. } | News::Saved { doc, .. } if *doc == opened.doc));
            match answer {
                Some(answer) => assert_eq!(answer, News::Held { doc: opened.doc, why: reason }),
                None => unanswered.push(format!("{call}() on {name} (opened {why:?}): no answer in two seconds; all that came was {seen:?}")),
            }
        }
        assert_eq!(read(&rig.folder.join(name)), opened.text, "and the file is never touched");
    }
    assert!(store.shutdown(SOON));
    set_read_only(&attribute, false);
    assert!(unanswered.is_empty(), "{unanswered:#?}");
}

/// A held save is to be tried again "at once on focus change" (research
/// design, save step 9; corrected design 3h). A scanner keeps the file from
/// being replaced until the quick retries are spent, then lets go; he
/// comes back to the window. The save must go through then, not at the next
/// slow retry a quarter of a minute later.
#[test]
fn a_held_save_is_tried_again_when_the_window_comes_back() {
    let rig = Rig::new("ao-focus-retry");
    let mut tuning = quick();
    tuning.retry_first = Duration::from_secs(30);
    tuning.retry_every = Duration::from_secs(30);
    let (mut store, _) = rig.open_with(tuning);
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let path = rig.folder.join("essay.md");
    let scanner = OpenOptions::new().read(true).share_mode(1 | 2).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed while a scanner had the file.");
    store.save(id, doc.to_text("\n"), edit);
    assert_eq!(expect(&mut store, "Held Busy", |news| matches!(news, News::Held { .. })), News::Held { doc: id, why: Hold::Busy });
    drop(scanner);
    store.set_active(false);
    store.set_active(true);
    let mut seen = Vec::new();
    let saved = wait_news(&mut store, &mut seen, Duration::from_secs(3), |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert!(saved.is_some(), "three seconds after the scanner let go and the window came back the text is still not saved; news was {seen:?}");
    assert!(store.shutdown(SOON));
}

// ------------------------------------------- a folder marked read-only

/// Windows marks folders read-only for reasons of its own (Documents
/// carries the attribute on many machines). It does not stop files being
/// made in them, and it must not stop the store.
#[test]
fn a_folder_carrying_the_read_only_attribute_takes_everything() {
    let rig = Rig::new("ao-ro-folder");
    fs::create_dir_all(rig.folder.join("Drafts")).unwrap();
    set_read_only(&rig.folder, true);
    set_read_only(&rig.folder.join("Drafts"), true);
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "Drafts\\essay.md", "In a folder marked read-only.");
    let edit = type_end(&mut store, id, &mut doc, " Mine.");
    fs::write(rig.folder.join("Drafts").join("essay.md"), "Theirs.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. }));
    store.resolve(id, Resolve::KeepBoth { text: doc.to_text("\n"), edit });
    let News::KeptBeside { rel, .. } = expect(&mut store, "KeptBeside", |news| matches!(news, News::KeptBeside { .. })) else { unreachable!() };
    assert_eq!(read(&rig.folder.join(rel)), "In a folder marked read-only. Mine.");
    let edit = replace_all(&mut store, id, &mut doc, "Theirs.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(store.rename("Drafts\\essay.md", "Drafts\\Sub\\renamed.md"), Ok(()));
    let edit = type_end(&mut store, id, &mut doc, " And more.");
    let went = store.discard("Drafts\\Sub\\renamed.md", Some((doc.to_text("\n"), edit))).expect("discarded");
    closed(&mut store, id);
    assert_eq!(read(&rig.folder.join(went)), "Theirs. And more.");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
    set_read_only(&rig.folder.join("Drafts"), false);
    set_read_only(&rig.folder, false);
}

// ------------------------------------------- a link

/// Research design, section 1: "Document paths are canonicalized at open so
/// a symlink is saved through, not replaced", and its test list: "a
/// symlinked file (the link must survive a save)". The text in the folder is
/// a link to a file kept somewhere else. A save must change that file and
/// leave the link a link.
///
/// Making a link to a file needs a privilege this machine does not give an
/// ordinary process; where it cannot be made the test says so and stops.
#[test]
fn a_text_that_is_a_link_is_saved_through_and_stays_a_link() {
    let rig = Rig::new("ao-symlink");
    let elsewhere = rig.data.with_file_name("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let real = elsewhere.join("the real file.md");
    let link = rig.folder.join("essay.md");
    fs::write(&real, "Kept somewhere else.").unwrap();
    if let Err(error) = std::os::windows::fs::symlink_file(&real, &link) {
        println!("NOT RUN: a link to a file could not be made here ({error})");
        return;
    }
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    assert_eq!(opened.text, "Kept somewhere else.");
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " And typed here.");
    store.save(opened.doc, doc.to_text("\n"), edit);
    let answer = expect(&mut store, "Saved or Held", |news| matches!(news, News::Saved { .. } | News::Held { .. }));
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link was replaced by an ordinary file; the store said {answer:?}");
    if matches!(answer, News::Saved { .. }) {
        assert_eq!(read(&real), "Kept somewhere else. And typed here.", "the store said saved, and the file the link leads to was not");
    }
    assert!(store.shutdown(SOON));
}

// ------------------------- opened while another program is rewriting it (fail)

/// Corrected design, flaw 12 and its test: "Read during a truncate window
/// never loads an empty text." Another program has truncated the file and
/// not yet written it again when the text is opened. Opening may say the
/// file is busy; it must not hand over an empty text as the text.
#[test]
fn a_text_opened_while_another_program_is_rewriting_it_is_never_shown_empty() {
    let rig = Rig::new("ao-open-truncated");
    let path = rig.folder.join("essay.md");
    fs::write(&path, "The whole essay, as it was before the other program began.").unwrap();
    let (mut store, _) = rig.open();
    let mut writer = OpenOptions::new().write(true).share_mode(SHARE_ALL).open(&path).unwrap();
    writer.set_len(0).unwrap();
    let result = store.open_text("essay.md");
    writer.write_all(b"The essay as the other program rewrote it.").unwrap();
    drop(writer);
    match result {
        Err(StoreError::Busy) => {}
        Ok(opened) => {
            let after = listen(&mut store, Duration::from_millis(900));
            let versions: Vec<(Option<String>, String)> = store.versions(opened.doc).unwrap().iter().map(|v| (v.label.clone(), store.read_version(opened.doc, &v.name).unwrap())).collect();
            assert!(
                !opened.text.is_empty(),
                "the text was handed over empty (read_only {:?}); what followed: {after:?}; versions kept meanwhile: {versions:?}",
                opened.read_only
            );
        }
        Err(other) => panic!("{other:?}"),
    }
    assert!(store.shutdown(SOON));
}

// ------------------------- deleted and made again by an editor (fail)

/// `News::Reappeared`: "A removed file is back under its name." Some
/// editors save by deleting the file and making it again. If the store
/// happens to look in between, it must either say the file was removed and
/// then that it is back, or say neither; it must not announce the return of
/// a file it never said had gone.
#[test]
fn an_editor_that_deletes_and_makes_the_file_again_is_not_announced_as_a_return() {
    let rig = Rig::new("ao-delete-recreate");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "As we saved it.");
    let path = rig.folder.join("essay.md");
    listen(&mut store, Duration::from_millis(150));
    fs::remove_file(&path).unwrap();
    // Longer than the settle after a folder notification, shorter than the
    // wait before a file is called removed.
    std::thread::sleep(Duration::from_millis(100));
    fs::write(&path, "As the editor saved it.").unwrap();
    let news = listen(&mut store, Duration::from_millis(800));
    let removed = news.iter().position(|n| matches!(n, News::Removed { .. }));
    let back = news.iter().position(|n| matches!(n, News::Reappeared { .. }));
    let News::ChangedOutside { text, clean, .. } = news.iter().find(|n| matches!(n, News::ChangedOutside { .. })).expect("the new contents are announced").clone() else { unreachable!() };
    assert_eq!((text.as_str(), clean), ("As the editor saved it.", true));
    let edit = replace_all(&mut store, id, &mut doc, &text);
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&path), "As the editor saved it.");
    assert!(store.shutdown(SOON));
    assert!(back.is_none() || removed.is_some_and(|at| at < back.unwrap()), "Reappeared without Removed: {news:?}");
}

// ------------------------- the folder renamed in Explorer, then a kill (fail)

/// He renames the writing folder in Explorer while a text is open. The
/// store finds the folder again by its file index and says so
/// (`News::FolderMoved`); saving carries on. He types a sentence and the
/// process is killed before the next save. At the next start, on the folder
/// under its new name, the sentence must be brought back into the text's
/// file, as after any other kill.
#[test]
fn the_folder_renamed_in_explorer_and_then_a_kill_brings_the_text_back_into_its_file() {
    let rig = Rig::new("ao-folder-kill");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "In the old folder.");
    let moved = rig.folder.with_file_name("writing renamed");
    let mut renamed = false;
    for _ in 0..100 {
        if fs::rename(&rig.folder, &moved).is_ok() {
            renamed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(renamed, "the folder could not be renamed while the store was idle");
    let edit = type_end(&mut store, id, &mut doc, " Saved after it moved.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit)).is_some(), "{seen:?}");
    let Some(News::FolderMoved { folder }) = seen.iter().find(|news| matches!(news, News::FolderMoved { .. })).cloned() else { panic!("{seen:?}") };
    type_end(&mut store, id, &mut doc, " Typed after that and never saved.");
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    // The interface was told where the folder went and opens it there.
    let (store, recovered) = Store::open(Setup { folder: folder.clone(), ..rig.setup(quick()) }).expect("the store opens on the renamed folder");
    let file = read(&moved.join("essay.md"));
    assert!(store.shutdown(SOON));
    assert_eq!(file, "In the old folder. Saved after it moved. Typed after that and never saved.", "recovery did {recovered:?}");
    assert!(matches!(recovered.as_slice(), [one] if one.how == Recovery::BroughtBack), "{recovered:?}");
}

/// The folder above the writing folder is renamed while the window is not
/// the active one (a watch held on the folder would stop that, which is why
/// `set_active(false)` drops it). The store must let the rename happen, find
/// the folder again and carry on.
#[test]
fn a_parent_folder_renamed_while_the_window_is_inactive_is_followed() {
    let rig = Rig::new("ao-parent");
    let parent = rig.folder.with_file_name("parent");
    let folder = parent.join("writing");
    fs::create_dir_all(&folder).unwrap();
    let (mut store, _) = Store::open(Setup { folder: folder.clone(), ..rig.setup(quick()) }).expect("the store opens");
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Under the old parent.");
    store.set_active(false);
    let moved = rig.folder.with_file_name("parent renamed");
    let started = Instant::now();
    let mut renamed = false;
    while started.elapsed() < Duration::from_secs(2) {
        if fs::rename(&parent, &moved).is_ok() {
            renamed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(renamed, "two seconds after the window went inactive the folder above still could not be renamed");
    let edit = type_end(&mut store, id, &mut doc, " Typed after it moved.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit)).is_some(), "{seen:?}");
    assert!(seen.iter().any(|news| matches!(news, News::FolderMoved { .. })), "{seen:?}");
    assert_eq!(read(&moved.join("writing").join("essay.md")), "Under the old parent. Typed after it moved.");
    store.close(id, None, None);
    closed(&mut store, id);
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

/// After the store has followed the folder to its new name, a second launch
/// on the folder (by that new name, which is the only one it has) must
/// still be told who has it. The lock was taken under the old name.
#[test]
fn after_the_folder_is_renamed_a_second_copy_is_still_told_who_has_it() {
    let rig = Rig::new("ao-folder-lock");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "In the old folder.");
    let moved = rig.folder.with_file_name("writing renamed");
    let mut renamed = false;
    for _ in 0..100 {
        if fs::rename(&rig.folder, &moved).is_ok() {
            renamed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(renamed, "the folder could not be renamed while the store was idle");
    let edit = type_end(&mut store, id, &mut doc, " Saved after it moved.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    assert!(wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit)).is_some(), "{seen:?}");
    let Some(News::FolderMoved { folder }) = seen.iter().find(|news| matches!(news, News::FolderMoved { .. })).cloned() else { panic!("{seen:?}") };
    type_end(&mut store, id, &mut doc, " Typed in the first copy and not yet saved.");
    store.flush();

    let second = Store::open(Setup { folder: folder.clone(), ..rig.setup(quick()) });
    let told = match second {
        Err(OpenError::Locked(owner)) => Ok(owner),
        Err(other) => Err(format!("{other:?}")),
        Ok((second, recovered)) => {
            second.abandon();
            Err(format!("a second store opened on {folder:?} while the first still has it; its recovery did {recovered:?}"))
        }
    };
    let where_it_thinks = store.folder().to_path_buf();
    assert!(store.shutdown(SOON));
    assert_eq!(told.map(|owner| owner.pid), Ok(std::process::id()));
    assert_eq!(where_it_thinks, folder, "and Store::folder() still gives the name the folder no longer has");
}

// ------------------------- renamed in Explorer while a save is under way

/// He renames the text in Explorer at the moment a save of it is under way.
/// Whichever comes first, there must be one file afterwards, under the name
/// he gave it, holding everything typed. If his rename lands between the
/// store's last look at the name and its own rename, the store writes the
/// new text under the old name as a file of its own: two files, the one he
/// renamed left stale, and the store working on in the other.
#[test]
fn a_text_renamed_in_explorer_while_a_save_is_under_way_is_never_left_as_two_files() {
    let rig = Rig::new("ao-rename-race");
    let mut tuning = quick();
    tuning.watch_settle = Duration::from_millis(5);
    tuning.missing_after = Duration::from_millis(30);
    let (mut store, _) = rig.open_with(tuning);
    if !store.volume().open_by_id {
        return;
    }
    const ROUNDS: u32 = 120;
    let mut state = 0x0DDB_1A5E_5BAD_5EEDu64;
    let mut doubled = Vec::new();
    let (mut before, mut asked_again) = (0, 0);
    for round in 0..ROUNDS {
        let old = rig.folder.join(format!("text {round}.md"));
        let new = rig.folder.join(format!("renamed {round}.md"));
        let (id, mut doc, _) = written(&mut store, &format!("text {round}.md"), "First.");
        let edit = type_end(&mut store, id, &mut doc, " Second.");
        store.save(id, doc.to_text("\n"), edit);
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let wait = Duration::from_micros(1500 + state % 7000);
        let from = Instant::now();
        while from.elapsed() < wait {
            std::hint::spin_loop();
        }
        let mut renamed = false;
        for _ in 0..500 {
            if fs::rename(&old, &new).is_ok() {
                renamed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(renamed, "round {round}: the text could not be renamed");
        let mut seen = Vec::new();
        let is_saved = |news: &News| matches!(news, News::Saved { doc, edit: e, .. } if *doc == id && *e == edit);
        if wait_news(&mut store, &mut seen, Duration::from_millis(120), is_saved).is_none() {
            // The save was dropped when the file went missing (see the test
            // after this one); the interface asks again, as it would at the
            // next pause.
            asked_again += 1;
            store.save(id, doc.to_text("\n"), edit);
            assert!(wait_news(&mut store, &mut seen, SOON, is_saved).is_some(), "round {round}: never saved; {seen:?}");
        }
        if !old.exists() && !seen.iter().any(|news| matches!(news, News::Moved { .. })) {
            wait_news(&mut store, &mut seen, Duration::from_millis(400), |news| matches!(news, News::Moved { doc, .. } if *doc == id));
        }
        store.close(id, None, None);
        closed(&mut store, id);
        if old.exists() {
            doubled.push(format!("round {round}, renamed {wait:?} after the save was asked for: {:?} reads {:?} and {:?} reads {:?}; the store said {:?}", old.file_name().unwrap(), read(&old), new.file_name().unwrap(), read(&new), seen.iter().filter(|n| !matches!(n, News::Tree(_))).collect::<Vec<_>>()));
        } else {
            assert_eq!(read(&new), "First. Second.", "round {round}");
            if read(&new) == "First. Second." && seen.iter().position(|n| matches!(n, News::Saved { .. })) < seen.iter().position(|n| matches!(n, News::Moved { .. })) {
                before += 1;
            }
        }
    }
    assert!(store.shutdown(SOON));
    println!("of {ROUNDS} rounds: the save finished before the rename was noticed in {before}; the save had to be asked for again in {asked_again}; two files in {}", doubled.len());
    assert!(doubled.is_empty(), "{} of {ROUNDS}: {:#?}", doubled.len(), &doubled[..doubled.len().min(3)]);
}

/// `Store::save`: "The answer is `News::Saved`, or `News::Held` with the
/// reason." A sync client renames the text (the rename was made on another
/// device) a moment before a save is asked for. The store waits a moment in
/// case the file comes back, then finds it under its new name and says so
/// (`News::Moved`). The save that was waiting must then be made. It is
/// forgotten: no `Saved`, no `Held`, and the words stay out of the file
/// until something else asks for a save.
#[test]
fn a_save_waiting_when_the_text_is_renamed_outside_is_made_once_the_rename_is_followed() {
    let rig = Rig::new("ao-rename-save");
    let (mut store, _) = rig.open();
    if !store.volume().open_by_id {
        return;
    }
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved under the old name.");
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Renamed elsewhere.md")).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " Typed as it was renamed.");
    store.save(id, doc.to_text("\n"), edit);
    let mut seen = Vec::new();
    let moved = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Moved { .. }));
    assert_eq!(moved, Some(News::Moved { doc: id, rel: PathBuf::from("Renamed elsewhere.md") }), "{seen:?}");
    let answer = wait_news(&mut store, &mut seen, Duration::from_secs(3), |news| matches!(news, News::Saved { .. } | News::Held { .. }));
    let file = read(&rig.folder.join("Renamed elsewhere.md"));
    assert!(store.shutdown(SOON));
    assert!(answer.is_some(), "three seconds after the rename was followed the save has had no answer; the file reads {file:?}; news was {seen:?}");
    assert_eq!(file, "Saved under the old name. Typed as it was renamed.");
}

// ------------------------- changed while the window is not the active one

/// With the window inactive the folder is not watched. A sync client's
/// update made then must be announced when the window comes back, and a
/// save asked for meanwhile must still find it.
#[test]
fn an_update_made_while_the_window_is_inactive_is_found_when_it_comes_back_and_by_any_save() {
    let rig = Rig::new("ao-inactive");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Ours.");
    let path = rig.folder.join("essay.md");
    store.set_active(false);
    std::thread::sleep(Duration::from_millis(100));
    fs::write(&path, "Theirs, written while he was elsewhere.").unwrap();
    listen(&mut store, Duration::from_millis(300));
    store.set_active(true);
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((text.as_str(), clean), ("Theirs, written while he was elsewhere.", true));
    let edit = replace_all(&mut store, id, &mut doc, &text);
    save_wait(&mut store, id, &doc, edit);

    store.set_active(false);
    std::thread::sleep(Duration::from_millis(100));
    let edit = type_end(&mut store, id, &mut doc, " And a word of his.");
    fs::write(&path, "Theirs again.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    let News::ChangedOutside { text, clean, .. } = expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. })) else { unreachable!() };
    assert_eq!((text.as_str(), clean), ("Theirs again.", false));
    assert_eq!(read(&path), "Theirs again.");
    store.resolve(id, Resolve::KeepMine);
    expect(&mut store, "Saved", |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
    assert_eq!(read(&path), "Theirs, written while he was elsewhere. And a word of his.");
    let kept: Vec<String> = store.versions(id).unwrap().iter().map(|v| store.read_version(id, &v.name).unwrap()).collect();
    assert!(kept.iter().any(|text| text == "Theirs again."), "their version went into the history before ours went over it: {kept:?}");
    assert!(kept.iter().any(|text| text == "Ours."), "{kept:?}");
    assert!(store.shutdown(SOON));
}

// ------------------------- a second copy of the program

/// A second launch reaches the folder by other spellings of its path: other
/// capitals, a trailing separator, `.` and `..` on the way, forward slashes,
/// the long-path prefix, and a junction that leads to it. Each must be told
/// who has the folder, and must have changed nothing.
#[test]
fn a_second_copy_reaching_the_folder_by_another_spelling_is_told_who_has_it() {
    let rig = Rig::new("ao-second-copy");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "The first copy has this open.");
    type_end(&mut store, id, &mut doc, " And has not saved this.");
    store.flush();
    let plain = rig.folder.to_string_lossy().into_owned();
    let mut spellings: Vec<(String, PathBuf)> = vec![
        ("capitals".to_string(), PathBuf::from(plain.to_uppercase())),
        ("small letters".to_string(), PathBuf::from(plain.to_lowercase())),
        ("a trailing separator".to_string(), PathBuf::from(format!("{plain}\\"))),
        ("a dot on the way".to_string(), rig.folder.join(".")),
        ("out and back in".to_string(), rig.folder.join("..").join("writing")),
        ("forward slashes".to_string(), PathBuf::from(plain.replace('\\', "/"))),
        ("the long-path prefix".to_string(), fs::canonicalize(&rig.folder).unwrap()),
    ];
    let junction = rig.folder.with_file_name("a junction to it");
    let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&rig.folder).output();
    if made.is_ok_and(|out| out.status.success()) {
        spellings.push(("a junction".to_string(), junction.clone()));
    } else {
        println!("no junction could be made here; that spelling was not tried");
    }
    let mut wrong = Vec::new();
    for (what, folder) in &spellings {
        match Store::open(Setup { folder: folder.clone(), ..rig.setup(quick()) }) {
            Err(OpenError::Locked(owner)) => {
                if owner.pid != std::process::id() || owner.note != "test" {
                    wrong.push(format!("{what}: refused, but the owner read as {owner:?}"));
                }
            }
            Err(other) => wrong.push(format!("{what}: {other:?}")),
            Ok((second, recovered)) => {
                wrong.push(format!("{what}: a second store opened on the same folder; its recovery did {recovered:?}"));
                second.abandon();
            }
        }
    }
    let _ = fs::remove_dir(&junction);
    assert_eq!(read(&rig.folder.join("essay.md")), "The first copy has this open.");
    assert_eq!(rig.listing(), ["essay.md"]);
    let edit = type_end(&mut store, id, &mut doc, " Now it has.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(read(&rig.folder.join("essay.md")), "The first copy has this open. And has not saved this. Now it has.");
    assert!(store.shutdown(SOON));
    assert!(wrong.is_empty(), "{wrong:#?}");
}

// ------------------------- a scanner at a journal (fail)

/// A killed session leaves a journal holding a sentence that is in no file.
/// At the next start another program (a virus scanner, an indexer, a backup
/// tool) happens to have that journal open for reading. Recovery takes any
/// open handle for another instance of the program and passes the journal
/// by without a word. Whatever it does, it must not leave the interface
/// believing there was nothing to recover: it must report the text, or
/// opening the text must bring the sentence back.
#[test]
fn a_scanner_reading_a_journal_at_start_does_not_hide_what_the_journal_holds() {
    let rig = Rig::new("ao-scan-start");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved words.");
    type_end(&mut store, id, &mut doc, " A sentence typed after the last save.");
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    let scanners = scan_journals(&rig);
    let (mut store, recovered) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    drop(scanners);
    let told = recovered.iter().any(|one| one.rel == Path::new("essay.md"));
    let shown = opened.text.contains("A sentence typed after the last save.");
    assert!(told || shown, "recovery reported {recovered:?} and the text opened as {:?} (restored {:?}); the journal is still there: {:?}", opened.text, opened.restored, rig.journals());
    assert!(store.shutdown(SOON));
}

/// The same, one step further. Not knowing anything is missing, he types on
/// and the process is killed a second time. Now the journal the scanner hid
/// is the older of two, and the next recovery replays the newer one and
/// removes both. The sentence from before the first kill must still be
/// somewhere: in a file of his, in a version, or in a journal.
#[test]
fn a_journal_passed_by_once_is_not_removed_unread_after_a_second_kill() {
    let rig = Rig::new("ao-scan-twice");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved words.");
    type_end(&mut store, id, &mut doc, " A sentence typed after the last save.");
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    let scanners = scan_journals(&rig);
    let (mut store, first) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    drop(scanners);
    let mut doc = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc, " Typed in the second sitting.");
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();

    let (store, second) = rig.open();
    assert!(store.shutdown(SOON));
    let scratch = rig.folder.parent().unwrap();
    assert!(anywhere(scratch, "Typed in the second sitting."));
    assert!(
        anywhere(scratch, "A sentence typed after the last save."),
        "the sentence is in no file, no version and no journal. First recovery: {first:?}; second: {second:?}; the folder: {:?}; the file reads {:?}; journals left: {:?}",
        rig.listing(),
        read(&rig.folder.join("essay.md")),
        rig.journals()
    );
}

/// A text is closed while another program keeps its file from being
/// replaced, so its last words stay in the journal (`News::Closed` says
/// `in_journal`). The builder's note: "Such a journal is brought back if the
/// text is reopened in the same session." With a scanner reading the journal
/// at that moment, it is not, and nothing says so.
#[test]
fn text_left_in_the_journal_comes_back_although_a_scanner_has_the_journal_open() {
    let rig = Rig::new("ao-scan-left");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Saved.");
    let path = rig.folder.join("essay.md");
    let holder = OpenOptions::new().read(true).share_mode(1 | 2).open(&path).unwrap();
    let edit = type_end(&mut store, id, &mut doc, " His last words before closing.");
    store.close(id, Some((doc.to_text("\n"), edit)), None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: true });
    drop(holder);
    // The journal thread lets go of the file in its own time.
    std::thread::sleep(Duration::from_millis(200));
    let scanners = scan_journals(&rig);
    let opened = store.open_text("essay.md").unwrap();
    drop(scanners);
    assert_eq!(opened.text, "Saved. His last words before closing.", "restored: {:?}; journals: {:?}", opened.restored, rig.journals());
    assert!(store.shutdown(SOON));
}

// ------------------------- a scanner at the save's temp file

/// While a text is saved three hundred times, two other programs open every
/// temp file they can find in the folder, one refusing to share it at all
/// and one sharing everything, and hold it a moment. Every save must go
/// through, the file must always read as the last text saved, and no temp
/// file may be left once the scanners stop.
#[test]
fn scanners_opening_every_temp_file_never_stop_a_save() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    let rig = Rig::new("ao-temp-scan");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Start.");
    let stop = Arc::new(AtomicBool::new(false));
    let caught = Arc::new(AtomicU32::new(0));
    let scanners: Vec<_> = [0u32, SHARE_ALL]
        .into_iter()
        .map(|share| {
            let (folder, stop, caught) = (rig.folder.clone(), stop.clone(), caught.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Ok(entries) = fs::read_dir(&folder) else { continue };
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name.starts_with('~') && name.ends_with(".tmp") {
                            if let Ok(file) = OpenOptions::new().read(true).share_mode(share).open(entry.path()) {
                                caught.fetch_add(1, Ordering::Relaxed);
                                std::thread::sleep(Duration::from_millis(1));
                                drop(file);
                            }
                        }
                    }
                }
            })
        })
        .collect();
    let mut held = Vec::new();
    for n in 0..300 {
        let edit = type_end(&mut store, id, &mut doc, &format!(" Word {n}."));
        store.save(id, doc.to_text("\n"), edit);
        let mut seen = Vec::new();
        let saved = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::Saved { edit: e, .. } if *e == edit));
        assert!(saved.is_some(), "save {n} never went through: {seen:?}");
        held.extend(seen.into_iter().filter(|news| matches!(news, News::Held { .. })));
        assert_eq!(read(&rig.folder.join("essay.md")), doc.to_text("\n"), "after save {n}");
    }
    stop.store(true, Ordering::Relaxed);
    for scanner in scanners {
        scanner.join().expect("a scanner thread");
    }
    store.close(id, None, None);
    closed(&mut store, id);
    assert!(store.shutdown(SOON));
    println!("the scanners got a temp file open {} times; saves reported held meanwhile: {held:?}", caught.load(Ordering::Relaxed));
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(held.is_empty(), "{held:?}");
}

// ------------------------- a file whose bytes are not on this disk

/// A file marked offline (as a cloud placeholder is) is listed as one and
/// its contents are never read by the scan of the folder.
#[test]
fn a_file_marked_offline_is_listed_as_a_placeholder_and_never_read_by_the_scan() {
    let rig = Rig::new("ao-offline");
    fs::write(rig.folder.join("here.md"), "on this disk").unwrap();
    fs::write(rig.folder.join("cloud.md"), "in the cloud").unwrap();
    add_attributes(&rig.folder.join("cloud.md"), ATTR_OFFLINE);
    let mut tuning = quick();
    tuning.scan_texts = true;
    let (mut store, _) = rig.open_with(tuning);
    let news = listen(&mut store, Duration::from_millis(700));
    let Some(News::Tree(tree)) = news.iter().rev().find(|n| matches!(n, News::Tree(_))).cloned() else { panic!("no tree: {news:?}") };
    assert_eq!(tree.find(Path::new("cloud.md")).map(|text| text.placeholder), Some(true));
    assert_eq!(tree.find(Path::new("here.md")).map(|text| text.placeholder), Some(false));
    let read_by_scan: Vec<&PathBuf> = news.iter().filter_map(|n| if let News::FileText { rel, .. } = n { Some(rel) } else { None }).collect();
    assert_eq!(read_by_scan, [&PathBuf::from("here.md")]);
    assert!(store.shutdown(SOON));
}

// ------------------------- a sync client and an idle window

/// Nobody is typing. A sync client updates the text forty times, in the
/// three ways such programs do it (rewriting in place, renaming a new file
/// over it, deleting it and making it again), and the interface follows each
/// update the way `News::ChangedOutside` tells it to. The store must never
/// write the file itself, never make a second file, and at the end the file
/// is the sync client's last version, still the very file it wrote.
#[test]
fn a_sync_client_updating_a_text_nobody_types_in_is_followed_and_never_rewritten() {
    let rig = Rig::new("ao-sync-idle");
    let path = rig.folder.join("essay.md");
    fs::write(&path, "The version that was here at the start.\n").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("essay.md").unwrap();
    let id = opened.doc;
    let mut doc = Doc::from_text(&opened.text);
    let mut latest = opened.edit;
    let mut last = String::new();
    for round in 0..40u32 {
        let version = format!("Version {round} from the other device.\r\n{}\r\n", "A line of it. ".repeat((round % 7) as usize));
        match round % 3 {
            0 => fs::write(&path, &version).unwrap(),
            1 => {
                let temp = rig.folder.join(".essay.md.sync-tmp");
                fs::write(&temp, &version).unwrap();
                fs::rename(&temp, &path).unwrap();
            }
            _ => {
                fs::remove_file(&path).unwrap();
                std::thread::sleep(Duration::from_millis(u64::from(round % 4)));
                fs::write(&path, &version).unwrap();
            }
        }
        let wrote = fs::metadata(&path).unwrap().modified().unwrap();
        // The interface: take the new text if nothing was typed since, and ask for a save.
        let mut seen = Vec::new();
        let taken = wait_news(&mut store, &mut seen, SOON, |news| matches!(news, News::ChangedOutside { text, .. } if text.replace('\n', "\r\n") == version));
        let Some(News::ChangedOutside { text, clean, edit, read_only, .. }) = taken else { panic!("round {round}: the update was never announced; news was {seen:?}") };
        assert!(clean && edit == latest && read_only.is_none(), "round {round}: clean {clean}, edit {edit} (the buffer is at {latest}), read_only {read_only:?}");
        latest = replace_all(&mut store, id, &mut doc, &text);
        let news = save_wait(&mut store, id, &doc, latest);
        assert!(!news.iter().any(|n| matches!(n, News::Held { .. } | News::KeptBeside { .. })), "round {round}: {news:?}");
        assert_eq!(fs::read(&path).unwrap(), version.as_bytes(), "round {round}");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), wrote, "round {round}: the store wrote the file itself");
        last = version;
    }
    let quiet = listen(&mut store, Duration::from_millis(300));
    assert!(!quiet.iter().any(|n| matches!(n, News::ChangedOutside { .. } | News::Held { .. })), "{quiet:?}");
    store.close(id, None, None);
    assert_eq!(closed(&mut store, id), News::Closed { doc: id, beside: None, in_journal: false });
    assert!(store.shutdown(SOON));
    assert_eq!(fs::read(&path).unwrap(), last.as_bytes());
    assert_eq!(rig.listing(), ["essay.md"]);
    assert!(rig.journals().is_empty());
}
