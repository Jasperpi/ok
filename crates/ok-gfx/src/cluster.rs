//! Splitting a string into the pieces that are drawn in one go.

/// Inclusive ranges of characters that take no cell of their own and ride
/// on the character before them: combining marks, joiners, variation
/// selectors. This is the same list as the zero-width table in the text
/// crate, copied because this crate may depend on nothing; the page lays
/// out by that table, and this copy only serves [`clusters`].
const ZERO_WIDTH: &[(u32, u32)] = &[
    (0x0300, 0x036F),
    (0x0483, 0x0489),
    (0x0591, 0x05BD),
    (0x05BF, 0x05BF),
    (0x05C1, 0x05C2),
    (0x05C4, 0x05C5),
    (0x05C7, 0x05C7),
    (0x0610, 0x061A),
    (0x064B, 0x065F),
    (0x0670, 0x0670),
    (0x06D6, 0x06DC),
    (0x06DF, 0x06E4),
    (0x06E7, 0x06E8),
    (0x06EA, 0x06ED),
    (0x0E31, 0x0E31),
    (0x0E34, 0x0E3A),
    (0x0E47, 0x0E4E),
    (0x1AB0, 0x1AFF),
    (0x1DC0, 0x1DFF),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x20D0, 0x20F0),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE2F),
    (0xFEFF, 0xFEFF),
    (0xE0100, 0xE01EF),
];

/// Inclusive ranges of characters that are instructions, not pictures:
/// zero-width spaces and joiners, direction marks, variation selectors, the
/// byte-order mark. After a letter they are passed over when it is drawn.
/// Some faces store a reveal picture in these slots (Gallant does for a
/// few), which must not be struck over the letter as if it were an accent.
const NO_PICTURE: &[(u32, u32)] = &[
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
    (0xE0100, 0xE01EF),
];

fn in_ranges(table: &[(u32, u32)], ch: char) -> bool {
    let cp = ch as u32;
    table.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp))
}

/// True for a character that takes no cell of its own and rides on the
/// character before it.
pub(crate) fn is_zero_width(ch: char) -> bool {
    ch as u32 >= 0x0300 && in_ranges(ZERO_WIDTH, ch)
}

/// True for a character that changes how its neighbours behave and has no
/// picture of its own.
pub(crate) fn has_no_picture(ch: char) -> bool {
    ch as u32 >= 0x0300 && in_ranges(NO_PICTURE, ch)
}

/// True for a character that is drawn on the letter before it when it
/// follows one in a cluster. A control code and an instruction with no
/// picture are passed over instead, and so is a soft hyphen, which marks a
/// place to break and is not an accent.
pub(crate) fn draws_on_letter(ch: char) -> bool {
    !ch.is_control() && !has_no_picture(ch) && ch != '\u{00AD}'
}

/// True for a character whose picture is rightly empty: a space of any
/// width, the Braille blank, a filler, or an instruction with no picture.
/// For any other character an empty picture means the face has nothing to
/// show for it, and it is treated as a character the face lacks, so that a
/// letter is never drawn as an empty cell.
pub(crate) fn is_blank(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{115F}' | '\u{1160}' | '\u{2800}' | '\u{3164}' | '\u{FFA0}') || has_no_picture(ch)
}

/// The pieces of `text` that each fill one place on the grid: a character
/// and the zero-width characters that follow it. A zero-width character
/// with nothing before it starts a piece of its own.
pub fn clusters(text: &str) -> Clusters<'_> {
    Clusters { rest: text }
}

/// The iterator [`clusters`] returns.
#[derive(Clone, Debug)]
pub struct Clusters<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Clusters<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let mut chars = self.rest.char_indices();
        chars.next()?;
        let end = chars.find(|&(_, ch)| !is_zero_width(ch)).map_or(self.rest.len(), |(at, _)| at);
        let (piece, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(piece)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_disjoint() {
        for table in [ZERO_WIDTH, NO_PICTURE] {
            for pair in table.windows(2) {
                assert!(pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0, "{:X?}", pair);
            }
        }
    }

    #[test]
    fn a_letter_takes_its_marks_with_it() {
        let got: Vec<&str> = clusters("mitsqanaqan\u{0313}.").collect();
        assert_eq!(got.len(), 12);
        assert_eq!(got[10], "n\u{0313}");
        assert_eq!(got[11], ".");
        let got: Vec<&str> = clusters("e\u{0302}\u{0301}a\u{200B}\u{FE0F}").collect();
        assert_eq!(got, vec!["e\u{0302}\u{0301}", "a\u{200B}\u{FE0F}"]);
    }

    #[test]
    fn a_mark_with_nothing_before_it_stands_alone() {
        let got: Vec<&str> = clusters("\u{0313}\u{0301}n").collect();
        assert_eq!(got, vec!["\u{0313}\u{0301}", "n"]);
        assert_eq!(clusters("").count(), 0);
        assert_eq!(clusters("\u{1F600}\u{2614}").count(), 2);
    }

    #[test]
    fn only_spaces_fillers_and_instructions_are_rightly_empty() {
        for ch in [' ', '\u{00A0}', '\u{2003}', '\u{202F}', '\u{3000}', '\u{2800}', '\u{200B}', '\u{FEFF}'] {
            assert!(is_blank(ch), "{:04X}", ch as u32);
        }
        for ch in ['A', '.', '\u{0301}', '\u{00AD}', '\u{2500}', '\u{FFFD}', '\u{1F600}'] {
            assert!(!is_blank(ch), "{:04X}", ch as u32);
        }
        assert!(is_zero_width('\u{0313}') && is_zero_width('\u{200D}') && !is_zero_width('n') && !is_zero_width('\u{00AD}'));
    }

    #[test]
    fn instructions_are_told_from_pictures() {
        for ch in ['\u{200B}', '\u{200D}', '\u{FE0F}', '\u{FEFF}', '\u{2060}', '\u{202E}'] {
            assert!(has_no_picture(ch), "{:04X}", ch as u32);
        }
        for ch in ['a', '\u{0313}', '\u{00AD}', '\u{00A0}', '\u{2028}', '\u{FFFD}'] {
            assert!(!has_no_picture(ch), "{:04X}", ch as u32);
        }
    }
}
