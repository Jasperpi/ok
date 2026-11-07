//! Arranging the page from the line: plain words such as `box this`,
//! `bigger`, `side by side` or `move right 6` read into an [`Order`], and
//! an order planned against the document as one edit, which Ctrl Z takes
//! back whole.
//!
//! What an order acts on: the selection if there is one, as whole lines;
//! otherwise the block the cursor is on the fence of, or the paragraph it
//! is in. Orders that change a block (`bigger`, `unbox`, `width 30`,
//! `move`, `call it`) take the innermost block the cursor is in. On an
//! empty line, `box`, `grid` and `board` make an empty one to type into.

use crate::blocks::{self, Fence, Kind, Node, Part, Spec};
use crate::doc::{Doc, Pos, Span};
use crate::markup::{classify, LineKind};

/// What the line was asked to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Order {
    /// A frame round it, perhaps with a name.
    Box(Option<String>),
    /// Take away the block it is in, leaving what was in it.
    Unbox,
    /// A size: 0 is small type, 1 the page's own, 2 and 3 double and
    /// triple.
    Size(u8),
    /// One size larger or smaller.
    Bigger,
    Smaller,
    /// Beside the parts after it: this many in all, or the next one.
    Columns(Option<u8>),
    /// In rows of this many.
    Grid(u8),
    /// On a free surface.
    Board,
    /// Cells wide, or as wide as there is room.
    Width(Option<usize>),
    Wider,
    Narrower,
    /// Up or down past the part beside it; on a board, by cells and rows.
    Move(Way, Option<usize>),
    /// To a place on its board.
    MoveTo(usize, usize),
    /// A name for the block.
    Name(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Way {
    Up,
    Down,
    Left,
    Right,
}

/// The one edit an order comes to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub span: Span,
    pub text: String,
    /// Where the cursor goes after.
    pub cursor: Pos,
    /// What Enter will do, said before it is pressed.
    pub will: String,
    /// What was done, said after.
    pub done: String,
}

/// Words that say nothing about what to do.
const FILLER: &[&str] = &[
    "please", "make", "it", "this", "that", "these", "those", "the", "a", "an", "put", "into", "in", "to", "turn", "paragraph", "text",
    "selection", "section", "part", "can", "you", "i", "want", "set", "now", "so", "is", "be", "them", "its", "all", "of", "and", "up?",
    "lines", "line", "words", "list", "block", "thing", "stuff", "here", "just", "again", "bit", "little", "more", "much", "way",
];

fn number(word: &str) -> Option<usize> {
    let word = word.trim_end_matches(|c: char| c == ',' || c == '.');
    word.parse().ok().or_else(|| {
        let at = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"].iter().position(|&w| w == word)?;
        Some(at)
    })
}

