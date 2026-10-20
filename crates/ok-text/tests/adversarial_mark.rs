//! Adversarial tests for the markup, links and fuzzy modules.
//!
//! Everything goes through the public API. Random tests are driven by a small
//! xorshift generator, so every run sees the same inputs, and compare against
//! slow reference readings of the contract or check its invariants.
//!
//! The tests in `mod reduced_failures`, at the end, each failed when they
//! were written. Each is the smallest case of one defect, since fixed, and
//! quotes the contract sentence it rests on. The property tests that had to
//! steer around those defects now run straight through them.

use ok_text::fuzzy::{fuzzy, rank, Match};
use ok_text::links::{key, link_at, links_in, retarget, LinkIndex, LinkRef, Outgoing};
use ok_text::markup::{classify, classify_line, hang_of, heading, outline, pieces, Heading, LineKind as K, Piece, Style as S};
use ok_text::{is_combining, str_width, Doc};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

// ---------------------------------------------------------------------------
// The generator and the hostile text it draws on
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() >> 16) as usize % n
        }
    }

    fn coin(&mut self) -> bool {
        self.below(2) == 0
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }

    fn concat(&mut self, parts: &[&str], most: usize) -> String {
        let n = self.below(most + 1);
        (0..n).map(|_| self.pick(parts)).collect()
    }
}

/// Words with no markup characters, no whitespace and no digits, but with
/// everything else a writer's text can hold: combining marks (single and
/// stacked), wide characters, emoji joined by U+200D, right-to-left text and
/// its marks, U+0130 and its relatives, Greek with a final sigma, and one
/// very long unbroken word.
const WORDS: &[&str] = &[
    "word",
    "Essay",
    "na\u{EF}ve",
    "re\u{301}sume\u{301}",
    "a\u{300}\u{301}\u{302}\u{303}\u{304}x",
    "mitsqanaqan\u{313}",
    "\u{65E5}\u{672C}\u{8A9E}",
    "\u{FF21}\u{FF22}",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{1F44D}\u{1F3FD}",
    "\u{2764}\u{FE0F}",
    "\u{5E9}\u{5DC}\u{5D5}\u{5DD}",
    "\u{645}\u{631}\u{62D}\u{628}\u{627}",
    "\u{200F}x\u{200E}",
    "\u{130}stanbul",
    "I\u{131}i\u{307}",
    "\u{3A3}\u{3AF}\u{3C3}\u{3C5}\u{3C6}\u{3BF}\u{3C2}",
    "Pneumonoultramicroscopicsilicovolcanoconiosis",
];

/// Every piece of markup the contract names, and the characters that sit
/// near its edges.
const MARKUP: &[&str] = &[
    "*", "**", "***", "****", "_", "__", "___", "`", "``", "```", "~~~", "[[", "]]", "[", "]", "(", ")", "|", "#", "##",
    "\\", ">", "-", "+", ".", "1.", "12)", "[ ]", "[x]", "http://", "https://", "HTTP://", "://", "h", ":", "/", "!", "?",
    ",", ";", "'", "\"", "\u{2019}", "\u{201D}", " ", " ", " ", "  ", "\t", "- ", "> ", "# ", "x", "a.b",
];

/// Characters that are hostile on their own: zero-width, wide, odd spaces,
/// controls and line separators that are not line breaks to `Doc`.
const ODD: &[&str] = &[
    "\u{301}", "\u{301}\u{301}\u{301}", "\u{313}", "\u{200D}", "\u{200B}", "\u{200F}", "\u{202E}", "\u{FEFF}", "\u{A0}",
    "\u{3000}", "\u{2028}", "\u{85}", "\u{0}", "\u{7F}", "\u{130}", "\u{131}", "\u{4E00}", "\u{1F600}", "\u{E9}",
    "\u{10FFFF}",
];

const KINDS: &[K] = &[
    K::Blank,
    K::Heading(0),
    K::Heading(1),
    K::Heading(6),
    K::Heading(255),
    K::Quote,
    K::Item,
    K::Fence,
    K::Code,
    K::Rule,
    K::Text,
];

fn hostile_line(rng: &mut Rng, most: usize) -> String {
    let n = rng.below(most + 1);
    (0..n)
        .map(|_| match rng.below(8) {
            0..=3 => rng.pick(MARKUP),
            4 | 5 => rng.pick(WORDS),
            _ => rng.pick(ODD),
        })
        .collect()
}

fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

// ---------------------------------------------------------------------------
// markup: helpers
// ---------------------------------------------------------------------------

fn show(line: &str, kind: K) -> Vec<(&str, S)> {
    pieces(line, kind).iter().map(|p| (&line[p.start..p.end], p.style)).collect()
}

/// The pieces of a line taken as the kind it really is.
fn real(line: &str) -> Vec<(&str, S)> {
    show(line, classify_line(line, false))
}

fn style_at(line: &str, at: usize) -> S {
    let found = pieces(line, classify_line(line, false));
    found.iter().find(|p| p.start <= at && at < p.end).map(|p| p.style).unwrap_or_else(|| panic!("no piece at {at} of {line:?}"))
}

fn styles_of(line: &str) -> Vec<S> {
    real(line).iter().map(|&(_, style)| style).collect()
}

/// Everything the contract promises of any `pieces` result.
fn sound(line: &str, kind: K) -> Vec<Piece> {
    let found = pieces(line, kind);
    if line.is_empty() {
        assert!(found.is_empty(), "an empty line gives an empty Vec ({kind:?})");
        return found;
    }
    assert!(!found.is_empty(), "{line:?} {kind:?}");
    assert_eq!(found[0].start, 0, "{line:?} {kind:?}");
    assert_eq!(found[found.len() - 1].end, line.len(), "{line:?} {kind:?}");
    for piece in &found {
        assert!(piece.start < piece.end, "empty piece in {line:?} {kind:?}");
        assert!(line.is_char_boundary(piece.start) && line.is_char_boundary(piece.end), "{line:?} {kind:?}");
    }
    for pair in found.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "gap or overlap in {line:?} {kind:?}");
        assert_ne!(pair[0].style, pair[1].style, "neighbours share a style in {line:?} {kind:?}");
    }
    match kind {
        K::Code | K::Fence => assert_eq!(found, [Piece { start: 0, end: line.len(), style: S::Plain }], "{line:?}"),
        K::Rule => assert_eq!(found, [Piece { start: 0, end: line.len(), style: S::Syntax }], "{line:?}"),
        _ => {}
    }
    found
}

/// The contract's block rules read as literally as they can be. Only spaces
/// count as indentation here, so the lines given to it hold no tabs.
fn ref_classify_line(line: &str, in_fence: bool) -> K {
    let spaces = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[spaces..];
    if spaces <= 3 && (rest.starts_with("```") || rest.starts_with("~~~")) {
        return K::Fence;
    }
    if in_fence {
        return K::Code;
    }
    if line.chars().all(char::is_whitespace) {
        return K::Blank;
    }
    let marks: Vec<char> = line.chars().filter(|&c| c != ' ').collect();
    if marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|&c| c == marks[0]) {
        return K::Rule;
    }
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && (line.len() == hashes || line[hashes..].starts_with(' ')) {
        return K::Heading(hashes as u8);
    }
    if rest.starts_with('>') {
        return K::Quote;
    }
    if ref_marker(rest).is_some() {
        return K::Item;
    }
    K::Text
}

