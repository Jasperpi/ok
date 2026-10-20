//! The keys, as one table: which key with which of Ctrl, Alt and Shift
//! asks for what. This is DESIGN.md's "Keys" section and nothing else; what
//! each command does is in `app.rs` and `page.rs`.
//!
//! The commands that belong to the teams filling the seams are named for
//! where they go: [`Command::ToLine`], [`Command::Find`], [`Command::Step`]
//! and [`Command::Keys`] are handled in `line.rs`, [`Command::Keep`] and
//! [`Command::Follow`] in `session.rs`.

use ok_host::{Key, Mods};

use crate::page::{Unit, Way};

/// What a key asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Move the cursor by a size, selecting with Shift.
    Travel { unit: Unit, way: Way, select: bool },
    /// Page Up and Page Down: one screen less a row.
    Screen { way: Way, select: bool },
    /// Backspace and Delete at a size.
    Erase { unit: Unit, way: Way },
    /// Alt Up and Alt Down: carry the paragraph.
    Carry(Way),
    /// Enter, or with `soft` Shift Enter.
    Enter { soft: bool },
    /// Tab (`Way::On`) and Shift Tab in a list.
    Nest(Way),
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// Ctrl T: to the next TK.
    NextTk,
    /// Ctrl = and Ctrl -: one step of scale up or down.
    Scale(i32),
    /// F11: between borderless full screen and a window.
    Fullscreen,
    /// Ctrl Q: leave.
    Quit,
    /// Esc: to the line. Handled in `line.rs`.
    ToLine,
    /// Ctrl F: Esc and `/` in one. Handled in `line.rs`.
    Find,
    /// F3 and Shift F3: step through hits. Handled in `line.rs`.
    Step(Way),
    /// F1: the keys, on the page. Handled in `line.rs`.
    Keys,
    /// Ctrl S: keep a version. Handled in `session.rs`.
    Keep,
    /// Ctrl Enter: follow the link the cursor is in. Handled in
    /// `session.rs`.
    Follow,
}

/// The command a key asks for, or `None` for a key that is not bound.
/// Nothing is ever bound on Ctrl with Alt, which is AltGr on other
/// keyboards and belongs to typing.
pub fn command(key: Key, mods: Mods) -> Option<Command> {
    use Command::*;
    if mods.ctrl && mods.alt {
        return None;
    }
    let (plain, select) = (!mods.ctrl && !mods.alt, mods.shift);
    let sized = |letter: Unit, word: Unit, sentence: Unit| {
        if mods.ctrl {
            word
        } else if mods.alt {
            sentence
        } else {
            letter
        }
    };
    let way = match key {
        Key::Left | Key::Up | Key::Home | Key::PageUp | Key::Backspace => Way::Back,
        _ => Way::On,
    };
    Some(match key {
        Key::Left | Key::Right => Travel { unit: sized(Unit::Letter, Unit::Word, Unit::Sentence), way, select },
        Key::Up | Key::Down if mods.alt => Carry(way),
        Key::Up | Key::Down => Travel { unit: sized(Unit::Row, Unit::Paragraph, Unit::Row), way, select },
        Key::Backspace | Key::Delete => Erase { unit: sized(Unit::Letter, Unit::Word, Unit::Sentence), way },
        Key::Home | Key::End if !mods.alt => Travel { unit: if mods.ctrl { Unit::Text } else { Unit::RowEnd }, way, select },
        Key::PageUp | Key::PageDown if plain => Screen { way, select },
        Key::Enter if mods.ctrl && !mods.shift => Follow,
        Key::Enter if plain => Enter { soft: mods.shift },
        Key::Tab if plain => Nest(if mods.shift { Way::Back } else { Way::On }),
        Key::Escape if !mods.any() => ToLine,
        Key::F(1) if !mods.any() => Keys,
        Key::F(3) if plain => Step(if mods.shift { Way::Back } else { Way::On }),
        Key::F(4) if mods.alt && !mods.shift => Quit,
        Key::F(11) if !mods.any() => Fullscreen,
        Key::Char(ch) if mods.ctrl => match (ch.to_ascii_lowercase(), mods.shift) {
            ('z', false) => Undo,
            ('z', true) | ('y', false) => Redo,
            ('x', false) => Cut,
            ('c', false) => Copy,
            ('v', false) => Paste,
            ('a', false) => SelectAll,
            ('t', false) => NextTk,
            // The key that carries = also carries +, with or without Shift.
            ('=', _) | ('+', _) => Scale(1),
            ('-', false) => Scale(-1),
            ('q', false) => Quit,
            ('f', false) => Find,
            ('s', false) => Keep,
            _ => return None,
        },
        _ => return None,
    })
}

