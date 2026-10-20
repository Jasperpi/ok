//! Attack, from outside the crate: names. A title that has to become a file
//! name; a name that differs from another only by its capitals; the names
//! Windows keeps for itself; a path longer than 260 characters; texts whose
//! names begin alike; and a `.discarded` folder that already holds the name.
//! The question behind each: does the store ever write through a name it had
//! no business writing through, or change a name that was not its to change?

use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ok_store::testing::{quick, save_wait, type_end, wait_news, Rig};
use ok_store::{check_name, safe_file_name, DocId, NameError, News, Recovery, Resolve, Setup, Store, StoreError};
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

/// Every file under `dir`, as (path inside it, bytes), sorted.
fn everything(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, rel: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let rel = rel.join(entry.file_name());
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                walk(&entry.path(), &rel, out);
            } else {
                out.push((rel.to_string_lossy().into_owned(), fs::read(entry.path()).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

// ------------------------------------------------- a title becomes a name (fail)

/// `safe_file_name` "turns a title someone typed into a file name stem that
/// `check_name` accepts". A title that begins with a device name and a dot
/// (Con. law outline, for a student of mock trial) comes back with an
/// underscore at its far end, which changes nothing about how it begins, and
/// `check_name` refuses it as reserved.
#[test]
fn a_title_that_begins_with_a_device_name_and_a_dot_becomes_a_name_that_passes() {
    let titles = ["Con. law outline", "Aux. cable notes", "NUL.pointer", "com1.settings", "Prn. queue", "lpt1.5 inches of margin", "CON .x"];
    let mut refused = Vec::new();
    for title in titles {
        let stem = safe_file_name(title);
        for name in [stem.clone(), format!("{stem}.md")] {
            if let Err(why) = check_name(&name) {
                refused.push(format!("{title:?} became {name:?}, refused as {why:?}"));
            }
        }
    }
    assert!(refused.is_empty(), "{refused:#?}");
}

/// The same from end to end: the interface says "Enter starts a new text
/// called Con. law outline", he presses Enter, and the text must start.
#[test]
fn a_text_called_con_law_outline_can_be_started() {
    let rig = Rig::new("ao-conlaw");
    let (mut store, _) = rig.open();
    let name = format!("{}.md", safe_file_name("Con. law outline"));
    let started = store.create_text(&name);
    assert!(started.is_ok(), "create_text({name:?}) gave {:?}", started.err());
    assert!(store.shutdown(SOON));
}

/// Titles of every awkward shape, a few thousand of them: whatever is typed,
/// what comes back must be a name `check_name` accepts, with and without an
/// ending. Nothing touches a disk here.
#[test]
fn every_title_becomes_a_name_that_passes() {
    let pieces = [" ", " ", ".", ".", "C", "O", "N", "a", "1", "?", ":", "\\", "/", "\t", "\u{00E9}", "*", "\"", "<", "|", "aux", "nul", "com", "lpt", "con", "prn", "9", "\u{00B9}", "~", "$", "notes"];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let mut refused: Vec<String> = Vec::new();
    let mut kinds: Vec<NameError> = Vec::new();
    for _ in 0..20_000 {
        let title: String = (0..1 + next(7)).map(|_| pieces[next(pieces.len())]).collect();
        let stem = safe_file_name(&title);
        for name in [stem.clone(), format!("{stem}.md")] {
            if let Err(why) = check_name(&name) {
                if !kinds.contains(&why) {
                    kinds.push(why);
                    refused.push(format!("{title:?} became {name:?}, refused as {why:?}"));
                }
            }
        }
    }
    assert!(refused.is_empty(), "one example of each way it goes wrong: {refused:#?}");
}

// ------------------------------------------------- capitals (fail)

/// The file is `The Orchard.md`. The interface opens it by a spelling of its
/// own (he typed `[[the orchard]]` and followed the link, or typed the title
/// on the line). The file's name is his and must stay as he wrote it.
#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn opening_a_text_by_another_spelling_of_its_name_does_not_rename_the_file() {
    let rig = Rig::new("ao-case-open");
    fs::write(rig.folder.join("The Orchard.md"), "Trees.").unwrap();
    let (mut store, _) = rig.open();
    let opened = store.open_text("the orchard.md").expect("Windows finds it whatever the capitals");
    let mut doc = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc, " More trees.");
    save_wait(&mut store, opened.doc, &doc, edit);
    assert_eq!(read(&rig.folder.join("The Orchard.md")), "Trees. More trees.");
    assert_eq!(rig.listing(), ["The Orchard.md"], "one save and the file has lost the capitals he gave it");
    assert!(store.shutdown(SOON));
}

/// He has `essay.md` open and, in Explorer, renames it `Essay.md`. Windows
/// lets him: the store holds no handle. The next save must not put the old
/// spelling back.
#[test]
fn capitals_he_changed_in_explorer_while_the_text_was_open_survive_the_next_save() {
    let rig = Rig::new("ao-case-explorer");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Lower case at first.");
    fs::rename(rig.folder.join("essay.md"), rig.folder.join("Essay.md")).expect("a rename that changes only capitals");
    assert_eq!(rig.listing(), ["Essay.md"]);
    let news = listen(&mut store, Duration::from_millis(500));
    let edit = type_end(&mut store, id, &mut doc, " Then he typed on.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(rig.listing(), ["Essay.md"], "his rename was undone by a save; the store had said {news:?}");
    assert!(store.shutdown(SOON));
}

/// Renaming a text to another spelling of its own name, through the store,
/// is a rename like any other and takes nothing with it.
#[test]
fn a_text_can_be_renamed_to_another_spelling_of_its_own_name() {
    let rig = Rig::new("ao-case-rename");
    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "Lower case at first.");
    assert_eq!(store.rename("essay.md", "Essay.md"), Ok(()));
    assert_eq!(rig.listing(), ["Essay.md"]);
    let edit = type_end(&mut store, id, &mut doc, " And on.");
    save_wait(&mut store, id, &doc, edit);
    assert_eq!(rig.listing(), ["Essay.md"]);
    assert_eq!(read(&rig.folder.join("Essay.md")), "Lower case at first. And on.");
    // A text that is not open, the same.
    fs::write(rig.folder.join("notes.txt"), "notes").unwrap();
    assert_eq!(store.rename("notes.txt", "Notes.txt"), Ok(()));
    assert_eq!(rig.listing(), ["Essay.md", "Notes.txt"]);
    assert!(!store.versions(id).unwrap().is_empty(), "its history went with it");
    store.close(id, None, None);
    closed(&mut store, id);
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
}

/// A name that differs from one already there only by capitals is the same
/// name to Windows, and must be refused wherever a new name is made.
#[test]
fn a_name_that_differs_only_by_capitals_is_taken() {
    let rig = Rig::new("ao-case-taken");
    fs::write(rig.folder.join("Notes.TXT"), "from another program").unwrap();
    fs::write(rig.folder.join("\u{00C9}cole.md"), "accented").unwrap();
    fs::write(rig.folder.join("other.md"), "other").unwrap();
    let (mut store, _) = rig.open();
    let (id, _, _) = written(&mut store, "essay.md", "Open.");
    assert_eq!(store.create_text("ESSAY.md").err(), Some(StoreError::AlreadyOpen(id)));
    for taken in ["notes.txt", "NOTES.txt", "\u{00E9}cole.md", "OTHER.MD"] {
        assert_eq!(store.create_text(taken).err(), Some(StoreError::Name(NameError::Taken)), "{taken}");
        assert_eq!(store.rename("essay.md", taken).err(), Some(StoreError::Name(NameError::Taken)), "{taken}");
    }
    assert_eq!(store.rename("other.md", "Essay.md").err(), Some(StoreError::Name(NameError::Taken)));
    store.close(id, None, None);
    closed(&mut store, id);
    assert_eq!(store.create_text("Essay.MD").err(), Some(StoreError::Name(NameError::Taken)));
    assert!(store.shutdown(SOON));
    let now = everything(&rig.folder);
    let names: Vec<&str> = now.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Notes.TXT", "essay.md", "other.md", "\u{00C9}cole.md"]);
    assert_eq!(read(&rig.folder.join("Notes.TXT")), "from another program");
}

// ------------------------------------------- names Windows keeps or forbids

/// Device names, forbidden characters, trailing dots and spaces, hidden
/// names, endings that are not a text's, and paths that leave the folder:
/// refused when a text is started and when one is renamed, with nothing
/// made in the folder or anywhere else.
#[test]
fn names_windows_keeps_or_forbids_are_refused_and_nothing_is_made() {
    let rig = Rig::new("ao-refused");
    fs::write(rig.folder.join("real.md"), "real").unwrap();
    let (mut store, _) = rig.open();
    let bad = [
        "CON.md",
        "con.txt",
        "NUL.md",
        "nul",
        "COM1.md",
        "LPT9.markdown",
        "COM\u{00B9}.md",
        "aux.notes.md",
        "sub\\PRN\\x.md",
        "what?.md",
        "a:b.md",
        "essay.md:stream",
        "a*b.md",
        "a<b>.md",
        "quote\".md",
        "pipe|.md",
        "tab\there.md",
        "dot.md.",
        "space.md ",
        "folder \\x.md",
        "folder.\\x.md",
        ".hidden.md",
        ".git\\x.md",
        "..\\escape.md",
        "sub\\..\\..\\escape.md",
        "C:\\absolute.md",
        "\\rooted.md",
        "\\\\server\\share\\x.md",
        "",
        "noending",
        "report.docx",
        ".md",
    ];
    for name in bad {
        let started = store.create_text(name);
        assert!(matches!(started, Err(StoreError::Name(_))), "create_text({name:?}) gave {started:?}");
        let renamed = store.rename("real.md", name);
        assert!(matches!(renamed, Err(StoreError::Name(_))), "rename to {name:?} gave {renamed:?}");
    }
    // Opening, discarding and rewriting by such a name fail too, and quietly.
    for name in ["CON.md", "what?.md", "a:b.md", "..\\escape.md", "nul"] {
        assert!(store.open_text(name).is_err(), "{name}");
        assert!(store.discard(name, None).is_err(), "{name}");
        assert!(store.rewrite(name, "", "x").is_err(), "{name}");
    }
    assert!(store.shutdown(SOON));
    assert_eq!(everything(&rig.folder), [("real.md".to_string(), b"real".to_vec())]);
    assert!(!rig.folder.with_file_name("escape.md").exists());
}

/// A title with a colon and a question mark in it becomes a text all the same.
#[test]
fn a_title_with_a_colon_and_a_question_mark_becomes_a_text() {
    let rig = Rig::new("ao-title");
    let (mut store, _) = rig.open();
    let name = format!("{}.md", safe_file_name("What is justice? A reply: part 2"));
    assert_eq!(name, "What is justice A reply part 2.md");
    let (id, _, _) = written(&mut store, &name, "It is.");
    store.close(id, None, None);
    closed(&mut store, id);
    assert!(store.shutdown(SOON));
    assert_eq!(rig.listing(), ["What is justice A reply part 2.md"]);
}

/// Names with a combining mark, in another script, with a character outside
/// the basic plane, and two that differ only in how an accent is spelled:
/// each is its own text through a save, a kill, a rename and a discard.
#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn names_with_combining_marks_other_scripts_and_two_spellings_of_an_accent_stay_apart() {
    let rig = Rig::new("ao-unicode");
    let names = ["mitsqanaqan\u{0313}.md", "\u{6587}\u{7AE0}.md", "\u{1F332} trees.md", "cafe\u{0301}.md", "caf\u{00E9}.md", "na\u{00EF}ve \u{2014} notes.txt"];
    let (mut store, _) = rig.open();
    for name in names {
        let (id, _, _) = written(&mut store, name, &format!("The text called {name}."));
        store.close(id, None, None);
        closed(&mut store, id);
    }
    let mut listed = rig.listing();
    let mut wanted: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    listed.sort();
    wanted.sort();
    assert_eq!(listed, wanted);
    for name in names {
        let opened = store.open_text(name).unwrap();
        assert_eq!(opened.text, format!("The text called {name}."));
        let mut doc = Doc::from_text(&opened.text);
        type_end(&mut store, opened.doc, &mut doc, " And a sentence never saved.");
    }
    store.flush();
    std::thread::sleep(Duration::from_millis(150));
    store.abandon();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), names.len(), "{recovered:?}");
    assert!(recovered.iter().all(|one| one.how == Recovery::BroughtBack), "{recovered:?}");
    for name in names {
        assert_eq!(read(&rig.folder.join(name)), format!("The text called {name}. And a sentence never saved."));
    }
    assert_eq!(store.rename("\u{6587}\u{7AE0}.md", "\u{6587}\u{7AE0} \u{4E8C}.md"), Ok(()));
    assert_eq!(store.rename("cafe\u{0301}.md", "caf\u{00E9}.md").err(), Some(StoreError::Name(NameError::Taken)));
    let went = store.discard("\u{1F332} trees.md", None).expect("discarded");
    assert_eq!(went, PathBuf::from(".discarded\\\u{1F332} trees.md"));
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
    assert_eq!(rig.listing().len(), names.len(), "five texts and .discarded: {:?}", rig.listing());
}

