//! Invariant 5 and section 7 of the corrected design at the one read that
//! does not use the deny-write open: opening a text while another program
//! is in the middle of writing it.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::time::Duration;

use ok_store::testing::{save_wait, type_end, wait_news, Rig};
use ok_store::{News, Store};
use ok_text::Doc;

const SOON: Duration = Duration::from_secs(10);

fn expect(store: &mut Store, what: &str, want: impl Fn(&News) -> bool) -> News {
    let mut seen = Vec::new();
    match wait_news(store, &mut seen, SOON, want) {
        Some(news) => news,
        None => panic!("waited for {what}; got only {seen:?}"),
    }
}

#[cfg(windows)]
/// The design's own test list: "Read during a truncate window never loads an
/// empty text." The store's test of that covers a text that is already open.
/// Opening one in that window falls back to a read with full sharing and
/// hands the interface whatever is there, which between another program's
/// truncate and its write is nothing.
#[test]
fn opening_a_text_between_another_programs_truncate_and_write_does_not_show_an_empty_page() {
    let rig = Rig::new("inv-open-truncated");
    let (mut store, _) = rig.open();
    let opened = store.create_text("essay.md").unwrap();
    let mut doc = Doc::new();
    let edit = type_end(&mut store, opened.doc, &mut doc, "Five paragraphs of essay.");
    save_wait(&mut store, opened.doc, &doc, edit);
    store.close(opened.doc, None, None);
    expect(&mut store, "Closed", |news| matches!(news, News::Closed { .. }));
    let path = rig.folder.join("essay.md");

    // Another program truncates the file and has not yet written its new version.
    let mut other = OpenOptions::new().write(true).share_mode(7).open(&path).unwrap();
    other.set_len(0).unwrap();
    let opened = store.open_text("essay.md");
    println!("opened in the truncate window: {:?}", opened.as_ref().map(|o| (o.text.clone(), o.read_only)));
    other.write_all(b"The outsider's whole new version.").unwrap();
    drop(other);
    let shown_empty = opened.as_ref().is_ok_and(|o| o.text.is_empty());
    if let Ok(opened) = &opened {
        // What the store does next: the whole text arrives as an outside change.
        let news = expect(&mut store, "ChangedOutside or Released", |news| matches!(news, News::ChangedOutside { .. } | News::Released { .. }));
        println!("after the writer let go: {news:?}");
        let versions = store.versions(opened.doc).unwrap();
        let kept: Vec<(Option<String>, String)> = versions.iter().map(|v| (v.label.clone(), store.read_version(opened.doc, &v.name).unwrap())).collect();
        println!("versions now: {kept:?}");
    }
    assert!(store.shutdown(SOON));
    assert!(!shown_empty, "a five-paragraph essay was opened as an empty text");
}
