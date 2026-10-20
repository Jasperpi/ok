//! A deliberately small markdown: only what a prose writer types. It says
//! what each line is and where its markup characters sit, so the interface
//! can style them. It classifies text; it never rewrites it.

use std::collections::HashMap;

use crate::links::Wiki;
use crate::width::{is_combining, str_width};

/// What one whole line is.
///
/// Wherever a marker wants a space after it, or allows spaces before it or
/// inside it, a tab does as well. The one exception is in front of a fence:
/// a tab is four cells wide, which is more indentation than a fence may have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    /// Empty, or nothing but whitespace.
    Blank,
    /// One to six `#` at the very start of the line, then a space or the end
    /// of the line. Carries the number of `#`.
    Heading(u8),
    /// Optional indentation, then `>`.
    Quote,
    /// Optional indentation, then `-`, `*` or `+` and a space, or digits,
    /// `.` or `)` and a space. A task box (`[ ]`, `[x]` or `[X]` and a
    /// space) straight after the marker belongs to it.
    Item,
    /// Three backticks or three tildes after at most three spaces. Opens a
    /// block of code, or closes the one that is open.
    Fence,
    /// Any line between an opening fence and its closing fence.
    Code,
    /// Three or more of the same one of `-`, `*` and `_`, with nothing else
    /// on the line but spaces.
    Rule,
    /// Anything else: a paragraph.
    Text,
}

/// The kind of every line. An opening fence with no closing fence turns
/// everything below it into code.
pub fn classify(lines: &[String]) -> Vec<LineKind> {
    let mut in_fence = false;
    lines
        .iter()
        .map(|line| {
            let kind = classify_line(line, in_fence);
            in_fence ^= kind == LineKind::Fence;
            kind
        })
        .collect()
}

/// The kind of one line, given whether a fence opened above it is still
/// open. Every `Fence` line flips that state, so a caller walking down a
/// document keeps it with `in_fence ^= kind == LineKind::Fence`.
pub fn classify_line(line: &str, in_fence: bool) -> LineKind {
    if is_fence(line) {
        LineKind::Fence
    } else if in_fence {
        LineKind::Code
    } else if line.chars().all(char::is_whitespace) {
        LineKind::Blank
    } else if is_rule(line) {
        // Before Item, so that "- - -" and "***" are rules and not lists.
        LineKind::Rule
    } else if let Some(head) = Head::of(line) {
        LineKind::Heading(head.level)
    } else if quote_end(line).is_some() {
        LineKind::Quote
    } else if item_end(line).is_some() {
        LineKind::Item
    } else {
        LineKind::Text
    }
}

/// Cells of hanging indent for the wrapped rows of this line, so that they
/// start under its text and not under its marker: the width of the
/// indentation plus the list marker and task box of an `Item`, or of the
/// indentation plus the `>` and one following space of a `Quote`. 0 for
/// every other kind.
///
/// The line is judged on its own, as if no fence were open; a caller that
/// knows the line is `Code` should not ask. Deep indentation gives a large
/// answer, which the caller must keep narrower than its window.
pub fn hang_of(line: &str) -> usize {
    str_width(line.get(..marker_len(line)).unwrap_or(""))
}

/// The same stretch [`hang_of`] measures, in bytes: where the text of an
/// `Item` or `Quote` starts. 0 for every other kind.
fn marker_len(line: &str) -> usize {
    match classify_line(line, false) {
        LineKind::Item => item_end(line).unwrap_or(0),
        LineKind::Quote => quote_end(line).map_or(0, |end| end + usize::from(is_gap(line.as_bytes().get(end)))),
        _ => 0,
    }
}

/// The level and title of a heading line: the title is trimmed, and has
/// neither the opening `#`s nor a closing run of `#`s. A run closes the
/// heading only when a space or tab sets it off from the title, so `# C#`
/// is still called `C#`. `None` when the line is not a heading. Fences are
/// not this function's business; see [`outline`].
pub fn heading(line: &str) -> Option<(u8, &str)> {
    let head = Head::of(line)?;
    Some((head.level, line.get(head.body..head.stop)?.trim()))
}

/// One entry of a document's outline.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heading {
    pub line: usize,
    pub level: u8,
    pub title: String,
}

/// The headings of a document in order, skipping anything inside fences.
pub fn outline(lines: &[String]) -> Vec<Heading> {
    let mut found = Vec::new();
    let mut in_fence = false;
    for (line, text) in lines.iter().enumerate() {
        match classify_line(text, in_fence) {
            LineKind::Fence => in_fence = !in_fence,
            LineKind::Heading(_) => {
                if let Some((level, title)) = heading(text) {
                    found.push(Heading { line, level, title: title.to_string() });
                }
            }
            _ => {}
        }
    }
    found
}

/// How a stretch of a line should look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Plain,
    /// The markup characters themselves, so the interface can fade them: the
    /// `#`s of a heading and the space after them, a list marker and task
    /// box, the `>` of a quote, emphasis asterisks and underscores,
    /// backticks, the `[[` and `]]` of a wiki link together with the
    /// `target|` part of an aliased one, the `[`, `](` and `)` of a markdown
    /// link, and an escaping backslash.
    Syntax,
    Strong,
    Emph,
    StrongEmph,
    /// The inside of a backtick code span.
    Code,
    /// The visible text of a wiki link or a markdown link.
    Link,
    /// A bare `http://` or `https://` address, or the address inside a
    /// markdown link.
    Url,
}

/// A stretch of a line, from byte `start` up to but not including `end`,
/// drawn in one style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Splits a line into styled pieces. They come in order, none is empty,
/// each starts where the one before ended, together they cover the line
/// exactly, and no two neighbours share a style. An empty line gives none.
///
/// `kind` is trusted: a `Code` or `Fence` line is one `Plain` piece and a
/// `Rule` one `Syntax` piece whatever the text says, and a line that does
/// not have the marker its kind promises is styled as a paragraph.
///
/// Inside a line: `**strong**`, `*emph*`, `_emph_`, `***both***`, backtick
/// code, `[[target]]`, `[[target|shown]]`, `[text](url)` and bare addresses.
/// Emphasis markers must hug their text. An underscore counts only alone
/// and at a word boundary: two or more together are ordinary text, and so
/// is one with a letter or digit on its outer side, seen through any
/// combining marks. A closer closes the nearest opener like it, and an
/// opener with no closer on the line is plain text.
///
/// A backslash makes the markup character after it plain, and is itself
/// `Syntax`. In the text or the address of a `[text](url)` it keeps a
/// bracket or parenthesis from ending that part, and is drawn with the part
/// it is in. It does nothing in code, in a `[[link]]`, whose target may be
/// a path written with backslashes, or in a bare address.
///
/// Styles never stack except as `StrongEmph`: nothing inside code or link
/// text is read as markup, and emphasis inside emphasis is plain. Emphasis
/// may run across code, links and addresses, which keep their own style.
pub fn pieces(line: &str, kind: LineKind) -> Vec<Piece> {
    let mut out = Out { pieces: Vec::new(), at: 0 };
    if kind == LineKind::Rule {
        out.to(line.len(), Style::Syntax);
    } else if let Some(layout) = Layout::of(line, kind) {
        out.to(layout.indent, Style::Plain);
        out.to(layout.text, Style::Syntax);
        let text = line.get(..layout.stop).unwrap_or(line);
        scan(text, layout.text).emit(&mut out);
        out.to(layout.stop, Style::Plain);
        out.to(layout.tail, Style::Syntax);
    }
    out.to(line.len(), Style::Plain);
    out.pieces
}

/// The wiki links in a line of the given kind. They come from the same scan
/// that styles the line, so what is drawn as a link and what can be followed
/// as one never disagree.
pub(crate) fn wikis(line: &str, kind: LineKind) -> Vec<Wiki> {
    match Layout::of(line, kind) {
        Some(layout) => scan(line.get(..layout.stop).unwrap_or(line), layout.text).wikis,
        None => Vec::new(),
    }
}