/// Reads plain words as an order, or `None` when they are not one (and
/// are then a title, as everything else on the line is).
pub fn read(typed: &str) -> Option<Order> {
    let lower = typed.trim().to_lowercase();
    let lower = lower.trim_end_matches(['.', '!', '?']);
    if lower.is_empty() {
        return None;
    }
    // A name is everything after "called", "named" or "titled", as typed.
    let (head, name) = match find_word(lower, &["called", "named", "titled"]) {
        Some((at, len)) => (&lower[..at], Some(typed.trim()[at + len..].trim().trim_end_matches(['.', '!', '?']).to_string())),
        None => (lower, None),
    };
    let words: Vec<&str> = head.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let has_any = |list: &[&str]| list.iter().any(|w| has(w));
    let numbers: Vec<usize> = words.iter().filter_map(|w| number(w)).collect();
    let others: Vec<&str> = words.iter().copied().filter(|w| !FILLER.contains(w) && number(w).is_none()).collect();
    // Every word must be one of these, a filler, or a number: anything
    // else is a title someone is looking for.
    let only = |allowed: &[&str]| others.iter().all(|w| allowed.contains(w));

    // Taking a block away.
    if only(&["unbox", "remove", "delete", "take", "out", "no", "box", "frame", "unframe", "plain", "flatten", "undo", "block", "get", "rid", "without", "border", "outline", "from"])
        && (has_any(&["unbox", "unframe", "flatten", "rid", "without"]) || (has_any(&["remove", "no", "take", "delete"]) && has_any(&["box", "frame", "border", "outline", "block"])))
    {
        return Some(Order::Unbox);
    }
    if let Some(name) = &name {
        if only(&["call", "name", "rename", "title", "label"]) && !others.is_empty() {
            return Some(Order::Name(name.clone()));
        }
    }
    // "call it Food", "rename to Food", "name the box Food".
    let first = words.first().copied().unwrap_or("");
    if name.is_none() && matches!(first, "call" | "rename" | "name" | "title" | "label") && words.len() >= 2 {
        let typed = typed.trim();
        let mut rest = typed[first.len()..].trim_start();
        let mut skipped = 0;
        loop {
            let (word, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if matches!(word.to_lowercase().as_str(), "it" | "this" | "that" | "the" | "to" | "box" | "block" | "frame" | "as") && !after.trim().is_empty() {
                rest = after.trim_start();
                skipped += 1;
            } else {
                break;
            }
        }
        let rest = rest.trim_end_matches(['.', '!', '?']).trim();
        if !rest.is_empty() && (skipped > 0 || first == "call") {
            return Some(Order::Name(rest.to_string()));
        }
    }
    // A frame.
    if has_any(&["box", "frame", "boxed", "framed", "border", "outline", "card"]) && only(&["box", "frame", "boxed", "framed", "border", "outline", "card", "around", "round", "with", "new", "add", "another", "draw", "give", "wrap"]) {
        return Some(Order::Box(name.filter(|n| !n.is_empty())));
    }
    // "box Packing": a name without "called", after the word alone.
    if let Some(rest) = lower.strip_prefix("box ").or_else(|| lower.strip_prefix("frame ")) {
        let rest_typed = typed.trim()[typed.trim().len() - rest.len()..].trim();
        if !rest.split_whitespace().any(|w| matches!(w, "this" | "it" | "that")) && !rest_typed.is_empty() {
            return Some(Order::Box(Some(rest_typed.to_string())));
        }
    }
    // Sizes.
    let size_words = ["big", "bigger", "large", "larger", "huge", "huger", "biggest", "giant", "enormous", "double", "triple", "blow", "small", "smaller", "smallest", "shrink", "tiny", "normal", "regular", "size", "back", "grow", "enlarge", "reduce", "less", "down", "medium", "font", "text", "type", "letters", "up"];
    if only(&size_words) && !others.is_empty() {
        if has_any(&["huge", "huger", "biggest", "giant", "enormous", "triple"]) {
            return Some(Order::Size(3));
        }
        if has_any(&["normal", "regular", "medium"]) {
            return Some(Order::Size(1));
        }
        if has_any(&["small", "smallest", "tiny"]) && !has("smaller") {
            return Some(Order::Size(0));
        }
        if has_any(&["smaller", "shrink", "reduce", "less"]) || (has("down") && !has("blow")) {
            return Some(Order::Smaller);
        }
        if has_any(&["bigger", "larger", "grow", "enlarge"]) || (has("blow") && has("up")) {
            return Some(Order::Bigger);
        }
        if has_any(&["big", "large", "double"]) {
            return Some(Order::Size(2));
        }
    }
    // Side by side.
    if only(&["columns", "column", "side", "by", "beside", "next", "split", "across", "row", "two", "three", "four", "halves", "half", "cols", "together", "other"])
        && (has_any(&["columns", "column", "cols", "beside", "split", "halves"]) || (has("side") && has("by")) || (has("next") && has("other")) || has("across"))
        && !has("half")
    {
        let n = numbers.first().map(|&n| n.clamp(2, 8) as u8).or(if has("halves") { Some(2) } else { None });
        return Some(Order::Columns(n));
    }
    if only(&["grid", "table", "tiles", "by", "x", "across", "wide"]) && has_any(&["grid", "tiles"]) {
        let n = numbers.first().copied().unwrap_or(2).clamp(1, 8) as u8;
        return Some(Order::Grid(n));
    }
    if only(&["board", "canvas", "freeform", "free", "form", "corkboard", "pinboard", "surface", "loose", "new", "add"]) && has_any(&["board", "canvas", "freeform", "corkboard", "pinboard"]) {
        return Some(Order::Board);
    }
    // Width.
    if only(&["width", "wide", "cells", "columns", "chars", "characters", "wider", "narrower", "thinner", "slimmer", "fatter", "full", "half", "page", "widen", "narrow"]) && !others.is_empty() {
        if has_any(&["wider", "fatter", "widen"]) {
            return Some(Order::Wider);
        }
        if has_any(&["narrower", "thinner", "slimmer", "narrow"]) {
            return Some(Order::Narrower);
        }
        if has("full") {
            return Some(Order::Width(None));
        }
        if let Some(&n) = numbers.first() {
            if has_any(&["width", "wide", "cells", "chars", "characters"]) {
                return Some(Order::Width(Some(n.max(1))));
            }
        }
        if has("half") {
            return Some(Order::Width(Some(0)));
        }
    }
    // Moving.
    if only(&["move", "go", "shift", "nudge", "drag", "slide", "up", "down", "left", "right", "by", "cells", "rows", "row", "cell", "step", "steps", "place", "at", "position", "over", "higher", "lower", "earlier", "later"])
        && has_any(&["move", "shift", "nudge", "slide", "drag", "higher", "lower", "earlier", "later"])
    {
        let way = if has_any(&["up", "higher", "earlier"]) {
            Way::Up
        } else if has_any(&["down", "lower", "later"]) {
            Way::Down
        } else if has("left") {
            Way::Left
        } else if has("right") {
            Way::Right
        } else if numbers.len() >= 2 {
            return Some(Order::MoveTo(numbers[0], numbers[1]));
        } else {
            return None;
        };
        return Some(Order::Move(way, numbers.first().copied()));
    }
    None
}

/// Where a word from `list` first stands alone in `text`: its byte and its
/// length.
fn find_word(text: &str, list: &[&str]) -> Option<(usize, usize)> {
    let mut at = 0;
    for word in text.split(' ') {
        if list.contains(&word) {
            return Some((at, word.len()));
        }
        at += word.len() + 1;
    }
    None
}

/// The document read for planning.
struct Read<'a> {
    doc: &'a Doc,
    kinds: Vec<LineKind>,
    tree: Vec<Part>,
}

