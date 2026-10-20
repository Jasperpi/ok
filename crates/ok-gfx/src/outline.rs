//! An outline (TTF or OTF) face, as far as this crate can know one.
//!
//! Turning outlines into pixels is the system's job and this crate cannot
//! call the system, so an outline face here is a cache: the interface asks
//! which characters are not in it yet, has the host rasterize those, and
//! puts the results in before it draws.

use std::collections::HashMap;

use crate::boxes;
use crate::cluster;
use crate::compose::compose;
use crate::font::{AlphaGlyph, MAX_SCALE};

/// The largest cell an outline face may claim, in pixels. Well beyond any
/// real setting (a 24-pixel face at scale 3 is about 45 by 80); it only
/// keeps a wild number from the host from turning into a huge picture when
/// a cluster is put together.
const MAX_CELL: u32 = 256;

/// One family at one pixel height, with the glyphs rasterized so far.
#[derive(Clone, Debug)]
pub struct OutlineFace {
    family: String,
    height: u32,
    cell_w: u32,
    cell_h: u32,
    ascent: u32,
    strike: u32,
    /// `None` records that the face was asked and has no such glyph, so it
    /// is not asked again every frame.
    glyphs: HashMap<char, Option<AlphaGlyph>>,
    blank: AlphaGlyph,
}

impl OutlineFace {
    /// An empty cache for `family` drawn `height` pixels tall. `cell` and
    /// `ascent` are what the host measured for that family at that height.
    pub fn new(family: &str, height: u32, cell: (u32, u32), ascent: u32) -> OutlineFace {
        let cell_w = cell.0.clamp(1, MAX_CELL);
        let cell_h = cell.1.clamp(1, MAX_CELL);
        OutlineFace {
            family: family.to_string(),
            height,
            cell_w,
            cell_h,
            ascent: ascent.clamp(1, cell_h),
            strike: 1,
            glyphs: HashMap::new(),
            blank: AlphaGlyph::default(),
        }
    }

    /// The family name, which is also what the face is called in settings.
    pub fn name(&self) -> &str {
        &self.family
    }

    /// The pixel height the glyphs were rasterized at.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Cell width and height in pixels.
    pub fn cell(&self) -> (u32, u32) {
        (self.cell_w, self.cell_h)
    }

    /// Rows from the top of the cell down to and including the baseline row.
    pub fn ascent(&self) -> u32 {
        self.ascent
    }

    /// How many pixels to the right a double strike lands. See
    /// [`OutlineFace::set_strike`].
    pub fn strike(&self) -> u32 {
        self.strike
    }

    /// Says how far a double strike moves, in pixels. A bitmap face is
    /// struck again one of its own pixels to the right, which at scale 3 is
    /// three pixels of the screen. An outline face has no pixels of its
    /// own: a host that rasterizes it at the final size (the height times
    /// the scale) and draws it at scale 1 sets this to that scale, so that
    /// bold is as heavy here as in Gallant at the same setting. It is 1
    /// until set.
    pub fn set_strike(&mut self, pixels: u32) {
        self.strike = pixels.clamp(1, MAX_SCALE);
    }

    /// How many cells a glyph takes. An outline glyph carries no advance,
    /// so one that reaches well into the next cell is taken to be a wide
    /// character.
    pub(crate) fn cells(&self, g: &AlphaGlyph) -> u32 {
        let w = self.cell_w as i64;
        1 + (g.left as i64 + g.width as i64 > w + w / 4) as u32
    }

    /// True when some of the glyph's ink lies where it will be drawn: on
    /// the rows of the cell and, for a character with a cell of its own,
    /// on the columns of the one or two cells it takes. A mark is asked
    /// only about rows, because it is moved sideways onto its letter
    /// wherever the rasterizer put it. The caller has checked that the
    /// glyph is sound.
    fn lands(&self, ch: char, g: &AlphaGlyph) -> bool {
        let (w, h) = (g.width as i64, g.height as i64);
        let (left, top) = (g.left as i64, g.top as i64);
        let rows = (-top).max(0)..(self.cell_h as i64 - top).min(h);
        let cols = if cluster::is_zero_width(ch) { 0..w } else { (-left).max(0)..((self.cells(g) * self.cell_w) as i64 - left).min(w) };
        if cols.start >= cols.end {
            return false;
        }
        rows.into_iter().any(|r| g.coverage[(r * w + cols.start) as usize..(r * w + cols.end) as usize].iter().any(|&v| v != 0))
    }