/// Builds the pieces of a line from left to right. Because each call styles
/// everything from where the last one stopped, the result cannot have gaps,
/// overlaps or two neighbours of one style.
struct Out {
    pieces: Vec<Piece>,
    at: usize,
}

impl Out {
    fn to(&mut self, end: usize, style: Style) {
        if end <= self.at {
            return;
        }
        match self.pieces.last_mut() {
            Some(last) if last.style == style => last.end = end,
            _ => self.pieces.push(Piece { start: self.at, end, style }),
        }
        self.at = end;
    }
}

/// How a line divides: plain indentation up to `indent`, block markup up to
/// `text`, inline text up to `stop`, closing markup up to `tail`, and plain
/// leftovers after that.
struct Layout {
    indent: usize,
    text: usize,
    stop: usize,
    tail: usize,
}

impl Layout {
    /// `None` for the kinds that hold no inline text at all.
    fn of(line: &str, kind: LineKind) -> Option<Layout> {
        let len = line.len();
        let (indent, text, stop, tail) = match kind {
            LineKind::Code | LineKind::Fence | LineKind::Rule => return None,
            LineKind::Blank | LineKind::Text => (0, 0, len, len),
            LineKind::Heading(_) => Head::of(line).map_or((0, 0, len, len), |head| (0, head.body, head.stop, head.tail)),
            LineKind::Quote => quote_end(line).map_or((0, 0, len, len), |end| (end - 1, end, len, len)),
            LineKind::Item => item_end(line).map_or((0, 0, len, len), |end| (indent_len(line), end, len, len)),
        };
        Some(Layout { indent, text, stop, tail })
    }
}

/// The parts of a heading line: its title sits somewhere in `body..stop`,
/// and a closing run of `#`s, if there is one, in `stop..tail`.
struct Head {
    level: u8,
    body: usize,
    stop: usize,
    tail: usize,
}

impl Head {
    fn of(line: &str) -> Option<Head> {
        let bytes = line.as_bytes();
        let level = bytes.iter().take_while(|&&b| b == b'#').count();
        if level == 0 || level > 6 {
            return None;
        }
        let body = match bytes.get(level) {
            None => level,
            Some(b' ' | b'\t') => level + 1,
            Some(_) => return None,
        };
        let end = body + line.get(body..)?.trim_end().len();
        let hashes = bytes.get(body..end)?.iter().rev().take_while(|&&b| b == b'#').count();
        let run = end - hashes;
        // A closing run counts only when it stands apart from the title, so
        // that "# C#" keeps its sharp.
        let closed = hashes > 0 && (run == body || is_gap(bytes.get(run - 1)));
        let (stop, tail) = if closed { (run, end) } else { (line.len(), line.len()) };
        Some(Head { level: level as u8, body, stop, tail })
    }
}

fn is_gap(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b' ' | b'\t'))
}

