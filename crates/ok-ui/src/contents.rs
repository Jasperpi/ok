//! The left side: the writing folder as a contents page, and the drawing of
//! any list of names and values with leader dots between them.
//!
//! A contents page never cuts a title in the middle of a word. A name too
//! long for its row gives up its leader dots first; if it is still too long
//! it turns over onto the next row, set in by two cells, and its value
//! stands on the last of its rows, so the values read straight down the
//! right edge. Only a single word wider than the pane is cut, at the edge.

use ok_text::str_width;

use crate::canvas::{fit_name, set_rows, Canvas, Ink};
use crate::flow::Part;
use crate::grid::Cols;
use crate::line::Pane;
use crate::look::{Strength, Surface};
use crate::machine::{self, thousands};
use crate::scene::Scene;

/// One row of a list: a folder or a text of the contents, an option and its
/// value, a version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// How far in the name is set: two cells for each level.
    pub depth: usize,
    pub name: String,
    /// What is set against the name at the right edge, already as it is to
    /// be read: a word count, an option's value. Empty for a folder, which
    /// then gets no leader dots either.
    pub value: String,
    /// Drawn in ink where the rest is at half strength: the open text.
    pub lit: bool,
}

/// Which row a list too long for its pane starts drawing from, so that the
/// row in `focus` is in sight, near the middle when it can be.
pub fn window(len: usize, visible: usize, focus: Option<usize>) -> usize {
    if len <= visible {
        return 0;
    }
    focus.unwrap_or(0).saturating_sub(visible / 2).min(len - visible)
}

/// [`window`] for a list whose entries are not all one row tall: `tall`
/// says how many rows each takes. The entry in `focus` is in sight, with
/// about half the pane above it when the list reaches that far, and the
/// pane is filled to its foot when the list is long enough to fill it.
fn window_of(len: usize, visible: usize, focus: Option<usize>, tall: &dyn Fn(usize) -> usize) -> usize {
    if len == 0 {
        return 0;
    }
    let focus = focus.unwrap_or(0).min(len - 1);
    let (mut from, mut above) = (focus, 0);
    while from > 0 && above + tall(from - 1) <= visible / 2 {
        above += tall(from - 1);
        from -= 1;
    }
    let mut below = 0;
    let mut at = focus;
    while at < len && below < visible {
        below += tall(at);
        at += 1;
    }
    while from > 0 && above + below + tall(from - 1) <= visible {
        above += tall(from - 1);
        from -= 1;
    }
    from
}

/// How one row of a list is struck.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Struck {
    /// The strength of the name and the value.
    pub words: Strength,
    /// The strength of the leader dots between them.
    pub dots: Strength,
    /// The row Enter would act on: the whole row in inverse video.
    pub bar: bool,
}

/// The most rows one name of the contents may take.
const TURNS: usize = 3;

/// How far a row that a name has turned over onto is set in.
const TURN_IN: usize = 2;

/// An entry's name as it is set across `cols`, on `most` rows at most: the
/// rows of the name, and whether leader dots follow the last of them.
fn name_rows(entry: &Entry, cols: Cols, most: usize) -> (Vec<&str>, bool) {
    let start = cols.first + (entry.depth.min(16) * 2) as i32;
    let room = (cols.last + 1 - start).max(0) as usize;
    let value = str_width(&entry.value);
    let name = entry.name.trim();
    if value == 0 {
        return (set_rows(name, room, most.max(1)), false);
    }
    // A value takes its own width and a space; leader dots take a space
    // and two dots more.
    let (bare, dotted) = (room.saturating_sub(value + 1), room.saturating_sub(value + 4));
    if str_width(name) <= dotted {
        return (vec![name], true);
    }
    if str_width(name) <= bare {
        return (vec![name], false);
    }
    if most <= 1 {
        let cut = fit_name(name, bare);
        return (vec![cut], str_width(cut) <= dotted);
    }
    let mut rows = set_rows(name, bare, 1);
    let rest = name.get(rows.first().map_or(0, |row| row.len())..).unwrap_or("").trim_start();
    let turned = bare.saturating_sub(TURN_IN);
    rows.extend(set_rows(rest, turned, most - 1));
    let dots = rows.len() > 1 && rows.last().is_some_and(|last| str_width(last) + 3 <= turned);
    (rows, dots)
}

