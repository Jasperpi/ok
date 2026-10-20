//! The two texts the program carries inside it, which the line shows on the
//! page for reading, and the plain copy of a text that `copy` puts on the
//! clipboard.

use ok_text::markup::{self, LineKind, Style};
use ok_text::Doc;

/// The keys, as a text the sheet sets like any other: its heading double
/// size, its tables kept to the 66 cells of the default measure.
pub const KEYS: &str = "\
# Keys

A letter is always a letter. The cursor is on the page or on the line, and Esc moves between them.

## Sizes

         Left, Right   Up, Down              Backspace, Delete
plain    letter        row                   letter
Ctrl     word          paragraph             word
Alt      sentence      carries the paragraph sentence

Shift with any movement selects. Hold Ctrl or Alt for a moment and the say row prints its row of this table.

## On the page

Enter                   Ends the paragraph and writes the blank
                        line between paragraphs. In a list, the
                        next item; on an empty item, ends it.
Shift Enter             A line break inside the paragraph.
Backspace               At the start of a paragraph, undoes what
                        Enter did.
Tab, Shift Tab          In a list, nests the item or brings it
                        back. After [[, takes the offered name.
Home, End               The ends of the row. With Ctrl, of the
                        text.
Page Up, Page Down      One screen less a row.
Ctrl Z                  Undo.
Ctrl Y, Ctrl Shift Z    Redo.
Ctrl X, Ctrl C, Ctrl V  The clipboard, as plain text.
Ctrl A                  Selects everything.
Ctrl S                  Keeps a version, and says how many bytes
                        are on disk.
Ctrl Enter              Follows the link the cursor is in.
Ctrl T                  To the next TK, wrapping.
Ctrl F                  Finds a phrase. F3 and Shift F3 step
                        through the hits.
Ctrl = and Ctrl -       Scale 1, 2, 3.
F11                     Between full screen and a window.
F1                      This page.
Esc                     To the line, and back.
Ctrl Q, Alt F4          Leaves. Nothing is asked because nothing
                        can be lost.

## On the line

nothing, then Enter     Goes back to where you last were.
a title                 Finds a text: words in any order. Up and
                        Down choose. No match starts a new text.
a # and letters         Goes to a heading.
/ and a phrase          Finds it in this text.
/old/new                Changes it, one at a time. Add /g for all.
rename and a title      Renames this text and mends its links.
set                     Every option, as a table.
earlier                 This text as it was.
discard                 Moves this text into .discarded.
copy                    The whole text to the clipboard with the
                        marks removed.
keys                    This page.
notices                 The licences of the faces and code.
quit                    Leaves.";

/// The licences of the faces and the code inside the .exe.
const NOTICES: &str = include_str!("../../../../assets/NOTICES.txt");

/// What goes above the notices on the page: a fence that is never closed,
/// which makes every line under it code, so that nothing in a licence is
/// read as a heading, a list or an emphasis; and a blank line to stand for
/// the head margin. The page is shown from that blank line down, so the
/// fence itself is never on the sheet.
const UNMARKED: &str = "```\n\n";

/// The first row of [`notices`] that may come onto the sheet: the blank
/// line under the fence.
pub const NOTICES_TOP: i64 = 1;

/// The notices as the page is to show them, and where each of their
/// sections begins: the number of its line in that text, and its title.
pub fn notices() -> (String, Vec<(usize, &'static str)>) {
    let above = UNMARKED.matches('\n').count();
    let lines: Vec<&str> = NOTICES.lines().collect();
    let blank = |at: usize| lines.get(at).is_none_or(|line| line.trim().is_empty());
    // A section's title stands alone under two blank lines.
    let sections = (2..lines.len()).filter(|&at| !blank(at) && blank(at - 1) && blank(at - 2) && blank(at + 1)).map(|at| (at + above, lines[at].trim())).collect();
    (format!("{UNMARKED}{NOTICES}"), sections)
}

/// One line with its marks left out: what the words say, and nothing of
/// how they were to be struck.
fn unmarked(line: &str, kind: LineKind) -> String {
    if matches!(kind, LineKind::Code) {
        return line.to_string();
    }
    let mut out = String::new();
    let mut after_link = false;
    for piece in markup::pieces(line, kind) {
        let text = line.get(piece.start..piece.end).unwrap_or("");
        match piece.style {
            Style::Syntax => after_link = text.ends_with("]("),
            // The address inside `[text](address)` is a mark too.
            Style::Url if after_link => after_link = false,
            _ => {
                out.push_str(text);
                after_link = false;
            }
        }
    }
    out.trim().to_string()
}

/// The whole text with the marks removed and each paragraph on one line,
/// lines joined by `\n`. Blank lines stay where the text has them, one at
/// a time; fences and rules, which are nothing but marks, go.
pub fn plain(doc: &Doc) -> String {
    let kinds = markup::classify(doc.lines());
    let mut out: Vec<String> = Vec::new();
    // The paragraph being joined, and the kind of line it is made of.
    let mut open: Option<(LineKind, String)> = None;
    for (line, kind) in doc.lines().iter().zip(kinds) {
        let words = unmarked(line, kind);
        let joins = matches!(kind, LineKind::Text | LineKind::Quote);
        match &mut open {
            Some((of, paragraph)) if joins && *of == kind => {
                if !words.is_empty() {
                    paragraph.push(' ');
                    paragraph.push_str(&words);
                }
                continue;
            }
            _ => out.extend(open.take().map(|(_, paragraph)| paragraph)),
        }
        match kind {
            LineKind::Blank => {
                if out.last().is_some_and(|last| !last.is_empty()) {
                    out.push(String::new());
                }
            }
            LineKind::Fence | LineKind::Rule => {}
            _ if joins => open = Some((kind, words)),
            _ => out.push(words),
        }
    }
    out.extend(open.map(|(_, paragraph)| paragraph));
    while out.last().is_some_and(|last| last.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ok_text::str_width;

    #[test]
    fn the_tables_of_the_keys_fit_the_default_measure() {
        for line in KEYS.lines().filter(|line| line.contains("  ")) {
            assert!(str_width(line) <= 66, "{} cells: {line}", str_width(line));
        }
        // Nothing in them is read as a list, a quote or a second heading
        // by accident.
        let lines: Vec<String> = KEYS.lines().map(str::to_string).collect();
        let kinds = markup::classify(&lines);
        let headings = kinds.iter().filter(|kind| matches!(kind, LineKind::Heading(_))).count();
        assert_eq!(headings, 4);
        assert!(kinds.iter().all(|kind| matches!(kind, LineKind::Heading(_) | LineKind::Text | LineKind::Blank)));
    }

    #[test]
    fn the_notices_are_whole_and_nothing_in_them_is_markup() {
        let (text, sections) = notices();
        assert!(text.starts_with("```\n\nok is a writing tool"));
        assert!(text.ends_with(NOTICES));
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let kinds = markup::classify(&lines);
        assert_eq!(kinds[0], LineKind::Fence);
        assert!(kinds[1..].iter().all(|kind| *kind == LineKind::Code), "a fence inside the notices would end the one above them");
        let titles: Vec<&str> = sections.iter().map(|(_, title)| *title).collect();
        assert_eq!(titles[..6], ["Gallant", "Gallant, from Jens Schweikhardt", "Gallant, from the FreeBSD Project", "Gallant, from the University of California", "Spleen", "Terminus"]);
        assert_eq!(titles.last().copied(), Some("GCC runtime library"));
        assert_eq!(titles.len(), 21);
        for (line, title) in &sections {
            assert_eq!(lines[*line], *title);
        }
    }

    #[test]
    fn a_plain_copy_has_no_marks_and_one_line_to_a_paragraph() {
        let text = "# The orchard\n\nThe *orchard* was **eleven** acres\nwhen he bought it, see [[Grandfather interview|the interview]].\n\n\n- [x] one `code`\n- two\n\n> a quote\n> goes on\n\n---\n\n```\nlet x = *1*;\n```\n[a link](https://example.com) and https://example.com/bare TK\n";
        let copy = plain(&Doc::from_text(text));
        let lines: Vec<&str> = copy.split('\n').collect();
        assert_eq!(
            lines,
            [
                "The orchard",
                "",
                "The orchard was eleven acres when he bought it, see the interview.",
                "",
                "one code",
                "two",
                "",
                "a quote goes on",
                "",
                "let x = *1*;",
                "a link and https://example.com/bare TK",
            ]
        );
        assert_eq!(plain(&Doc::from_text("")), "");
        assert_eq!(plain(&Doc::from_text("\n\n\n")), "");
    }
}
