//! How many cells a character takes on the grid.

/// Inclusive ranges of characters that sit on top of the one before them:
/// combining marks, joiners, variation selectors and other zero-width things.
const ZERO: &[(u32, u32)] = &[
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

/// Inclusive ranges of characters that take two cells. A character's width
/// never depends on the face in use, or the cursor would drift away from the
/// text when the face changes. The symbol ranges from U+231A to U+2B55 are
/// the ones Unicode makes wide; Gallant stores exactly these double-width.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2630, 0x2637),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x268A, 0x268F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0x303E),
    (0x3041, 0x33FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xA000, 0xA4CF),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE30, 0xFE4F),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x1F300, 0x1F64F),
    (0x1F680, 0x1F6FF),
    (0x1F900, 0x1FAFF),
    (0x20000, 0x3FFFD),
];

fn in_ranges(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// True for a character drawn over the previous cell instead of in its own.
pub fn is_combining(ch: char) -> bool {
    let cp = ch as u32;
    cp >= 0x0300 && in_ranges(ZERO, cp)
}

/// Cells taken by `ch`: 0 for a combining mark, 2 for a wide character, 4 for
/// a tab (a fixed width, not a tab stop), otherwise 1.
pub fn cell_width(ch: char) -> usize {
    let cp = ch as u32;
    if cp < 0x0300 {
        return if ch == '\t' { 4 } else { 1 };
    }
    if in_ranges(ZERO, cp) {
        0
    } else if cp >= 0x1100 && in_ranges(WIDE, cp) {
        2
    } else {
        1
    }
}

/// Cells taken by a string.
pub fn str_width(s: &str) -> usize {
    s.chars().map(cell_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_disjoint() {
        for table in [ZERO, WIDE] {
            for pair in table.windows(2) {
                assert!(pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0, "{:X?}", pair);
            }
        }
    }

    #[test]
    fn widths() {
        assert_eq!(cell_width('a'), 1);
        assert_eq!(cell_width('\u{00F1}'), 1);
        assert_eq!(cell_width('\u{0313}'), 0);
        assert_eq!(cell_width('\u{200D}'), 0);
        assert_eq!(cell_width('\u{4E00}'), 2);
        assert_eq!(cell_width('\u{2614}'), 2);
        assert_eq!(cell_width('\u{2713}'), 1);
        assert_eq!(cell_width('\u{2014}'), 1);
        assert_eq!(cell_width('\t'), 4);
        assert!(is_combining('\u{0301}'));
        assert!(!is_combining('n'));
        // mitsqanaqan with a combining comma above the last n is 11 cells.
        assert_eq!(str_width("mitsqanaqan\u{0313}"), 11);
    }
}