/// What the say row prints while Ctrl or Alt has been held for a moment
/// with nothing else pressed: that key's row of the table of sizes.
pub fn held(mods: Mods) -> Option<&'static str> {
    match (mods.ctrl, mods.alt, mods.shift) {
        (true, false, false) => Some("Ctrl moves by word and by paragraph, and deletes by word."),
        (false, true, false) => Some("Alt moves and deletes by sentence, and carries the paragraph."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ok_text::str_width;

    const SHIFT_CTRL: Mods = Mods { ctrl: true, alt: false, shift: true };
    const SHIFT_ALT: Mods = Mods { ctrl: false, alt: true, shift: true };
    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false };

    /// The table of sizes, cell by cell.
    #[test]
    fn the_sizes_are_letter_word_and_sentence_across_and_row_and_paragraph_down() {
        let travel = |unit, way, select| Some(Command::Travel { unit, way, select });
        assert_eq!(command(Key::Left, Mods::NONE), travel(Unit::Letter, Way::Back, false));
        assert_eq!(command(Key::Right, Mods::NONE), travel(Unit::Letter, Way::On, false));
        assert_eq!(command(Key::Left, Mods::CTRL), travel(Unit::Word, Way::Back, false));
        assert_eq!(command(Key::Right, Mods::CTRL), travel(Unit::Word, Way::On, false));
        assert_eq!(command(Key::Left, Mods::ALT), travel(Unit::Sentence, Way::Back, false));
        assert_eq!(command(Key::Right, Mods::ALT), travel(Unit::Sentence, Way::On, false));
        assert_eq!(command(Key::Up, Mods::NONE), travel(Unit::Row, Way::Back, false));
        assert_eq!(command(Key::Down, Mods::NONE), travel(Unit::Row, Way::On, false));
        assert_eq!(command(Key::Up, Mods::CTRL), travel(Unit::Paragraph, Way::Back, false));
        assert_eq!(command(Key::Down, Mods::CTRL), travel(Unit::Paragraph, Way::On, false));
        assert_eq!(command(Key::Up, Mods::ALT), Some(Command::Carry(Way::Back)));
        assert_eq!(command(Key::Down, Mods::ALT), Some(Command::Carry(Way::On)));
        let erase = |unit, way| Some(Command::Erase { unit, way });
        assert_eq!(command(Key::Backspace, Mods::NONE), erase(Unit::Letter, Way::Back));
        assert_eq!(command(Key::Delete, Mods::NONE), erase(Unit::Letter, Way::On));
        assert_eq!(command(Key::Backspace, Mods::CTRL), erase(Unit::Word, Way::Back));
        assert_eq!(command(Key::Delete, Mods::CTRL), erase(Unit::Word, Way::On));
        assert_eq!(command(Key::Backspace, Mods::ALT), erase(Unit::Sentence, Way::Back));
        assert_eq!(command(Key::Delete, Mods::ALT), erase(Unit::Sentence, Way::On));
        // Shift with any movement selects.
        assert_eq!(command(Key::Left, Mods::SHIFT), travel(Unit::Letter, Way::Back, true));
        assert_eq!(command(Key::Right, SHIFT_CTRL), travel(Unit::Word, Way::On, true));
        assert_eq!(command(Key::Left, SHIFT_ALT), travel(Unit::Sentence, Way::Back, true));
        assert_eq!(command(Key::Down, Mods::SHIFT), travel(Unit::Row, Way::On, true));
        assert_eq!(command(Key::Up, SHIFT_CTRL), travel(Unit::Paragraph, Way::Back, true));
        assert_eq!(command(Key::Home, Mods::SHIFT), travel(Unit::RowEnd, Way::Back, true));
        assert_eq!(command(Key::End, SHIFT_CTRL), travel(Unit::Text, Way::On, true));
        assert_eq!(command(Key::PageDown, Mods::SHIFT), Some(Command::Screen { way: Way::On, select: true }));
    }

    #[test]
    fn every_other_row_of_the_keys_table() {
        assert_eq!(command(Key::Enter, Mods::NONE), Some(Command::Enter { soft: false }));
        assert_eq!(command(Key::Enter, Mods::SHIFT), Some(Command::Enter { soft: true }));
        assert_eq!(command(Key::Tab, Mods::NONE), Some(Command::Nest(Way::On)));
        assert_eq!(command(Key::Tab, Mods::SHIFT), Some(Command::Nest(Way::Back)));
        assert_eq!(command(Key::Home, Mods::NONE), Some(Command::Travel { unit: Unit::RowEnd, way: Way::Back, select: false }));
        assert_eq!(command(Key::End, Mods::NONE), Some(Command::Travel { unit: Unit::RowEnd, way: Way::On, select: false }));
        assert_eq!(command(Key::Home, Mods::CTRL), Some(Command::Travel { unit: Unit::Text, way: Way::Back, select: false }));
        assert_eq!(command(Key::PageUp, Mods::NONE), Some(Command::Screen { way: Way::Back, select: false }));
        assert_eq!(command(Key::PageDown, Mods::NONE), Some(Command::Screen { way: Way::On, select: false }));
        let ctrl = |ch: char| command(Key::Char(ch), Mods::CTRL);
        assert_eq!(ctrl('z'), Some(Command::Undo));
        assert_eq!(ctrl('y'), Some(Command::Redo));
        assert_eq!(command(Key::Char('z'), SHIFT_CTRL), Some(Command::Redo));
        assert_eq!((ctrl('x'), ctrl('c'), ctrl('v'), ctrl('a')), (Some(Command::Cut), Some(Command::Copy), Some(Command::Paste), Some(Command::SelectAll)));
        assert_eq!(ctrl('s'), Some(Command::Keep));
        assert_eq!(command(Key::Enter, Mods::CTRL), Some(Command::Follow));
        assert_eq!(ctrl('t'), Some(Command::NextTk));
        assert_eq!(ctrl('f'), Some(Command::Find));
        assert_eq!(command(Key::F(3), Mods::NONE), Some(Command::Step(Way::On)));
        assert_eq!(command(Key::F(3), Mods::SHIFT), Some(Command::Step(Way::Back)));
        assert_eq!((ctrl('='), ctrl('-')), (Some(Command::Scale(1)), Some(Command::Scale(-1))));
        assert_eq!(command(Key::Char('='), SHIFT_CTRL), Some(Command::Scale(1)));
        assert_eq!(command(Key::F(11), Mods::NONE), Some(Command::Fullscreen));
        assert_eq!(command(Key::F(1), Mods::NONE), Some(Command::Keys));
        assert_eq!(command(Key::Escape, Mods::NONE), Some(Command::ToLine));
        assert_eq!(ctrl('q'), Some(Command::Quit));
        assert_eq!(command(Key::F(4), Mods::ALT), Some(Command::Quit));
    }

    #[test]
    fn nothing_is_bound_on_ctrl_with_alt_and_unlisted_chords_do_nothing() {
        let keys = [Key::Left, Key::Up, Key::Enter, Key::Backspace, Key::Home, Key::Tab, Key::Escape, Key::F(11), Key::Char('q'), Key::Char('s')];
        for key in keys {
            assert_eq!(command(key, CTRL_ALT), None, "{key:?}");
            assert_eq!(command(key, Mods { ctrl: true, alt: true, shift: true }), None, "{key:?}");
        }
        for (key, mods) in [(Key::Char('q'), Mods::ALT), (Key::Char('w'), Mods::CTRL), (Key::Char('s'), SHIFT_CTRL), (Key::Tab, Mods::CTRL), (Key::Escape, Mods::SHIFT), (Key::F(2), Mods::NONE), (Key::Char('a'), Mods::NONE), (Key::Enter, Mods::ALT), (Key::Home, Mods::ALT), (Key::PageUp, Mods::CTRL)] {
            assert_eq!(command(key, mods), None, "{key:?} {mods:?}");
        }
    }

    #[test]
    fn what_a_held_key_says_fits_the_say_row() {
        for mods in [Mods::CTRL, Mods::ALT] {
            let sentence = held(mods).unwrap();
            assert!(str_width(sentence) <= 66 && sentence.ends_with('.') && !sentence.contains('!'));
        }
        assert_eq!(held(Mods::NONE), None);
        assert_eq!(held(Mods::SHIFT), None);
        assert_eq!(held(SHIFT_CTRL), None);
    }
}