// ------------------------------------------------- a long path

/// A writing folder whose own path is past 260 characters, a text some
/// folders further down in it: started, saved, closed, opened, brought back
/// after a kill, kept beside after a conflict, renamed, discarded and listed.
#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn a_path_longer_than_260_characters_goes_through_everything() {
    let rig = Rig::new("ao-long-path");
    let mut folder = rig.folder.clone();
    for n in 0..4 {
        folder.push(format!("a folder with a long name, number {n}, to get past the old limit"));
    }
    let setup = |tuning| Setup { folder: folder.clone(), ..rig.setup(tuning) };
    let (mut store, _) = Store::open(setup(quick())).expect("the store opens on a long path");
    let sub = format!("{}\\{}", "A sub-folder whose name goes on".repeat(3), "and another inside it ".repeat(4).trim());
    let rel = format!("{sub}\\{}.md", "The title of a text that says rather a lot ".repeat(4).trim());
    let full = folder.join(&rel);
    assert!(full.as_os_str().len() > 600, "only {} long", full.as_os_str().len());

    let (id, _, _) = written(&mut store, &rel, "First words.");
    assert_eq!(read(&full), "First words.");
    let tree = expect(&mut store, "a Tree with the text", |news| matches!(news, News::Tree(tree) if tree.find(Path::new(&rel)).is_some()));
    let News::Tree(tree) = tree else { unreachable!() };
    assert_eq!(tree.find(Path::new(&rel)).map(|text| text.bytes), Some(12));
    store.close(id, None, None);
    closed(&mut store, id);

    // Opened again, typed in, and killed before a save.
    let opened = store.open_text(&rel).unwrap();
    let mut doc2 = Doc::from_text(&opened.text);
    type_end(&mut store, opened.doc, &mut doc2, " Typed and never saved.");
    store.flush();
    std::thread::sleep(Duration::from_millis(100));
    store.abandon();
    let (mut store, recovered) = Store::open(setup(quick())).expect("it opens again");
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].how, Recovery::BroughtBack);
    assert_eq!(read(&full), "First words. Typed and never saved.");

    // Another program's version while there are unsaved edits: both kept.
    let opened = store.open_text(&rel).unwrap();
    let mut doc3 = Doc::from_text(&opened.text);
    let edit = type_end(&mut store, opened.doc, &mut doc3, " Mine.");
    fs::write(&full, "Theirs.").unwrap();
    store.save(opened.doc, doc3.to_text("\n"), edit);
    expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. }));
    store.resolve(opened.doc, Resolve::KeepBoth { text: doc3.to_text("\n"), edit });
    let News::KeptBeside { rel: beside, .. } = expect(&mut store, "KeptBeside", |news| matches!(news, News::KeptBeside { .. })) else { unreachable!() };
    assert_eq!(read(&folder.join(&beside)), "First words. Typed and never saved. Mine.");
    assert_eq!(read(&full), "Theirs.");
    let edit = ok_store::testing::replace_all(&mut store, opened.doc, &mut doc3, "Theirs.");
    save_wait(&mut store, opened.doc, &doc3, edit);
    store.close(opened.doc, None, None);
    closed(&mut store, opened.doc);

    // Renamed to a name as long, then discarded.
    let renamed = format!("{sub}\\{}.md", "Another title that also says rather a lot ".repeat(4).trim());
    assert_eq!(store.rename(&rel, &renamed), Ok(()));
    assert_eq!(read(&folder.join(&renamed)), "Theirs.");
    let went = store.discard(&renamed, None).expect("it can be discarded");
    assert_eq!(read(&folder.join(&went)), "Theirs.");
    assert!(store.shutdown(SOON));
    assert!(rig.journals().is_empty());
    let left: Vec<String> = everything(&folder).into_iter().map(|(name, _)| name).collect();
    assert_eq!(left.len(), 2, "the discarded text and the copy kept beside: {left:?}");
    assert!(!left.iter().any(|name| name.ends_with(".tmp")));
}