/// Bytes of the list marker at the start of `rest`: the marker, its space,
/// and a task box with its space if one follows at once.
fn ref_marker(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut at = match bytes.first()? {
        b'-' | b'*' | b'+' => 1,
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    if bytes.get(at) != Some(&b' ') {
        return None;
    }
    at += 1;
    if ["[ ] ", "[x] ", "[X] "].iter().any(|task| rest[at..].starts_with(task)) {
        at += 4;
    }
    Some(at)
}

fn ref_hang(line: &str) -> usize {
    let spaces = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[spaces..];
    match ref_classify_line(line, false) {
        K::Item => spaces + ref_marker(rest).unwrap(),
        K::Quote => spaces + 1 + usize::from(rest[1..].starts_with(' ')),
        _ => 0,
    }
}

const BLOCK_PARTS: &[&str] = &[
    "#", "#", "##", "###", "######", "#######", " ", " ", "  ", "   ", "    ", ">", "-", "-", "*", "*", "_", "+", "1", "23",
    ".", ")", "[ ]", "[x]", "[X]", "[y]", "```", "~~~", "``", "~~", "word", "a", "\u{E9}", "\u{A0}", "\u{3000}", "- ", "* ",
    "> ", "# ", "1. ", "2) ", "- [ ] ", "\u{4E00}", "n\u{313}",
];

// ---------------------------------------------------------------------------
// markup: block level
// ---------------------------------------------------------------------------

#[test]
fn heading_is_one_to_six_hashes_then_a_space_or_the_end() {
    for n in 1..=6usize {
        let hashes = "#".repeat(n);
        assert_eq!(classify_line(&hashes, false), K::Heading(n as u8));
        assert_eq!(classify_line(&format!("{hashes} "), false), K::Heading(n as u8));
        assert_eq!(classify_line(&format!("{hashes} Title"), false), K::Heading(n as u8));
        assert_eq!(classify_line(&format!("{hashes}Title"), false), K::Text);
    }
    assert_eq!(classify_line("####### Seven", false), K::Text);
    assert_eq!(classify_line("#######", false), K::Text);
    assert_eq!(classify_line("#hashtag", false), K::Text);
    assert_eq!(classify_line("# \u{5E9}\u{5DC}\u{5D5}\u{5DD}", false), K::Heading(1));
    assert_eq!(classify_line("## \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", false), K::Heading(2));
}

#[test]
fn quote_is_optional_spaces_then_an_angle() {
    for line in [">", "> quoted", ">quoted", " > quoted", "       > deep", "> > nested", "> - item", "> # heading"] {
        assert_eq!(classify_line(line, false), K::Quote, "{line:?}");
    }
    for line in ["a > b", "\\> escaped", "x> y"] {
        assert_eq!(classify_line(line, false), K::Text, "{line:?}");
    }
}

#[test]
fn item_is_a_marker_and_a_space() {
    for line in [
        "- item", "* item", "+ item", "1. item", "1) item", "007. item", "1234567890123456789012345678901234567890. item",
        "   - item", "- ", "- [ ] task", "- [x] done", "- [X] done", "12) [ ] task", "* [ ] task", "- - nested",
    ] {
        assert_eq!(classify_line(line, false), K::Item, "{line:?}");
    }
    for line in ["-item", "*item", "+item", "1.item", "1 . item", "a. item", "-", "+", "1.", "1)", ". item", "1: item", "1.5 is a number", "1984 was a year"] {
        assert_eq!(classify_line(line, false), K::Text, "{line:?}");
    }
}

#[test]
fn fence_is_three_backticks_or_tildes_after_up_to_three_spaces() {
    for line in ["```", "~~~", " ```", "  ~~~", "   ```", "```rust", "~~~~", "````", "``` [[x]] *y*", "~~~ ~~~"] {
        assert_eq!(classify_line(line, false), K::Fence, "{line:?}");
        assert_eq!(classify_line(line, true), K::Fence, "{line:?}");
    }
    for line in ["    ```", "     ~~~", "``", "~~", "`` `", "~ ~ ~", "x```", "- ```"] {
        assert_ne!(classify_line(line, false), K::Fence, "{line:?}");
        assert_eq!(classify_line(line, true), K::Code, "{line:?}");
    }
}

#[test]
fn code_is_every_line_between_fences_and_an_unclosed_fence_runs_to_the_end() {
    let text = lines("before\n```\nx\n# h\n- i\n\n---\n> q\n```\nafter\n# Real");
    assert_eq!(
        classify(&text),
        [K::Text, K::Fence, K::Code, K::Code, K::Code, K::Code, K::Code, K::Code, K::Fence, K::Text, K::Heading(1)]
    );
    let text = lines("a\n~~~\nb\n# c\n\n- d");
    assert_eq!(classify(&text), [K::Text, K::Fence, K::Code, K::Code, K::Code, K::Code]);
    assert_eq!(classify(&[]), []);
    assert_eq!(classify(&lines("```")), [K::Fence]);
    for line in ["", "   ", "# h", "- i", "---", "> q", "plain"] {
        assert_eq!(classify_line(line, true), K::Code, "{line:?}");
    }
}

#[test]
fn rule_is_three_or_more_of_one_mark_and_wins_over_item() {
    for line in ["---", "***", "___", "- - -", "* * *", "_ _ _", "-----", "-  -  -", "**  *", "- - - - - - - -", "__ _"] {
        assert_eq!(classify_line(line, false), K::Rule, "{line:?}");
    }
    for line in ["--", "__", "- -", "-*-", "*-*", "--- x", "- - - x", "+++", "+ + +", "===", "-_-", "***bold***", "--- -x"] {
        assert_ne!(classify_line(line, false), K::Rule, "{line:?}");
    }
    assert_eq!(classify_line("- -", false), K::Item);
    assert_eq!(classify_line("+ + +", false), K::Item);
}

#[test]
fn blank_is_empty_or_only_whitespace() {
    for line in ["", " ", "      ", "\t", " \t ", "\u{A0}", "\u{3000}", " \u{2003}\u{2028} "] {
        assert_eq!(classify_line(line, false), K::Blank, "{line:?}");
    }
    assert_eq!(classify_line(&" ".repeat(100_000), false), K::Blank);
    for line in [".", "\u{301}", " x "] {
        assert_eq!(classify_line(line, false), K::Text, "{line:?}");
    }
}

#[test]
fn random_lines_classify_as_the_contract_reads() {
    let mut rng = Rng::new(1);
    let mut seen = BTreeMap::new();
    for _ in 0..40_000 {
        let line = match rng.below(4) {
            0 => rng.concat(&["-", "-", "- ", " -", "*", "* ", "_", "_ ", " ", "  ", "x"], 7),
            _ => rng.concat(BLOCK_PARTS, 6),
        };
        let want = ref_classify_line(&line, false);
        assert_eq!(classify_line(&line, false), want, "{line:?}");
        assert_eq!(classify_line(&line, true), ref_classify_line(&line, true), "{line:?} in a fence");
        assert_eq!(hang_of(&line), ref_hang(&line), "hang of {line:?}");
        match (heading(&line), want) {
            (Some((level, _)), K::Heading(n)) => assert_eq!(level, n, "{line:?}"),
            (None, K::Heading(_)) | (Some(_), _) => panic!("heading() and classify_line() disagree on {line:?}"),
            (None, _) => {}
        }
        *seen.entry(format!("{want:?}").chars().take(4).collect::<String>()).or_insert(0) += 1;
    }
    // Every kind a lone line can be turned up often enough to mean something.
    assert!(seen.len() >= 7 && seen.values().all(|&n| n > 200), "{seen:?}");
}

#[test]
fn random_documents_track_fences_line_by_line() {
    let mut rng = Rng::new(2);
    for _ in 0..2000 {
        let text: Vec<String> = (0..rng.below(24)).map(|_| rng.concat(BLOCK_PARTS, 3)).collect();
        let mut in_fence = false;
        let mut want = Vec::new();
        let mut heads = Vec::new();
        for (at, line) in text.iter().enumerate() {
            let kind = ref_classify_line(line, in_fence);
            if kind == K::Fence {
                in_fence = !in_fence;
            }
            if let K::Heading(level) = kind {
                heads.push((at, level));
            }
            want.push(kind);
        }
        assert_eq!(classify(&text), want, "{text:?}");
        let found: Vec<(usize, u8)> = outline(&text).iter().map(|h| (h.line, h.level)).collect();
        assert_eq!(found, heads, "{text:?}");
        for head in outline(&text) {
            assert_eq!(heading(&text[head.line]).map(|(level, title)| (level, title.to_string())), Some((head.level, head.title)));
        }
    }
}

#[test]
fn hang_of_covers_the_marker_and_nothing_else() {
    let cases: &[(&str, usize)] = &[
        ("- item", 2),
        ("* item", 2),
        ("+ item", 2),
        ("1. item", 3),
        ("10) item", 4),
        ("- [ ] task", 6),
        ("- [x] task", 6),
        ("- [X] task", 6),
        ("  - item", 4),
        ("   12. [X] task", 11),
        ("- \u{4E00}\u{4E00} wide text does not widen the marker", 2),
        ("- [y] not a box", 2),
        ("- [ ]", 2),
        ("> quote", 2),
        (">quote", 1),
        ("  > quote", 4),
        (">", 1),
        ("> ", 2),
        (">  two spaces", 2),
        ("", 0),
        ("    ", 0),
        ("# Heading", 0),
        ("plain text", 0),
        ("```", 0),
        ("---", 0),
        ("- - -", 0),
        ("***", 0),
        ("-item", 0),
        ("1.item", 0),
        ("\u{4E00}- item", 0),
    ];
    for &(line, hang) in cases {
        assert_eq!(hang_of(line), hang, "{line:?}");
    }
    // The extremes: nothing but indentation, and a marker a long way in.
    let deep = format!("{}- x", " ".repeat(50_000));
    assert_eq!(hang_of(&deep), 50_002);
    assert_eq!(hang_of(&" ".repeat(50_000)), 0);
    assert_eq!(hang_of(&format!("{}>", " ".repeat(9))), 10);
}

#[test]
fn random_hostile_lines_hang_no_wider_than_they_are() {
    let mut rng = Rng::new(3);
    for _ in 0..30_000 {
        let line = hostile_line(&mut rng, 10);
        let hang = hang_of(&line);
        assert!(hang <= str_width(&line), "{line:?}");
        let kind = classify_line(&line, false);
        if !matches!(kind, K::Item | K::Quote) {
            assert_eq!(hang, 0, "{line:?} is {kind:?}");
        } else {
            assert!(hang > 0, "{line:?} is {kind:?}");
        }
    }
}

#[test]
fn heading_gives_the_level_and_the_trimmed_title() {
    assert_eq!(heading("# Title"), Some((1, "Title")));
    assert_eq!(heading("## Two words ##"), Some((2, "Two words")));
    assert_eq!(heading("###   padded   "), Some((3, "padded")));
    assert_eq!(heading("###### six ######"), Some((6, "six")));
    assert_eq!(heading("# closed by more ########   "), Some((1, "closed by more")));
    assert_eq!(heading("#"), Some((1, "")));
    assert_eq!(heading("# "), Some((1, "")));
    assert_eq!(heading("## ##"), Some((2, "")));
    assert_eq!(heading("# a # b"), Some((1, "a # b")));
    assert_eq!(heading("# \u{3000}wide space\u{3000}"), Some((1, "wide space")));
    assert_eq!(heading("#no"), None);
    assert_eq!(heading("####### seven"), None);
    assert_eq!(heading("text # not"), None);
    assert_eq!(heading(""), None);
}

#[test]
fn random_headings_give_back_what_was_put_in() {
    let mut rng = Rng::new(4);
    for _ in 0..20_000 {
        let level = 1 + rng.below(6);
        let words: Vec<&str> = (0..rng.below(4)).map(|_| if rng.below(6) == 0 { "C#x" } else { rng.pick(WORDS) }).collect();
        let title = words.join(" ");
        let mut line = "#".repeat(level);
        if !title.is_empty() || rng.coin() {
            line.push_str(&" ".repeat(1 + rng.below(3)));
        }
        line.push_str(&title);
        if !title.is_empty() && rng.coin() {
            line.push_str(&" ".repeat(1 + rng.below(2)));
            line.push_str(&"#".repeat(1 + rng.below(8)));
        }
        line.push_str(&" ".repeat(rng.below(3)));
        assert_eq!(heading(&line), Some((level as u8, title.as_str())), "{line:?}");
        assert_eq!(classify_line(&line, false), K::Heading(level as u8), "{line:?}");
        assert_eq!(hang_of(&line), 0, "{line:?}");
        // The #s and the space after them are Syntax, and the title is not.
        let found = sound(&line, K::Heading(level as u8));
        assert_eq!((found[0].start, found[0].style), (0, S::Syntax), "{line:?}");
        assert!(found[0].end >= (level + 1).min(line.len()), "{line:?}");
        if let Some(first) = words.first() {
            let at = line.find(first).unwrap();
            assert_eq!(style_at(&line, at), S::Plain, "{line:?}");
        }
    }
}

#[test]
fn outline_lists_the_headings_outside_fences_in_order() {
    let text = lines("# One\ntext\n```\n# hidden\n```\n## Two ##\n   ~~~\n# also hidden\n~~~\n###### \u{130}stanbul\n#\n~~~\n# never closed");
    assert_eq!(
        outline(&text),
        [
            Heading { line: 0, level: 1, title: "One".to_string() },
            Heading { line: 5, level: 2, title: "Two".to_string() },
            Heading { line: 9, level: 6, title: "\u{130}stanbul".to_string() },
            Heading { line: 10, level: 1, title: String::new() },
        ]
    );
    assert_eq!(outline(&[]), []);
}

// ---------------------------------------------------------------------------
// markup: pieces
// ---------------------------------------------------------------------------

#[test]
fn block_markers_are_syntax() {
    // "the #s and the space after them"
    for (line, marker, text) in [("# Title", 2, 2), ("###### Title", 7, 7), ("#", 1, 1)] {
        for at in 0..marker {
            assert_eq!(style_at(line, at), S::Syntax, "{line:?} at {at}");
        }
        if text < line.len() {
            assert_eq!(style_at(line, text), S::Plain, "{line:?}");
        }
    }
    // "list markers and task boxes"
    for (line, marker, text) in [("- item", 1, 2), ("12. item", 3, 4), ("3) item", 2, 3), ("- [ ] task", 5, 6), ("  * [x] task", 7, 8)] {
        let indent = line.len() - line.trim_start().len();
        for at in 0..indent {
            assert_eq!(style_at(line, at), S::Plain, "{line:?} at {at}");
        }
        for at in indent..marker {
            assert_eq!(style_at(line, at), S::Syntax, "{line:?} at {at}");
        }
        assert_eq!(style_at(line, text), S::Plain, "{line:?}");
    }
    // "the > of a quote"
    assert_eq!(style_at("> quote", 0), S::Syntax);
    assert_eq!(style_at("> quote", 2), S::Plain);
    assert_eq!(style_at("   >quote", 3), S::Syntax);
    assert_eq!(style_at("   >quote", 4), S::Plain);
}

#[test]
fn strong_emph_and_both() {
    use S::*;
    assert_eq!(real("**strong**"), [("**", Syntax), ("strong", Strong), ("**", Syntax)]);
    assert_eq!(real("*emph*"), [("*", Syntax), ("emph", Emph), ("*", Syntax)]);
    assert_eq!(real("_emph_"), [("_", Syntax), ("emph", Emph), ("_", Syntax)]);
    assert_eq!(real("***both***"), [("***", Syntax), ("both", StrongEmph), ("***", Syntax)]);
    assert_eq!(
        real("a **b c** d *e* f _g_ h ***i***."),
        [
            ("a ", Plain),
            ("**", Syntax),
            ("b c", Strong),
            ("**", Syntax),
            (" d ", Plain),
            ("*", Syntax),
            ("e", Emph),
            ("*", Syntax),
            (" f ", Plain),
            ("_", Syntax),
            ("g", Emph),
            ("_", Syntax),
            (" h ", Plain),
            ("***", Syntax),
            ("i", StrongEmph),
            ("***", Syntax),
            (".", Plain),
        ]
    );
    // Hostile text inside the markers changes nothing.
    assert_eq!(real("*\u{5E9}\u{5DC}\u{5D5}\u{5DD}*"), [("*", Syntax), ("\u{5E9}\u{5DC}\u{5D5}\u{5DD}", Emph), ("*", Syntax)]);
    assert_eq!(real("**n\u{313}**"), [("**", Syntax), ("n\u{313}", Strong), ("**", Syntax)]);
    assert_eq!(
        real("_\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}_"),
        [("_", Syntax), ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", Emph), ("_", Syntax)]
    );
}

#[test]
fn underscores_count_only_at_word_boundaries() {
    for line in [
        "snake_case",
        "snake_case_name",
        "a snake_case_name here",
        "__init__py",
        "x_y_z and a_b_c",
        "1_000_000",
        "\u{65E5}\u{672C}_\u{8A9E}_\u{65E5}",
        "\u{E9}_x_\u{E9}",
        "trailing_ and _leading",
        "foo_bar_ baz",
    ] {
        assert_eq!(styles_of(line), [S::Plain], "{line:?}");
    }
    assert_eq!(styles_of("an _emph_ word"), [S::Plain, S::Syntax, S::Emph, S::Syntax, S::Plain]);
    assert_eq!(styles_of("(_emph_)"), [S::Plain, S::Syntax, S::Emph, S::Syntax, S::Plain]);
    assert_eq!(real("_snake_case_"), [("_", S::Syntax), ("snake_case", S::Emph), ("_", S::Syntax)]);
}

/// "snake_case is plain", in every script. A word here is anything whose
/// first and last characters with a cell of their own are letters or digits,
/// so words that end in a combining mark or an invisible one are in: see
/// `reduced_failures::an_underscore_after_a_combining_mark_is_inside_a_word`.
#[test]
fn random_names_joined_by_underscores_are_plain() {
    let lettered = |edge: Option<char>| edge.is_some_and(char::is_alphanumeric);
    let words: Vec<&str> = WORDS
        .iter()
        .copied()
        .chain(["x9", "2nd", "\u{E9}t\u{E9}"])
        .filter(|w| lettered(w.chars().find(|&c| !is_combining(c))) && lettered(w.chars().rev().find(|&c| !is_combining(c))))
        .collect();
    assert!(words.len() >= 15 && words.contains(&"re\u{301}sume\u{301}") && words.contains(&"\u{200F}x\u{200E}"));
    let mut rng = Rng::new(23);
    for _ in 0..20_000 {
        let mut name = String::from(rng.pick(&words));
        for _ in 0..1 + rng.below(3) {
            name.push_str(rng.pick(&["_", "_", "_", "__"]));
            name.push_str(rng.pick(&words));
        }
        let (before, after) = (rng.pick(&["", "see ", "(", "\u{201C}", "- ", "> "]), rng.pick(&["", " now", ")", ".", "\u{201D}", ", and"]));
        // Inside the name no underscore is at a word boundary. One at either
        // end is, but with nothing to pair with it is still plain.
        for line in [format!("{before}{name}{after}"), format!("{before}{name}_{after}"), format!("{before}_{name}{after}")] {
            let found = sound(&line, classify_line(&line, false));
            assert!(found.iter().all(|p| !matches!(p.style, S::Emph | S::Strong | S::StrongEmph)), "{line:?}");
            let at = line.find(&name).unwrap();
            assert!(found.iter().any(|p| p.style == S::Plain && p.start <= at && at + name.len() <= p.end), "{line:?}");
        }
        // One at each end is a pair, and the name between them is its text.
        let line = format!("{before}_{name}_{after}");
        let at = line.find(&name).unwrap();
        let found = sound(&line, classify_line(&line, false));
        assert!(found.contains(&Piece { start: at, end: at + name.len(), style: S::Emph }), "{line:?}");
    }
}

/// One pair of markers round hostile text, with every kind of neighbour:
/// it is emphasis exactly when both markers hug the text and, for
/// underscores, when both stand at word boundaries.
#[test]
fn random_single_pairs_are_emphasis_exactly_when_they_hug() {
    // What may stand before and after the pair, and whether the character
    // that touches the marker is part of a word. A letter is still the end
    // of a word with a combining mark or a joiner on it: see
    // `reduced_failures`.
    const BEFORE: &[(&str, bool)] = &[
        ("", false), ("a ", false), ("word ", false), ("(", false), ("\u{201C}", false), ("x\u{2014}", false), ("\u{1F600}", false),
        ("x", true), ("\u{E9}", true), ("\u{4E00}", true), ("9", true), ("\u{5E9}", true), ("\u{130}", true),
        ("re\u{301}", true), ("n\u{313}", true), ("x\u{200D}", true), ("a\u{300}\u{301}", true),
    ];
    const AFTER: &[(&str, bool)] = &[
        ("", false), (" b", false), (".", false), (")", false), ("\u{201D}", false), ("\u{2014}x", false), ("\u{1F600}", false),
        ("x", true), ("\u{E9}", true), ("\u{4E00}", true), ("9", true), ("\u{5E9}", true), ("\u{130}", true),
    ];
    const MARKS: &[(&str, S)] = &[("*", S::Emph), ("**", S::Strong), ("***", S::StrongEmph), ("_", S::Emph)];
    let mut rng = Rng::new(24);
    let (mut drawn, mut refused) = (0, 0);
    for _ in 0..40_000 {
        let ((before, word_before), (after, word_after)) = (rng.pick(BEFORE), rng.pick(AFTER));
        let (mark, style) = rng.pick(MARKS);
        let (lead, trail) = (rng.pick(&["", "", " ", "\t"]), rng.pick(&["", "", " ", "\t"]));
        let core = if rng.coin() { rng.pick(WORDS).to_string() } else { format!("{} {}", rng.pick(WORDS), rng.pick(WORDS)) };
        let inside = format!("{lead}{core}{trail}");
        let line = format!("{before}{mark}{inside}{mark}{after}");
        if classify_line(&line, false) != K::Text {
            continue;
        }
        let hugs = lead.is_empty() && trail.is_empty();
        let bounded = mark != "_" || (!word_before && !word_after);
        let mut b = Build::default();
        if hugs && bounded {
            b.add(before, S::Plain);
            b.add(mark, S::Syntax);
            b.add(&inside, style);
            b.add(mark, S::Syntax);
            b.add(after, S::Plain);
            drawn += 1;
        } else {
            b.add(&line, S::Plain);
            refused += 1;
        }
        let got: Vec<(String, S)> = sound(&line, K::Text).iter().map(|p| (line[p.start..p.end].to_string(), p.style)).collect();
        assert_eq!(got, b.want, "{line:?}");
    }
    assert!(drawn > 5000 && refused > 5000, "{drawn} {refused}");
}

#[test]
fn code_spans_interpret_nothing_inside() {
    use S::*;
    assert_eq!(real("`code`"), [("`", Syntax), ("code", Code), ("`", Syntax)]);
    let inner = "*a* **b** _c_ [[d]] [e](f) https://g.h \\* \\";
    let line = format!("x `{inner}` y");
    assert_eq!(real(&line), [("x ", Plain), ("`", Syntax), (inner, Code), ("`", Syntax), (" y", Plain)]);
    assert_eq!(real("`a` and `b`"), [("`", Syntax), ("a", Code), ("`", Syntax), (" and ", Plain), ("`", Syntax), ("b", Code), ("`", Syntax)]);
    assert_eq!(real("`\u{130}\u{4E00}n\u{313}`"), [("`", Syntax), ("\u{130}\u{4E00}n\u{313}", Code), ("`", Syntax)]);
}

#[test]
fn wiki_links_and_markdown_links() {
    use S::*;
    assert_eq!(real("[[target]]"), [("[[", Syntax), ("target", Link), ("]]", Syntax)]);
    assert_eq!(real("[[target|shown]]"), [("[[target|", Syntax), ("shown", Link), ("]]", Syntax)]);
    assert_eq!(
        real("see [[essays/common app|my essay]] now"),
        [("see ", Plain), ("[[essays/common app|", Syntax), ("my essay", Link), ("]]", Syntax), (" now", Plain)]
    );
    assert_eq!(real("[text](url)"), [("[", Syntax), ("text", Link), ("](", Syntax), ("url", Url), (")", Syntax)]);
    assert_eq!(
        real("a [b c](https://d.e/f) g"),
        [("a ", Plain), ("[", Syntax), ("b c", Link), ("](", Syntax), ("https://d.e/f", Url), (")", Syntax), (" g", Plain)]
    );
    assert_eq!(
        real("[[\u{65E5}\u{672C}|\u{5E9}\u{5DC}]]"),
        [("[[\u{65E5}\u{672C}|", Syntax), ("\u{5E9}\u{5DC}", Link), ("]]", Syntax)]
    );
}

#[test]
fn bare_addresses_end_before_trailing_punctuation() {
    use S::*;
    assert_eq!(real("https://example.com/a"), [("https://example.com/a", Url)]);
    assert_eq!(real("http://example.com"), [("http://example.com", Url)]);
    assert_eq!(real("see https://example.com/a now"), [("see ", Plain), ("https://example.com/a", Url), (" now", Plain)]);
    for tail in [".", ",", ";", ":", "!", "?", ")", "\"", "'", "\u{201D}", "\u{2019}", "...", "?!", ").", ".)", ".\u{201D}", ",\u{2019}"] {
        let line = format!("at https://example.com/a{tail}");
        assert_eq!(real(&line), [("at ", Plain), ("https://example.com/a", Url), (tail, Plain)], "{line:?}");
        let line = format!("at https://example.com/a{tail} and on");
        assert_eq!(real(&line)[1], ("https://example.com/a", Url), "{line:?}");
    }
    // Punctuation inside an address stays in it.
    assert_eq!(real("https://a.b/c?d=e;f:g,h!i"), [("https://a.b/c?d=e;f:g,h!i", Url)]);
    assert_eq!(real("(https://a.b/c)"), [("(", Plain), ("https://a.b/c", Url), (")", Plain)]);
    // Neither scheme alone is an address.
    assert_eq!(styles_of("http:// and https:// and ftp://x.y and http:/x"), [Plain]);
}

#[test]
fn random_addresses_are_found_whole() {
    const BODY: &[&str] = &["a", "b", "example", ".", "/", "-", "_", "~", "%20", "?q=", "&", "#frag", "=", "+", "1", "9", "\u{E9}", "\u{4E00}"];
    const TAILS: &[&str] = &[".", ",", ";", ":", "!", "?", ")", "'", "\"", "\u{201D}", "\u{2019}"];
    const BEFORE: &[&str] = &["", "see ", "(", "\u{201C}", "\"", "- ", "> ", "\u{4E00} ", "\u{5E9}\u{5DC} "];
    let mut rng = Rng::new(5);
    for _ in 0..20_000 {
        let scheme = rng.pick(&["http://", "https://"]);
        let address = format!("{scheme}{}{}", rng.concat(BODY, 6), rng.pick(&["a", "z", "0", "/", "\u{4E00}"]));
        let tail = rng.concat(TAILS, 3);
        let after = rng.pick(&["", " and more", " \u{5E9}", "\t"]);
        let line = format!("{}{address}{tail}{after}", rng.pick(BEFORE));
        let found = sound(&line, classify_line(&line, false));
        let urls: Vec<&str> = found.iter().filter(|p| p.style == S::Url).map(|p| &line[p.start..p.end]).collect();
        assert_eq!(urls, [address.as_str()], "{line:?}");
    }
}

#[test]
fn an_opener_with_no_closer_is_plain_text() {
    for line in [
        "*open", "**open", "***open", "_open", "__open", "`open", "``open", "[[open", "[[open]", "[[open|x", "[open](url",
        "[open]", "[open](", "open*", "open**", "open_", "open`", "open]]", "open)", "a *b and **c and _d and `e and [[f and [g](h",
        "*a **b _c",
    ] {
        assert_eq!(styles_of(line), [S::Plain], "{line:?}");
    }
}

#[test]
fn emphasis_markers_must_hug_their_text() {
    for line in [
        "2 * 3 * 4", "a * b * c", "a ** b ** c", "a _ b _ c", "a *b * c", "a * b* c", "** not strong **", "_ not emph _",
        "x *\u{A0}y\u{A0}* z", "x *\u{3000}y\u{3000}* z",
    ] {
        assert_eq!(styles_of(line), [S::Plain], "{line:?}");
    }
}

#[test]
fn a_backslash_makes_the_markup_character_after_it_plain() {
    use S::*;
    // The characters the contract's own rules give a meaning to.
    for c in ['*', '_', '`', '[', ']', '(', ')', '#', '>', '-', '+', '\\'] {
        let line = format!("a \\{c} b");
        assert_eq!(real(&line), [("a ", Plain), ("\\", Syntax), (&line[3..], Plain)], "{line:?}");
    }
    assert_eq!(real("\\*not emph\\*"), [("\\", Syntax), ("*not emph", Plain), ("\\", Syntax), ("*", Plain)]);
    assert_eq!(real("\\**not strong\\**").iter().filter(|p| p.1 == Strong).count(), 0);
    assert_eq!(real("\\_not emph\\_"), [("\\", Syntax), ("_not emph", Plain), ("\\", Syntax), ("_", Plain)]);
    assert_eq!(real("\\`not code\\`"), [("\\", Syntax), ("`not code", Plain), ("\\", Syntax), ("`", Plain)]);
    assert_eq!(real("\\[not a link](url)"), [("\\", Syntax), ("[not a link](url)", Plain)]);
    assert_eq!(real("\\# not a heading"), [("\\", Syntax), ("# not a heading", Plain)]);
    assert_eq!(real("\\> not a quote"), [("\\", Syntax), ("> not a quote", Plain)]);
    assert_eq!(real("\\- not an item"), [("\\", Syntax), ("- not an item", Plain)]);
    assert_eq!(real("\\+ not an item"), [("\\", Syntax), ("+ not an item", Plain)]);
    // A backslash before anything else is just a backslash.
    assert_eq!(styles_of("C:\\Users\\me\\notes and a\\ b \\"), [Plain]);
    assert_eq!(styles_of("\\\u{E9} \\\u{4E00} \\\u{301}"), [Plain]);
}

#[test]
fn code_and_fence_lines_are_plain_and_rule_lines_are_syntax() {
    for line in ["# **x** [[y]] `z` https://a.b", "```rust", "~~~", "   ```", "- [ ] *task*", "\u{130}\u{301}\u{4E00}"] {
        assert_eq!(show(line, K::Code), [(line, S::Plain)]);
        assert_eq!(show(line, K::Fence), [(line, S::Plain)]);
    }
    for line in ["---", "***", "___", " - - - ", "* * *"] {
        assert_eq!(real(line), [(line, S::Syntax)]);
    }
    for kind in KINDS {
        assert_eq!(pieces("", *kind), [], "{kind:?}");
    }
}

#[test]
fn there_is_no_nesting_but_both() {
    use S::*;
    assert_eq!(real("**a *b* c**"), [("**", Syntax), ("a *b* c", Strong), ("**", Syntax)]);
    assert_eq!(real("*a **b** c*"), [("*", Syntax), ("a **b** c", Emph), ("*", Syntax)]);
    assert_eq!(real("*a _b_ c*"), [("*", Syntax), ("a _b_ c", Emph), ("*", Syntax)]);
    assert_eq!(real("[*t* `c`](u)"), [("[", Syntax), ("*t* `c`", Link), ("](", Syntax), ("u", Url), (")", Syntax)]);
    assert_eq!(real("[[a|*b* `c`]]"), [("[[a|", Syntax), ("*b* `c`", Link), ("]]", Syntax)]);
}

/// A line under construction together with the pieces it must come to.
#[derive(Default)]
struct Build {
    line: String,
    want: Vec<(String, S)>,
}

impl Build {
    fn add(&mut self, text: &str, style: S) {
        if text.is_empty() {
            return;
        }
        self.line.push_str(text);
        match self.want.last_mut() {
            Some(last) if last.1 == style => last.0.push_str(text),
            _ => self.want.push((text.to_string(), style)),
        }
    }
}

/// Lines made only of well-formed constructs, each set off by spaces, with
/// hostile words inside them. There is exactly one way to read such a line.
#[test]
fn random_well_formed_lines_split_exactly_as_built() {
    use S::*;
    const JOINS: &[&str] = &[
        " ", " ", " ", ", ", ". ", "; ", ": ", " (", ") ", " \u{2014} ", "\u{3000}", "\u{A0}", "(", ")", ",", ".", "'", "\"", "!", "?",
    ];
    const AFTER_ADDRESS: &[&str] = &[" ", " ", ", ", ". ", "; ", ": ", ") ", "! ", "? ", "' ", "\" ", "\u{3000}", "\u{A0}", ".\u{201D} "];
    let mut rng = Rng::new(6);
    let mut drawn = [0usize; 8];
    for round in 0..30_000 {
        let mut b = Build::default();
        let kind = match rng.below(12) {
            0 => {
                let level = 1 + rng.below(6);
                b.add(&"#".repeat(level), Syntax);
                b.add(" ", Syntax);
                K::Heading(level as u8)
            }
            1 | 2 => {
                b.add(rng.pick(&["", "  ", "     "]), Plain);
                b.add(rng.pick(&["- ", "* ", "+ ", "1. ", "23) ", "- [ ] ", "* [x] ", "7. [X] "]), Syntax);
                K::Item
            }
            3 => {
                b.add(rng.pick(&["", " ", "   "]), Plain);
                b.add(">", Syntax);
                b.add(" ", Plain);
                K::Quote
            }
            _ => K::Text,
        };
        let mut after_address = false;
        for n in 0..1 + rng.below(5) {
            if n > 0 {
                // Punctuation may touch a construct on either side. After an
                // address it has to be the kind that ends one.
                b.add(rng.pick(if after_address { AFTER_ADDRESS } else { JOINS }), Plain);
            }
            let (w, v) = (rng.pick(WORDS), rng.pick(WORDS));
            let two = format!("{w} {v}");
            let inside = if rng.coin() { w } else { two.as_str() };
            let which = rng.below(12);
            after_address = which >= 10;
            match which {
                0 | 1 => b.add(w, Plain),
                2 => {
                    b.add("*", Syntax);
                    b.add(inside, Emph);
                    b.add("*", Syntax);
                }
                3 => {
                    b.add("**", Syntax);
                    b.add(inside, Strong);
                    b.add("**", Syntax);
                }
                4 => {
                    b.add("***", Syntax);
                    b.add(inside, StrongEmph);
                    b.add("***", Syntax);
                }
                5 => {
                    b.add("_", Syntax);
                    b.add(inside, Emph);
                    b.add("_", Syntax);
                }
                6 => {
                    b.add("`", Syntax);
                    b.add(inside, S::Code);
                    b.add("`", Syntax);
                }
                7 => {
                    b.add("[[", Syntax);
                    b.add(inside, Link);
                    b.add("]]", Syntax);
                }
                8 => {
                    b.add(&format!("[[{w}|"), Syntax);
                    b.add(inside, Link);
                    b.add("]]", Syntax);
                }
                9 => {
                    b.add("[", Syntax);
                    b.add(inside, Link);
                    b.add("](", Syntax);
                    b.add(&format!("notes/{v}"), Url);
                    b.add(")", Syntax);
                }
                _ => b.add(&format!("https://{w}.example/{v}"), Url),
            }
        }
        assert_eq!(classify_line(&b.line, false), kind, "{:?}", b.line);
        let found = sound(&b.line, kind);
        let got: Vec<(String, S)> = found.iter().map(|p| (b.line[p.start..p.end].to_string(), p.style)).collect();
        assert_eq!(got, b.want, "{:?}", b.line);
        for piece in &found {
            drawn[piece.style as usize] += 1;
        }
        // And while it was being typed, a character at a time, every state
        // of it split soundly too.
        if round % 10 == 0 {
            for (at, _) in b.line.char_indices() {
                let typed = &b.line[..at];
                sound(typed, classify_line(typed, false));
            }
        }
    }
    assert!(drawn.iter().all(|&n| n > 1000), "{drawn:?}");
}

#[test]
fn random_plain_prose_is_one_plain_piece() {
    const JOINS: &[&str] = &[" ", " ", ", ", ". ", "; ", ": ", "! ", "? ", " (", ") ", "'", "\"", "\u{2014}", "\u{3000}", "\u{A0}", " - ", " 3 "];
    let mut rng = Rng::new(7);
    for _ in 0..10_000 {
        let mut line = String::from(rng.pick(WORDS));
        for _ in 0..rng.below(8) {
            line.push_str(rng.pick(JOINS));
            line.push_str(rng.pick(WORDS));
        }
        assert_eq!(classify_line(&line, false), K::Text, "{line:?}");
        assert_eq!(real(&line), [(line.as_str(), S::Plain)]);
        assert_eq!(hang_of(&line), 0);
        assert_eq!(links_in(&line), []);
    }
}

#[test]
fn random_hostile_lines_always_split_into_sound_pieces() {
    let mut rng = Rng::new(8);
    let mut drawn = [0usize; 8];
    for round in 0..40_000 {
        let line = hostile_line(&mut rng, if round % 50 == 0 { 400 } else { 14 });
        for &kind in KINDS {
            sound(&line, kind);
        }
        for piece in sound(&line, classify_line(&line, false)) {
            drawn[piece.style as usize] += 1;
        }
        if let Some((level, title)) = heading(&line) {
            assert!((1..=6).contains(&level) && title == title.trim() && line.contains(title), "{line:?}");
        }
    }
    assert!(drawn.iter().all(|&n| n > 200), "{drawn:?}");
}

/// What is drawn as a wiki link and what `links_in` finds are one and the
/// same, and every other `Link` piece is the text of a markdown link.
#[test]
fn random_lines_draw_exactly_the_links_that_can_be_followed() {
    let mut rng = Rng::new(9);
    let mut links_seen = 0;
    for _ in 0..40_000 {
        let line: String = (0..rng.below(8))
            .map(|_| match rng.below(4) {
                0 => rng.pick(&["[[n]]", "[[n|s]]", "[[n#h]]", "[[ n # h | s ]]", "[[n|]]", "[t](u)", "[[\u{130}|n\u{313}]]", "[[a/b.md]]"]).to_string(),
                _ => hostile_line(&mut rng, 3),
            })
            .collect();
        let found = sound(&line, classify_line(&line, false));
        let links = links_in(&line);
        links_seen += links.len();
        let style_at = |at: usize| found.iter().find(|p| p.start <= at && at < p.end).map(|p| p.style);
        for link in &links {
            // The brackets are Syntax, the text a reader sees is Link, and
            // nothing between the brackets is anything else.
            for at in [link.start, link.start + 1, link.end - 2, link.end - 1] {
                assert_eq!(style_at(at), Some(S::Syntax), "{line:?} at {at}");
            }
            for at in link.start..link.end {
                assert!(matches!(style_at(at), Some(S::Syntax | S::Link)), "{line:?} at {at}");
            }
            let shown = link.shown.unwrap_or(link.target);
            let at = shown.as_ptr() as usize - line.as_ptr() as usize;
            assert_eq!(style_at(at), Some(S::Link), "{line:?} at {at}");
            assert_eq!(style_at(at + shown.len() - 1), Some(S::Link), "{line:?}");
        }
        for piece in found.iter().filter(|p| p.style == S::Link) {
            let wiki = links.iter().any(|l| l.start < piece.start && piece.end < l.end);
            assert!(wiki || line[piece.end..].starts_with("]("), "{line:?}");
        }
    }
    assert!(links_seen > 5000, "{links_seen}");
}

/// A block marker is drawn as Syntax and the text after it reads exactly as
/// it would as a paragraph of its own.
#[test]
fn block_markers_leave_the_inline_reading_alone() {
    let mut rng = Rng::new(21);
    let mut styled = 0;
    for _ in 0..30_000 {
        let text = if rng.coin() { hostile_line(&mut rng, 10) } else { format!("{} {}", rng.pick(WORDS), hostile_line(&mut rng, 8)) };
        let inline: Vec<(String, S)> = sound(&text, K::Text).iter().map(|p| (text[p.start..p.end].to_string(), p.style)).collect();
        styled += inline.iter().filter(|(_, style)| !matches!(style, S::Plain | S::Syntax)).count();
        let boxed = ["[ ] ", "[x] ", "[X] ", "[ ]\t", "[x]\t", "[X]\t"].iter().any(|task| text.starts_with(task));
        let closed = text.trim_end().ends_with('#');
        let cases: &[(&str, &str, K, bool)] = &[
            ("", "- ", K::Item, !boxed),
            ("   ", "* ", K::Item, !boxed),
            ("", "12. ", K::Item, !boxed),
            (" ", "3) ", K::Item, !boxed),
            ("", "- [ ] ", K::Item, true),
            ("  ", "+ [X] ", K::Item, true),
            ("", ">", K::Quote, true),
            ("    ", ">", K::Quote, true),
            ("", "# ", K::Heading(1), !closed),
            ("", "###### ", K::Heading(6), !closed),
        ];
        for &(indent, marker, kind, fair) in cases {
            if !fair {
                continue;
            }
            let mut b = Build::default();
            b.add(indent, S::Plain);
            b.add(marker, S::Syntax);
            for (part, style) in &inline {
                b.add(part, *style);
            }
            let got: Vec<(String, S)> = sound(&b.line, kind).iter().map(|p| (b.line[p.start..p.end].to_string(), p.style)).collect();
            assert_eq!(got, b.want, "{:?} as {kind:?}", b.line);
        }
    }
    assert!(styled > 1000, "{styled}");
}

/// "A backslash before a markup character makes that character plain": with
/// one before every such character, nothing on the line is markup any more.
#[test]
fn escaping_every_markup_character_leaves_nothing_styled() {
    const ESCAPED: &[char] = &['\\', '`', '*', '_', '[', ']', '(', ')', '#', '>', '-', '+'];
    let mut rng = Rng::new(22);
    let mut escapes = 0;
    for _ in 0..30_000 {
        // No addresses: a backslash is not what ends one.
        let text = hostile_line(&mut rng, 12).replace("://", ":");
        let mut line = String::new();
        let mut put = Vec::new();
        for c in text.chars() {
            if ESCAPED.contains(&c) {
                put.push(line.len());
                line.push('\\');
            }
            line.push(c);
        }
        let kind = classify_line(&line, false);
        assert!(!matches!(kind, K::Heading(_) | K::Quote | K::Rule) || put.is_empty(), "{line:?} is {kind:?}");
        let found = sound(&line, kind);
        if kind == K::Fence {
            continue;
        }
        let style_at = |at: usize| found.iter().find(|p| p.start <= at && at < p.end).map(|p| p.style);
        assert!(found.iter().all(|p| matches!(p.style, S::Plain | S::Syntax)), "{line:?} {found:?}");
        for &at in &put {
            assert_eq!(style_at(at), Some(S::Syntax), "{line:?} at {at}");
            assert_eq!(style_at(at + 1), Some(S::Plain), "{line:?} at {}", at + 1);
        }
        assert_eq!(links_in(&line), [], "{line:?}");
        escapes += put.len();
    }
    assert!(escapes > 50_000, "{escapes}");
}

// ---------------------------------------------------------------------------
// links: finding them in a line
// ---------------------------------------------------------------------------

fn link<'a>(start: usize, end: usize, target: &'a str, shown: Option<&'a str>, heading: Option<&'a str>) -> LinkRef<'a> {
    LinkRef { start, end, target, shown, heading }
}

