//! Blocks: the structure written with fences of colons, as Pandoc and most
//! Markdown tools read them.
//!
//! ```text
//! ::: box Packing
//! tent
//! :::
//! ```
//!
//! A line of three or more colons followed by words opens a block; a line
//! of colons alone closes the innermost one open. The first word says what
//! the block is, the rest name it or place it:
//!
//! | First word | The block |
//! | --- | --- |
//! | `box`, or any word not below | a frame round what is in it, the words after `box` (or all the words) as its name |
//! | `plain` | a group with nothing drawn, to size, move or narrow |
//! | `big`, `huge` | what is in it at twice or three times the size |
//! | `small` | what is in it in small type, half a cell across |
//! | `columns` | what is in it side by side, one column for each part |
//! | `grid N` | what is in it in rows of N |
//! | `board` | a free surface: each block in it sits where its `at` puts it |
//!
//! Anywhere among the words, `width N` makes a block N cells wide, and
//! `at X Y` puts a block on a board X cells from its left and Y rows down.
//! A fence inside fenced code is code. A fence that opens and never closes
//! is only a line: a block appears when it is closed, so typing the first
//! fence does not reflow the rest of the text.

/// What a block does with what is in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A frame round it.
    Box,
    /// Nothing drawn.
    Plain,
    /// At this many times the size.
    Size(u8),
    /// In small type, half a cell across.
    Small,
    /// Side by side, one column a part.
    Columns,
    /// In rows of this many.
    Grid(u8),
    /// A free surface.
    Board,
}

/// The opening fence of a block, read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spec {
    pub kind: Kind,
    /// The words that name it: for a box, what is printed in its top edge
    /// after the fence.
    pub name: String,
    /// Cells wide, when the fence says.
    pub width: Option<usize>,
    /// On a board: cells from its left and rows from its top.
    pub at: Option<(usize, usize)>,
}

/// One fence line, read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fence {
    Open(Spec),
    Close,
}

/// The widest a block or grid may ask to be, and the most columns.
const MOST_CELLS: usize = 400;
const MOST_COLUMNS: u8 = 8;

/// The number of colons a fence line starts with, after at most three
/// spaces; `None` if fewer than three.
fn colons(line: &str) -> Option<(usize, usize)> {
    let pad = line.bytes().take_while(|&b| b == b' ').count();
    if pad > 3 {
        return None;
    }
    let run = line.bytes().skip(pad).take_while(|&b| b == b':').count();
    (run >= 3).then_some((pad, run))
}

/// True if the line is a fence, opening or closing. Cheap: the classifier
/// asks this of every line.
pub fn is_fence(line: &str) -> bool {
    colons(line).is_some()
}

/// Reads a fence line. `None` when the line is not one.
pub fn fence(line: &str) -> Option<Fence> {
    let (pad, run) = colons(line)?;
    let rest = line[pad + run..].trim();
    // Pandoc's attribute braces and dots are read as words.
    let words: Vec<&str> = rest.split(|c: char| c.is_whitespace() || c == '{' || c == '}').filter(|w| !w.is_empty()).map(|w| w.trim_start_matches('.')).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Some(Fence::Close);
    }
    let mut width = None;
    let mut at = None;
    let mut named: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let number = |k: usize| words.get(i + k).and_then(|w| w.trim_end_matches(',').parse::<usize>().ok());
        let lower = word.to_ascii_lowercase();
        if matches!(lower.as_str(), "width" | "wide") {
            if let Some(n) = number(1) {
                width = Some(n.clamp(1, MOST_CELLS));
                i += 2;
                continue;
            }
        }
        if let Some(n) = lower.strip_prefix("width=").and_then(|n| n.parse::<usize>().ok()) {
            width = Some(n.clamp(1, MOST_CELLS));
            i += 1;
            continue;
        }
        if lower == "at" {
            // `at 10 4` or `at 10,4`.
            if let Some((x, y)) = word_pair(words.get(i + 1).copied(), words.get(i + 2).copied()) {
                at = Some((x.min(MOST_CELLS), y.min(10_000)));
                i += if words.get(i + 1).is_some_and(|w| w.contains(',') && w.len() > 1 && !w.ends_with(',')) { 2 } else { 3 };
                continue;
            }
        }
        named.push(word);
        i += 1;
    }
    let first = named.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
    let (kind, skip) = match first.as_str() {
        "box" | "frame" => (Kind::Box, 1),
        "plain" | "group" => (Kind::Plain, 1),
        "big" | "large" | "double" => (Kind::Size(2), 1),
        "huge" | "triple" => (Kind::Size(3), 1),
        "small" | "fine" | "tiny" => (Kind::Small, 1),
        "columns" | "cols" | "side" | "row" => (Kind::Columns, 1),
        "grid" => {
            let n = named.get(1).and_then(|w| w.parse::<u8>().ok());
            match n {
                Some(n) => (Kind::Grid(n.clamp(1, MOST_COLUMNS)), 2),
                None => (Kind::Grid(2), 1),
            }
        }
        "board" => (Kind::Board, 1),
        "" => (Kind::Box, 0),
        _ => (Kind::Box, 0),
    };
    let name = named.get(skip..).unwrap_or(&[]).join(" ");
    Some(Fence::Open(Spec { kind, name, width, at }))
}