// ------------------------------------------------- names that begin alike

/// Texts and folders whose names are each other's beginnings. Everything
/// the store can do is done to `essay.md`; every other file must come out
/// byte for byte as it went in.
#[test]
#[cfg(windows)] // Windows paths: `\` between folders, any capitals.
fn texts_whose_names_begin_alike_are_never_taken_for_each_other() {
    let rig = Rig::new("ao-prefix");
    let others = [
        "essa.md",
        "essay.markdown",
        "essay.md.md",
        "essay.md.txt",
        "essay.txt",
        "essay 2.md",
        "essay-2.md",
        "essay (mine).md",
        "~essay.md.tmp",
        "essay.md\u{2024}md.md",
        "essay\\essay.md",
        "essay.md folder\\essay.md",
        ".discarded\\essay-2.md",
    ];
    for name in others {
        let path = rig.folder.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("the contents of {name}")).unwrap();
    }
    let untouched = |rig: &Rig, when: &str| {
        for name in others {
            assert_eq!(read(&rig.folder.join(name)), format!("the contents of {name}"), "{name} {when}");
        }
    };
    let essay = rig.folder.join("essay.md");
    let (mut store, _) = rig.open();

    // Saved, cut down (a version is kept), and a conflict settled by keeping both.
    let (id, mut doc, _) = written(&mut store, "essay.md", &"A long first draft. ".repeat(200));
    let edit = ok_store::testing::replace_all(&mut store, id, &mut doc, "Cut right down.");
    save_wait(&mut store, id, &doc, edit);
    let edit = type_end(&mut store, id, &mut doc, " Mine.");
    fs::write(&essay, "Theirs.").unwrap();
    store.save(id, doc.to_text("\n"), edit);
    expect(&mut store, "ChangedOutside", |news| matches!(news, News::ChangedOutside { .. }));
    store.resolve(id, Resolve::KeepBoth { text: doc.to_text("\n"), edit });
    expect(&mut store, "KeptBeside", |news| matches!(news, News::KeptBeside { .. }));
    untouched(&rig, "after a conflict");

    // Killed with something unsaved, and brought back.
    store.abandon();
    let (store, recovered) = rig.open();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].rel, PathBuf::from("essay.md"));
    untouched(&rig, "after a recovery");

    // Renamed away and back, then discarded twice over.
    assert_eq!(store.rename("essay.md", "essay.md.md").err(), Some(StoreError::Name(NameError::Taken)));
    assert_eq!(store.rename("essay.md", "essay\\essay.md").err(), Some(StoreError::Name(NameError::Taken)));
    assert_eq!(store.rename("essay.md", "essay 3.md"), Ok(()));
    assert_eq!(store.rename("essay 3.md", "essay.md"), Ok(()));
    let first = store.discard("essay.md", None).expect("discarded");
    assert_eq!(first, PathBuf::from(".discarded\\essay.md"));
    fs::write(&essay, "A second essay by the same name.").unwrap();
    let second = store.discard("essay.md", None).expect("discarded again");
    assert_eq!(second, PathBuf::from(".discarded\\essay-3.md"), "essay-2.md was already there");
    untouched(&rig, "after two discards");
    assert!(store.shutdown(SOON));
    untouched(&rig, "at the end");
}