/// The links of a line found the slow, obvious way. Right only for lines
/// with no backticks, backslashes or tildes, where nothing can hide a link.
fn ref_links(line: &str) -> Vec<LinkRef<'_>> {
    let bytes = line.as_bytes();
    let filled = |part: &&str| !part.is_empty();
    let mut found = Vec::new();
    let mut at = 0;
    while at + 1 < bytes.len() {
        if &bytes[at..at + 2] == b"[[" && bytes.get(at + 2) != Some(&b'[') {
            let rest = &line[at + 2..];
            let close = match (rest.find("]]"), rest.find("[[")) {
                (Some(close), Some(open)) if open < close => None,
                (close, _) => close,
            };
            if let Some(close) = close {
                let inner = &rest[..close];
                let (left, shown) = match inner.find('|') {
                    Some(bar) => (&inner[..bar], Some(inner[bar + 1..].trim())),
                    None => (inner, None),
                };
                let (target, head) = match left.find('#') {
                    Some(hash) => (&left[..hash], Some(left[hash + 1..].trim())),
                    None => (left, None),
                };
                if !target.trim().is_empty() {
                    let end = at + 2 + close + 2;
                    found.push(link(at, end, target.trim(), shown.filter(filled), head.filter(filled)));
                    at = end;
                    continue;
                }
            }
        }
        at += 1;
    }
    found
}