/// Draws one entry of a list across `cols` from row `at` down, on `most`
/// rows at most, and hands back how many rows it took: the name, leader
/// dots, and the value right-aligned to the last column on the last row.
fn set(c: &mut Canvas, surface: Surface, cols: Cols, at: i32, entry: &Entry, struck: Struck, most: usize) -> i32 {
    let bar = struck.bar;
    let words = Ink::at(if bar { Strength::INK } else { struck.words }).inverse(bar);
    let (rows, dots) = name_rows(entry, cols, most);
    let tall = rows.len().max(1) as i32;
    if bar {
        for down in 0..tall {
            c.text(surface, cols.first, at + down, &" ".repeat(cols.width() as usize), words);
        }
    }
    let start = cols.first + (entry.depth.min(16) * 2) as i32;
    let mut end = start;
    for (down, row) in rows.iter().enumerate() {
        let from = start + if down > 0 { TURN_IN as i32 } else { 0 };
        end = from + c.text(surface, from, at + down as i32, row, words);
    }
    let value = str_width(&entry.value) as i32;
    if value == 0 || value > cols.width() {
        return tall;
    }
    let last = at + tall - 1;
    let (from, to) = (end + 1, cols.last - value - 1);
    if dots && to > from {
        let leaders = Ink::at(if bar { Strength::INK } else { struck.dots }).inverse(bar);
        c.text(surface, from, last, &".".repeat((to - from + 1) as usize), leaders);
    }
    c.text_ending(surface, cols.last, last, &entry.value, words);
    tall
}

/// Draws one entry of a list on one row across `cols`: the name, cut at a
/// whole word if it must be, leader dots, and the value right-aligned to
/// the last column.
pub fn row(c: &mut Canvas, surface: Surface, cols: Cols, at: i32, entry: &Entry, struck: Struck) {
    set(c, surface, cols, at, entry, struck, 1);
}

/// The contents pane: the folder, or the rows the line is answering with.
pub fn paint(c: &mut Canvas, s: &Scene) {
    let grid = c.grid();
    let Some(cols) = grid.contents else { return };
    let answer = s.line.answer().filter(|answer| answer.pane == Pane::Contents);
    let (rows, bar) = match answer {
        Some(answer) => (answer.rows, answer.bar),
        None => (s.session.contents(), None),
    };
    let (first, last) = grid.contents_rows();
    let visible = (last - first + 1).max(0) as usize;
    // The folder is set as a contents page: an empty row before each folder
    // name, so that its texts stand under it as a section. A list the line
    // is driving is a plain table.
    let gap = |at: usize| answer.is_none() && at > 0 && rows.get(at).is_some_and(|entry| entry.value.is_empty());
    let tall = |at: usize| rows.get(at).map_or(1, |entry| name_rows(entry, cols, TURNS).0.len().max(1)) + usize::from(gap(at));
    let from = window_of(rows.len(), visible, bar.or_else(|| rows.iter().position(|entry| entry.lit)), &tall);
    // A list the line is driving is wide awake whatever the flow says.
    let shade = |rest: Strength| if answer.is_some() { rest } else { s.flow.part(Part::Contents, rest, s.now_ms) };
    let mut at_row = first;
    for (at, entry) in rows.iter().enumerate().skip(from) {
        if at > from && gap(at) {
            at_row += 1;
        }
        let needs = name_rows(entry, cols, TURNS).0.len().max(1) as i32;
        if at_row + needs - 1 > last {
            break;
        }
        let struck = Struck { words: shade(if entry.lit { Strength::INK } else { Strength::HALF }), dots: shade(Strength::FAINT), bar: bar == Some(at) };
        at_row += if entry.lit && answer.is_none() && !entry.value.is_empty() {
            // The open text's count is the page's own running one, so it is
            // current at every keystroke without the session recounting.
            let live = Entry { value: thousands(s.page.words()), ..entry.clone() };
            set(c, Surface::Desk, cols, at_row, &live, struck, TURNS)
        } else {
            set(c, Surface::Desk, cols, at_row, entry, struck, TURNS)
        };
    }
}

/// The most rows a list may take when it rises over the foot of the sheet.
const RISE: usize = 8;