/// A run of whole lines, first and last.
type Lines = (usize, usize);

impl<'a> Read<'a> {
    fn of(doc: &'a Doc) -> Read<'a> {
        let kinds = classify(doc.lines());
        let tree = blocks::tree(doc.line_count(), &|i| if kinds[i] == LineKind::Div { blocks::fence(doc.line(i)) } else { None });
        Read { doc, kinds, tree }
    }

    fn blank(&self, line: usize) -> bool {
        self.kinds.get(line) == Some(&LineKind::Blank)
    }

    /// The blocks the line is in, outermost first, with the parts each
    /// holds; the document's own parts come first.
    fn path(&self, line: usize) -> Vec<(&[Part], Option<&Node>)> {
        let mut out: Vec<(&[Part], Option<&Node>)> = vec![(&self.tree, None)];
        loop {
            let parts = out.last().expect("never empty").0;
            let inner = parts.iter().find_map(|part| match part {
                Part::Node(node) if node.open < line && line < node.close => Some(node),
                _ => None,
            });
            match inner {
                Some(node) => out.push((&node.kids, Some(node))),
                None => return out,
            }
        }
    }

    /// The innermost block the line is in or on a fence of.
    fn block(&self, line: usize) -> Option<&Node> {
        let path = self.path(line);
        let (parts, node) = *path.last().expect("never empty");
        parts
            .iter()
            .find_map(|part| match part {
                Part::Node(node) if node.open == line || node.close == line => Some(node),
                _ => None,
            })
            .or(node)
    }

    /// The parts beside the line: the list the line is a part of, and which
    /// of them holds it. A paragraph is a part; so is a block.
    fn siblings(&self, line: usize) -> (Vec<Lines>, Option<usize>) {
        let path = self.path(line);
        let (parts, _) = *path.last().expect("never empty");
        let mut out: Vec<Lines> = Vec::new();
        let mut open = false;
        for part in parts {
            match part {
                Part::Line(i) if self.blank(*i) => open = false,
                Part::Line(i) => {
                    if open {
                        out.last_mut().expect("open").1 = *i;
                    } else if self.kinds[*i] == LineKind::Heading(1) || matches!(self.kinds[*i], LineKind::Heading(_)) {
                        out.push((*i, *i));
                        continue;
                    } else {
                        out.push((*i, *i));
                        open = true;
                    }
                }
                Part::Node(node) => {
                    out.push((node.open, node.close));
                    open = false;
                }
            }
        }
        let at = out.iter().position(|&(a, b)| a <= line && line <= b);
        (out, at)
    }

    /// The lines an order acts on: the selection as whole lines, or the
    /// part the cursor is in. `None` on a blank line with nothing selected.
    fn subject(&self, cursor: Span) -> Option<Lines> {
        if cursor.start != cursor.end {
            let last = if cursor.end.col == 0 && cursor.end.line > cursor.start.line { cursor.end.line - 1 } else { cursor.end.line };
            return Some((cursor.start.line, last));
        }
        let line = cursor.start.line;
        if self.blank(line) {
            return None;
        }
        let (parts, at) = self.siblings(line);
        at.map(|at| parts[at])
    }

    fn lines(&self, (a, b): Lines) -> String {
        (a..=b).map(|i| self.doc.line(i)).collect::<Vec<_>>().join("\n")
    }

    fn span(&self, (a, b): Lines) -> Span {
        Span { start: Pos::new(a, 0), end: Pos::new(b, self.doc.line(b).len()) }
    }

    /// Colons for a fence round these lines: one more than the most any
    /// fence inside has, so the outer fence reads as the outer one.
    fn colons(&self, (a, b): Lines) -> String {
        let most = (a..=b).filter(|&i| self.kinds[i] == LineKind::Div).map(|i| self.doc.line(i).trim_start().bytes().take_while(|&c| c == b':').count()).max();
        ":".repeat(most.map_or(3, |n| n + 1))
    }

    fn fence_spec(&self, line: usize) -> Option<Spec> {
        match blocks::fence(self.doc.line(line)) {
            Some(Fence::Open(spec)) => Some(spec),
            _ => None,
        }
    }
}

/// The parts beside a line, as its first and last lines, and which of them
/// holds it: the paragraphs and blocks of the block it is in, or of the
/// document. A block's fence line holds the block.
pub fn parts_beside(doc: &Doc, line: usize) -> (Vec<(usize, usize)>, Option<usize>) {
    Read::of(doc).siblings(line)
}

/// Puts lines between two fences: one edit, and the cursor kept on the
/// same character it was on.
fn wrap(read: &Read, lines: Lines, head: &str, cursor: Pos, will: String, done: String) -> Plan {
    let colons = read.colons(lines);
    let text = format!("{colons} {head}\n{}\n{colons}", read.lines(lines));
    Plan { span: read.span(lines), text, cursor: Pos::new(cursor.line + 1, cursor.col), will, done }
}

/// A fence line with its width and place written afresh, the rest of it as
/// it was.
fn restyle(line: &str, width: Option<Option<usize>>, at: Option<Option<(usize, usize)>>, name: Option<&str>) -> String {
    let pad = line.len() - line.trim_start().len();
    let colons = line[pad..].bytes().take_while(|&b| b == b':').count();
    let mut words: Vec<String> = Vec::new();
    let mut old_width = None;
    let mut old_at = None;
    let rest: Vec<&str> = line[pad + colons..].split_whitespace().collect();
    let mut i = 0;
    while i < rest.len() {
        let word = rest[i].to_ascii_lowercase();
        if matches!(word.as_str(), "width" | "wide") && rest.get(i + 1).and_then(|w| w.parse::<usize>().ok()).is_some() {
            old_width = rest[i + 1].parse().ok();
            i += 2;
            continue;
        }
        if word == "at" {
            let pair = rest.get(i + 1).and_then(|a| {
                if let Some((x, y)) = a.split_once(',').filter(|(_, y)| !y.is_empty()) {
                    Some(((x.parse().ok()?, y.parse().ok()?), 2))
                } else {
                    Some(((a.trim_end_matches(',').parse().ok()?, rest.get(i + 2)?.parse().ok()?), 3))
                }
            });
            if let Some((pair, used)) = pair {
                old_at = Some(pair);
                i += used;
                continue;
            }
        }
        words.push(rest[i].to_string());
        i += 1;
    }
    if let Some(name) = name {
        // The kind word stays; the name after it is replaced.
        let keep = words.first().filter(|w| matches!(w.to_ascii_lowercase().as_str(), "box" | "frame" | "plain" | "group" | "big" | "huge" | "columns" | "board" | "grid")).cloned();
        let grid_n = if keep.as_deref() == Some("grid") { words.get(1).filter(|w| w.parse::<u8>().is_ok()).cloned() } else { None };
        words = keep.into_iter().chain(grid_n).collect();
        if words.is_empty() {
            words.push(String::from("box"));
        }
        words.extend(name.split_whitespace().map(str::to_string));
    }
    let width = width.unwrap_or(old_width);
    let at = at.unwrap_or(old_at);
    let mut out = format!("{}{}", &line[..pad], ":".repeat(colons));
    for word in &words {
        out.push(' ');
        out.push_str(word);
    }
    if let Some((x, y)) = at {
        out.push_str(&format!(" at {x},{y}"));
    }
    if let Some(w) = width {
        out.push_str(&format!(" width {w}"));
    }
    out
}

/// Plans an order. `cursor` is the selection, or the cursor alone as an
/// empty span; `measure` the cells of a row, for widths. `Err` is a
/// sentence saying why it cannot be done, starting with `?` as the line's
/// mistakes do.
pub fn plan(doc: &Doc, cursor: Span, head: Pos, measure: usize, order: &Order) -> Result<Plan, String> {
    let read = Read::of(doc);
    let line = head.line.min(doc.line_count().saturating_sub(1));
    let head = Pos::new(line, head.col.min(doc.line(line).len()));
    let subject = read.subject(cursor);
    match order {
        Order::Box(name) => {
            let fence = match name {
                Some(name) => format!("box {name}"),
                None => String::from("box"),
            };
            let called = name.as_ref().map(|n| format!(" called {n}")).unwrap_or_default();
            match subject {
                Some(lines) => Ok(wrap(&read, lines, &fence, head, format!("Enter puts {} in a box{called}.", what(&read, lines)), format!("Boxed{called}. Ctrl Z takes it back."))),
                None => Ok(Plan {
                    span: read.span((line, line)),
                    text: format!("::: {fence}\n\n:::"),
                    cursor: Pos::new(line + 1, 0),
                    will: format!("Enter makes an empty box{called} here."),
                    done: String::from("A box. Type in it."),
                }),
            }
        }
        Order::Unbox => {
            let node = read.block(line).ok_or("? The cursor is in no block.")?;
            let inside = if node.close > node.open + 1 { read.lines((node.open + 1, node.close - 1)) } else { String::new() };
            let gone = if node.close > node.open + 1 { 1 } else { 0 };
            let to = if line > node.open { Pos::new((line - gone).min(node.close.saturating_sub(2).max(node.open)), head.col) } else { Pos::new(node.open, 0) };
            let span = read.span((node.open, node.close));
            let (will, done) = (format!("Enter takes away the {} and keeps what is in it.", kind_name(&node.spec)), format!("The {} is gone; what was in it stays.", kind_name(&node.spec)));
            if node.close == node.open + 1 {
                return Ok(Plan { span, text: String::new(), cursor: Pos::new(node.open, 0), will, done });
            }
            Ok(Plan { span, text: inside, cursor: to, will, done })
        }
        Order::Size(_) | Order::Bigger | Order::Smaller => {
            // The size block the cursor is in, if it is in one.
            let sizing = |n: &&Node| matches!(n.spec.kind, Kind::Size(_) | Kind::Small);
            let sized = read.path(line).iter().rev().find_map(|&(_, node)| node.filter(sizing)).or_else(|| read.block(line).filter(sizing));
            let now = sized.map_or(1, |node| match node.spec.kind {
                Kind::Size(n) => n,
                Kind::Small => 0,
                _ => 1,
            });
            let want = match order {
                Order::Size(n) => (*n).min(3),
                Order::Bigger => (now + 1).min(3),
                _ => now.saturating_sub(1),
            };
            if want == now {
                return Err(match (order, now) {
                    (Order::Smaller, 0) => String::from("? Small is the smallest size."),
                    (Order::Size(1), 1) => String::from("? That is the page's own size already."),
                    (Order::Bigger, 3) => String::from("? Triple is the largest size."),
                    _ => String::from("? It is that size already."),
                });
            }
            let word = ["small", "", "big", "huge"][want as usize];
            let size_name = ["small type", "the page's own size", "double size", "triple size"][want as usize];
            let (prep, cap) = if want == 0 { ("in", "In") } else { ("at", "At") };
            match sized {
                Some(_) if want == 1 => {
                    let mut plan = plan(doc, cursor, head, measure, &Order::Unbox)?;
                    plan.will = String::from("Enter sets it back at the page's own size.");
                    plan.done = String::from("Back at the page's own size.");
                    Ok(plan)
                }
                Some(node) => {
                    let old = doc.line(node.open);
                    let pad = old.len() - old.trim_start().len();
                    let colons = old[pad..].bytes().take_while(|&b| b == b':').count();
                    let rest = restyle(old, None, None, None);
                    let attrs = rest.split_once(" at ").map(|(_, r)| format!(" at {r}")).or_else(|| rest.split_once(" width ").map(|(_, r)| format!(" width {r}"))).unwrap_or_default();
                    let text = format!("{}{} {word}{attrs}", &old[..pad], ":".repeat(colons));
                    Ok(Plan { span: read.span((node.open, node.open)), text, cursor: head, will: format!("Enter sets it {prep} {size_name}."), done: format!("{cap} {size_name}.") })
                }
                None => {
                    let lines = subject.ok_or("? Put the cursor in the words to size, or select them.")?;
                    Ok(wrap(&read, lines, word, head, format!("Enter sets {} {prep} {size_name}.", what(&read, lines)), format!("{cap} {size_name}. Ctrl Z takes it back.")))
                }
            }
        }
        Order::Columns(count) => {
            if cursor.start != cursor.end {
                let lines = subject.expect("a selection");
                return Ok(wrap(&read, lines, "columns", head, format!("Enter sets {} side by side.", what(&read, lines)), String::from("Side by side. Ctrl Z takes it back.")));
            }
            let (parts, at) = read.siblings(line);
            let at = at.ok_or("? Put the cursor in a paragraph or a block to set beside the next.")?;
            let count = count.map_or(2, |n| n as usize);
            let last = (at + count - 1).min(parts.len() - 1);
            if last == at {
                return Err(String::from("? There is nothing after this to set beside it."));
            }
            let lines = (parts[at].0, parts[last].1);
            let n = last - at + 1;
            Ok(wrap(&read, lines, "columns", head, format!("Enter sets this and the {} after it side by side.", if n == 2 { String::from("one") } else { format!("{} parts", n - 1) }), format!("{n} side by side. Ctrl Z takes it back.")))
        }
        Order::Grid(n) => {
            let fence = format!("grid {n}");
            if let Some(lines) = subject.filter(|_| cursor.start != cursor.end) {
                return Ok(wrap(&read, lines, &fence, head, format!("Enter sets {} in rows of {n}.", what(&read, lines)), format!("In rows of {n}. Ctrl Z takes it back.")));
            }
            let (parts, at) = read.siblings(line);
            match at {
                Some(at) => {
                    // This part and those after it, up to a heading.
                    let mut last = at;
                    while last + 1 < parts.len() && !matches!(read.kinds[parts[last + 1].0], LineKind::Heading(_)) {
                        last += 1;
                    }
                    let lines = (parts[at].0, parts[last].1);
                    Ok(wrap(&read, lines, &fence, head, format!("Enter sets the {} parts from here in rows of {n}.", last - at + 1), format!("In rows of {n}. Ctrl Z takes it back.")))
                }
                None => {
                    let cells: Vec<String> = (0..*n).map(|_| String::from("::: box\n\n:::")).collect();
                    Ok(Plan {
                        span: read.span((line, line)),
                        text: format!(":::: {fence}\n{}\n::::", cells.join("\n")),
                        cursor: Pos::new(line + 2, 0),
                        will: format!("Enter makes a row of {n} empty boxes here."),
                        done: format!("{n} boxes in a row. Type in the first."),
                    })
                }
            }
        }
        Order::Board => match subject {
            Some(lines) => {
                let colons = read.colons(lines);
                let outer = if colons.len() == 3 { String::from("::::") } else { format!("{colons}:") };
                let text = format!("{outer} board\n{colons} Note at 0,0\n{}\n{colons}\n{outer}", read.lines(lines));
                Ok(Plan { span: read.span(lines), text, cursor: Pos::new(head.line + 2, head.col), will: format!("Enter puts {} on a board, where it can go anywhere.", what(&read, lines)), done: String::from("On a board. Say move right 10, or drag it.") })
            }
            None => Ok(Plan {
                span: read.span((line, line)),
                text: String::from(":::: board\n::: Note at 0,0\n\n:::\n::::"),
                cursor: Pos::new(line + 2, 0),
                will: String::from("Enter makes a board here, with one note on it."),
                done: String::from("A board. Type in the note; say move right 10, or drag it."),
            }),
        },
        Order::Width(_) | Order::Wider | Order::Narrower => {
            let node = read.block(line);
            let now = node.and_then(|n| n.spec.width).unwrap_or(measure);
            let want = match order {
                Order::Width(Some(0)) => Some(measure / 2),
                Order::Width(w) => *w,
                Order::Wider => Some(now + 8),
                _ => Some(now.saturating_sub(8).max(8)),
            };
            let want = want.filter(|&w| w < measure).map(|w| w.max(8));
            let said = want.map_or(String::from("as wide as the page"), |w| format!("{w} cells wide"));
            match node {
                Some(node) => {
                    let text = restyle(doc.line(node.open), Some(want), None, None);
                    Ok(Plan { span: read.span((node.open, node.open)), text, cursor: head, will: format!("Enter makes the {} {said}.", kind_name(&node.spec)), done: format!("Now {said}.") })
                }
                None => {
                    let lines = subject.ok_or("? Put the cursor in the words to narrow, or select them.")?;
                    let w = want.ok_or("? It is as wide as the page already.")?;
                    Ok(wrap(&read, lines, &format!("plain width {w}"), head, format!("Enter sets {} {said}.", what(&read, lines)), format!("Now {said}. Ctrl Z takes it back.")))
                }
            }
        }
        Order::Name(name) => {
            let node = read.block(line).ok_or("? The cursor is in no block to name.")?;
            let text = restyle(doc.line(node.open), None, None, Some(name));
            Ok(Plan { span: read.span((node.open, node.open)), text, cursor: head, will: format!("Enter calls the {} {name}.", kind_name(&node.spec)), done: format!("Called {name}.") })
        }
        Order::MoveTo(x, y) => {
            let node = read.path(line).iter().rev().find_map(|&(_, n)| n.filter(|n| n.spec.at.is_some())).or_else(|| read.block(line).filter(|n| n.spec.at.is_some()));
            let node = node.ok_or("? Only a note on a board has a place to move to.")?;
            let text = restyle(doc.line(node.open), None, Some(Some((*x, *y))), None);
            Ok(Plan { span: read.span((node.open, node.open)), text, cursor: head, will: format!("Enter moves it to {x} across, {y} down."), done: format!("At {x} across, {y} down.") })
        }
        Order::Move(way, by) => {
            // On a board: by cells and rows.
            let piece = read.path(line).iter().rev().find_map(|&(_, n)| n.filter(|n| n.spec.at.is_some())).or_else(|| read.block(line).filter(|n| n.spec.at.is_some()));
            if let Some(node) = piece {
                let (x, y) = node.spec.at.unwrap_or_default();
                let step = by.unwrap_or(match way {
                    Way::Left | Way::Right => 6,
                    _ => 2,
                });
                let (x, y) = match way {
                    Way::Left => (x.saturating_sub(step), y),
                    Way::Right => (x + step, y),
                    Way::Up => (x, y.saturating_sub(step)),
                    Way::Down => (x, y + step),
                };
                let text = restyle(doc.line(node.open), None, Some(Some((x, y))), None);
                return Ok(Plan { span: read.span((node.open, node.open)), text, cursor: head, will: format!("Enter moves it to {x} across, {y} down."), done: format!("At {x} across, {y} down.") });
            }
            if matches!(way, Way::Left | Way::Right) {
                return Err(String::from("? Left and right move a note on a board. This is in the flow of the text: say move up or move down."));
            }
            let (parts, at) = read.siblings(line);
            let at = at.ok_or("? Put the cursor in the paragraph or block to move.")?;
            let times = by.unwrap_or(1).max(1);
            let other = match way {
                Way::Up => at.checked_sub(times).ok_or("? Nothing is above it to move past.")?,
                _ => {
                    let to = at + times;
                    if to >= parts.len() {
                        return Err(String::from("? Nothing is below it to move past."));
                    }
                    to
                }
            };
            let (lo, hi) = (at.min(other), at.max(other));
            let moving = parts[at];
            let between: Vec<Lines> = parts[lo..=hi].iter().copied().filter(|&p| p != moving).collect();
            // The parts keep the blank lines that were between them.
            let gaps: Vec<String> = (lo..hi).map(|k| if parts[k + 1].0 > parts[k].1 + 1 { "\n".repeat(parts[k + 1].0 - parts[k].1 - 1) } else { String::new() }).collect();
            let order_now: Vec<Lines> = if *way == Way::Up { std::iter::once(moving).chain(between).collect() } else { between.into_iter().chain(std::iter::once(moving)).collect() };
            let mut text = String::new();
            let mut cursor_line = 0;
            let mut lines_so_far = parts[lo].0;
            for (k, &part) in order_now.iter().enumerate() {
                if part == moving {
                    cursor_line = lines_so_far + (head.line - moving.0);
                }
                text.push_str(&read.lines(part));
                lines_so_far += part.1 - part.0 + 1;
                if k + 1 < order_now.len() {
                    text.push('\n');
                    text.push_str(&gaps[k]);
                    lines_so_far += gaps[k].len();
                }
            }
            let span = Span { start: Pos::new(parts[lo].0, 0), end: Pos::new(parts[hi].1, doc.line(parts[hi].1).len()) };
            let word = if *way == Way::Up { "up" } else { "down" };
            Ok(Plan { span, text, cursor: Pos::new(cursor_line, head.col), will: format!("Enter moves {} {word} past {}.", what(&read, moving), if times == 1 { String::from("the next part") } else { format!("{times} parts") }), done: format!("Moved {word}. Ctrl Z takes it back.") })
        }
    }
}

fn kind_name(spec: &Spec) -> &'static str {
    match spec.kind {
        Kind::Box => "box",
        Kind::Plain => "block",
        Kind::Size(_) | Kind::Small => "sizing",
        Kind::Columns => "columns",
        Kind::Grid(_) => "grid",
        Kind::Board => "board",
    }
}

