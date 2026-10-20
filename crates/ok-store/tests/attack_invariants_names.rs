//! The two name helpers as the interface will chain them: a title he typed
//! goes through `safe_file_name`, gets its ending, and is handed to
//! `create_text` or `rename`.

use std::time::Duration;

use ok_store::testing::{save_wait, type_end, Rig};
use ok_store::{check_name, safe_file_name, NameError, StoreError};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

/// `safe_file_name`: "Turns a title someone typed into a file name stem
/// that `check_name` accepts ... a device name gains an underscore."
///
/// The underscore goes on the end of the whole stem. Windows (and
/// `check_name`) judge a device name by what stands before the first dot, so
/// a title whose first word is one and is followed by a dot stays reserved,
/// and the text cannot be started or named: "Con. Law outline" is refused.
#[test]
fn a_title_made_safe_is_a_name_the_store_accepts() {
    let titles = [
        "Rebuttal notes",
        "What is 2/3 of a \"draft\"?",
        "CON",
        "lpt1",
        "...and then",
        "Con. Law outline",
        "Aux. verbs in Spanish",
        "Nul. hypothesis",
        "COM1.5 kilobaud",
        "prn.final.draft",
        "nul.md",
    ];
    let mut refused = Vec::new();
    for title in titles {
        let name = format!("{}.md", safe_file_name(title));
        if let Err(why) = check_name(&name) {
            refused.push((title, name, why));
        }
    }
    println!("titles whose safe name is refused: {refused:?}");

    // And through the store, to show it is not only the helper's own opinion.
    let rig = Rig::new("inv-names");
    let (mut store, _) = rig.open();
    let name = format!("{}.md", safe_file_name("Con. Law outline"));
    let started = store.create_text(&name).map(|opened| opened.doc);
    println!("create_text({name:?}) answered {started:?}");
    assert!(store.shutdown(SOON));
    assert_ne!(started.err(), Some(StoreError::Name(NameError::Reserved)), "a name made by safe_file_name was refused as a device name");
    assert!(refused.is_empty(), "safe_file_name made names that check_name refuses: {refused:?}");
}

/// The design's line: "rename and a title: renames the text". Changing only
/// the capitals of a name is a rename like any other.
#[test]
fn a_text_can_be_renamed_to_another_spelling_of_its_own_name() {
    let rig = Rig::new("inv-case-rename");
    let (mut store, _) = rig.open();
    let opened = store.create_text("the orchard.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Rows of trees.");
    save_wait(&mut store, opened.doc, &doc, edit);
    let renamed = store.rename("the orchard.md", "The Orchard.md");
    println!("renaming to another spelling answered {renamed:?}; the folder holds {:?}", rig.listing());
    let edit = type_end(&mut store, opened.doc, &mut doc, " And a wall.");
    save_wait(&mut store, opened.doc, &doc, edit);
    let listing = rig.listing();
    assert!(store.shutdown(SOON));
    assert_eq!(renamed, Ok(()));
    assert_eq!(listing, ["The Orchard.md"], "the name on disk after the rename and a save");
    assert_eq!(std::fs::read_to_string(rig.folder.join("The Orchard.md")).unwrap(), doc.to_text("\n"));
}
