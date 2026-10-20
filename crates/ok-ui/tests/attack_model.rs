//! The editing torture test: thousands of random keys, pastes, clicks and
//! resizes, every one checked against a reference model of the text and
//! against what must always be true of the page and the screen.
//!
//! A campaign runs many seeds over several starting texts. Each run stops
//! at its first failure; the failure is cut down to the few events that
//! cause it and printed, one for each kind of failure found. What has
//! already been reduced to a named test in `attack_found.rs` is passed over
//! here (see `Known`), so that these campaigns go on to the next thing.
//!
//! `OK_ATTACK_SCALE=20 cargo test -p ok-ui --offline --test attack_model`
//! runs twenty times as many seeds.

mod attack_support;

use std::collections::BTreeMap;

use attack_support::{random_events, report, run, scale, Checks, Ev, Known, Rng, Setup};

/// The sizes a window is dragged through while keys are going in.
const SIZES: [(usize, usize); 9] = [(960, 528), (960, 528), (1920, 1080), (640, 480), (400, 300), (1280, 720), (180, 44), (13, 23), (0, 0)];

/// What the named tests in `attack_found.rs` already hold.
fn known() -> Known {
    Known::default()
}

fn checks() -> Checks {
    Checks { known: known(), draw: true, undo_all: true }
}

/// Runs `seeds` seeds of `count` events over every start, and fails with
/// one reduced run for each kind of failure.
fn campaign(salt: u64, starts: &[Setup], seeds: u64, count: usize, checks: Checks) {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let mut runs = 0;
    for seed in 1..=seeds * scale() {
        for (which, setup) in starts.iter().enumerate() {
            let mut rng = Rng::new(salt * 1_000_000 + seed * 1000 + which as u64);
            let mut setup = setup.clone();
            setup.smooth = seed % 3 == 0;
            let mut events: Vec<Ev> = Vec::with_capacity(count + 8);
            while events.len() < count {
                random_events(&mut rng, &mut events, &SIZES);
            }
            runs += 1;
            if let Err((_, what)) = run(&setup, &events, checks) {
                if !found.contains_key(&what.kind) && found.len() < 12 {
                    if let Some((kind, text)) = report(&setup, &events, checks, seed) {
                        found.entry(kind).or_insert(text);
                    }
                }
            }
        }
    }
    let all: Vec<&str> = found.values().map(String::as_str).collect();
    assert!(found.is_empty(), "{} kinds of failure in {runs} runs:\n\n{}", found.len(), all.join("\n"));
}

const ESSAY: &str = "# The orchard\n\nThe orchard was eleven acres when my grandfather bought it and eleven acres when he stopped farming it, and in between he never once owned a tape measure longer than his arm. He kept the trees in rows he paced out in boots, one long step and one short.\n\n## What he did\n\n- thinned the fruit\n- kept *all* three\n  - [ ] and a nested one\n  - [x] done\n1. first\n2. second\n\n> It felt like **waste**. I filled buckets with fruit that would never ripen.\n\n```\nlet rows = 12;\n\n- not an item\n```\n\nHe let me think that for about a week, TK the year he planted it. See [[Grandfather interview]] and http://example.com/a.\n\n---\n\nI think about that tree whenever I am asked to add one more thing.";

fn hostile() -> String {
    format!(
        "e\u{0301}e\u{0301}\u{0308} \u{4E00}\u{4E8C}\u{4E09} tab\there \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467} and \u{2764}\u{FE0F}.\n\u{0301}\u{0301}leading marks\n\n# \u{4E00}\u{4E8C} wide heading {}\n\n> \u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{200F}quoted\n- \u{1F600} item\n\t- tabbed item\n",
        "wide\u{4E00} ".repeat(12)
    )
}

fn long_word() -> String {
    format!("before\n\n{}\n\nafter the word", "x".repeat(5000))
}

fn many_lines() -> String {
    (0..70).map(|n| if n % 9 == 4 { String::new() } else { format!("line {n} of the text, with a few words on it") }).collect::<Vec<_>>().join("\n")
}

#[test]
fn random_keys_from_an_empty_text_match_the_model() {
    campaign(1, &[Setup::new("")], 60, 400, checks());
}

#[test]
fn random_keys_over_an_essay_with_every_mark_match_the_model() {
    campaign(2, &[Setup::new(ESSAY), Setup::new(ESSAY).sized(1920, 1080)], 20, 300, checks());
}

#[test]
fn random_keys_over_marks_wide_characters_tabs_and_joined_emoji_match_the_model() {
    campaign(3, &[Setup::new(&hostile())], 40, 300, checks());
}

#[test]
fn random_keys_round_a_word_of_five_thousand_characters_match_the_model() {
    campaign(4, &[Setup::new(&long_word())], 12, 200, checks());
}

#[test]
fn random_keys_in_a_text_of_only_blank_lines_match_the_model() {
    campaign(5, &[Setup::new("\n\n\n\n\n\n\n"), Setup::new(" \n\t\n  \n")], 30, 300, checks());
}

#[test]
fn random_keys_in_a_text_that_came_with_crlf_match_the_model() {
    campaign(6, &[Setup::new("one\r\ntwo\r\n\r\n- three\r\n- four\rfive\r\n")], 20, 300, checks());
}

#[test]
fn random_keys_in_a_text_longer_than_the_sheet_match_the_model() {
    campaign(7, &[Setup::new(&many_lines()), Setup::new(&many_lines()).sized(1920, 1080)], 16, 300, checks());
}

#[test]
fn random_keys_with_the_platen_off_match_the_model() {
    let mut off = Setup::new(&many_lines());
    off.platen = false;
    campaign(8, &[off], 12, 300, checks());
}

/// Keys that arrive before the window has been given a size, and after it
/// has been taken down to nothing.
#[test]
fn random_keys_before_the_window_has_a_size_match_the_model() {
    campaign(9, &[Setup::new("").sized(0, 0), Setup::new(ESSAY).sized(0, 0), Setup::new(ESSAY).sized(5, 2000)], 16, 200, checks());
}