#[test]
fn links_have_a_target_a_heading_and_a_shown_part() {
    assert_eq!(links_in("[[a#b|c]]"), [link(0, 9, "a", Some("c"), Some("b"))]);
    assert_eq!(links_in("[[a]]"), [link(0, 5, "a", None, None)]);
    assert_eq!(links_in("[[a|c]]"), [link(0, 7, "a", Some("c"), None)]);
    assert_eq!(links_in("[[a#b]]"), [link(0, 7, "a", None, Some("b"))]);
    assert_eq!(links_in("x [[  essays/common app  ]] y"), [link(2, 27, "essays/common app", None, None)]);
    assert_eq!(links_in("[[ a # b | c ]]"), [link(0, 15, "a", Some("c"), Some("b"))]);
    assert_eq!(links_in("[[a]][[b]] [[c]]"), [link(0, 5, "a", None, None), link(5, 10, "b", None, None), link(11, 16, "c", None, None)]);
    assert_eq!(
        links_in("\u{4E00}[[\u{130}stanbul#\u{5E9}|n\u{313}]]\u{301}"),
        [link(3, 23,"\u{130}stanbul", Some("n\u{313}"), Some("\u{5E9}"))]
    );
    assert_eq!(links_in("# [[a]] and - [[b]]").len(), 2);
    assert_eq!(links_in("- [ ] [[a]]").len(), 1);
    assert_eq!(links_in("> [[a]]").len(), 1);
}

#[test]
fn empty_targets_are_not_links() {
    for line in ["[[]]", "[[ ]]", "[[   ]]", "[[\u{3000}]]", "[[", "]]", "[[a", "a]]", "[a]", "[ [a] ]", ""] {
        assert_eq!(links_in(line), [], "{line:?}");
    }
}

#[test]
fn links_do_not_nest_and_code_hides_them() {
    assert_eq!(links_in("[[a [[b]] c]]"), [link(4, 9, "b", None, None)]);
    assert_eq!(links_in("`[[a]]`"), []);
    assert_eq!(links_in("x `[[a]]` y `z [[b|c]]` [[d]]"), [link(24, 29, "d", None, None)]);
    assert_eq!(links_in("`code` [[a]] `code`"), [link(7, 12, "a", None, None)]);
    // A lone backtick opens nothing.
    assert_eq!(links_in("` [[a]]"), [link(2, 7, "a", None, None)]);
}

#[test]
fn link_at_takes_any_column_from_the_first_bracket_to_the_last() {
    let line = "\u{E9}\u{4E00} [[c\u{301}d|\u{1F600}]] ef";
    let found = links_in(line);
    assert_eq!(found.len(), 1);
    let (start, end) = (found[0].start, found[0].end);
    assert_eq!((start, end), (6, 19));
    // Every column, on a character boundary or inside one, and well past the end.
    for col in 0..line.len() + 5 {
        assert_eq!(link_at(line, col), (start <= col && col < end).then_some(found[0]), "col {col}");
    }
    assert_eq!(link_at(line, usize::MAX), None);
    assert_eq!(link_at("", 0), None);
    assert_eq!(link_at("", 7), None);
    assert_eq!(link_at("[[a]][[b]]", 4).map(|l| l.target), Some("a"));
    assert_eq!(link_at("[[a]][[b]]", 5).map(|l| l.target), Some("b"));
    assert_eq!(link_at("[[a]][[b]]", 10), None);
}

#[test]
fn random_lines_hold_exactly_the_links_a_slow_search_finds() {
    const PARTS: &[&str] = &[
        "[[", "]]", "[[", "]]", "[[", "]]", "[", "]", "|", "#", " ", "  ", "a", "B c", "(", ")", "*", "_", "- ", "> ", "# ",
        "1. ", "[ ] ", "[t](u)", "http://x.y", "[[n]]", "[[n#h|s]]", "[[ n | s ]]", "\t", ".md", "/",
    ];
    let mut rng = Rng::new(10);
    let (mut total, mut parted) = (0, 0);
    for _ in 0..40_000 {
        let line: String = (0..rng.below(16))
            .map(|_| match rng.below(5) {
                0 => rng.pick(WORDS),
                1 if rng.coin() => rng.pick(ODD),
                _ => rng.pick(PARTS),
            })
            .collect();
        let want = ref_links(&line);
        let found = links_in(&line);
        assert_eq!(found, want, "{line:?}");
        total += found.len();
        parted += found.iter().filter(|l| l.shown.is_some() || l.heading.is_some()).count();
        let mut last_end = 0;
        for link in &found {
            assert!(last_end <= link.start && link.end <= line.len(), "{line:?}");
            last_end = link.end;
            assert!(line[link.start..].starts_with("[[") && line[..link.end].ends_with("]]"), "{line:?}");
            assert!(!link.target.is_empty() && link.target == link.target.trim(), "{line:?}");
            assert!(!link.target.contains(['|', '#']), "{line:?}");
        }
        for col in 0..line.len() + 2 {
            let want = want.iter().find(|l| l.start <= col && col < l.end).copied();
            assert_eq!(link_at(&line, col), want, "{line:?} col {col}");
        }
    }
    assert!(total > 8000 && parted > 1000, "{total} {parted}");
}

