//! Frames of the line at work, for looking at.
//!
//! Each test reads the screen back as text, and writes the same screen as
//! a picture when `OK_SHOTS` names a folder to write it in:
//!
//! ```text
//! OK_SHOTS=C:\Users\jaspe\Projects\ok\shots cargo test -p ok-ui --offline --test line_frames
//! ```

mod line_support;

use std::path::PathBuf;

use line_support::*;
use ok_ui::flow::Rest;
use ok_ui::harness::Rig;
use ok_ui::{line, Options};

/// The essay of the example desk on a full screen, with the whole example
/// folder behind it and the real line in front.
fn essay() -> Rig {
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let (shelf, _) = Shelf::read(&desk, "College/The orchard");
    let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    rig.size(1920, 1080);
    rig
}

/// Writes the screen as `w2-line-NAME.png` in the folder `OK_SHOTS` names.
fn shot(rig: &mut Rig, name: &str) {
    let Some(folder) = std::env::var_os("OK_SHOTS") else { return };
    let path = PathBuf::from(folder).join(format!("w2-line-{name}.png"));
    rig.shot(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
}

#[test]
fn a_title_half_typed_with_the_bar_in_the_contents() {
    let mut rig = essay();
    line_types(&mut rig, "grand");
    assert_eq!(said(&mut rig), "Enter opens College\\Grandfather interview.");
    assert!(bar(&mut rig).unwrap().starts_with("Grandfather interview ."));
    shot(&mut rig, "title");
    rig.wait(250);
    assert!(read(&mut rig, 47, 112, 47).ends_with("Grandfather interview    630 words"));
    shot(&mut rig, "title-rested");
    rig.press("esc");
    line_types(&mut rig, "o");
    shot(&mut rig, "title-many");
}

#[test]
fn the_empty_line_and_what_it_understands() {
    let mut rig = essay();
    rig.press("esc");
    shot(&mut rig, "empty");
    rig.wait(1200);
    assert!(read(&mut rig, 47, 112, 47).starts_with("ok a title   # heading"));
    shot(&mut rig, "understood");
}

#[test]
fn a_find_with_its_hits() {
    let mut rig = essay();
    rig.press("ctrl+home");
    line_types(&mut rig, "/three");
    assert!(said(&mut rig).starts_with("1 of "));
    shot(&mut rig, "find");
    rig.type_text("/two/g");
    shot(&mut rig, "change-all");
}

#[test]
fn a_heading_gone_to_from_the_labels() {
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let (shelf, _) = Shelf::read(&desk, "Mock trial/Cross of the detective");
    let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    rig.size(1920, 1080);
    rig.press("ctrl+home");
    line_types(&mut rig, "#");
    rig.press("down");
    rig.press("down");
    shot(&mut rig, "heading");
}

#[test]
fn the_set_table_with_the_bar_on_look_while_a_dark_look_is_tried() {
    let mut rig = essay();
    line_types(&mut rig, "set");
    assert!(bar(&mut rig).unwrap().starts_with("look ."));
    shot(&mut rig, "set");
    rig.type_text(" look ");
    for _ in 0..6 {
        rig.press("down");
    }
    assert_eq!(bar(&mut rig).as_deref(), Some("vt100"));
    assert_eq!(said(&mut rig), "Enter keeps look at vt100. Esc puts it back to manila.");
    shot(&mut rig, "set-look-vt100");
    rig.press("up");
    shot(&mut rig, "set-look-amber");
    rig.press("esc");
    line_types(&mut rig, "set measure 5");
    shot(&mut rig, "set-not-yet");
    rig.type_text("6");
    shot(&mut rig, "set-measure");
    rig.press("esc");
    line_types(&mut rig, "set cursor underline");
    rig.press("enter");
    rig.press("esc");
    shot(&mut rig, "cursor-underline");
}

#[test]
fn the_keys_page() {
    let mut rig = essay();
    rig.press("f1");
    assert_eq!(rig.screen().big(ok_ui::grid::Cols { first: 47, last: 112 }, 1), "Keys");
    shot(&mut rig, "keys");
    rig.press("pagedown");
    shot(&mut rig, "keys-2");
}

#[test]
fn the_notices_page() {
    let mut rig = essay();
    line_types(&mut rig, "notices");
    rig.press("enter");
    assert_eq!(read(&mut rig, 47, 112, 1), "ok is a writing tool for Windows: one window, one .exe, and a page");
    shot(&mut rig, "notices");
    rig.press("pagedown");
    shot(&mut rig, "notices-2");
}

#[test]
fn a_name_offered_after_two_brackets() {
    let mut rig = essay();
    rig.type_text(" I heard the same thing in [[gr");
    assert!(bar(&mut rig).unwrap().starts_with("Grandfather interview ."));
    shot(&mut rig, "link");
}

#[test]
fn the_table_risen_over_the_foot_of_a_small_window() {
    let desk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/desk");
    let (shelf, _) = Shelf::read(&desk, "College/The orchard");
    let mut rig = Rig::with_seams(Options::default(), Box::new(shelf), line::start(), Box::new(Rest));
    rig.size(1280, 720);
    line_types(&mut rig, "orch");
    shot(&mut rig, "small-title");
    rig.press("esc");
    line_types(&mut rig, "set look ");
    rig.press("down");
    shot(&mut rig, "small-set");
}