    /// How many glyphs are held (characters the face lacks are not counted).
    pub fn len(&self) -> usize {
        self.glyphs.values().filter(|g| g.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once `ch` has been put in, whether as a glyph or as lacking.
    pub fn knows(&self, ch: char) -> bool {
        self.glyphs.contains_key(&ch)
    }

    pub fn has(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    /// The glyph to draw for a character of text, by the same rules a
    /// bitmap face follows: nothing for control codes, a blank cell for the
    /// no-break spaces, the hyphen for a non-breaking hyphen.
    pub fn glyph(&self, ch: char) -> Option<&AlphaGlyph> {
        let held = |c: char| self.glyphs.get(&c).and_then(|g| g.as_ref());
        match ch as u32 {
            0x00..=0x1F | 0x7F => None,
            0xA0 | 0x202F => Some(&self.blank),
            0x2011 => ['\u{2010}', '\u{2011}', '-'].into_iter().find_map(held),
            _ => held(ch),
        }
    }

    /// Puts in the host's answer for one character: its glyph, or `None`
    /// when the face has none.
    ///
    /// Two kinds of glyph are recorded as lacking. One whose coverage is
    /// shorter than its size says, so it can never be read past its end.
    /// And one that would draw nothing for a character that is not a
    /// space: a glyph with no ink at all (what a rasterizer hands back for
    /// a combining mark it drew outside the cell), or one whose ink all
    /// lies outside the cells it is cut to. Kept as a glyph, either would
    /// make the character vanish. Lacking, it is borrowed from the
    /// fallback or boxed.
    pub fn insert(&mut self, ch: char, glyph: Option<AlphaGlyph>) {
        let kept = glyph.filter(|g| g.is_sound() && (cluster::is_blank(ch) || self.lands(ch, g)));
        self.glyphs.insert(ch, kept);
    }

    /// Forgets every glyph, for when the family or height changes under it.
    pub fn clear(&mut self) {
        self.glyphs.clear();
    }

    /// The characters that drawing `text` may need and that have not been
    /// put in yet, each once, in the order met.
    ///
    /// That is more than the characters of `text` itself, and it is all of
    /// them: whatever a cluster may be drawn with is asked for when the
    /// cluster is first seen, so that no picture changes later because some
    /// other text happened to bring a glyph in. A letter followed by marks
    /// may be drawn with one precomposed glyph, i and j lose their dots
    /// under a mark, a non-breaking hyphen is drawn as either hyphen, the
    /// letter x and the underscore are measured to place marks and
    /// underlines, and the two plain box-drawing lines are measured before
    /// any box-drawing character is drawn.
    pub fn missing(&self, text: &str) -> Vec<char> {
        let mut out: Vec<char> = Vec::new();
        let mut want = |ch: char| {
            let never_drawn = matches!(ch as u32, 0x00..=0x1F | 0x7F | 0xA0 | 0x202F) || cluster::has_no_picture(ch);
            if !never_drawn && !self.glyphs.contains_key(&ch) && !out.contains(&ch) {
                out.push(ch);
            }
        };
        want('x');
        want('_');
        for piece in cluster::clusters(text) {
            let mut chars = piece.chars();
            let Some(first) = chars.next() else { continue };
            want(first);
            if first == '\u{2011}' {
                want('\u{2010}');
                want('-');
            }
            if boxes::covers(first) {
                want('\u{2500}');
                want('\u{2502}');
            }
            // The letter as far as it has joined the marks after it. The
            // first mark that joins nothing ends that, as it does when the
            // cluster is drawn.
            let mut joined = Some(first);
            let mut marked = false;
            for mark in chars.filter(|&m| cluster::draws_on_letter(m)) {
                want(mark);
                marked = true;
                joined = joined.and_then(|letter| compose(letter, mark));
                if let Some(letter) = joined {
                    want(letter);
                }
            }
            match first {
                'i' if marked => want('\u{0131}'),
                'j' if marked => want('\u{0237}'),
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot() -> AlphaGlyph {
        AlphaGlyph { width: 1, height: 1, left: 0, top: 0, coverage: vec![255] }
    }

    #[test]
    fn missing_lists_each_needed_character_once() {
        let mut f = OutlineFace::new("Courier New", 22, (13, 25), 19);
        assert_eq!(f.missing("abba"), vec!['x', '_', 'a', 'b']);
        f.insert('a', Some(dot()));
        f.insert('b', None);
        f.insert('x', Some(dot()));
        f.insert('_', Some(dot()));
        assert!(f.missing("abba").is_empty());
        assert!(f.knows('b') && !f.has('b') && f.has('a'));
        assert_eq!((f.len(), f.is_empty()), (3, false));
        f.clear();
        assert!(f.is_empty() && !f.knows('a'));
    }

    #[test]
    fn missing_asks_for_what_a_cluster_may_turn_into() {
        let mut f = OutlineFace::new("f", 22, (13, 25), 19);
        f.insert('x', None);
        f.insert('_', None);
        // e, circumflex, acute: the two-mark Vietnamese letter and the step on the way.
        assert_eq!(f.missing("e\u{0302}\u{0301}"), vec!['e', '\u{0302}', '\u{00EA}', '\u{0301}', '\u{1EBF}']);
        // n with a comma above has no precomposed form.
        assert_eq!(f.missing("n\u{0313}"), vec!['n', '\u{0313}']);
        // i and j under a mark that does not join need their dotless forms.
        assert_eq!(f.missing("i\u{0313}"), vec!['i', '\u{0313}', '\u{0131}']);
        assert_eq!(f.missing("j\u{0313}"), vec!['j', '\u{0313}', '\u{0237}']);
        assert_eq!(f.missing("in"), vec!['i', 'n']);
        // The dotless letter is wanted even when the mark joins, in case
        // the face turns out to lack the joined letter.
        assert_eq!(f.missing("i\u{0301}"), vec!['i', '\u{0301}', '\u{00ED}', '\u{0131}']);
        // A mark that joins nothing ends the joining, as it does when the
        // cluster is drawn: the acute after the comma is placed by hand.
        assert_eq!(f.missing("e\u{0313}\u{0301}"), vec!['e', '\u{0313}', '\u{0301}']);
        // Characters with no picture are stepped over, and never asked for.
        assert_eq!(f.missing("e\u{200D}\u{0301}\u{FE0F}"), vec!['e', '\u{0301}', '\u{00E9}']);
        // A non-breaking hyphen may be drawn with either hyphen.
        assert_eq!(f.missing("\u{2011}"), vec!['\u{2011}', '\u{2010}', '-']);
        assert!(f.missing("\t\r\n\u{00A0}\u{202F}\u{7F}\u{200B}").is_empty());
    }

    #[test]
    fn lookup_rules_match_the_bitmap_faces() {
        let mut f = OutlineFace::new("f", 22, (13, 25), 19);
        f.insert('\u{0007}', Some(dot()));
        f.insert('\u{00A0}', Some(dot()));
        f.insert('-', Some(dot()));
        assert!(f.glyph('\u{0007}').is_none());
        assert_eq!(f.glyph('\u{00A0}').unwrap().coverage.len(), 0);
        assert_eq!(f.glyph('\u{202F}').unwrap().width, 0);
        assert!(f.glyph('\u{2011}').is_some());
        assert!(f.glyph('q').is_none());
    }

    #[test]
    fn a_glyph_that_would_draw_nothing_is_recorded_as_lacking() {
        let block = |width: u32, height: u32, left: i32, top: i32| AlphaGlyph { width, height, left, top, coverage: vec![255; (width * height) as usize] };
        let mut f = OutlineFace::new("f", 22, (12, 22), 17);
        for (ch, glyph, kept) in [
            ('a', block(6, 8, 3, 9), true),
            // Wholly below, above, left and right of where it is cut to.
            ('b', block(6, 8, 3, 22), false),
            ('c', block(6, 8, 3, -8), false),
            ('d', block(6, 8, -6, 9), false),
            ('e', block(6, 8, 40, 9), false),
            // One row or one column inside is enough.
            ('f', block(6, 8, 3, 21), true),
            ('g', block(6, 8, -5, 9), true),
            // Reaching into the next cell makes it a wide character, which keeps that cell.
            ('h', block(6, 8, 14, 9), true),
            // A mark may hang back over the cell before, or lie anywhere across; not above or below.
            ('\u{0301}', block(3, 2, -8, 1), true),
            ('\u{0313}', block(3, 2, 90, 1), true),
            ('\u{0323}', block(3, 2, 4, 30), false),
            // No ink at all: only a space is a glyph then.
            (' ', AlphaGlyph::default(), true),
            ('\u{2003}', block(0, 0, 0, 0), true),
            ('k', AlphaGlyph { width: 3, height: 3, left: 4, top: 4, coverage: vec![0; 9] }, false),
        ] {
            f.insert(ch, Some(glyph));
            assert!(f.knows(ch), "{ch:?}");
            assert_eq!(f.has(ch), kept, "{ch:?}");
        }
        assert_eq!((f.cells(f.glyph('a').unwrap()), f.cells(f.glyph('h').unwrap())), (1, 2));
    }

    #[test]
    fn the_strike_is_one_pixel_until_the_host_says_otherwise() {
        let mut f = OutlineFace::new("f", 66, (36, 66), 51);
        assert_eq!(f.strike(), 1);
        f.set_strike(3);
        assert_eq!(f.strike(), 3);
        f.set_strike(0);
        assert_eq!(f.strike(), 1);
        f.set_strike(u32::MAX);
        assert_eq!(f.strike(), MAX_SCALE);
    }

    #[test]
    fn unsound_glyphs_and_wild_metrics_are_tamed() {
        let mut f = OutlineFace::new("f", 0, (0, u32::MAX), u32::MAX);
        assert_eq!(f.cell(), (1, MAX_CELL));
        assert_eq!(f.ascent(), MAX_CELL);
        f.insert('a', Some(AlphaGlyph { width: 9, height: 9, left: 0, top: 0, coverage: vec![1; 80] }));
        f.insert('b', Some(AlphaGlyph { width: u32::MAX, height: u32::MAX, left: 0, top: 0, coverage: vec![] }));
        assert!(f.knows('a') && !f.has('a') && !f.has('b'));
    }
}