// ---------------------------------------------------------------------------
// links: key
// ---------------------------------------------------------------------------

#[test]
fn key_is_the_comparison_form_of_a_name() {
    assert_eq!(key("essays/common app"), "essays/common app");
    assert_eq!(key("Essays\\Common App.md"), "essays/common app");
    assert_eq!(key("  /essays/common app/  "), "essays/common app");
    assert_eq!(key("\\essays\\common app\\"), "essays/common app");
    assert_eq!(key("Common   App.MARKDOWN"), "common app");
    assert_eq!(key("NOTES.TXT"), "notes");
    assert_eq!(key("notes.Md"), "notes");
    assert_eq!(key("a\t \u{3000}b"), "a b");
    assert_eq!(key("notes.mdx"), "notes.mdx");
    assert_eq!(key("notes.text"), "notes.text");
    assert_eq!(key("md"), "md");
    assert_eq!(key("txt"), "txt");
    assert_eq!(key(""), "");
    assert_eq!(key("   "), "");
    assert_eq!(key("\u{C9}COLE"), "\u{E9}cole");
    assert_eq!(key("\u{130}STANBUL"), "\u{130}STANBUL".to_lowercase());
    assert_eq!(key("\u{39F}\u{394}\u{39F}\u{3A3}"), "\u{3BF}\u{3B4}\u{3BF}\u{3C2}");
    assert_eq!(key("\u{65E5}\u{672C}\u{8A9E}.md"), "\u{65E5}\u{672C}\u{8A9E}");
}

#[test]
fn random_names_built_in_parts_have_the_key_of_their_parts() {
    const GAPS: &[&str] = &[" ", "  ", "\t", " \t ", "\u{A0}", "\u{3000}"];
    const EDGES: &[&str] = &["", "", " ", "/", "\\", " / ", "\t\\/ "];
    const EXTS: &[&str] = &["", "", ".md", ".MD", ".Md", ".markdown", ".MarkDown", ".txt", ".TXT"];
    let mut rng = Rng::new(11);
    for _ in 0..20_000 {
        let folders: Vec<Vec<&str>> = (0..1 + rng.below(3)).map(|_| (0..1 + rng.below(3)).map(|_| rng.pick(WORDS)).collect()).collect();
        let plain: Vec<String> = folders.iter().map(|words| words.join(" ")).collect();
        // The key also folds look-alike apostrophes and quotes and drops invisible characters.
        let want: String = plain
            .join("/")
            .chars()
            .filter(|c| !matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}'))
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
                '\u{201C}' | '\u{201D}' => '"',
                c => c,
            })
            .collect::<String>()
            .to_lowercase();

        let mut name = String::from(rng.pick(EDGES));
        for (n, words) in folders.iter().enumerate() {
            if n > 0 {
                name.push(rng.pick(&['/', '\\']));
            }
            for (m, word) in words.iter().enumerate() {
                if m > 0 {
                    name.push_str(rng.pick(GAPS));
                }
                name.push_str(word);
            }
        }
        name.push_str(rng.pick(EXTS));
        name.push_str(rng.pick(EDGES));
        assert_eq!(key(&name), want, "{name:?}");
    }
}

#[test]
fn random_hostile_names_have_well_formed_keys() {
    const PARTS: &[&str] = &["/", "\\", " ", "  ", "\t", ".md", ".MD", ".markdown", ".txt", ".", "a", "B", "md", "\u{A0}", "\u{3000}"];
    let mut rng = Rng::new(12);
    for _ in 0..40_000 {
        let name: String = (0..rng.below(9))
            .map(|_| match rng.below(4) {
                0 => rng.pick(WORDS),
                1 => rng.pick(ODD),
                _ => rng.pick(PARTS),
            })
            .collect();
        let k = key(&name);
        assert!(!k.contains('\\'), "{name:?} -> {k:?}");
        assert!(!k.starts_with('/') && !k.ends_with('/'), "{name:?} -> {k:?}");
        assert_eq!(k, k.trim(), "{name:?}");
        assert!(!k.contains("  ") && k.chars().all(|c| c == ' ' || !c.is_whitespace()), "{name:?} -> {k:?}");
        assert_eq!(k, k.to_lowercase(), "{name:?}");
        // Whitespace and slashes at either end go, whatever the mix.
        assert_eq!(key(&format!(" /\\\t{name} \\/ ")), k, "{name:?}");
        // Which way the slashes lean never matters.
        assert_eq!(key(&name.replace('/', "\\")), key(&name.replace('\\', "/")), "{name:?}");
    }
}

// ---------------------------------------------------------------------------
// links: the index
// ---------------------------------------------------------------------------

fn out(target: &str, resolved: Option<&str>) -> Outgoing {
    Outgoing { target: target.to_string(), resolved: resolved.map(str::to_string) }
}

#[test]
fn resolve_takes_the_full_name_then_the_last_component() {
    let mut index = LinkIndex::new();
    for name in ["essays/common app", "drafts/2024/common app", "notes", "b/leaf", "a/leaf", "deep/er/leaf", "x/notes"] {
        index.set(name, "");
    }
    // A known name whose key equals key(target).
    assert_eq!(index.resolve("essays/common app"), Some("essays/common app"));
    assert_eq!(index.resolve("  Essays\\Common  App.MD "), Some("essays/common app"));
    assert_eq!(index.resolve("notes"), Some("notes"));
    // Failing that, a name whose last path component has that key; the
    // fewest path components win, then the first alphabetically by key.
    assert_eq!(index.resolve("common app"), Some("essays/common app"));
    assert_eq!(index.resolve("Common App.txt"), Some("essays/common app"));
    assert_eq!(index.resolve("leaf"), Some("a/leaf"));
    index.remove("a/leaf");
    assert_eq!(index.resolve("leaf"), Some("b/leaf"));
    index.remove("b/leaf");
    assert_eq!(index.resolve("leaf"), Some("deep/er/leaf"));
    index.set("leaf", "");
    assert_eq!(index.resolve("leaf"), Some("leaf"));
    // Only the last component counts, not a longer tail of the path.
    assert_eq!(index.resolve("2024/common app"), None);
    assert_eq!(index.resolve("essays"), None);
    assert_eq!(index.resolve("nothing"), None);
    assert_eq!(index.resolve(""), None);
    assert_eq!(LinkIndex::new().resolve("x"), None);
}

#[test]
fn names_are_sorted_by_key_and_contains_follows_set_and_remove() {
    let mut index = LinkIndex::new();
    assert_eq!(index.names(), [] as [&str; 0]);
    for name in ["zebra", "apple/pie", "Mango", "apple", "\u{E9}clair", "\u{65E5}\u{8A18}"] {
        index.set(name, "");
        assert!(index.contains(name));
    }
    let names = index.names();
    let mut sorted = names.clone();
    sorted.sort_by_key(|name| key(name));
    assert_eq!(names, sorted);
    assert_eq!(names.len(), 6);
    index.remove("Mango");
    assert!(!index.contains("Mango"));
    index.remove("never there");
    assert_eq!(index.names().len(), 5);
}

#[test]
fn outgoing_incoming_and_dangling() {
    let mut index = LinkIndex::new();
    index.set("a", "to [[b]], again [[B|shown]], then [[Ghost]] and [[c#part]].\n```\n[[fenced]]\n```\nand `[[coded]]` [[a]]");
    index.set("b", "back to [[a]]\r\n~~~\r\n[[ghost]]\r\n~~~\r\n[[Other Ghost]]\r[[ghost]]");
    index.set("dir/c", "[[b]] [[b]] [[dir/c]]");
    assert_eq!(index.outgoing("a"), [out("b", Some("b")), out("Ghost", None), out("c", Some("dir/c")), out("a", Some("a"))]);
    assert_eq!(index.outgoing("b"), [out("a", Some("a")), out("Other Ghost", None), out("ghost", None)]);
    assert_eq!(index.outgoing("dir/c"), [out("b", Some("b")), out("dir/c", Some("dir/c"))]);
    assert_eq!(index.outgoing("nobody"), []);
    // The names of OTHER texts holding a link that resolves to the name.
    assert_eq!(index.incoming("a"), ["b"]);
    assert_eq!(index.incoming("b"), ["a", "dir/c"]);
    assert_eq!(index.incoming("dir/c"), ["a"]);
    assert_eq!(index.incoming("nobody"), [] as [&str; 0]);
    let dangling: Vec<String> = index.dangling().iter().map(|target| key(target)).collect();
    assert_eq!(dangling, ["ghost", "other ghost"]);

    // Re-indexing replaces what a text linked to.
    index.set("b", "nothing now");
    assert_eq!(index.outgoing("b"), []);
    assert_eq!(index.incoming("a"), [] as [&str; 0]);
    assert_eq!(index.dangling(), ["Ghost"]);
    // A new text makes a dangling link whole.
    index.set("notes/ghost", "");
    assert_eq!(index.dangling(), [] as [&str; 0]);
    assert_eq!(index.incoming("notes/ghost"), ["a"]);
    index.remove("notes/ghost");
    assert_eq!(index.dangling(), ["Ghost"]);
}

#[test]
fn rename_keeps_the_outgoing_links() {
    let mut index = LinkIndex::new();
    index.set("old", "[[x]] [[y|why]] [[old]] [[ghost]]");
    index.set("x", "[[old]]");
    index.set("y", "");
    let before: Vec<String> = index.outgoing("old").into_iter().map(|o| o.target).collect();
    index.rename("old", "folder/new");
    assert!(!index.contains("old") && index.contains("folder/new"));
    assert_eq!(index.names(), ["folder/new", "x", "y"]);
    let after: Vec<String> = index.outgoing("folder/new").into_iter().map(|o| o.target).collect();
    assert_eq!(after, before);
    assert_eq!(index.incoming("x"), ["folder/new"]);
    assert_eq!(index.incoming("y"), ["folder/new"]);
    // Links to the old name now lead nowhere until they are retargeted.
    assert_eq!(index.outgoing("x"), [out("old", None)]);
    assert_eq!(index.resolve("new"), Some("folder/new"));
}

/// One stretch of a text under construction: either bytes that no rename may
/// touch, or the target of a link that can be seen and followed.
enum Chunk {
    Raw(String),
    Target(String),
}

fn raw(chunks: &mut Vec<Chunk>, text: &str) {
    chunks.push(Chunk::Raw(text.to_string()));
}

fn render(chunks: &[Chunk]) -> String {
    chunks
        .iter()
        .map(|chunk| match chunk {
            Chunk::Raw(text) | Chunk::Target(text) => text.as_str(),
        })
        .collect()
}

fn targets_of(chunks: &[Chunk]) -> Vec<&str> {
    chunks
        .iter()
        .filter_map(|chunk| match chunk {
            Chunk::Target(target) => Some(target.as_str()),
            Chunk::Raw(_) => None,
        })
        .collect()
}

const EOLS: &[&str] = &["\n", "\n", "\r\n", "\r"];

/// A whole file with links in every setting: paragraphs, headings, items and
/// quotes, padded and with `#heading` and `|shown` parts, and links that must
/// not count: in code spans, in fenced code, and below an unclosed fence.
fn make_text(rng: &mut Rng, pool: &[&str]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    if rng.below(8) == 0 {
        raw(&mut chunks, "\u{FEFF}");
    }
    for n in 0..rng.below(7) {
        if n > 0 {
            raw(&mut chunks, rng.pick(EOLS));
        }
        match rng.below(12) {
            0 => {
                let eol = rng.pick(EOLS);
                let (open, close) = rng.pick(&[("```", "```"), ("~~~", "~~~"), ("```rust", "```"), ("   ```", "  ~~~~"), ("~~~", "```")]);
                raw(&mut chunks, &format!("{open}{eol}[[{}]] and{eol}# [[{}|x]]{eol}{close}", rng.pick(pool), rng.pick(pool)));
            }
            1 => {}
            2 => raw(&mut chunks, rng.pick(&["- - -", "***", "    ", "plain words"])),
            _ => {
                raw(&mut chunks, rng.pick(&["", "", "", "# ", "### ", "- ", "- [ ] ", "12. ", "> ", "  * ", "word "]));
                for _ in 0..1 + rng.below(4) {
                    match rng.below(9) {
                        0 => raw(&mut chunks, rng.pick(WORDS)),
                        1 => raw(&mut chunks, &format!("`[[{}]]`", rng.pick(pool))),
                        2 => raw(&mut chunks, "[text](https://example.com/x)"),
                        3 => raw(&mut chunks, "*emph* **strong** [[]] [[ ]]"),
                        _ => {
                            let (left, right) = rng.pick(&[("", ""), ("", ""), (" ", " "), ("  ", ""), ("", "\t")]);
                            raw(&mut chunks, &format!("[[{left}"));
                            chunks.push(Chunk::Target(rng.pick(pool).to_string()));
                            raw(&mut chunks, right);
                            raw(&mut chunks, rng.pick(&["", "", "", "#Section", "|shown text", " # sec | shown ", "|", "#"]));
                            raw(&mut chunks, "]]");
                        }
                    }
                    raw(&mut chunks, " ");
                }
            }
        }
    }
    if rng.below(6) == 0 {
        let eol = rng.pick(EOLS);
        raw(&mut chunks, &format!("{eol}~~~{eol}[[{}]]{eol}still code [[{}]]", rng.pick(pool), rng.pick(pool)));
    }
    chunks
}

