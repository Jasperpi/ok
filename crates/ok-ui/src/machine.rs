//! The machine's two lines at the foot of the sheet: the say row, the one
//! place it speaks in sentences, and the machine line, with `ok`, whatever
//! is typed on the line, and the facts.

use ok_gfx::clusters;
use ok_text::str_width;

use crate::canvas::{fit, set_rows, Canvas, Ink};
use crate::flow::Part;
use crate::grid::Grid;
use crate::look::{Strength, Surface};
use crate::scene::Scene;

/// A count as it is read: `2,481`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A count of words as it is read: `1 word`, `1,204 words`.
pub fn words(n: usize) -> String {
    format!("{} {}", thousands(n), if n == 1 { "word" } else { "words" })
}

/// What separates one fact from the next.
const BETWEEN: &str = "    ";

/// The sentence on the say row as it is set. The machine never speaks in
/// half a word: a sentence that fits the measure is one row, and one that
/// does not turns over at the end of a sentence or between words, its first
/// part going up onto the row kept empty under the text and its last on
/// the say row. It never goes over the text; on a sheet so narrow that two
/// rows are not enough, what is left over is dropped at a whole word.
pub fn said<'a>(grid: &Grid, s: &'a Scene) -> Vec<&'a str> {
    let Some(say) = grid.say else { return Vec::new() };
    let room = (say + 1 - grid.text_rows).clamp(1, 2) as usize;
    match s.line.say().or(s.said) {
        Some(sentence) => set_rows(sentence, grid.measure.max(0) as usize, room),
        None => Vec::new(),
    }
}

/// The facts, most worth keeping last: the name of the text, the words in
/// it or in the selection, and the words typed today when there are any.
/// A text with a word limit reads `412 of 650 words`.
fn facts(s: &Scene) -> Vec<String> {
    let mut facts = Vec::new();
    if !s.session.name().is_empty() {
        facts.push(s.session.name().to_string());
    }
    facts.push(match (s.page.selected_words(), s.session.limit()) {
        (Some(selected), _) => format!("{} selected", words(selected)),
        (None, Some(limit)) => format!("{} of {}", thousands(s.page.words()), words(limit)),
        (None, None) => words(s.page.words()),
    });
    if s.session.words_today() > 0 {
        facts.push(format!("{} today", thousands(s.session.words_today())));
    }
    facts
}

/// The say row and the machine line.
pub fn paint(c: &mut Canvas, s: &Scene) {
    let grid = c.grid();
    let measure = grid.measure.max(0) as usize;
    if let Some(say) = grid.say {
        let rows = said(grid, s);
        let top = say + 1 - rows.len() as i32;
        for (down, row) in rows.iter().enumerate() {
            c.text(Surface::Sheet, grid.text.first, top + down as i32, row, Ink::HALF);
        }
    }
    let Some(row) = grid.machine else { return };

    // A failed write is the one alarming thing: the whole line goes inverse.
    let alarm = s.session.failing();
    if alarm {
        c.text(Surface::Sheet, grid.text.first, row, &" ".repeat(measure), Ink::INK.inverse(true));
    }
    if s.session.on_disk() {
        c.text(Surface::Sheet, grid.ok_col(), row, "ok", Ink::INK.inverse(alarm));
    }

    let mut taken = 2;
    if s.line.is_open() {
        let typed = s.line.typed();
        let room = (grid.text.last + 1 - grid.typing_col()).max(0) as usize;
        // An empty line may show what it understands, faint, in the place
        // where typing will go.
        let (shown, strength) = match s.line.hint().filter(|_| typed.is_empty()) {
            Some(hint) => (fit(hint, room), Strength::FAINT),
            None => (fit(typed, room), Strength::INK),
        };
        c.text(Surface::Sheet, grid.typing_col(), row, shown, Ink::at(strength).inverse(alarm));
        // The cursor sits on the character at its cell, or on bare paper
        // after the last one.
        let caret = s.line.caret().min(room.saturating_sub(1));
        let mut cell = 0;
        let under = clusters(shown).find(|piece| {
            let here = cell == caret;
            cell += str_width(piece);
            here
        });
        c.cursor(Surface::Sheet, grid.typing_col() + caret as i32, row, under.unwrap_or(""), 1, Ink::INK);
        taken = 3 + str_width(shown).max(caret + 1);
    }

    // The facts give way from the left when the line needs the room.
    let mut facts = facts(s);
    while !facts.is_empty() && taken + 2 + str_width(&facts.join(BETWEEN)) > measure {
        facts.remove(0);
    }
    if !facts.is_empty() {
        let strength = s.flow.part(Part::Facts, Strength::HALF, s.now_ms);
        c.text_ending(Surface::Sheet, grid.text.last, row, &facts.join(BETWEEN), Ink::at(strength).inverse(alarm));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_a_person_writes_them() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(412), "412");
        assert_eq!(thousands(1204), "1,204");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(thousands(usize::MAX), "18,446,744,073,709,551,615");
        assert_eq!(words(0), "0 words");
        assert_eq!(words(1), "1 word");
        assert_eq!(words(2481), "2,481 words");
    }
}