/// When the pane that should answer the line is not on screen, its rows
/// rise from the say row over the foot of the sheet, above whatever the
/// say row's sentence has turned over onto.
pub fn rise(c: &mut Canvas, s: &Scene) {
    let grid = c.grid();
    let Some(answer) = s.line.answer() else { return };
    let on_screen = match answer.pane {
        Pane::Contents => grid.contents.is_some(),
        Pane::Labels => grid.labels.is_some(),
        Pane::Page => true,
    };
    let Some(say) = grid.say else { return };
    if on_screen || answer.rows.is_empty() {
        return;
    }
    let foot = say + 1 - machine::said(grid, s).len().max(1) as i32;
    let visible = RISE.min(answer.rows.len()).min(foot.max(0) as usize);
    let from = window(answer.rows.len(), visible, answer.bar);
    let top = foot - visible as i32;
    c.erase(Surface::Sheet, grid.sheet.first, top, grid.sheet.width(), visible as i32);
    for (at, entry) in answer.rows.iter().enumerate().skip(from).take(visible) {
        let struck = Struck { words: if entry.lit { Strength::INK } else { Strength::HALF }, dots: Strength::FAINT, bar: answer.bar == Some(at) };
        row(c, Surface::Sheet, grid.text, top + (at - from) as i32, entry, struck);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(depth: usize, name: &str, value: &str) -> Entry {
        Entry { depth, name: name.to_string(), value: value.to_string(), lit: false }
    }

    #[test]
    fn a_long_list_is_windowed_round_its_focus() {
        assert_eq!(window(10, 44, Some(9)), 0);
        assert_eq!(window(100, 10, None), 0);
        assert_eq!(window(100, 10, Some(3)), 0);
        assert_eq!(window(100, 10, Some(50)), 45);
        assert_eq!(window(100, 10, Some(99)), 90);
        assert_eq!(window(100, 0, Some(99)), 99);
        assert_eq!(window(0, 0, None), 0);
    }

    #[test]
    fn a_list_of_uneven_rows_is_windowed_the_same_way() {
        let one = |_: usize| 1;
        for (len, visible, focus) in [(10, 44, Some(9)), (100, 10, None), (100, 10, Some(3)), (100, 10, Some(50)), (100, 10, Some(99)), (0, 0, None)] {
            assert_eq!(window_of(len, visible, focus, &one), window(len, visible, focus), "{len} {visible} {focus:?}");
        }
        // Every third entry takes two rows: fewer of them fit above.
        let uneven = |at: usize| if at % 3 == 0 { 2 } else { 1 };
        assert_eq!(window_of(100, 10, Some(50), &uneven), 46);
        assert_eq!(window_of(100, 10, Some(99), &uneven), 93);
        assert_eq!(window_of(5, 10, Some(4), &uneven), 0);
    }

    #[test]
    fn a_name_gives_up_its_dots_and_then_turns_over_but_is_never_cut_in_a_word() {
        let wide = Cols { first: 2, last: 36 };
        let narrow = Cols { first: 2, last: 22 };
        assert_eq!(name_rows(&entry(1, "Essay plan", "210"), wide, 3), (vec!["Essay plan"], true));
        assert_eq!(name_rows(&entry(1, "Essay plan", "210"), narrow, 3), (vec!["Essay plan"], true));
        // Nineteen cells: twelve letters keep their dots, fifteen do not.
        assert_eq!(name_rows(&entry(1, "Budget speech", "540"), narrow, 3), (vec!["Budget speech"], false));
        assert_eq!(name_rows(&entry(1, "Leadership essay", "348"), narrow, 3), (vec!["Leadership", "essay"], true));
        assert_eq!(name_rows(&entry(1, "Grandfather interview", "630"), narrow, 3), (vec!["Grandfather", "interview"], true));
        assert_eq!(name_rows(&entry(1, "Homecoming run of show", "760"), narrow, 3), (vec!["Homecoming run", "of show"], true));
        // One row only: cut at a whole word, the dots kept when they fit.
        assert_eq!(name_rows(&entry(1, "Homecoming run of show", "760"), narrow, 1), (vec!["Homecoming run"], false));
        assert_eq!(name_rows(&entry(1, "Leadership essay", "348"), narrow, 1), (vec!["Leadership"], true));
        // A folder has no value and no dots, and turns over like any name.
        assert_eq!(name_rows(&entry(0, "Student government", ""), narrow, 3), (vec!["Student government"], false));
        assert_eq!(name_rows(&entry(0, "A folder with a very long name indeed", ""), narrow, 3), (vec!["A folder with a very", "long name indeed"], false));
    }
}