// ------------------------------------------------- .discarded already holds it

#[cfg(windows)]
/// `.discarded` already holds a file by the name, a folder by the next name
/// the store would try, and a read-only file by the one after. The discard
/// must land on a name of its own and leave all three as they were.
#[test]
fn a_discard_never_lands_on_what_dot_discarded_already_holds() {
    let rig = Rig::new("ao-discard");
    let bin = rig.folder.join(".discarded");
    fs::create_dir_all(bin.join("essay-2.md")).unwrap();
    fs::write(bin.join("ESSAY.md"), "discarded long ago").unwrap();
    fs::write(bin.join("essay-2.md").join("inside.md"), "in a folder named like a text").unwrap();
    fs::write(bin.join("essay-3.md"), "read-only").unwrap();
    let mut perms = fs::metadata(bin.join("essay-3.md")).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(bin.join("essay-3.md"), perms.clone()).unwrap();

    let (mut store, _) = rig.open();
    let (id, mut doc, _) = written(&mut store, "essay.md", "The one being discarded now.");
    let edit = type_end(&mut store, id, &mut doc, " Its last words.");
    let went = store.discard("essay.md", Some((doc.to_text("\n"), edit))).expect("it is discarded");
    closed(&mut store, id);
    assert_eq!(went, PathBuf::from(".discarded\\essay-4.md"));
    assert_eq!(read(&rig.folder.join(&went)), "The one being discarded now. Its last words.");
    assert_eq!(read(&bin.join("ESSAY.md")), "discarded long ago");
    assert_eq!(read(&bin.join("essay-2.md").join("inside.md")), "in a folder named like a text");
    assert_eq!(read(&bin.join("essay-3.md")), "read-only");

    // A read-only text is moved attribute and all, or refused and left where it is.
    fs::write(rig.folder.join("locked.md"), "locked").unwrap();
    fs::set_permissions(rig.folder.join("locked.md"), perms.clone()).unwrap();
    let locked = match store.discard("locked.md", None) {
        Ok(went) => rig.folder.join(went),
        Err(_) => rig.folder.join("locked.md"),
    };
    assert_eq!(read(&locked), "locked");
    assert!(fs::metadata(&locked).unwrap().permissions().readonly());

    // A text another program has open and will not let be moved stays where it is.
    fs::write(rig.folder.join("held.md"), "held").unwrap();
    let holder = OpenOptions::new().read(true).share_mode(1 | 2).open(rig.folder.join("held.md")).unwrap();
    assert_eq!(store.discard("held.md", None).err(), Some(StoreError::Busy));
    assert_eq!(read(&rig.folder.join("held.md")), "held");
    drop(holder);

    // Where the text would go, `.discarded` holds a file where a folder is needed.
    fs::write(bin.join("Drafts"), "a file called Drafts").unwrap();
    fs::create_dir_all(rig.folder.join("Drafts")).unwrap();
    fs::write(rig.folder.join("Drafts").join("x.md"), "a draft").unwrap();
    let refused = store.discard("Drafts\\x.md", None);
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(read(&rig.folder.join("Drafts").join("x.md")), "a draft", "what could not be moved is still where it was");
    assert_eq!(read(&bin.join("Drafts")), "a file called Drafts");
    assert!(store.shutdown(SOON));

    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    for path in [bin.join("essay-3.md"), locked] {
        fs::set_permissions(path, perms.clone()).unwrap();
    }
}