fn word_pair(a: Option<&str>, b: Option<&str>) -> Option<(usize, usize)> {
    let a = a?;
    if let Some((x, y)) = a.split_once(',') {
        if !y.is_empty() {
            return Some((x.parse().ok()?, y.parse().ok()?));
        }
    }
    Some((a.trim_end_matches(',').parse().ok()?, b?.parse().ok()?))
}

/// A block found in a document: its fences and what is between them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Node {
    pub open: usize,
    pub close: usize,
    pub spec: Spec,
    pub kids: Vec<Part>,
}

/// What a block, or the document, holds: lines and blocks, in order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Part {
    Line(usize),
    Node(Node),
}

/// The blocks of a document. `fence_at(i)` reads line `i` if it is a fence
/// outside code, and says nothing otherwise. A close with nothing open, and
/// an open that is never closed, are plain lines.
pub fn tree(count: usize, fence_at: &dyn Fn(usize) -> Option<Fence>) -> Vec<Part> {
    struct Open {
        line: usize,
        spec: Spec,
        kids: Vec<Part>,
    }
    let mut top: Vec<Part> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    for line in 0..count {
        match fence_at(line) {
            Some(Fence::Open(spec)) => stack.push(Open { line, spec, kids: Vec::new() }),
            Some(Fence::Close) if !stack.is_empty() => {
                let open = stack.pop().expect("not empty");
                let node = Part::Node(Node { open: open.line, close: line, spec: open.spec, kids: open.kids });
                match stack.last_mut() {
                    Some(parent) => parent.kids.push(node),
                    None => top.push(node),
                }
            }
            _ => match stack.last_mut() {
                Some(parent) => parent.kids.push(Part::Line(line)),
                None => top.push(Part::Line(line)),
            },
        }
    }
    // Opens never closed: the fence is a line, what followed it moves up.
    while let Some(open) = stack.pop() {
        let into = match stack.last_mut() {
            Some(parent) => &mut parent.kids,
            None => &mut top,
        };
        into.push(Part::Line(open.line));
        into.extend(open.kids);
    }
    top
}