const TARGETS: &[&str] = &[
    "alpha", "Alpha", "ALPHA.md", "notes/alpha", "Notes\\Alpha.TXT", "deep/notes/alpha", "beta", "Beta.markdown", "x/beta",
    "gamma", "dir/gamma", "ghost", "Ghost", "no such  text", "No Such Text", "\u{130}stanbul", "\u{C9}cole", "e\u{301}cole",
    "\u{65E5}\u{672C}\u{8A9E}", "a/b/c", "c", "b/c", "leaf", "w/leaf", "missing/leaf",
];

const NAMES: &[&str] = &[
    "alpha", "notes/alpha", "deep/notes/alpha", "Beta", "x/beta", "y/beta", "gamma", "dir/gamma", "\u{130}stanbul",
    "notes/\u{C9}cole", "\u{65E5}\u{672C}\u{8A9E}", "a/b/c", "c", "z/leaf", "y/leaf", "x/w/leaf", "No Such Text",
];

struct MText {
    name: String,
    key: String,
    targets: Vec<String>,
}

/// A slow index that answers every question by reading every text.
#[derive(Default)]
struct Model {
    texts: Vec<MText>,
}

impl Model {
    fn at(&self, name: &str) -> Option<usize> {
        let k = key(name);
        self.texts.iter().position(|text| text.key == k)
    }

    fn set(&mut self, name: &str, targets: Vec<String>) {
        self.remove(name);
        self.texts.push(MText { name: name.to_string(), key: key(name), targets });
    }

    fn remove(&mut self, name: &str) -> Option<MText> {
        self.at(name).map(|at| self.texts.remove(at))
    }

    fn rename(&mut self, old: &str, new: &str) {
        if let Some(text) = self.remove(old) {
            self.set(new, text.targets);
        }
    }

    fn resolve(&self, target: &str) -> Option<&str> {
        let k = key(target);
        if k.is_empty() {
            return None;
        }
        if let Some(text) = self.texts.iter().find(|text| text.key == k) {
            return Some(&text.name);
        }
        self.texts
            .iter()
            .filter(|text| key(text.name.rsplit(['/', '\\']).next().unwrap_or("")) == k)
            .min_by_key(|text| (text.key.split('/').count(), text.key.clone()))
            .map(|text| text.name.as_str())
    }

    fn sorted(&self) -> Vec<&MText> {
        let mut all: Vec<&MText> = self.texts.iter().collect();
        all.sort_by(|a, b| a.key.cmp(&b.key));
        all
    }
}

#[test]
fn random_edits_to_an_index_match_a_slow_model() {
    let mut rng = Rng::new(13);
    let mut index = LinkIndex::new();
    let mut model = Model::default();
    let (mut by_leaf, mut hubs, mut dangled, mut renamed) = (0, 0, 0, 0);
    for step in 0..2500 {
        match rng.below(10) {
            0..=5 => {
                let name = rng.pick(NAMES);
                let chunks = make_text(&mut rng, TARGETS);
                let text = render(&chunks);
                let targets: Vec<String> = targets_of(&chunks).into_iter().map(str::to_string).collect();

                // The same text read through Doc and the line functions holds
                // exactly the links that were put where they can be seen.
                let doc = Doc::from_text(&text);
                let kinds = classify(doc.lines());
                let seen: Vec<String> = doc
                    .lines()
                    .iter()
                    .zip(&kinds)
                    .filter(|(_, kind)| !matches!(kind, K::Code | K::Fence))
                    .flat_map(|(line, _)| links_in(line))
                    .map(|link| link.target.to_string())
                    .collect();
                assert_eq!(seen, targets, "{text:?}");

                index.set(name, &text);
                model.set(name, targets);
            }
            6 | 7 => {
                let name = rng.pick(NAMES);
                index.remove(name);
                model.remove(name);
            }
            _ => {
                // Onto a free name only: the contract does not say what
                // renaming onto another text does.
                let (old, new) = (rng.pick(NAMES), rng.pick(NAMES));
                if model.at(new).is_none() || key(old) == key(new) {
                    renamed += usize::from(model.at(old).is_some());
                    index.rename(old, new);
                    model.rename(old, new);
                }
            }
        }

        let sorted = model.sorted();
        let names: Vec<&str> = sorted.iter().map(|text| text.name.as_str()).collect();
        assert_eq!(index.names(), names, "step {step}");
        let resolved: BTreeMap<&str, Option<&str>> = TARGETS.iter().chain(NAMES).map(|&target| (target, model.resolve(target))).collect();
        for (&target, &want) in &resolved {
            assert_eq!(index.resolve(target), want, "step {step}: resolve({target:?})");
        }
        for &name in NAMES {
            let here = model.at(name).map(|at| &model.texts[at]);
            assert_eq!(index.contains(name), here.is_some(), "step {step}: contains({name:?})");

            let mut keys = BTreeSet::new();
            let outgoing: Vec<Outgoing> = here
                .map(|text| text.targets.iter().filter(|target| keys.insert(key(target))).map(|target| out(target, resolved[target.as_str()])).collect())
                .unwrap_or_default();
            assert_eq!(index.outgoing(name), outgoing, "step {step}: outgoing({name:?})");

            let incoming: Vec<&str> = match here {
                Some(me) => sorted
                    .iter()
                    .filter(|other| other.key != me.key && other.targets.iter().any(|target| resolved[target.as_str()] == Some(me.name.as_str())))
                    .map(|other| other.name.as_str())
                    .collect(),
                None => Vec::new(),
            };
            assert_eq!(index.incoming(name), incoming, "step {step}: incoming({name:?})");
            hubs += usize::from(incoming.len() > 2);
        }

        // Dangling targets: one for each key that resolves to nothing, in key
        // order, spelled some way a text really wrote it.
        let mut spelled: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for text in &model.texts {
            for target in &text.targets {
                if resolved[target.as_str()].is_none() {
                    spelled.entry(key(target)).or_default().push(target);
                }
            }
        }
        let dangling = index.dangling();
        assert_eq!(dangling.iter().map(|target| key(target)).collect::<Vec<_>>(), spelled.keys().cloned().collect::<Vec<_>>(), "step {step}");
        for target in &dangling {
            assert!(spelled[&key(target)].contains(&target.as_str()), "step {step}: {target:?} was never written");
        }
        dangled += usize::from(dangling.len() > 2);
        by_leaf += usize::from(resolved["leaf"].is_some()) + usize::from(resolved["c"].is_some_and(|name| name != "c"));
    }
    assert!(by_leaf > 300 && hubs > 300 && dangled > 300 && renamed > 100, "{by_leaf} {hubs} {dangled} {renamed}");
}