/// Bytes of leading spaces and tabs.
fn indent_len(line: &str) -> usize {
    line.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

fn is_fence(line: &str) -> bool {
    let bytes = line.as_bytes();
    let pad = bytes.iter().take_while(|&&b| b == b' ').count();
    let rest = bytes.get(pad..).unwrap_or(&[]);
    pad <= 3 && (rest.starts_with(b"```") || rest.starts_with(b"~~~"))
}

fn is_rule(line: &str) -> bool {
    let mut mark = 0;
    let mut count = 0;
    for byte in line.bytes() {
        match byte {
            b' ' | b'\t' => {}
            b'-' | b'*' | b'_' if mark == 0 || mark == byte => {
                mark = byte;
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// The byte just past the `>` of a quote line.
fn quote_end(line: &str) -> Option<usize> {
    let at = indent_len(line);
    (line.as_bytes().get(at) == Some(&b'>')).then_some(at + 1)
}

/// The byte where the text of a list item starts: past the indentation, the
/// marker, its space, and a task box if one follows at once.
fn item_end(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut at = indent_len(line);
    match bytes.get(at)? {
        b'-' | b'*' | b'+' => at += 1,
        b'0'..=b'9' => {
            at += bytes.get(at..)?.iter().take_while(|b| b.is_ascii_digit()).count();
            if !matches!(bytes.get(at), Some(b'.' | b')')) {
                return None;
            }
            at += 1;
        }
        _ => return None,
    }
    if !is_gap(bytes.get(at)) {
        return None;
    }
    at += 1;
    if let Some([b'[', b' ' | b'x' | b'X', b']', b' ' | b'\t']) = bytes.get(at..at + 4) {
        at += 4;
    }
    Some(at)
}

/// Something the inline scan found. Whatever lies between two of these is
/// ordinary text.
#[derive(Clone, Copy)]
enum Tok {
    /// A stretch whose style is settled.
    Fixed { start: usize, end: usize, style: Style },
    /// A run of `*` or `_` that may open or close emphasis.
    Run { start: usize, end: usize, mark: u8, opens: bool, closes: bool },
}

/// The result of scanning the inline text of one line.
struct Inline {
    toks: Vec<Tok>,
    wikis: Vec<Wiki>,
}

impl Inline {
    fn fixed(&mut self, start: usize, end: usize, style: Style) {
        if start < end {
            self.toks.push(Tok::Fixed { start, end, style });
        }
    }

    /// Pairs up the emphasis runs and writes everything found, and the text
    /// between, to `out`.
    ///
    /// A run that can close closes the nearest run before it of the same
    /// character and length that is still open, and with it shuts off
    /// whatever opened in between. So of two openers and one closer it is
    /// the later opener that is closed, and the earlier stays text: an
    /// asterisk typed ahead of `*emphasis*` does not take it over. Where one
    /// pair lies inside another only the outer one is drawn, which is what
    /// keeps emphasis from nesting.
    fn emit(&self, out: &mut Out) {
        // For each run that opens a pair, the run that closes it. Every
        // opener is put on its list once and taken off once, which keeps a
        // line full of unmatched openers linear.
        let mut closer_of: Vec<Option<usize>> = vec![None; self.toks.len()];
        let mut open: [Vec<usize>; 4] = Default::default();
        for (index, tok) in self.toks.iter().enumerate() {
            let Tok::Run { start, end, mark, opens, closes } = *tok else { continue };
            let class = run_class(mark, end - start);
            let opener = if closes { open[class].pop() } else { None };
            if let Some(opener) = opener {
                closer_of[opener] = Some(index);
                for inner in &mut open {
                    while inner.last().is_some_and(|&at| at > opener) {
                        inner.pop();
                    }
                }
            } else if opens {
                open[class].push(index);
            }
        }
        let mut text = Style::Plain;
        let mut closer = None;
        for (index, tok) in self.toks.iter().enumerate() {
            match *tok {
                Tok::Fixed { start, end, style } => {
                    out.to(start, text);
                    out.to(end, style);
                }
                Tok::Run { start, end, .. } if closer == Some(index) => {
                    out.to(start, text);
                    out.to(end, Style::Syntax);
                    text = Style::Plain;
                    closer = None;
                }
                Tok::Run { start, end, .. } if closer.is_none() && closer_of[index].is_some() => {
                    out.to(start, text);
                    out.to(end, Style::Syntax);
                    text = match end - start {
                        1 => Style::Emph,
                        2 => Style::Strong,
                        _ => Style::StrongEmph,
                    };
                    closer = closer_of[index];
                }
                Tok::Run { .. } => {}
            }
        }
    }
}

/// Which of the four kinds of emphasis run this is: one, two or three
/// asterisks, or an underscore.
fn run_class(mark: u8, len: usize) -> usize {
    if mark == b'_' {
        3
    } else {
        len.clamp(1, 3) - 1
    }
}

/// Scans `text` from byte `from` for inline markup. `text` is a line cut off
/// where its inline text stops, so every offset is an offset into the line.
///
/// Whatever starts first wins, and nothing is looked for inside code, a link
/// or an address. The work is linear in the line: every search for a closer
/// either succeeds and is stepped over, or gives up at the next place a
/// search could start.
///
/// That last part leans on escapes being read the same way everywhere.
/// Nothing the scan steps over ends in a backslash that could still escape
/// something (an address gives a trailing one back), so the scan arrives at
/// a markup character only when the backslashes before it pair off among
/// themselves. The search inside a `[text](url)` pairs them the same way,
/// which is why it always stops at the next bracket the scan itself would
/// stop at.
fn scan(text: &str, from: usize) -> Inline {
    let bytes = text.as_bytes();
    let mut found = Inline { toks: Vec::new(), wikis: Vec::new() };
    let mut ticks = None;
    let mut at = from;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\\' if escapes(bytes, at) => {
                found.fixed(at, at + 1, Style::Syntax);
                at += 2;
            }
            b'`' => {
                let len = run_len(bytes, at);
                let ticks = ticks.get_or_insert_with(|| Ticks::new(bytes));
                match ticks.closer(at + len, len) {
                    Some(close) => {
                        found.fixed(at, at + len, Style::Syntax);
                        found.fixed(at + len, close, Style::Code);
                        found.fixed(close, close + len, Style::Syntax);
                        at = close + len;
                    }
                    None => at += len,
                }
            }
            b'[' => {
                if let Some(wiki) = Wiki::at(text, at) {
                    let (start, end) = wiki.visible();
                    found.fixed(at, start, Style::Syntax);
                    found.fixed(start, end, Style::Link);
                    found.fixed(end, wiki.end, Style::Syntax);
                    at = wiki.end;
                    found.wikis.push(wiki);
                } else if let Some((label_end, close)) = markdown_link(bytes, at) {
                    found.fixed(at, at + 1, Style::Syntax);
                    found.fixed(at + 1, label_end, Style::Link);
                    found.fixed(label_end, label_end + 2, Style::Syntax);
                    found.fixed(label_end + 2, close, Style::Url);
                    found.fixed(close, close + 1, Style::Syntax);
                    at = close + 1;
                } else {
                    at += 1;
                }
            }
            b'*' | b'_' => {
                let end = at + run_len(bytes, at);
                found.toks.push(emphasis_run(text, at, end, byte));
                at = end;
            }
            b'h' | b'H' => match url_end(text, at) {
                Some(end) => {
                    found.fixed(at, end, Style::Url);
                    at = end;
                }
                None => at += 1,
            },
            _ => at += 1,
        }
    }
    found
}

/// The characters a backslash can make plain: the ones that mean something
/// in this markdown.
fn is_markup(byte: u8) -> bool {
    matches!(byte, b'\\' | b'`' | b'*' | b'_' | b'[' | b']' | b'(' | b')' | b'#' | b'>' | b'-' | b'+' | b'.' | b'~')
}

/// Whether the backslash at `at` makes the byte after it plain.
fn escapes(bytes: &[u8], at: usize) -> bool {
    bytes.get(at + 1).is_some_and(|&next| is_markup(next))
}

/// How many times the byte at `at` repeats from there on.
fn run_len(bytes: &[u8], at: usize) -> usize {
    match bytes.get(at) {
        Some(&byte) => bytes.get(at..).unwrap_or(&[]).iter().take_while(|&&b| b == byte).count(),
        None => 0,
    }
}

/// Decides whether a run of `*` or `_` may open or close emphasis. It must
/// hug its text: an opener has text right after it and a closer right before
/// it. An underscore also needs a word boundary on its outer side, which is
/// what leaves `snake_case` alone. Asterisks come in ones, twos and threes
/// and an underscore only singly; longer runs are just text.
fn emphasis_run(text: &str, start: usize, end: usize, mark: u8) -> Tok {
    let before = text.get(..start).unwrap_or("");
    let after = text.get(end..).unwrap_or("");
    let mut opens = after.chars().next().is_some_and(|c| !c.is_whitespace());
    let mut closes = before.chars().next_back().is_some_and(|c| !c.is_whitespace());
    if mark == b'_' {
        // A word does not end at the marks that sit on its last letter, nor
        // start at ones that come before its first, so the boundary is
        // judged by the nearest character with a cell of its own.
        let in_word = |nearest: Option<char>| nearest.is_some_and(char::is_alphanumeric);
        opens &= !in_word(before.chars().rev().find(|&c| !is_combining(c)));
        closes &= !in_word(after.chars().find(|&c| !is_combining(c)));
    }
    let longest = if mark == b'_' { 1 } else { 3 };
    if end - start > longest {
        opens = false;
        closes = false;
    }
    Tok::Run { start, end, mark, opens, closes }
}

/// Every run of backticks in a line. A code span opened by a run closes at
/// the next run of exactly the same length, and looking that up here spares
/// rescanning the line for each opener.
struct Ticks {
    /// Start and length of each run, in order.
    runs: Vec<(usize, usize)>,
    /// How many runs the scan has already moved past.
    passed: usize,
    /// For each length, where the last run of that length starts.
    last: HashMap<usize, usize>,
}

impl Ticks {
    fn new(bytes: &[u8]) -> Ticks {
        let mut ticks = Ticks { runs: Vec::new(), passed: 0, last: HashMap::new() };
        let mut at = 0;
        while let Some(&byte) = bytes.get(at) {
            if byte == b'`' {
                let len = run_len(bytes, at);
                ticks.runs.push((at, len));
                ticks.last.insert(len, at);
                at += len;
            } else {
                at += 1;
            }
        }
        ticks
    }

    /// Where the first run of exactly `len` backticks at or after `from`
    /// starts. `from` must not go backwards between calls.
    fn closer(&mut self, from: usize, len: usize) -> Option<usize> {
        if *self.last.get(&len)? < from {
            return None;
        }
        while self.runs.get(self.passed).is_some_and(|run| run.0 < from) {
            self.passed += 1;
        }
        self.runs.get(self.passed..)?.iter().find(|run| run.1 == len).map(|run| run.0)
    }
}

/// Reads a `[text](address)` whose `[` is at `at`, giving the positions of
/// its `]` and of its closing `)`.
///
/// The text holds no brackets and the address none either, though it may
/// hold balanced parentheses; a bracket or parenthesis with a backslash
/// before it is neither, only a character of the part it is in. Giving up
/// at the first real bracket is what lets `[[wiki links]]` always win, and
/// what keeps a line of unclosed links from being rescanned to its end for
/// each one.
fn markdown_link(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut label_end = at + 1;
    loop {
        match bytes.get(label_end)? {
            b'\\' if escapes(bytes, label_end) => label_end += 2,
            b']' => break,
            b'[' => return None,
            _ => label_end += 1,
        }
    }
    if label_end == at + 1 || bytes.get(label_end + 1) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut close = label_end + 2;
    loop {
        match bytes.get(close)? {
            b'\\' if escapes(bytes, close) => close += 1,
            b'(' => depth += 1,
            b')' if depth == 0 => break,
            b')' => depth -= 1,
            b'[' | b']' => return None,
            _ => {}
        }
        close += 1;
    }
    (close > label_end + 2).then_some((label_end, close))
}

/// Where the bare address starting at `at` ends, if one starts there.
///
/// It may follow anything, so that it is found in writing that puts no
/// spaces between words. It runs to the first character no address holds
/// (see [`ends_address`]), and then gives back what belongs to the sentence
/// around it and not to the address: `. , ; : ! ?`, an apostrophe, emphasis
/// markers, a backslash, which is left to escape whatever comes next, and a
/// `)` that has no `(` inside the address.
fn url_end(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let starts = |scheme: &[u8]| bytes.get(at..at + scheme.len()).is_some_and(|s| s.eq_ignore_ascii_case(scheme));
    let scheme = if starts(b"https://") {
        8
    } else if starts(b"http://") {
        7
    } else {
        return None;
    };
    let rest = text.get(at + scheme..)?;
    let mut len = 0;
    let mut open = 0usize;
    let mut shut = 0usize;
    for (i, c) in rest.char_indices() {
        if ends_address(c) {
            break;
        }
        match c {
            '(' => open += 1,
            ')' => shut += 1,
            _ => {}
        }
        len = i + c.len_utf8();
    }
    let mut address = rest.get(..len)?;
    while let Some(last) = address.chars().next_back() {
        let stray = match last {
            ')' => shut > open,
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_' | '~' | '\\' => true,
            _ => false,
        };
        if !stray {
            break;
        }
        if last == ')' {
            shut -= 1;
        }
        address = address.get(..address.len() - last.len_utf8())?;
    }
    (!address.is_empty()).then_some(at + scheme + address.len())
}

/// True for a character that cannot be part of a bare address: whitespace,
/// controls, the brackets and quotes that are put round one, and the
/// punctuation beyond ASCII that prose is set with: guillemets, the dashes,
/// curly quotes and ellipsis of general punctuation, and the CJK and
/// full-width marks. Letters and digits that share those blocks, and
/// everything else beyond ASCII, may be in an address.
fn ends_address(c: char) -> bool {
    if c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '[' | ']' | '`' | '"') {
        return true;
    }
    let prose = matches!(c, '\u{00AB}' | '\u{00BB}' | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205E}' | '\u{3001}'..='\u{303F}' | '\u{FF01}'..='\u{FF65}');
    prose && !c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::Style::*;
    use super::*;
    use crate::links::links_in;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    /// The pieces of a line as (text, style) pairs, judged as the kind the
    /// line really is.
    fn show(line: &str) -> Vec<(&str, Style)> {
        show_as(line, classify_line(line, false))
    }

    fn show_as(line: &str, kind: LineKind) -> Vec<(&str, Style)> {
        pieces(line, kind).iter().map(|p| (&line[p.start..p.end], p.style)).collect()
    }

    struct Rng(u32);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as usize % n.max(1)
        }

        fn line(&mut self, parts: &[&str], most: usize) -> String {
            (0..self.below(most + 1)).map(|_| parts[self.below(parts.len())]).collect()
        }
    }

    const PARTS: &[&str] = &[
        "*", "**", "***", "****", "_", "__", "`", "``", "[[", "]]", "[", "]", "(", ")", "|", "#", "\\", " ", "  ", "\t",
        "word", "snake_case", "\u{00E9}", "\u{4E00}", "n\u{0313}", "https://a.b/c_d", "http://", "HTTPS://X.Y).", "- ",
        "> ", "# ", "## ", "1. ", "12) ", "[ ] ", "[x] ", "```", "~~~", "---", ".", ",", "!", "\u{201D}", "a", "B", "+ ",
        "[t](u)", "[[n|s]]", "[[n#h]]", "*x", "x*", "**x", "x**", "***x", "x***", "_x", "x_", "__x", "x__", "`c`",
        "*x", "x*", "**x", "x**", "_x", "x_",
    ];

    const KINDS: &[LineKind] = &[
        LineKind::Blank,
        LineKind::Heading(1),
        LineKind::Heading(6),
        LineKind::Quote,
        LineKind::Item,
        LineKind::Fence,
        LineKind::Code,
        LineKind::Rule,
        LineKind::Text,
    ];

    #[test]
    fn classifies_each_kind_of_line() {
        let cases: &[(&str, LineKind)] = &[
            ("", LineKind::Blank),
            (" \t ", LineKind::Blank),
            ("\u{00A0}", LineKind::Blank),
            ("# One", LineKind::Heading(1)),
            ("###### Six", LineKind::Heading(6)),
            ("##", LineKind::Heading(2)),
            ("####### Seven", LineKind::Text),
            ("#hashtag", LineKind::Text),
            (" # indented", LineKind::Text),
            ("> quoted", LineKind::Quote),
            ("   >quoted", LineKind::Quote),
            (">", LineKind::Quote),
            ("- item", LineKind::Item),
            ("* item", LineKind::Item),
            ("  + item", LineKind::Item),
            ("1. item", LineKind::Item),
            ("42) item", LineKind::Item),
            ("- [ ] task", LineKind::Item),
            ("- ", LineKind::Item),
            ("-", LineKind::Text),
            ("-item", LineKind::Text),
            ("1.5 is a number", LineKind::Text),
            ("1234567890. item", LineKind::Item),
            ("1984 was a year", LineKind::Text),
            ("```", LineKind::Fence),
            ("   ~~~text", LineKind::Fence),
            ("    ```", LineKind::Text),
            ("``", LineKind::Text),
            ("---", LineKind::Rule),
            ("***", LineKind::Rule),
            ("- - -", LineKind::Rule),
            (" _ _ _ _ ", LineKind::Rule),
            ("--", LineKind::Text),
            ("-*-", LineKind::Text),
            ("--- x", LineKind::Text),
            ("***both***", LineKind::Text),
            ("Just a paragraph.", LineKind::Text),
            ("\\# not a heading", LineKind::Text),
        ];
        for &(line, kind) in cases {
            assert_eq!(classify_line(line, false), kind, "{line:?}");
        }
        assert_eq!(classify_line("# not a heading", true), LineKind::Code);
        assert_eq!(classify_line("", true), LineKind::Code);
        assert_eq!(classify_line("~~~", true), LineKind::Fence);
    }

    #[test]
    fn fences_hold_code_and_an_unclosed_one_runs_to_the_end() {
        use LineKind::*;
        let text = lines("para\n```rust\n# x\n\n- y\n```\n# Real\n~~~\n> z");
        assert_eq!(classify(&text), [Text, Fence, Code, Code, Code, Fence, Heading(1), Fence, Code]);
        assert_eq!(classify(&[]), []);
    }

    #[test]
    fn hanging_indent_covers_the_marker() {
        let cases: &[(&str, usize)] = &[
            ("- item", 2),
            ("  * item", 4),
            ("10. item", 4),
            ("3) item", 3),
            ("- [ ] task", 6),
            ("  - [X] task", 8),
            ("- [y] not a box", 2),
            ("-  [ ] not straight after", 2),
            ("\t- item", 6),
            ("> quote", 2),
            (">quote", 1),
            ("  > quote", 4),
            (">", 1),
            ("- - -", 0),
            ("# Heading", 0),
            ("plain", 0),
            ("", 0),
            ("```", 0),
        ];
        for &(line, hang) in cases {
            assert_eq!(hang_of(line), hang, "{line:?}");
        }
        assert_eq!(marker_len("\t- [x] done"), 7);
        assert_eq!(&"  > quote"[marker_len("  > quote")..], "quote");
    }

    #[test]
    fn heading_gives_level_and_title() {
        assert_eq!(heading("# Title"), Some((1, "Title")));
        assert_eq!(heading("###   Spaced out   "), Some((3, "Spaced out")));
        assert_eq!(heading("## Closed ##"), Some((2, "Closed")));
        assert_eq!(heading("## Closed ####   "), Some((2, "Closed")));
        assert_eq!(heading("# C#"), Some((1, "C#")));
        assert_eq!(heading("# Escaped \\#"), Some((1, "Escaped \\#")));
        assert_eq!(heading("#"), Some((1, "")));
        assert_eq!(heading("## "), Some((2, "")));
        assert_eq!(heading("### ###"), Some((3, "")));
        assert_eq!(heading("#\tTabbed"), Some((1, "Tabbed")));
        assert_eq!(heading("# \u{00C9}t\u{00E9} #"), Some((1, "\u{00C9}t\u{00E9}")));
        assert_eq!(heading("#no"), None);
        assert_eq!(heading("####### no"), None);
        assert_eq!(heading(" # no"), None);
        assert_eq!(heading(""), None);
    }

    #[test]
    fn outline_skips_fences() {
        let text = lines("# One\ntext\n```\n# not this\n```\n## Two ##\n~~~\n# nor this");
        assert_eq!(
            outline(&text),
            [
                Heading { line: 0, level: 1, title: "One".to_string() },
                Heading { line: 5, level: 2, title: "Two".to_string() },
            ]
        );
    }

    #[test]
    fn emphasis() {
        assert_eq!(
            show("**strong** and *emph*"),
            [("**", Syntax), ("strong", Strong), ("**", Syntax), (" and ", Plain), ("*", Syntax), ("emph", Emph), ("*", Syntax)]
        );
        assert_eq!(show("***both***"), [("***", Syntax), ("both", StrongEmph), ("***", Syntax)]);
        assert_eq!(show("an _emph_."), [("an ", Plain), ("_", Syntax), ("emph", Emph), ("_", Syntax), (".", Plain)]);
        assert_eq!(show("snake_case_name"), [("snake_case_name", Plain)]);
        assert_eq!(show("_one_two_"), [("_", Syntax), ("one_two", Emph), ("_", Syntax)]);
        assert_eq!(show("2 * 3 * 4"), [("2 * 3 * 4", Plain)]);
        assert_eq!(show("a * b* c"), [("a * b* c", Plain)]);
        assert_eq!(show("*open"), [("*open", Plain)]);
        assert_eq!(show("**open*"), [("**open*", Plain)]);
        assert_eq!(show("****four****"), [("****four****", Plain)]);
        assert_eq!(show("a*b*c"), [("a", Plain), ("*", Syntax), ("b", Emph), ("*", Syntax), ("c", Plain)]);
    }

    #[test]
    fn a_closer_closes_the_nearest_opener() {
        // An opener typed ahead of emphasis that is already there does not
        // take it over; it waits, as text, for a closer of its own.
        assert_eq!(show("*new and *old*"), [("*new and ", Plain), ("*", Syntax), ("old", Emph), ("*", Syntax)]);
        assert_eq!(
            show("*Note: the *real* cost"),
            [("*Note: the ", Plain), ("*", Syntax), ("real", Emph), ("*", Syntax), (" cost", Plain)]
        );
        assert_eq!(show("_a _b_"), [("_a ", Plain), ("_", Syntax), ("b", Emph), ("_", Syntax)]);
        assert_eq!(show("**a **b**"), [("**a ", Plain), ("**", Syntax), ("b", Strong), ("**", Syntax)]);
        // Once it has one, the pair inside it is text.
        assert_eq!(show("*new* and *old*").len(), 7);
        assert_eq!(show("*a *b* c*"), [("*", Syntax), ("a *b* c", Emph), ("*", Syntax)]);
        // A closer shuts off what opened inside its pair.
        assert_eq!(show("*a **b* c**"), [("*", Syntax), ("a **b", Emph), ("*", Syntax), (" c**", Plain)]);
        assert_eq!(show("**a *b** c*"), [("**", Syntax), ("a *b", Strong), ("**", Syntax), (" c*", Plain)]);
        assert_eq!(show("*a **b *c** d*"), [("*", Syntax), ("a **b *c** d", Emph), ("*", Syntax)]);
        // A closer with nothing to close is text, and may open instead.
        assert_eq!(show("a* b*c*"), [("a* b", Plain), ("*", Syntax), ("c", Emph), ("*", Syntax)]);
    }

    #[test]
    fn underscores_mark_emphasis_only_singly() {
        for line in ["__strong__", "the __init__ method", "___both___", "name: ____", "__a_ _b__", "_a__"] {
            assert_eq!(show(line), [(line, Plain)], "{line:?}");
        }
        // A pair of them is still text inside a single pair.
        assert_eq!(show("_a__b_"), [("_", Syntax), ("a__b", Emph), ("_", Syntax)]);
    }

    #[test]
    fn a_word_boundary_is_judged_through_combining_marks() {
        // The same name spelled with a precomposed letter and with a letter
        // and a combining accent: the underscore after it is inside a word.
        for line in ["r\u{00E9}sum\u{00E9}_draft_ two", "re\u{0301}sume\u{0301}_draft_ two", "mitsqanaqan\u{0313}_atlas_"] {
            assert_eq!(show(line), [(line, Plain)], "{line:?}");
        }
        // Marks stacked on the letter, and characters with no cell at all.
        for line in ["a\u{0300}\u{0301}\u{0302}_b_", "a\u{200D}_b_", "a\u{200E}_b_ c", "_a_\u{200D}b", "_a_\u{0301}b"] {
            assert_eq!(show(line), [(line, Plain)], "{line:?}");
        }
        // A mark does not make a boundary into a word either.
        assert_eq!(
            show("an _e\u{0301}_, so"),
            [("an ", Plain), ("_", Syntax), ("e\u{0301}", Emph), ("_", Syntax), (", so", Plain)]
        );
        assert_eq!(show("(\u{0301}_a_"), [("(\u{0301}", Plain), ("_", Syntax), ("a", Emph), ("_", Syntax)]);
        // Asterisks do not care where words are.
        assert_eq!(
            show("re\u{0301}sume\u{0301}*draft*"),
            [("re\u{0301}sume\u{0301}", Plain), ("*", Syntax), ("draft", Emph), ("*", Syntax)]
        );
    }

    #[test]
    fn emphasis_does_not_nest() {
        assert_eq!(show("**a *b* c**"), [("**", Syntax), ("a *b* c", Strong), ("**", Syntax)]);
        assert_eq!(show("*a **b** c*"), [("*", Syntax), ("a **b** c", Emph), ("*", Syntax)]);
        assert_eq!(show("*a _b_ c*"), [("*", Syntax), ("a _b_ c", Emph), ("*", Syntax)]);
        assert_eq!(show("[*t*](u)"), [("[", Syntax), ("*t*", Link), ("](", Syntax), ("u", Url), (")", Syntax)]);
        assert_eq!(show("[[a|*b*]]"), [("[[a|", Syntax), ("*b*", Link), ("]]", Syntax)]);
    }

    #[test]
    fn emphasis_runs_across_code_and_links_which_keep_their_style() {
        assert_eq!(
            show("*see [[note]] now*"),
            [("*", Syntax), ("see ", Emph), ("[[", Syntax), ("note", Link), ("]]", Syntax), (" now", Emph), ("*", Syntax)]
        );
        assert_eq!(show("**`x`**"), [("**`", Syntax), ("x", Code), ("`**", Syntax)]);
        assert_eq!(
            show("*at https://a.b/c_d*"),
            [("*", Syntax), ("at ", Emph), ("https://a.b/c_d", Url), ("*", Syntax)]
        );
    }

    #[test]
    fn code_spans() {
        assert_eq!(
            show("run `a *b* [[c]] \\` now"),
            [("run ", Plain), ("`", Syntax), ("a *b* [[c]] \\", Code), ("`", Syntax), (" now", Plain)]
        );
        assert_eq!(show("``a ` b``"), [("``", Syntax), ("a ` b", Code), ("``", Syntax)]);
        assert_eq!(show("an open ` tick"), [("an open ` tick", Plain)]);
        assert_eq!(show("`a`` b"), [("`a`` b", Plain)]);
        assert_eq!(show("\\`a` b`"), [("\\", Syntax), ("`a", Plain), ("`", Syntax), (" b", Code), ("`", Syntax)]);
    }

    #[test]
    fn wiki_links() {
        assert_eq!(show("[[target]]"), [("[[", Syntax), ("target", Link), ("]]", Syntax)]);
        assert_eq!(show("[[target|shown]]"), [("[[target|", Syntax), ("shown", Link), ("]]", Syntax)]);
        assert_eq!(show("[[a#b]]"), [("[[", Syntax), ("a#b", Link), ("]]", Syntax)]);
        assert_eq!(show("[[ a | b c ]]"), [("[[ a | ", Syntax), ("b c", Link), (" ]]", Syntax)]);
        assert_eq!(show("[[a|]]"), [("[[", Syntax), ("a", Link), ("|]]", Syntax)]);
        assert_eq!(show("[[]] [[ ]] [[open"), [("[[]] [[ ]] [[open", Plain)]);
        assert_eq!(show("[[[a]]]"), [("[", Plain), ("[[", Syntax), ("a", Link), ("]]", Syntax), ("]", Plain)]);
        assert_eq!(show("[[a [[b]]"), [("[[a ", Plain), ("[[", Syntax), ("b", Link), ("]]", Syntax)]);
        assert_eq!(show("\\[[a]]"), [("\\", Syntax), ("[[a]]", Plain)]);
    }

    #[test]
    fn markdown_links_and_addresses() {
        assert_eq!(
            show("[text](https://a.b/c)"),
            [("[", Syntax), ("text", Link), ("](", Syntax), ("https://a.b/c", Url), (")", Syntax)]
        );
        assert_eq!(show("[w](a_(b))"), [("[", Syntax), ("w", Link), ("](", Syntax), ("a_(b)", Url), (")", Syntax)]);
        assert_eq!(show("[text] (no)"), [("[text] (no)", Plain)]);
        assert_eq!(show("[text](open"), [("[text](open", Plain)]);
        assert_eq!(show("[](x) [y]()"), [("[](x) [y]()", Plain)]);
        assert_eq!(
            show("see https://example.com/a_b, ok."),
            [("see ", Plain), ("https://example.com/a_b", Url), (", ok.", Plain)]
        );
        assert_eq!(show("(http://example.com)"), [("(", Plain), ("http://example.com", Url), (")", Plain)]);
        assert_eq!(
            show("https://en.wikipedia.org/wiki/Rust_(language)."),
            [("https://en.wikipedia.org/wiki/Rust_(language)", Url), (".", Plain)]
        );
        assert_eq!(show("\u{201C}https://a.b?\u{201D}"), [("\u{201C}", Plain), ("https://a.b", Url), ("?\u{201D}", Plain)]);
        assert_eq!(show("\"https://a.b\""), [("\"", Plain), ("https://a.b", Url), ("\"", Plain)]);
        assert_eq!(show("http:// https://. ftp://a.b http:/a.b"), [("http:// https://. ftp://a.b http:/a.b", Plain)]);
        assert_eq!(show("HTTPS://A.B"), [("HTTPS://A.B", Url)]);
    }

    #[test]
    fn an_address_is_found_wherever_it_starts_and_ends_where_prose_resumes() {
        // No space is needed in front of one.
        assert_eq!(
            show("\u{8A73}\u{7D30}\u{306F}https://a.b/c\u{3002}"),
            [("\u{8A73}\u{7D30}\u{306F}", Plain), ("https://a.b/c", Url), ("\u{3002}", Plain)]
        );
        assert_eq!(show("xhttp://a.b"), [("x", Plain), ("http://a.b", Url)]);
        assert_eq!(show("e\u{0301}https://a.b"), [("e\u{0301}", Plain), ("https://a.b", Url)]);
        // Quotes of every kind, dashes, an ellipsis and CJK punctuation are
        // prose, whichever way a language turns its quotes.
        let enders = [
            "\u{2018}", "\u{2019}", "\u{201C}", "\u{201D}", "\u{201E}", "\u{00AB}", "\u{00BB}", "\u{2039}", "\u{203A}", "\u{2014}",
            "\u{2026}", "\u{3001}", "\u{3002}", "\u{300D}", "\u{300F}", "\u{FF09}", "\u{FF01}", "\u{FF1F}", "\u{FF0C}",
        ];
        for ender in enders {
            let line = format!("https://a.b/c{ender}d");
            assert_eq!(show(&line), [("https://a.b/c", Url), (&line[13..], Plain)], "{line:?}");
            let line = format!("https://a.b/c.{ender}");
            assert_eq!(show(&line), [("https://a.b/c", Url), (&line[13..], Plain)], "{line:?}");
        }
        // Letters and digits from beyond ASCII stay in, full-width ones too.
        for inside in ["\u{00E9}", "\u{65E5}\u{672C}", "\u{3005}", "\u{FF21}\u{FF11}", "\u{1F600}", "e\u{0301}"] {
            let line = format!("https://a.b/{inside}/c");
            assert_eq!(show(&line), [(line.as_str(), Url)], "{line:?}");
        }
        // A backslash at the end is given back, to escape what follows it.
        assert_eq!(show("https://a.b/c\\* d"), [("https://a.b/c", Url), ("\\", Syntax), ("* d", Plain)]);
        assert_eq!(show("https://a.b/c\\[d](e)"), [("https://a.b/c", Url), ("\\", Syntax), ("[d](e)", Plain)]);
        assert_eq!(show("https://a.b\\c\\ d"), [("https://a.b\\c", Url), ("\\ d", Plain)]);
    }

    #[test]
    fn a_backslash_makes_markup_plain() {
        assert_eq!(show("\\*not\\*"), [("\\", Syntax), ("*not", Plain), ("\\", Syntax), ("*", Plain)]);
        assert_eq!(show("a\\\\*b*"), [("a", Plain), ("\\", Syntax), ("\\", Plain), ("*", Syntax), ("b", Emph), ("*", Syntax)]);
        assert_eq!(show("C:\\Users\\me and a \\"), [("C:\\Users\\me and a \\", Plain)]);
        assert_eq!(show("1\\. not a list"), [("1", Plain), ("\\", Syntax), (". not a list", Plain)]);
    }

    #[test]
    fn a_backslash_keeps_a_bracket_from_ending_part_of_a_link() {
        // An escaped closer closes nothing, so these are not links at all.
        assert_eq!(show("[a\\](b)"), [("[a", Plain), ("\\", Syntax), ("](b)", Plain)]);
        assert_eq!(show("[a](b\\)"), [("[a](b", Plain), ("\\", Syntax), (")", Plain)]);
        // Inside a link that does close, escaped brackets are its text.
        assert_eq!(show("[a\\]b](c)"), [("[", Syntax), ("a\\]b", Link), ("](", Syntax), ("c", Url), (")", Syntax)]);
        assert_eq!(show("[a\\[b](c)"), [("[", Syntax), ("a\\[b", Link), ("](", Syntax), ("c", Url), (")", Syntax)]);
        assert_eq!(show("[a](b\\)c)"), [("[", Syntax), ("a", Link), ("](", Syntax), ("b\\)c", Url), (")", Syntax)]);
        assert_eq!(show("[a](b\\(c)"), [("[", Syntax), ("a", Link), ("](", Syntax), ("b\\(c", Url), (")", Syntax)]);
        assert_eq!(show("[a](\\[b\\])"), [("[", Syntax), ("a", Link), ("](", Syntax), ("\\[b\\]", Url), (")", Syntax)]);
        assert_eq!(show("[\\]](\\))"), [("[", Syntax), ("\\]", Link), ("](", Syntax), ("\\)", Url), (")", Syntax)]);
        // A backslash before a backslash is spent on it.
        assert_eq!(show("[a\\\\](b)"), [("[", Syntax), ("a\\\\", Link), ("](", Syntax), ("b", Url), (")", Syntax)]);
        assert_eq!(show("[a](b\\\\)"), [("[", Syntax), ("a", Link), ("](", Syntax), ("b\\\\", Url), (")", Syntax)]);
        // And one before an ordinary character is only a backslash.
        assert_eq!(show("[a\\b](c\\d)"), [("[", Syntax), ("a\\b", Link), ("](", Syntax), ("c\\d", Url), (")", Syntax)]);
        // Wiki links do not read escapes: a name may be a path.
        assert_eq!(show("[[a\\]]"), [("[[", Syntax), ("a\\", Link), ("]]", Syntax)]);
    }

    #[test]
    fn block_markers_are_syntax() {
        assert_eq!(show("# Title"), [("# ", Syntax), ("Title", Plain)]);
        assert_eq!(show("#"), [("#", Syntax)]);
        assert_eq!(
            show("## A *b* ##  "),
            [("## ", Syntax), ("A ", Plain), ("*", Syntax), ("b", Emph), ("*", Syntax), (" ", Plain), ("##", Syntax), ("  ", Plain)]
        );
        assert_eq!(show("### ###"), [("### ###", Syntax)]);
        assert_eq!(show("- item"), [("- ", Syntax), ("item", Plain)]);
        assert_eq!(show("  12. *x*"), [("  ", Plain), ("12. *", Syntax), ("x", Emph), ("*", Syntax)]);
        assert_eq!(show("- [x] done"), [("- [x] ", Syntax), ("done", Plain)]);
        assert_eq!(show("* a*"), [("* ", Syntax), ("a*", Plain)]);
        assert_eq!(show("  > quote"), [("  ", Plain), (">", Syntax), (" quote", Plain)]);
        assert_eq!(show("---"), [("---", Syntax)]);
        assert_eq!(show(" * * * "), [(" * * * ", Syntax)]);
        assert_eq!(show("   "), [("   ", Plain)]);
        assert_eq!(show(""), []);
    }

    #[test]
    fn code_and_fence_lines_are_one_plain_piece() {
        assert_eq!(show_as("# **x** [[y]]", LineKind::Code), [("# **x** [[y]]", Plain)]);
        assert_eq!(show("```rust"), [("```rust", Plain)]);
        assert_eq!(show_as("", LineKind::Code), []);
    }

    #[test]
    fn a_kind_the_line_does_not_fit_is_styled_as_a_paragraph() {
        assert_eq!(show_as("*x*", LineKind::Heading(2)), [("*", Syntax), ("x", Emph), ("*", Syntax)]);
        assert_eq!(show_as("plain", LineKind::Item), [("plain", Plain)]);
        assert_eq!(show_as("plain", LineKind::Quote), [("plain", Plain)]);
        assert_eq!(show_as("# T", LineKind::Text), [("# T", Plain)]);
        assert_eq!(show_as("words", LineKind::Rule), [("words", Syntax)]);
    }

    /// Checks everything `pieces` promises about its result, for one line
    /// taken as one kind.
    fn sound_pieces(line: &str, kind: LineKind) -> Vec<Piece> {
        let found = pieces(line, kind);
        if line.is_empty() {
            assert!(found.is_empty());
            return found;
        }
        assert_eq!(found[0].start, 0, "{line:?} {kind:?}");
        assert_eq!(found[found.len() - 1].end, line.len(), "{line:?} {kind:?}");
        for piece in &found {
            assert!(piece.start < piece.end, "{line:?} {kind:?}");
            assert!(line.is_char_boundary(piece.start) && line.is_char_boundary(piece.end), "{line:?} {kind:?}");
        }
        for pair in found.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "{line:?} {kind:?}");
            assert_ne!(pair[0].style, pair[1].style, "{line:?} {kind:?}");
        }
        match kind {
            LineKind::Code | LineKind::Fence => assert_eq!(found, [Piece { start: 0, end: line.len(), style: Plain }]),
            LineKind::Rule => assert_eq!(found, [Piece { start: 0, end: line.len(), style: Syntax }]),
            _ => {}
        }
        found
    }

    #[test]
    fn random_lines_always_split_into_sound_pieces() {
        let mut rng = Rng(0x9E37_79B9);
        let mut styles = [0; 8];
        let mut kinds = [0; 8];
        for _ in 0..6000 {
            let line = rng.line(PARTS, 12);
            for &kind in KINDS {
                sound_pieces(&line, kind);
            }
            let real = classify_line(&line, false);
            for piece in sound_pieces(&line, real) {
                styles[piece.style as usize] += 1;
            }
            let slot = match real {
                LineKind::Blank => 0,
                LineKind::Heading(_) => 1,
                LineKind::Quote => 2,
                LineKind::Item => 3,
                LineKind::Fence => 4,
                LineKind::Rule => 5,
                LineKind::Text => 6,
                LineKind::Code => 7,
            };
            kinds[slot] += 1;
        }
        // The lines were varied enough to draw every style and to be every
        // kind a line can be on its own, which is all of them but Code.
        assert!(styles.iter().all(|&n| n > 100), "{styles:?}");
        assert!(kinds[..7].iter().all(|&n| n > 20) && kinds[7] == 0, "{kinds:?}");
    }

    /// The pairing `Inline::emit` does, found the slow and obvious way: each
    /// closer reads back for the nearest opener of its kind that nothing
    /// has closed or shut off, and then every pair lying inside another is
    /// dropped.
    fn emit_slowly(found: &Inline, out: &mut Out) {
        let mut live = vec![false; found.toks.len()];
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (index, tok) in found.toks.iter().enumerate() {
            let Tok::Run { start, end, mark, opens, closes } = *tok else { continue };
            let same = |at: usize| matches!(found.toks[at], Tok::Run { start: s, end: e, mark: m, .. } if m == mark && e - s == end - start);
            match (0..index).rev().find(|&at| live[at] && same(at)).filter(|_| closes) {
                Some(opener) => {
                    pairs.push((opener, index));
                    live[opener..index].fill(false);
                }
                None => live[index] = opens,
            }
        }
        let outer = |&(open, close): &(usize, usize)| !pairs.iter().any(|&(o, c)| o < open && close < c);
        let drawn: Vec<(usize, usize)> = pairs.iter().copied().filter(outer).collect();
        let mut text = Plain;
        for (index, tok) in found.toks.iter().enumerate() {
            match *tok {
                Tok::Fixed { start, end, style } => {
                    out.to(start, text);
                    out.to(end, style);
                }
                Tok::Run { start, end, .. } => {
                    let after = if drawn.iter().any(|&(open, _)| open == index) {
                        [Emph, Strong, StrongEmph][end - start - 1]
                    } else if drawn.iter().any(|&(_, close)| close == index) {
                        Plain
                    } else {
                        continue;
                    };
                    out.to(start, text);
                    out.to(end, Syntax);
                    text = after;
                }
            }
        }
    }

    #[test]
    fn random_emphasis_pairs_up_as_the_slow_way_does() {
        let parts = ["*", "**", "***", "****", "_", "__", "___", " ", "a", "b ", " c", "\\", "`", "[[n]]", "\u{00E9}", "."];
        let mut rng = Rng(0x0DDB_A115);
        let mut styled = 0;
        for _ in 0..20_000 {
            let line = rng.line(&parts, 14);
            let mut out = Out { pieces: Vec::new(), at: 0 };
            emit_slowly(&scan(&line, 0), &mut out);
            out.to(line.len(), Plain);
            assert_eq!(pieces(&line, LineKind::Text), out.pieces, "{line:?}");
            styled += out.pieces.iter().filter(|p| matches!(p.style, Emph | Strong | StrongEmph)).count();
        }
        assert!(styled > 2000, "{styled}");
    }

    #[test]
    fn random_backtick_runs_close_at_the_next_run_of_their_length() {
        let mut rng = Rng(0x07EA_CAFE);
        let mut closed = 0;
        for _ in 0..5000 {
            let line = rng.line(&["`", "`", "``", "```", "x", "xy "], 24);
            let bytes = line.as_bytes();
            let mut ticks = Ticks::new(bytes);
            let mut at = 0;
            while at < bytes.len() {
                if bytes[at] != b'`' {
                    at += 1;
                    continue;
                }
                let len = run_len(bytes, at);
                // The slow way: walk every later run and take the first of
                // the same length.
                let mut slow = None;
                let mut ahead = at + len;
                while ahead < bytes.len() && slow.is_none() {
                    let run = if bytes[ahead] == b'`' { run_len(bytes, ahead) } else { 0 };
                    if run == len {
                        slow = Some(ahead);
                    }
                    ahead += run.max(1);
                }
                assert_eq!(ticks.closer(at + len, len), slow, "{line:?} at {at}");
                closed += usize::from(slow.is_some());
                at = slow.map_or(at + len, |close| close + len);
            }
        }
        assert!(closed > 2000, "{closed}");
    }

    /// Characters from every corner of Unicode, thick with markup, so that a
    /// slice at the wrong byte would be found out.
    fn soup(rng: &mut Rng, most: usize) -> String {
        const MARKS: &[char] = &['*', '_', '`', '[', ']', '(', ')', '#', '>', '-', '+', '\\', '|', ' ', '\t', '~', '.', '1', 'h', ':', '/'];
        (0..rng.below(most + 1))
            .map(|_| match rng.below(8) {
                0..=3 => MARKS[rng.below(MARKS.len())],
                4 => char::from_u32(rng.below(0x80) as u32).unwrap_or('a'),
                5 => char::from_u32(0x80 + rng.below(0x780) as u32).unwrap_or('\u{00E9}'),
                6 => char::from_u32(0x800 + rng.below(0xF800) as u32).unwrap_or('\u{4E00}'),
                _ => char::from_u32(0x1_0000 + rng.below(0x10_0000) as u32).unwrap_or('\u{1F600}'),
            })
            .filter(|&c| c != '\n' && c != '\r')
            .collect()
    }

    #[test]
    fn random_unicode_never_breaks_a_character() {
        let mut rng = Rng(0xFEED_F00D);
        for _ in 0..20_000 {
            let line = soup(&mut rng, 24);
            for &kind in KINDS {
                sound_pieces(&line, kind);
            }
            sound_pieces(&line, classify_line(&line, false));
            assert!(line.is_char_boundary(marker_len(&line)), "{line:?}");
            assert!(hang_of(&line) <= str_width(&line), "{line:?}");
            if let Some((_, title)) = heading(&line) {
                assert!(line.contains(title), "{line:?}");
            }
            for link in links_in(&line) {
                assert!(line.is_char_boundary(link.start) && line.is_char_boundary(link.end), "{line:?}");
                assert!(line[link.start..link.end].contains(link.target), "{line:?}");
            }
        }
    }

    #[test]
    fn random_lines_keep_the_line_functions_in_step() {
        let mut rng = Rng(0x1234_5678);
        for _ in 0..6000 {
            let line = rng.line(PARTS, 8);
            let kind = classify_line(&line, false);
            let marker = marker_len(&line);
            assert!(line.is_char_boundary(marker), "{line:?}");
            assert_eq!(hang_of(&line), str_width(&line[..marker]), "{line:?}");
            assert_eq!(marker > 0, matches!(kind, LineKind::Item | LineKind::Quote), "{line:?}");
            match heading(&line) {
                Some((level, title)) => {
                    assert_eq!(kind, LineKind::Heading(level), "{line:?}");
                    assert_eq!(title, title.trim());
                    assert!(line.contains(title));
                }
                None => assert!(!matches!(kind, LineKind::Heading(_)), "{line:?}"),
            }
            assert_eq!(classify_line(&line, true) == LineKind::Fence, kind == LineKind::Fence);
            if kind != LineKind::Fence {
                assert_eq!(classify_line(&line, true), LineKind::Code);
            }
        }
    }

    #[test]
    fn random_documents_classify_line_by_line() {
        let mut rng = Rng(0xDEAD_BEEF);
        for _ in 0..400 {
            let text: Vec<String> = (0..rng.below(30)).map(|_| rng.line(PARTS, 3)).collect();
            let kinds = classify(&text);
            assert_eq!(kinds.len(), text.len());
            let mut in_fence = false;
            for (line, &kind) in text.iter().zip(&kinds) {
                assert_eq!(kind, classify_line(line, in_fence));
                assert_eq!(kind == LineKind::Code, in_fence && kind != LineKind::Fence);
                in_fence ^= kind == LineKind::Fence;
            }
            let heads = outline(&text);
            let expected = kinds.iter().filter(|k| matches!(k, LineKind::Heading(_))).count();
            assert_eq!(heads.len(), expected);
            for head in &heads {
                assert_eq!(kinds[head.line], LineKind::Heading(head.level));
            }
        }
    }

    /// What is drawn as a wiki link and what `links_in` reports are the
    /// same thing.
    #[test]
    fn random_lines_draw_exactly_the_links_that_can_be_followed() {
        let mut rng = Rng(0x0BAD_CAFE);
        for _ in 0..6000 {
            let line = rng.line(PARTS, 12);
            let found = pieces(&line, classify_line(&line, false));
            let style_at = |at: usize| found.iter().find(|p| p.start <= at && at < p.end).map(|p| p.style);
            let links = links_in(&line);
            for link in &links {
                assert_eq!(&line[link.start..link.start + 2], "[[", "{line:?}");
                assert_eq!(&line[link.end - 2..link.end], "]]", "{line:?}");
                assert_eq!(style_at(link.start), Some(Syntax), "{line:?}");
                assert_eq!(style_at(link.end - 1), Some(Syntax), "{line:?}");
                let inside: Vec<Style> =
                    found.iter().filter(|p| p.start >= link.start && p.end <= link.end).map(|p| p.style).collect();
                assert!(inside.contains(&Link), "{line:?}");
                assert!(inside.iter().all(|s| matches!(s, Syntax | Link)), "{line:?}");
            }
            // Every other stretch drawn as a link is a markdown link, which
            // always ends in "](".
            for piece in found.iter().filter(|p| p.style == Link) {
                let wiki = links.iter().any(|l| l.start < piece.start && piece.end < l.end);
                assert!(wiki || line[piece.end..].starts_with("]("), "{line:?}");
            }
        }
    }

    /// Lines built to make a careless scanner go back over itself. Each is
    /// far too long to finish in reasonable time unless the work is linear.
    #[test]
    fn hostile_lines_are_still_linear() {
        let hostile = [
            "*a ".repeat(60_000),
            "a*".repeat(60_000),
            "**a* ".repeat(40_000),
            "_a ".repeat(60_000),
            "[a](b ".repeat(40_000),
            "[[a ".repeat(50_000),
            "[".repeat(150_000),
            format!("[a]({}", "(".repeat(150_000)),
            "`a ``b ".repeat(30_000),
            (1..500).map(|n| "`".repeat(n) + "x").collect::<String>(),
            "\\".repeat(150_000),
            "http://x) ".repeat(20_000),
            format!("http://{}", ")".repeat(150_000)),
            "h".repeat(150_000),
            "#".repeat(150_000),
            format!("# {}", "#".repeat(150_000)),
            "1".repeat(150_000),
            " ".repeat(150_000),
            // Escapes inside links, and brackets an address has just ended
            // at with a backslash before them.
            format!("[{}", "\\[".repeat(75_000)),
            format!("[a]({}", "\\)".repeat(75_000)),
            "[a\\](b ".repeat(25_000),
            "[\\] [\\[ ".repeat(20_000),
            format!("[{}", "http://a\\[".repeat(15_000)),
            format!("[{}](", "http://a\\[".repeat(15_000)),
            "http://a\\\\\\ ".repeat(15_000),
            // Underscores with marks stacked on either side of them.
            "_\u{0301}".repeat(60_000),
            "a\u{0301}\u{0301}\u{0301}_".repeat(20_000),
            format!("a{}_b_{}c", "\u{0301}".repeat(75_000), "\u{0301}".repeat(75_000)),
        ];
        for line in &hostile {
            for kind in [classify_line(line, false), LineKind::Text, LineKind::Heading(1), LineKind::Item] {
                let found = pieces(line, kind);
                assert_eq!(found.last().map(|p| p.end), Some(line.len()));
            }
            let _ = (hang_of(line), heading(line), links_in(line));
        }
    }
}