/// The parts of a block laid side by side or in a grid: each block in it is
/// one, and so is each paragraph of loose lines. A blank line belongs to the
/// part before it, or, at the start, the part after it.
pub fn cells(kids: &[Part], blank: &dyn Fn(usize) -> bool) -> Vec<Vec<Part>> {
    let mut out: Vec<Vec<Part>> = Vec::new();
    let mut leading: Vec<Part> = Vec::new();
    // True while the last part is a paragraph still taking lines.
    let mut open_paragraph = false;
    for kid in kids {
        match kid {
            Part::Line(i) if blank(*i) => {
                open_paragraph = false;
                match out.last_mut() {
                    Some(last) => last.push(kid.clone()),
                    None => leading.push(kid.clone()),
                }
            }
            Part::Line(_) => {
                if open_paragraph {
                    out.last_mut().expect("open").push(kid.clone());
                } else {
                    let mut part = std::mem::take(&mut leading);
                    part.push(kid.clone());
                    out.push(part);
                    open_paragraph = true;
                }
            }
            Part::Node(_) => {
                let mut part = std::mem::take(&mut leading);
                part.push(kid.clone());
                out.push(part);
                open_paragraph = false;
            }
        }
    }
    if !leading.is_empty() {
        match out.last_mut() {
            Some(last) => last.extend(leading),
            None => out.push(leading),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(line: &str) -> Spec {
        match fence(line) {
            Some(Fence::Open(spec)) => spec,
            other => panic!("{line:?} read as {other:?}"),
        }
    }

    #[test]
    fn fences_are_read() {
        assert_eq!(fence("not a fence"), None);
        assert_eq!(fence(":: two"), None);
        assert_eq!(fence(":::"), Some(Fence::Close));
        assert_eq!(fence("::::   "), Some(Fence::Close));
        assert_eq!(fence("    ::: four spaces is code"), None);
        let spec = open("::: box Packing list");
        assert_eq!((spec.kind, spec.name.as_str()), (Kind::Box, "Packing list"));
        let spec = open("::: Ideas");
        assert_eq!((spec.kind, spec.name.as_str()), (Kind::Box, "Ideas"));
        assert_eq!(open("::: big").kind, Kind::Size(2));
        assert_eq!(open("::: huge").kind, Kind::Size(3));
        assert_eq!(open("::: small").kind, Kind::Small);
        assert_eq!(open("::: grid 3").kind, Kind::Grid(3));
        assert_eq!(open("::: grid").kind, Kind::Grid(2));
        assert_eq!(open("::: grid 99").kind, Kind::Grid(8));
        assert_eq!(open(":::: columns").kind, Kind::Columns);
        assert_eq!(open("::: {.box}").kind, Kind::Box);
    }

    #[test]
    fn width_and_place_are_read_anywhere() {
        let spec = open("::: box Food width 30 at 12 4");
        assert_eq!((spec.name.as_str(), spec.width, spec.at), ("Food", Some(30), Some((12, 4))));
        let spec = open("::: note at 3,5 Villain");
        assert_eq!((spec.kind, spec.name.as_str(), spec.at), (Kind::Box, "note Villain", Some((3, 5))));
        let spec = open("::: plain width=20");
        assert_eq!((spec.kind, spec.width), (Kind::Plain, Some(20)));
        // A word that only looks like an attribute stays in the name.
        assert_eq!(open("::: box at the beach").name, "at the beach");
    }

    #[test]
    fn blocks_nest_and_unclosed_fences_are_lines() {
        let lines = ["a", "::: columns", "::: box A", "x", ":::", "y", ":::", "::: box never closed", "z", ":::"];
        let parts = tree(lines.len(), &|i| fence(lines[i]));
        // The last close closes "never closed"; nothing is left open.
        assert_eq!(parts.len(), 3);
        let lines = ["::: box open", "x", "::: box inner", "y", ":::"];
        let parts = tree(lines.len(), &|i| fence(lines[i]));
        assert_eq!(parts[0], Part::Line(0));
        assert_eq!(parts[1], Part::Line(1));
        assert!(matches!(&parts[2], Part::Node(node) if node.open == 2 && node.close == 4));
        let lines = [":::", "x"];
        assert_eq!(tree(2, &|i| fence(lines[i])), vec![Part::Line(0), Part::Line(1)]);
    }

    #[test]
    fn parts_side_by_side_are_blocks_and_paragraphs() {
        let lines = ["", "one", "one more", "", "::: box", "b", ":::", "", "two", ""];
        let parts = tree(lines.len(), &|i| fence(lines[i]));
        let found = cells(&parts, &|i| lines[i].is_empty());
        let firsts: Vec<usize> = found
            .iter()
            .map(|part| match &part[0] {
                Part::Line(i) => *i,
                Part::Node(node) => node.open,
            })
            .collect();
        assert_eq!(firsts, [0, 4, 8]);
        assert_eq!(found[0].len(), 4);
    }
}