/// No model here: only what the index's own answers must mean to each other,
/// with names and texts as hostile as they come.
#[test]
fn random_hostile_indexes_agree_with_themselves() {
    const PARTS: &[&str] = &["[[", "]]", "[[", "]]", "|", "#", "/", "\\", " ", "\n", "\r\n", "\r", "```\n", "~~~", "`", ".md", "a", "B", "c d", "- ", "# "];
    let mut rng = Rng::new(14);
    let soup = |rng: &mut Rng, most: usize| -> String {
        (0..rng.below(most + 1))
            .map(|_| match rng.below(6) {
                0 => rng.pick(WORDS),
                1 => rng.pick(ODD),
                _ => rng.pick(PARTS),
            })
            .collect()
    };
    let mut index = LinkIndex::new();
    for step in 0..6000 {
        let name = if rng.below(3) == 0 { soup(&mut rng, 4) } else { rng.pick(&["a", "b", "c d", "x/a", "x/y/a", "B", "x\\c  d.md"]).to_string() };
        match rng.below(8) {
            0 => index.remove(&name),
            1 => {
                let new = soup(&mut rng, 3);
                index.rename(&name, &new);
            }
            _ => {
                let text = soup(&mut rng, 30);
                index.set(&name, &text);
            }
        }
        if step % 7 != 0 {
            continue;
        }
        let names = index.names();
        for pair in names.windows(2) {
            assert!(key(pair[0]) < key(pair[1]), "step {step}: {names:?}");
        }
        let dangling = index.dangling();
        for pair in dangling.windows(2) {
            assert!(key(&pair[0]) < key(&pair[1]), "step {step}: {dangling:?}");
        }
        for target in &dangling {
            assert_eq!(index.resolve(target), None, "step {step}");
        }
        for &name in &names {
            assert!(index.contains(name), "step {step}");
            assert_eq!(index.resolve(name), Some(name), "step {step}");
            let outgoing = index.outgoing(name);
            let mut keys = BTreeSet::new();
            for link in &outgoing {
                assert!(keys.insert(key(&link.target)), "step {step}: two entries for one key in {outgoing:?}");
                assert_eq!(index.resolve(&link.target), link.resolved.as_deref(), "step {step}");
                match &link.resolved {
                    Some(to) if key(to) != key(name) => assert!(index.incoming(to).contains(&name), "step {step}: {name:?} -> {to:?}"),
                    Some(_) => {}
                    None => assert!(dangling.iter().any(|d| key(d) == key(&link.target)), "step {step}: {link:?} not dangling"),
                }
            }
            let incoming = index.incoming(name);
            assert!(!incoming.contains(&name), "step {step}");
            for pair in incoming.windows(2) {
                assert!(key(pair[0]) < key(pair[1]), "step {step}: {incoming:?}");
            }
            for from in incoming {
                assert!(index.outgoing(from).iter().any(|link| link.resolved.as_deref() == Some(name)), "step {step}: {from:?} -> {name:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// links: retarget
// ---------------------------------------------------------------------------

#[test]
fn retarget_rewrites_the_target_and_nothing_else() {
    assert_eq!(retarget("[[old]]", "old", "new").as_deref(), Some("[[new]]"));
    assert_eq!(retarget("[[Old.md#part|shown]]", "old", "dir/new").as_deref(), Some("[[dir/new#part|shown]]"));
    assert_eq!(retarget("[[  OLD  # part | shown ]]", "old", "new").as_deref(), Some("[[  new  # part | shown ]]"));
    assert_eq!(retarget("a [[old]] b [[older]] c [[x/old]] d [[old|o]]", "old", "new").as_deref(), Some("a [[new]] b [[older]] c [[x/old]] d [[new|o]]"));
    assert_eq!(retarget("[[notes\\old.TXT]]", "Notes/Old", "n").as_deref(), Some("[[n]]"));
    // Line endings and everything else stay byte for byte.
    assert_eq!(
        retarget("\u{FEFF}# [[old]]\r\n\r- [ ] [[old]]\n> [[old]]\r", "old", "new").as_deref(),
        Some("\u{FEFF}# [[new]]\r\n\r- [ ] [[new]]\n> [[new]]\r")
    );
    // Code is not a place for links.
    assert_eq!(retarget("`[[old]]`\n```\n[[old]]\n```\n~~~\n[[old]]", "old", "new"), None);
    assert_eq!(retarget("`[[old]]` [[old]]", "old", "new").as_deref(), Some("`[[old]]` [[new]]"));
    // None when nothing changed.
    assert_eq!(retarget("[[other]] and words", "old", "new"), None);
    assert_eq!(retarget("", "old", "new"), None);
    assert_eq!(retarget("[[new]]", "new", "new"), None);
    assert_eq!(retarget("[[\u{130}stanbul]] [[\u{65E5}\u{672C}]]", "\u{65E5}\u{672C}", "n\u{313}").as_deref(), Some("[[\u{130}stanbul]] [[n\u{313}]]"));
}

#[test]
fn random_retargets_touch_nothing_but_matching_targets() {
    const NEW: &[&str] = &["renamed", "folder/New Name", "Alpha", "alpha", "\u{130}stanbul 2", "x", "\u{65E5}\u{8A18}", "n\u{313}"];
    let mut rng = Rng::new(15);
    let (mut rewritten, mut untouched) = (0, 0);
    for _ in 0..12_000 {
        let chunks = make_text(&mut rng, TARGETS);
        let text = render(&chunks);
        let (old, new) = (rng.pick(TARGETS), rng.pick(NEW));
        let want: String = chunks
            .iter()
            .map(|chunk| match chunk {
                Chunk::Target(target) if key(target) == key(old) => new,
                Chunk::Raw(text) | Chunk::Target(text) => text.as_str(),
            })
            .collect();
        let found = retarget(&text, old, new);
        if want == text {
            assert_eq!(found, None, "{text:?} {old:?} -> {new:?}");
            untouched += 1;
        } else {
            assert_eq!(found.as_deref(), Some(want.as_str()), "{text:?} {old:?} -> {new:?}");
            rewritten += 1;
            // Done once, there is nothing left to do.
            if key(old) != key(new) {
                assert_eq!(retarget(&want, old, new), None, "{want:?}");
            }
        }
    }
    assert!(rewritten > 1500 && untouched > 1500, "{rewritten} {untouched}");
}

// ---------------------------------------------------------------------------
// fuzzy
// ---------------------------------------------------------------------------

fn score(needle: &str, candidate: &str) -> i32 {
    fuzzy(needle, candidate).map(|m| m.score).unwrap_or_else(|| panic!("{needle:?} should match {candidate:?}"))
}

fn hits(needle: &str, candidate: &str) -> Vec<usize> {
    fuzzy(needle, candidate).map(|m| m.hits).unwrap_or_else(|| panic!("{needle:?} should match {candidate:?}"))
}

/// One character without regard to case, for characters whose lowercase is
/// one character. The two lowercase sigmas are one letter.
fn low(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some('\u{3C2}'), None) => '\u{3C3}',
        (Some(one), None) => one,
        _ => c,
    }
}

/// A whole string without regard to case.
fn lowered(text: &str) -> String {
    text.to_lowercase().replace('\u{3C2}', "\u{3C3}")
}

fn is_subsequence(needle: &[char], hay: &[char]) -> bool {
    let mut hay = hay.iter();
    needle.iter().all(|want| hay.any(|c| c == want))
}

/// Letters for needles and candidates. U+0130 is left out here: its
/// lowercase is two characters, which the one-character `low` above cannot
/// follow. It has `reduced_failures::a_dotted_capital_i_ties_with_the_equal_candidate`
/// to itself.
const LETTERS: &[char] = &[
    'a', 'b', 'c', 'A', 'B', 'x', 'X', '\u{E9}', '\u{C9}', '\u{3A3}', '\u{3C3}', '\u{3C2}', '\u{4E00}', '\u{5E9}', '\u{1F600}',
    '\u{301}', '\u{200D}', '\u{131}', '\u{DF}', '\u{212A}', 'k',
];
const SEPARATORS: &[char] = &[' ', '/', '\\', '-', '_', '.'];

fn letters(rng: &mut Rng, pool: &[char], least: usize, most: usize) -> String {
    (0..least + rng.below(most - least + 1)).map(|_| rng.pick(pool)).collect()
}

#[test]
fn fuzzy_is_a_case_insensitive_subsequence_match() {
    assert_eq!(hits("abc", "abc"), [0, 1, 2]);
    assert_eq!(hits("abc", "xaxbxc"), [1, 3, 5]);
    assert_eq!(hits("ABC", "xaxbxc"), [1, 3, 5]);
    assert_eq!(hits("abc", "XAXBXC"), [1, 3, 5]);
    assert_eq!(fuzzy("abc", "acb"), None);
    assert_eq!(fuzzy("abc", "ab"), None);
    assert_eq!(fuzzy("aa", "a"), None);
    assert_eq!(fuzzy("a", ""), None);
    assert_eq!(fuzzy("\u{E9}", "e"), None);
    // Whitespace in the needle is ignored.
    assert_eq!(hits("a b\tc", "abc"), [0, 1, 2]);
    assert_eq!(hits("  abc  ", "abc"), [0, 1, 2]);
    assert_eq!(hits("a\u{3000}c", "abc"), [0, 2]);
    // Hits are byte offsets, on character boundaries.
    assert_eq!(hits("\u{C9}t", "\u{E9}\u{4E00}t"), [0, 5]);
    assert_eq!(hits("\u{1F469}", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"), [7]);
    assert_eq!(hits("n", "an\u{313}"), [1]);
    assert_eq!(hits("\u{5DC}", "\u{5E9}\u{5DC}\u{5D5}\u{5DD}"), [2]);
    assert_eq!(hits("\u{3C3}\u{3C3}", "\u{3A3}\u{3C2}"), [0, 2]);
}

#[test]
fn random_matches_are_subsequences_with_honest_hits() {
    let mut rng = Rng::new(16);
    let (mut matched, mut missed) = (0, 0);
    for _ in 0..40_000 {
        let pool: Vec<char> = LETTERS.iter().chain(SEPARATORS).chain(&['\t', '\u{3000}']).copied().collect();
        let needle = letters(&mut rng, &pool, 0, 5);
        let candidate = letters(&mut rng, &pool, 0, 24);
        let wanted: Vec<char> = needle.chars().filter(|c| !c.is_whitespace()).map(low).collect();
        let hay: Vec<char> = candidate.chars().map(low).collect();
        let Some(found) = fuzzy(&needle, &candidate) else {
            assert!(!is_subsequence(&wanted, &hay), "{needle:?} is in {candidate:?}");
            missed += 1;
            continue;
        };
        assert!(is_subsequence(&wanted, &hay), "{needle:?} is not in {candidate:?}");
        assert_eq!(found.hits.len(), wanted.len(), "{needle:?} in {candidate:?}");
        assert!(found.hits.windows(2).all(|pair| pair[0] < pair[1]), "{needle:?} in {candidate:?}");
        for (hit, want) in found.hits.iter().zip(&wanted) {
            assert!(candidate.is_char_boundary(*hit), "{needle:?} in {candidate:?}");
            assert_eq!(candidate[*hit..].chars().next().map(low), Some(*want), "{needle:?} in {candidate:?}");
        }
        matched += usize::from(!wanted.is_empty());
    }
    assert!(matched > 3000 && missed > 3000, "{matched} {missed}");
}

/// The embedding of `needle` in `candidate` at the given character columns,
/// alone: every other character that could match a needle character is
/// swapped for a letter that cannot. Hits keep their places, their case, and
/// whatever stands before them, so the alignment scores as it did, and no
/// other alignment is left.
fn isolate(needle: &[char], candidate: &[char], cols: &[usize]) -> String {
    candidate
        .iter()
        .enumerate()
        .map(|(col, &c)| {
            if !cols.contains(&col) && needle.iter().any(|&n| low(n) == low(c)) {
                'q'
            } else {
                c
            }
        })
        .collect()
}

fn each_alignment(needle: &[char], candidate: &[char], cols: &mut Vec<usize>, visit: &mut dyn FnMut(&[usize])) {
    if cols.len() == needle.len() {
        visit(cols);
        return;
    }
    let from = cols.last().map_or(0, |&col| col + 1);
    for col in from..candidate.len() {
        if low(candidate[col]) == low(needle[cols.len()]) {
            cols.push(col);
            each_alignment(needle, candidate, cols, visit);
            cols.pop();
        }
    }
}

fn random_alignment(rng: &mut Rng, needle: &[char], candidate: &[char]) -> Option<Vec<usize>> {
    let mut last = vec![0; needle.len()];
    let mut end = candidate.len();
    for i in (0..needle.len()).rev() {
        end = (0..end).rev().find(|&col| low(candidate[col]) == low(needle[i]))?;
        last[i] = end;
    }
    let mut cols = Vec::new();
    let mut from = 0;
    for i in 0..needle.len() {
        let open: Vec<usize> = (from..=last[i]).filter(|&col| low(candidate[col]) == low(needle[i])).collect();
        let col = rng.pick(&open);
        cols.push(col);
        from = col + 1;
    }
    Some(cols)
}

/// "It must find the BEST alignment, not the leftmost." Without knowing how
/// anything is weighed: a candidate in which one alignment has been left
/// standing alone can never outscore the candidate it was cut from, because
/// that alignment was there to be chosen.
#[test]
fn no_alignment_left_alone_outscores_the_candidate_it_came_from() {
    const NEEDLE: &[char] = &['a', 'b', 'c', 'A', 'B'];
    const HAY: &[char] = &['a', 'a', 'b', 'b', 'c', 'A', 'B', 'C', 'x', 'y', 'X', ' ', '-', '_', '/', '.', '\\'];
    let mut rng = Rng::new(17);
    let mut checked = 0;
    for round in 0..3000 {
        // Small ones are checked against every alignment there is, large
        // ones against a sample.
        let small = round % 3 == 0;
        let needle: Vec<char> = letters(&mut rng, NEEDLE, 1, if small { 4 } else { 7 }).chars().collect();
        let candidate: Vec<char> = letters(&mut rng, HAY, 1, if small { 13 } else { 70 }).chars().collect();
        let needle_text: String = needle.iter().collect();
        let candidate_text: String = candidate.iter().collect();
        let Some(best) = fuzzy(&needle_text, &candidate_text) else { continue };
        let mut check = |cols: &[usize]| {
            let alone = isolate(&needle, &candidate, cols);
            let found = fuzzy(&needle_text, &alone).unwrap_or_else(|| panic!("{needle_text:?} is still in {alone:?}"));
            assert_eq!(found.hits, cols, "{needle_text:?} can only sit one way in {alone:?}");
            assert!(
                best.score >= found.score,
                "{needle_text:?} in {candidate_text:?}: hits {:?} score {}, but hits {cols:?} alone score {}",
                best.hits,
                best.score,
                found.score
            );
            checked += 1;
        };
        if small {
            each_alignment(&needle, &candidate, &mut Vec::new(), &mut check);
        } else {
            for _ in 0..40 {
                check(&random_alignment(&mut rng, &needle, &candidate).unwrap());
            }
        }
        // The hits reported are themselves an alignment worth what is claimed
        // for them, when cutting the rest away cannot change what kind of
        // candidate this is.
        let squashed: String = candidate_text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !lowered(&squashed).starts_with(&lowered(&needle_text)) {
            let alone = isolate(&needle, &candidate, &best.hits);
            assert_eq!(fuzzy(&needle_text, &alone), Some(best.clone()), "{needle_text:?} in {candidate_text:?}");
        }
    }
    assert!(checked > 50_000, "{checked}");
}

#[test]
fn the_best_alignment_is_not_the_leftmost() {
    // The leftmost alignment starts inside a word and jumps; the best one is
    // a run at the start of a word.
    assert_eq!(hits("ab", "xaxb ab"), [5, 6]);
    assert_eq!(hits("app", "an apple"), [3, 4, 5]);
    assert_eq!(hits("cat", "concatenate/cat"), [12, 13, 14]);
    assert_eq!(hits("abc", "xaxxbxxc.abc"), [9, 10, 11]);
    assert_eq!(hits("\u{E9}t\u{E9}", "x\u{E9}xtx\u{E9} \u{C9}t\u{E9}"), [9, 11, 12]);
}

#[test]
fn scores_reward_and_penalise_what_the_contract_names() {
    // A hit at the very start of the candidate.
    assert!(score("d", "dxxx") > score("d", "xdxx"));
    // A hit just after a separator.
    for separator in SEPARATORS {
        assert!(score("d", &format!("xx{separator}d")) > score("d", "xxxd"), "{separator:?}");
        assert!(score("dx", &format!("ab{separator}dx")) > score("dx", "abcdx"), "{separator:?}");
    }
    // Runs of consecutive hits.
    assert!(score("ft", "xdraft") > score("ft", "xdrafxt"));
    assert!(score("abc", "xabcx") > score("abc", "xabxc"));
    // Exact-case hits.
    assert!(score("D", "xD") > score("D", "xd"));
    assert!(score("d", "xd") > score("d", "xD"));
    assert!(score("Draft", "x Draft") > score("Draft", "x draft"));
    // Gaps between hits.
    assert!(score("dt", "xdt") > score("dt", "xdat"));
    assert!(score("dt", "xdat") > score("dt", "xdaat"));
    // Hits deep into the candidate.
    assert!(score("d", "xd") > score("d", "xxd"));
    assert!(score("d", "xxd") > score("d", "xxxxd"));
    // And, lightly, the candidate's length.
    assert!(score("dr", "xdra") > score("dr", "xdraft"));
    assert!(score("draft", "x drafts and a great deal more besides") > score("draft", "x d r a f t"));
}

#[test]
fn an_equal_candidate_outscores_every_other_and_a_prefix_outscores_scatter() {
    for other in ["drafts", "draft 2", "a draft", "xdraft", "d-r-a-f-t", "draftdraft", " draft", "draft ", "DRAFT.", "Draft/Draft"] {
        assert!(score("draft", "draft") > score("draft", other), "{other:?}");
        assert!(score("draft", "DRAFT") > score("draft", other), "{other:?}");
        assert!(score("DrAfT", "dRaFt") > score("DrAfT", other), "{other:?}");
    }
    for other in ["commonapp", "common  app", "common app essay", "Common Apple", "c o m m o n a p p"] {
        assert!(score("common app", "Common App") > score("common app", other), "{other:?}");
    }
    assert!(score("\u{E9}t\u{E9}", "\u{C9}T\u{C9}") > score("\u{E9}t\u{E9}", "\u{E9}t\u{E9}s"));
    assert!(score("\u{3C3}", "\u{3A3}") > score("\u{3C3}", "\u{3A3}\u{3A3}"));

    for scattered in ["d-r-a-f-t", "d r a f t", "dxraft", "x draft", "a draft", "Dr. Aft", "d/r/a/f/t", "drxaft"] {
        assert!(score("draft", "draft notes") > score("draft", scattered), "{scattered:?}");
        assert!(score("draft", "DRAFTS of a very long essay about nothing much at all") > score("draft", scattered), "{scattered:?}");
    }
}

#[test]
fn random_equal_candidates_outscore_all_others() {
    let pool: Vec<char> = LETTERS.iter().chain(SEPARATORS).copied().collect();
    let mut rng = Rng::new(18);
    let mut compared = 0;
    for _ in 0..8000 {
        let needle = letters(&mut rng, &pool, 1, 7);
        if needle.trim().is_empty() {
            continue;
        }
        // The same text with its case shaken, wherever that is still the
        // same text to anyone reading without regard to case.
        let equal: String = needle
            .chars()
            .map(|c| {
                let mut upper = c.to_uppercase();
                match (rng.below(3), upper.next(), upper.next()) {
                    (0, Some(up), None) if low(up) == low(c) => up,
                    (1, _, _) => low(c),
                    _ => c,
                }
            })
            .collect();
        if lowered(&equal) != lowered(&needle) {
            continue;
        }
        let best = score(&needle, &equal);
        for _ in 0..12 {
            let mut other: Vec<char> = needle.chars().collect();
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(other.len() + 1);
                let c = rng.pick(&pool);
                match rng.below(4) {
                    0 => other.insert(at, c),
                    1 if at < other.len() => drop(other.remove(at)),
                    2 if at < other.len() => other[at] = c,
                    _ => other.extend(needle.chars()),
                }
            }
            let other: String = other.into_iter().collect();
            if lowered(&other) == lowered(&needle) {
                continue;
            }
            if let Some(found) = fuzzy(&needle, &other) {
                assert!(best > found.score, "{needle:?}: {equal:?} scores {best}, {other:?} scores {}", found.score);
                compared += 1;
            }
        }
    }
    assert!(compared > 10_000, "{compared}");
}

#[test]
fn random_prefix_candidates_outscore_scattered_ones() {
    // Nothing here is a letter a needle can hold, in either case.
    const FILL: &[char] = &['q', 'y', 'Q', ' ', '-', '/', '.', '_', '\\', '\u{4E8C}'];
    let mut rng = Rng::new(19);
    for _ in 0..8000 {
        let needle = letters(&mut rng, LETTERS, 2, 7);
        let shaken: String = needle.chars().map(|c| if rng.coin() { low(c) } else { c }).collect();
        let prefixed = shaken + &letters(&mut rng, FILL, 0, 50);
        // The same letters, in the needle's own case, with something between
        // at least two of them, and often sitting just after a separator.
        let count = needle.chars().count();
        let split = 1 + rng.below(count - 1);
        let mut scattered = String::new();
        for (i, c) in needle.chars().enumerate() {
            if i == split || (i > 0 && rng.below(3) == 0) {
                scattered.push(rng.pick(FILL));
            }
            scattered.push(c);
        }
        assert!(score(&needle, &prefixed) > score(&needle, &scattered), "{needle:?}: {prefixed:?} against {scattered:?}");
    }
}

#[test]
fn rank_is_best_first_with_ties_in_input_order() {
    let names = ["a draft", "draft", "nothing", "drafts", "d r a f t", "draft", "DRAFT", "x"];
    let ranked = rank("draft", names);
    assert_eq!(ranked.len(), 6);
    assert!(ranked.iter().all(|(index, _)| ![2, 7].contains(index)));
    assert_eq!((ranked[0].0, ranked[1].0), (1, 5));
    for (index, found) in &ranked {
        assert_eq!(Some(found), fuzzy("draft", names[*index]).as_ref());
    }
    // An empty or all-whitespace needle: everything, in input order, score 0, no hits.
    for needle in ["", " ", " \t\u{3000} "] {
        let all = rank(needle, names);
        let want: Vec<(usize, Match)> = (0..names.len()).map(|index| (index, Match { score: 0, hits: Vec::new() })).collect();
        assert_eq!(all, want, "{needle:?}");
    }
    assert_eq!(rank("x", Vec::<&str>::new()), []);
    assert_eq!(rank("", Vec::<&str>::new()), []);
    assert_eq!(rank("zzz", names), []);
}

#[test]
fn random_ranks_are_the_matches_sorted_and_nothing_else() {
    let pool: Vec<char> = ['a', 'b', 'A', ' ', '/', '\u{E9}', '\u{130}', 'i', 'I'].to_vec();
    let mut rng = Rng::new(20);
    let mut tied = 0;
    for _ in 0..1500 {
        let needle = letters(&mut rng, &pool, 0, 3);
        let mut texts: Vec<String> = (0..rng.below(30)).map(|_| letters(&mut rng, &pool, 0, 8)).collect();
        // Repeats, so that there are ties to keep in order.
        for _ in 0..rng.below(6) {
            if !texts.is_empty() {
                let again = texts[rng.below(texts.len())].clone();
                texts.insert(rng.below(texts.len() + 1), again);
            }
        }
        let ranked = rank(&needle, texts.iter().map(String::as_str));
        let mut want: Vec<(usize, Match)> = texts.iter().enumerate().filter_map(|(index, text)| Some((index, fuzzy(&needle, text)?))).collect();
        want.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
        assert_eq!(ranked, want, "{needle:?} {texts:?}");
        tied += ranked.windows(2).filter(|pair| pair[0].1.score == pair[1].1.score).count();
        if needle.trim().is_empty() {
            assert!(ranked.iter().enumerate().all(|(at, (index, found))| at == *index && found.score == 0 && found.hits.is_empty()));
            assert_eq!(ranked.len(), texts.len());
        }
    }
    assert!(tied > 500, "{tied}");
}

// ---------------------------------------------------------------------------
// Work that must stay in step with the size of its input
// ---------------------------------------------------------------------------

/// Lines built to send a careless scanner back over its own tracks. Linear
/// work gets through each in milliseconds; quadratic work would take minutes.
#[test]
fn hostile_lines_do_not_send_the_scanner_back_over_itself() {
    let hostile: Vec<(&str, String)> = vec![
        ("backtick staircase", (1..500).chain((1..500).rev()).map(|n| "`".repeat(n) + " ").collect()),
        ("backticks never closed", (1..700).map(|n| "`".repeat(n) + "x").collect()),
        ("alternating backticks", "` `` ".repeat(60_000)),
        ("escaped backticks", "\\` ` ".repeat(60_000)),
        ("addresses with long tails", "http://a...)))) ".repeat(20_000)),
        ("address of punctuation", format!("http://{}", ".".repeat(300_000))),
        ("address of parentheses", format!("https://a/{}{}", "(".repeat(150_000), ")".repeat(150_000))),
        ("addresses that fail", "http://. ".repeat(40_000)),
        ("schemes after letters", "xhttp://".repeat(40_000)),
        ("almost schemes", "http:/".repeat(50_000)),
        ("unclosed wiki link", format!("[[{}", "|#".repeat(150_000))),
        ("wiki link of spaces", format!("[[{}]]", " ".repeat(300_000))),
        ("wiki openers and single closers", "[[a] ".repeat(60_000)),
        ("triple brackets", "[[[a]] ".repeat(40_000)),
        ("labels with no address", "[a] ".repeat(70_000)),
        ("labels then open addresses", "[a](b ".repeat(50_000)),
        ("an address of nested parentheses", format!("[a]({}", "(x".repeat(150_000))),
        ("emphasis soup", "*a _b **c __d ***e ___f ".repeat(12_000)),
        ("closers with no openers", "a* b_ c** d__ e*** ".repeat(15_000)),
        ("underscored words", "snake_case_".repeat(25_000)),
        ("escapes", "\\*\\_\\[".repeat(50_000)),
        ("a stack of combining marks", format!("a{}", "\u{301}".repeat(150_000))),
        ("joined emoji", "\u{1F468}\u{200D}".repeat(40_000)),
        ("heading closers", format!("# {}", "# ".repeat(150_000))),
        ("item digits", format!("{}. x", "9".repeat(300_000))),
        ("a rule that is not one", format!("{}x", "- ".repeat(150_000))),
        ("a rule", "- ".repeat(150_000)),
        ("a quote far in", format!("{}> x", " ".repeat(300_000))),
        ("one long word", "x".repeat(400_000)),
    ];
    for (what, line) in &hostile {
        let started = Instant::now();
        let kind = classify_line(line, false);
        for kind in [kind, K::Text, K::Heading(1), K::Item, K::Quote] {
            let found = pieces(line, kind);
            assert_eq!(found.last().map(|p| p.end), Some(line.len()), "{what}");
        }
        let _ = (hang_of(line), heading(line), links_in(line), link_at(line, line.len() / 2), key(line));
        let _ = retarget(line, "a", "b");
        let mut index = LinkIndex::new();
        index.set("x", line);
        let _ = (index.outgoing("x"), index.dangling());
        let took = started.elapsed();
        assert!(took.as_secs() < 20, "{what}: {} bytes took {took:?}", line.len());
    }
}

#[test]
fn a_large_index_answers_without_walking_the_folder() {
    let started = Instant::now();
    let mut index = LinkIndex::new();
    let count = 20_000;
    for n in 0..count {
        let text = format!("[[hub]] [[note {}]] [[folder {}/note {}]] [[missing {}]]", (n + 1) % count, (n + 7) % 50, (n + 7) % count, n % 100);
        index.set(&format!("folder {}/note {n}", n % 50), &text);
    }
    index.set("hub", "[[note 1]]");
    assert_eq!(index.dangling().len(), 100);
    let built = started.elapsed();
    let started = Instant::now();
    for n in 0..count {
        let name = format!("folder {}/note {n}", n % 50);
        assert_eq!(index.resolve(&format!("Note {n}.md")), Some(name.as_str()));
        assert_eq!(index.outgoing(&name).len(), 4);
        assert!(!index.incoming(&name).is_empty());
        if n % 4 == 0 {
            index.set(&name, "[[hub]]");
        }
    }
    assert_eq!(index.incoming("hub").len(), count);
    // Every fourth text was set again without its missing link, and those
    // texts were the only ones holding 25 of the 100 missing targets.
    assert_eq!(index.dangling().len(), 75);
    let asked = started.elapsed();
    assert!(built.as_secs() < 30 && asked.as_secs() < 30, "built in {built:?}, asked in {asked:?}");
}

#[test]
fn ranking_a_folder_of_names_is_quick() {
    let names: Vec<String> = (0..30_000).map(|n| format!("essays/{}/draft {n} of the common application essay", n % 97)).collect();
    let started = Instant::now();
    for needle in ["c", "co", "com", "comm", "common app", "draft 2999", "eca", "zzz"] {
        let ranked = rank(needle, names.iter().map(String::as_str));
        assert!(ranked.windows(2).all(|pair| pair[0].1.score >= pair[1].1.score));
    }
    // One long line against a long needle is still one table, not a stall.
    let long = "the quick brown fox jumps over the lazy dog ".repeat(200);
    assert!(fuzzy("the lazy dog jumps over the quick brown fox", &long).is_some());
    let took = started.elapsed();
    assert!(took.as_secs() < 60, "{took:?}");
}

// ---------------------------------------------------------------------------
// Reduced failures. Every test below failed when it was written. The defects
// are fixed; the tests stay to keep them so.
// ---------------------------------------------------------------------------

mod reduced_failures {
    use super::*;

    /// Contract: "_emph_ (underscores count only at word boundaries, so
    /// snake_case is plain)".
    ///
    /// The underscore after "re\u{301}sume\u{301}" stands between two letters
    /// of one name, exactly as it does in "r\u{E9}sum\u{E9}_draft_", which is
    /// rightly plain. But the character before it is the combining acute,
    /// which `char::is_alphanumeric` does not count, so the scanner took the
    /// place for a word boundary and drew "draft" as emphasis. The same
    /// happened after "mitsqanaqan\u{313}", and after any letter followed by a
    /// zero-width character (U+200D, U+200E, a Thai tone mark): everything
    /// `width::is_combining` already knows to sit on the letter before it.
    #[test]
    fn an_underscore_after_a_combining_mark_is_inside_a_word() {
        assert_eq!(styles_of("r\u{E9}sum\u{E9}_draft_ two"), [S::Plain], "precomposed");
        assert_eq!(styles_of("re\u{301}sume\u{301}_draft_ two"), [S::Plain], "decomposed");
        assert_eq!(styles_of("mitsqanaqan\u{313}_atlas_"), [S::Plain]);
        for unseen in ["\u{200D}", "\u{200E}", "\u{E48}", "\u{300}\u{301}\u{302}"] {
            assert_eq!(styles_of(&format!("word{unseen}_draft_ two")), [S::Plain], "{unseen:?}");
        }
    }

    /// Contract: "A backslash before a markup character makes that character
    /// plain", and "An opener with no closer on the same line is plain text".
    ///
    /// The `]` is escaped, so the label never closes and nothing here is a
    /// link. `pieces` read straight past the backslash and drew a link whose
    /// text was "a\". (The other way round, an escaped `[` inside a label
    /// stopped "[a\[b](c)" from being a link at all.)
    #[test]
    fn an_escaped_bracket_does_not_close_a_markdown_link_label() {
        use S::*;
        assert_eq!(real("[a\\](b)"), [("[a", Plain), ("\\", Syntax), ("](b)", Plain)]);
        assert_eq!(real("[a\\[b](c)"), [("[", Syntax), ("a\\[b", Link), ("](", Syntax), ("c", Url), (")", Syntax)]);
        assert_eq!(real("[a\\]b](c)"), [("[", Syntax), ("a\\]b", Link), ("](", Syntax), ("c", Url), (")", Syntax)]);
    }

    /// The same sentences, for the address: the `)` is escaped, so the
    /// address never closes. `pieces` drew a link to "b\", and closed
    /// "[a](b\)c)" early at the escaped parenthesis.
    #[test]
    fn an_escaped_parenthesis_does_not_close_a_markdown_link_address() {
        use S::*;
        assert_eq!(real("[a](b\\)"), [("[a](b", Plain), ("\\", Syntax), (")", Plain)]);
        assert_eq!(real("[a](b\\)c)"), [("[", Syntax), ("a", Link), ("](", Syntax), ("b\\)c", Url), (")", Syntax)]);
    }

    /// Contract: "A candidate equal to the needle ignoring case must outscore
    /// every other candidate."
    ///
    /// "I" is "i" ignoring case. U+0130 is not: its lowercase is "i\u{307}".
    /// But the matcher folded a character to the first character of its
    /// lowercase, which made U+0130 an "i" too, so both candidates landed in
    /// the tier kept for equal ones and tied, and `rank` listed them in input
    /// order. (Read the Turkish way, where U+0130 is the capital of "i" and
    /// "I" is not, the tie is just as wrong.) The same folding made the
    /// needle "i\u{307}" miss the candidate U+0130 altogether, and scored "i"
    /// far above "i\u{307}" for the needle U+0130.
    #[test]
    fn a_dotted_capital_i_ties_with_the_equal_candidate() {
        assert_ne!("\u{130}".to_lowercase(), "i");
        let (equal, other) = (score("i", "I"), score("i", "\u{130}"));
        assert!(equal > other, "\"I\" scores {equal} and U+0130 scores {other}");
        assert_eq!(rank("i", ["\u{130}", "I"]).iter().map(|(index, _)| *index).collect::<Vec<_>>(), [1, 0]);
        assert!(score("istanbul", "ISTANBUL") > score("istanbul", "\u{130}STANBUL"));
        // Its own lowercase is its equal, and matches it either way round.
        assert_eq!(hits("i\u{307}", "\u{130}"), [0]);
        assert!(score("\u{130}", "i\u{307}") > score("\u{130}", "i\u{307}s"));
        assert_eq!(fuzzy("\u{130}", "i"), None);
    }

    /// Contract: "It must find the BEST alignment, not the leftmost."
    ///
    /// The candidate holds the needle's letters scattered at its front and
    /// then, after filler, the needle itself as one run at the start of a
    /// word. With a little filler the run is found. With enough that the
    /// table would pass 4,194,304 cells (here a 70-character needle and a
    /// line of 61,000), `fuzzy` gave up on the table and returned the
    /// leftmost alignment, with the low score that goes with it. The last
    /// size is there so that no larger table quietly brings that back.
    #[test]
    fn a_long_candidate_still_gets_the_best_alignment() {
        let needle = "ab".repeat(35);
        let front = "axbx".repeat(35);
        for filler in [1_000, 61_000, 400_000] {
            let candidate = format!("{front}{} {needle}", "x".repeat(filler));
            let run_starts = front.len() + filler + 1;
            let found = fuzzy(&needle, &candidate).unwrap();
            let want: Vec<usize> = (run_starts..run_starts + needle.len()).collect();
            assert!(found.hits == want, "with {filler} of filler the hits start {:?}, not at {run_starts}", &found.hits[..6]);
        }
    }
}

#[test]
fn look_alike_spellings_of_a_name_share_one_key() {
    use ok_text::links::key;
    assert_eq!(key("Dad\u{2019}s notes"), key("Dad's notes"));
    assert_eq!(key("Dad\u{02BC}s\u{00A0}notes"), key("dad's notes"));
    assert_eq!(key("Essay\u{200B} plan"), key("essay plan"));
    assert_eq!(key("\u{201C}Why\u{201D} essay.md"), key("\"why\" essay"));
    assert_eq!(key("co\u{00AD}operate"), key("cooperate"));
}