/// How a run of lines is spoken of.
fn what(read: &Read, (a, b): Lines) -> String {
    if read.kinds[a] == LineKind::Div {
        if let Some(spec) = read.fence_spec(a) {
            return format!("this {}", kind_name(&spec));
        }
    }
    let paragraphs = (a..=b).filter(|&i| !read.blank(i) && (i == a || read.blank(i - 1))).count();
    match (paragraphs, read.kinds[a]) {
        (1, LineKind::Heading(_)) => String::from("this heading"),
        (1, LineKind::Item) if b > a => String::from("this list"),
        (1, LineKind::Item) => String::from("this item"),
        (1, _) => String::from("this paragraph"),
        (n, _) => format!("these {n} paragraphs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos::new(line, col)
    }

    fn at(pos: Pos) -> Span {
        Span { start: pos, end: pos }
    }

    /// The text after an order is carried out at a cursor.
    fn after(text: &str, cursor: Pos, typed: &str) -> (String, Pos) {
        let mut doc = Doc::from_text(text);
        let order = read(typed).unwrap_or_else(|| panic!("{typed:?} not read"));
        let plan = plan(&doc, at(cursor), cursor, 66, &order).unwrap_or_else(|e| panic!("{typed:?}: {e}"));
        doc.replace(plan.span, &plan.text);
        (doc.lines().join("\n"), plan.cursor)
    }

    #[test]
    fn plain_words_are_read_as_orders() {
        assert_eq!(read("box this"), Some(Order::Box(None)));
        assert_eq!(read("put it in a box"), Some(Order::Box(None)));
        assert_eq!(read("Put this in a box called Packing list"), Some(Order::Box(Some(String::from("Packing list")))));
        assert_eq!(read("box Packing"), Some(Order::Box(Some(String::from("Packing")))));
        assert_eq!(read("frame it"), Some(Order::Box(None)));
        assert_eq!(read("unbox"), Some(Order::Unbox));
        assert_eq!(read("remove the box"), Some(Order::Unbox));
        assert_eq!(read("take it out of the box"), Some(Order::Unbox));
        assert_eq!(read("make it big"), Some(Order::Size(2)));
        assert_eq!(read("bigger"), Some(Order::Bigger));
        assert_eq!(read("blow it up"), Some(Order::Bigger));
        assert_eq!(read("huge"), Some(Order::Size(3)));
        assert_eq!(read("smaller"), Some(Order::Smaller));
        assert_eq!(read("make it small"), Some(Order::Size(0)));
        assert_eq!(read("tiny text"), Some(Order::Size(0)));
        assert_eq!(read("normal size"), Some(Order::Size(1)));
        assert_eq!(read("side by side"), Some(Order::Columns(None)));
        assert_eq!(read("three columns"), Some(Order::Columns(Some(3))));
        assert_eq!(read("columns 2"), Some(Order::Columns(Some(2))));
        assert_eq!(read("grid 3"), Some(Order::Grid(3)));
        assert_eq!(read("make a grid of 4"), Some(Order::Grid(4)));
        assert_eq!(read("board"), Some(Order::Board));
        assert_eq!(read("width 30"), Some(Order::Width(Some(30))));
        assert_eq!(read("make it 40 wide"), Some(Order::Width(Some(40))));
        assert_eq!(read("wider"), Some(Order::Wider));
        assert_eq!(read("full width"), Some(Order::Width(None)));
        assert_eq!(read("move up"), Some(Order::Move(Way::Up, None)));
        assert_eq!(read("move right 10"), Some(Order::Move(Way::Right, Some(10))));
        assert_eq!(read("move it to 12 4"), Some(Order::MoveTo(12, 4)));
        assert_eq!(read("call it Food"), Some(Order::Name(String::from("Food"))));
        // Titles stay titles.
        assert_eq!(read("Grandfather interview"), None);
        assert_eq!(read("essay plan"), None);
        assert_eq!(read("the big sleep review"), None);
        assert_eq!(read("moving day"), None);
        assert_eq!(read("title ideas"), None);
        assert_eq!(read("name list"), None);
        assert_eq!(read("name it Supper"), Some(Order::Name(String::from("Supper"))));
    }

    #[test]
    fn box_wraps_the_paragraph_and_unbox_takes_it_away() {
        let text = "intro\n\none\ntwo\n\nend";
        let (boxed, cursor) = after(text, p(3, 1), "box this called Notes");
        assert_eq!(boxed, "intro\n\n::: box Notes\none\ntwo\n:::\n\nend");
        assert_eq!(cursor, p(4, 1));
        let (back, cursor) = after(&boxed, cursor, "unbox");
        assert_eq!(back, text);
        assert_eq!(cursor, p(3, 1));
        // On an empty line, an empty box to type in.
        let (made, cursor) = after("a\n\nb", p(1, 0), "box");
        assert_eq!(made, "a\n::: box\n\n:::\nb");
        assert_eq!(cursor, p(2, 0));
    }

    #[test]
    fn a_box_round_a_box_has_the_longer_fence() {
        let (boxed, _) = after("::: box\nx\n:::", p(0, 0), "box this");
        assert_eq!(boxed, ":::: box\n::: box\nx\n:::\n::::");
    }

    #[test]
    fn sizes_step_up_and_down() {
        let (big, cursor) = after("words", p(0, 2), "bigger");
        assert_eq!(big, "::: big\nwords\n:::");
        let (huge, cursor) = after(&big, cursor, "bigger");
        assert_eq!(huge, "::: huge\nwords\n:::");
        let (back, cursor) = after(&huge, cursor, "smaller");
        assert_eq!(back, "::: big\nwords\n:::");
        let (plain, cursor) = after(&back, cursor, "smaller");
        assert_eq!(plain, "words");
        let (small, cursor) = after(&plain, cursor, "smaller");
        assert_eq!(small, "::: small\nwords\n:::");
        let doc = Doc::from_text(&small);
        assert!(plan(&doc, at(cursor), cursor, 66, &Order::Smaller).is_err());
        let (normal, _) = after(&small, cursor, "bigger");
        assert_eq!(normal, "words");
        let (small, _) = after("words", p(0, 0), "small");
        assert_eq!(small, "::: small\nwords\n:::");
    }

    #[test]
    fn side_by_side_takes_the_next_part() {
        let (set, _) = after("a\n\n::: box\nb\n:::\n\nc", p(0, 0), "side by side");
        assert_eq!(set, ":::: columns
a

::: box
b
:::
::::

c");
        assert!(plan(&Doc::from_text("alone"), at(p(0, 0)), p(0, 0), 66, &Order::Columns(None)).is_err());
    }

    #[test]
    fn notes_on_a_board_move_by_cells_and_rows() {
        let text = ":::: board\n::: Note at 4 2\nx\n:::\n::::";
        let (moved, _) = after(text, p(2, 0), "move right 6");
        assert_eq!(moved, ":::: board\n::: Note at 10,2\nx\n:::\n::::");
        let (moved, _) = after(&moved, p(2, 0), "move up");
        assert_eq!(moved, ":::: board\n::: Note at 10,0\nx\n:::\n::::");
        let (moved, _) = after(&moved, p(2, 0), "move to 3 5");
        assert_eq!(moved, ":::: board\n::: Note at 3,5\nx\n:::\n::::");
    }

    #[test]
    fn parts_move_past_each_other_in_the_flow() {
        let (moved, cursor) = after("a\n\nb\nb2\n\nc", p(3, 1), "move up");
        assert_eq!(moved, "b\nb2\n\na\n\nc");
        assert_eq!(cursor, p(1, 1));
        let (moved, cursor) = after("a\n\nb\n\nc", p(0, 0), "move down");
        assert_eq!(moved, "b\n\na\n\nc");
        assert_eq!(cursor, p(2, 0));
    }

    #[test]
    fn widths_and_names_rewrite_the_fence() {
        let (narrow, _) = after("::: box Food\nx\n:::", p(1, 0), "width 30");
        assert_eq!(narrow, "::: box Food width 30\nx\n:::");
        let (named, _) = after(&narrow, p(1, 0), "call it Supper");
        assert_eq!(named, "::: box Supper width 30\nx\n:::");
        let (wide, _) = after(&named, p(1, 0), "full width");
        assert_eq!(wide, "::: box Supper\nx\n:::");
        let (plain, _) = after("words", p(0, 0), "width 20");
        assert_eq!(plain, "::: plain width 20\nwords\n:::");
    }

    #[test]
    fn a_board_and_a_grid_can_be_made_from_nothing() {
        let (board, cursor) = after("", p(0, 0), "make a board");
        assert_eq!(board, ":::: board\n::: Note at 0,0\n\n:::\n::::");
        assert_eq!(cursor, p(2, 0));
        let (grid, _) = after("", p(0, 0), "grid 2");
        assert_eq!(grid, ":::: grid 2\n::: box\n\n:::\n::: box\n\n:::\n::::");
    }
}
