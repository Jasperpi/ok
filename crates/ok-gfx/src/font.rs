use std::collections::HashMap;
use std::fmt;

use crate::cluster;
use crate::color::Rgb;
use crate::frame::{Frame, Rect};
use crate::hexbox;

/// One glyph of a bitmap face. Each row is a `u32` whose top bit is the
/// leftmost pixel; `width` is in font pixels (one cell, or two for a wide
/// glyph).
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    pub width: u32,
    pub rows: &'a [u32],
}

impl Glyph<'_> {
    /// The first and last rows that hold any ink, or `None` for a blank.
    pub fn ink_rows(&self) -> Option<(u32, u32)> {
        let first = self.rows.iter().position(|&r| r != 0)?;
        let last = self.rows.iter().rposition(|&r| r != 0)?;
        Some((first as u32, last as u32))
    }
}

/// A glyph with 8-bit coverage, the form a system rasterizer hands back for
/// an outline (TTF/OTF) face. `coverage` is `width * height` bytes, top row
/// first, already at final pixel size.
#[derive(Clone, Debug, Default)]
pub struct AlphaGlyph {
    pub width: u32,
    pub height: u32,
    /// Offset of the bitmap's top-left corner from the cell's top-left corner.
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

impl AlphaGlyph {
    /// True when `coverage` really holds `width * height` bytes, so a glyph
    /// handed over with the wrong size is never read past its end.
    pub fn is_sound(&self) -> bool {
        match (self.width as usize).checked_mul(self.height as usize) {
            Some(n) => self.coverage.len() >= n,
            None => false,
        }
    }
}

/// How a glyph is laid down.
#[derive(Clone, Copy, Debug)]
pub struct Pen {
    pub color: Rgb,
    /// 255 is opaque.
    pub alpha: u8,
    /// Each font pixel becomes a `scale` by `scale` block.
    pub scale: u32,
    /// Thickens every stroke by one font pixel to the right: the glyph
    /// struck a second time. An outline face has no pixels of its own and
    /// is struck by what its host set (see `OutlineFace::set_strike`).
    pub bold: bool,
}

impl Pen {
    pub const fn new(color: Rgb) -> Pen {
        Pen { color, alpha: 255, scale: 1, bold: false }
    }

    pub const fn scale(mut self, scale: u32) -> Pen {
        self.scale = scale;
        self
    }

    pub const fn alpha(mut self, alpha: u8) -> Pen {
        self.alpha = alpha;
        self
    }

    pub const fn bold(mut self, bold: bool) -> Pen {
        self.bold = bold;
        self
    }

    /// The scale as a pixel multiple that is safe to multiply with: at least
    /// 1, and small enough that a cell times the scale cannot overflow.
    pub(crate) fn step(&self) -> i32 {
        self.scale.clamp(1, MAX_SCALE) as i32
    }
}

/// The largest pixel multiple any drawing call honours.
pub(crate) const MAX_SCALE: u32 = 64;

/// The widest and tallest cell a bitmap face may have. A row is one `u32`,
/// so a glyph, wide ones included, is at most 32 pixels across.
pub(crate) const MAX_CELL_W: u32 = 32;
pub(crate) const MAX_CELL_H: u32 = 64;

/// Why a face could not be read. `line` is 0 when the fault is not on one
/// line (a binary file, or a file that is missing something).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError {
    pub line: usize,
    pub what: String,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.what)
        } else {
            write!(f, "line {}: {}", self.line, self.what)
        }
    }
}

impl std::error::Error for FontError {}

pub(crate) fn err<T>(line: usize, what: impl Into<String>) -> Result<T, FontError> {
    Err(FontError { line, what: what.into() })
}

#[derive(Clone)]
struct Slot {
    start: u32,
    width: u32,
    /// The picture has no ink at all.
    empty: bool,
}

/// Code points below this are looked up in a flat table; the few above it
/// (private use, presentation forms) go through a map.
const FLAT: usize = 0x3000;

/// A fixed-cell bitmap face held as one bit per pixel.
#[derive(Clone)]
pub struct BitmapFont {
    name: String,
    cell_w: u32,
    cell_h: u32,
    ascent: u32,
    x_top: u32,
    underline: (u32, u32),
    strike: (u32, u32),
    rows: Vec<u32>,
    slots: Vec<Slot>,
    flat: Vec<u32>,
    rest: HashMap<u32, u32>,
    mapped: usize,
    wide: usize,
    /// One cell of nothing: the picture of a no-break space.
    blank: Vec<u32>,
}

impl fmt::Debug for BitmapFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitmapFont({:?}, {}x{}, {} glyphs)", self.name, self.cell_w, self.cell_h, self.mapped)
    }
}

impl BitmapFont {
    /// A face with no glyphs yet. The caller has already checked the cell
    /// against [`MAX_CELL_W`] and [`MAX_CELL_H`].
    pub(crate) fn empty(name: &str, cell_w: u32, cell_h: u32) -> BitmapFont {
        let cell_w = cell_w.clamp(1, MAX_CELL_W);
        let cell_h = cell_h.clamp(1, MAX_CELL_H);
        BitmapFont {
            name: name.to_string(),
            cell_w,
            cell_h,
            ascent: cell_h,
            x_top: 0,
            underline: (cell_h - 1, 1),
            strike: (cell_h / 2, 1),
            rows: Vec::new(),
            slots: Vec::new(),
            flat: vec![0; FLAT],
            rest: HashMap::new(),
            mapped: 0,
            wide: 0,
            blank: vec![0; cell_h as usize],
        }
    }

    /// Stores a picture and returns its number, which [`BitmapFont::map`]
    /// ties to one or more code points. `rows` is top bit leftmost; rows
    /// beyond the cell height are dropped and missing ones are blank.
    pub(crate) fn add(&mut self, width: u32, rows: &[u32]) -> u32 {
        let width = width.clamp(1, 32);
        let keep = if width >= 32 { u32::MAX } else { !(u32::MAX >> width) };
        let start = self.rows.len();
        let h = self.cell_h as usize;
        self.rows.extend((0..h).map(|r| rows.get(r).copied().unwrap_or(0) & keep));
        let empty = self.rows[start..].iter().all(|&row| row == 0);
        self.slots.push(Slot { start: start as u32, width, empty });
        self.slots.len() as u32
    }

    /// Makes `cp` show picture `id`. A later call for the same code point
    /// replaces the earlier one.
    pub(crate) fn map(&mut self, cp: u32, id: u32) {
        let Some(new) = self.slot(id) else { return };
        let new_wide = new.width > self.cell_w;
        let old = self.id_of(cp);
        match self.slot(old) {
            Some(s) => {
                if s.width > self.cell_w {
                    self.wide -= 1;
                }
            }
            None => self.mapped += 1,
        }
        if new_wide {
            self.wide += 1;
        }
        if (cp as usize) < FLAT {
            self.flat[cp as usize] = id;
        } else {
            self.rest.insert(cp, id);
        }
    }

    /// Adds or replaces the glyph for one code point.
    pub(crate) fn insert(&mut self, cp: u32, width: u32, rows: &[u32]) {
        let id = self.add(width, rows);
        self.map(cp, id);
    }

    /// Sets the measurements every face carries. `ascent` is what the file
    /// said, if it said anything; a format with no metrics (.hex, PSF) has
    /// its baseline read off the letters themselves.
    pub(crate) fn finish(mut self, ascent: Option<u32>) -> BitmapFont {
        let h = self.cell_h;
        let bottom_of = |f: &BitmapFont, chars: &[char]| {
            chars.iter().find_map(|&c| f.raw_glyph(c as u32)?.ink_rows()).map(|(_, last)| last + 1)
        };
        let top_of = |f: &BitmapFont, chars: &[char]| {
            chars.iter().find_map(|&c| f.raw_glyph(c as u32)?.ink_rows()).map(|(first, _)| first)
        };
        let ascent = ascent
            .or_else(|| bottom_of(&self, &['H', 'x', 'A', 'n', '0']))
            .unwrap_or((h * 17 + 11) / 22)
            .clamp(1, h);
        let x_top = top_of(&self, &['x', 'n', 'o', 'a']).unwrap_or(ascent * 7 / 17).min(ascent - 1);
        let thick = (h / 16).max(1);
        let underline = match self.raw_glyph('_' as u32).and_then(|g| g.ink_rows()) {
            Some((first, last)) => (first, last - first + 1),
            None => ((ascent + 1).min(h - thick.min(h)), thick.min(h)),
        };
        let mid = (x_top + ascent - 1) / 2;
        let strike = (mid.min(h - underline.1.min(h)), underline.1.min(h));
        self.ascent = ascent;
        self.x_top = x_top;
        self.underline = underline;
        self.strike = strike;
        self
    }

    /// Reads the `.hex` form used by GNU Unifont and FreeBSD's vt faces.
    ///
    /// Each glyph is one line, `CODEPOINT:HEXDIGITS`: one to six hex digits,
    /// a colon, then the rows top to bottom, each row padded on the right to
    /// a whole number of bytes. FreeBSD's files open with `# Width: N` and
    /// `# Height: N`; without them the Unifont cell of 8 by 16 is assumed. A
    /// glyph with twice the bytes per row is a double-width glyph. Anything
    /// else on a line is an error rather than something to guess at, so a
    /// damaged file is refused instead of half read.
    pub fn from_hex(name: &str, src: &str) -> Result<BitmapFont, FontError> {
        let (mut cell_w, mut cell_h) = (8u32, 16u32);
        let mut font: Option<BitmapFont> = None;
        let mut rows: Vec<u32> = Vec::new();
        // Notepad puts a byte-order mark in front of what it saves.
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);

        for (i, line) in src.lines().enumerate() {
            let n = i + 1;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                let comment = comment.trim();
                for (key, target) in [("Width:", &mut cell_w), ("Height:", &mut cell_h)] {
                    let Some(value) = comment.strip_prefix(key) else { continue };
                    if font.is_some() {
                        return err(n, format!("{key} must come before the first glyph"));
                    }
                    let value = value.trim();
                    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return err(n, format!("{key} is not followed by a number"));
                    }
                    *target = value.parse().unwrap_or(u32::MAX);
                }
                continue;
            }
            if !(1..=MAX_CELL_W).contains(&cell_w) || !(1..=MAX_CELL_H).contains(&cell_h) {
                return err(n, format!("unsupported cell size {cell_w} by {cell_h}"));
            }
            let font = font.get_or_insert_with(|| BitmapFont::empty(name, cell_w, cell_h));

            let Some((cp, digits)) = line.split_once(':') else {
                return err(n, "expected CODEPOINT:HEX");
            };
            if cp.is_empty() || cp.len() > 6 || !cp.bytes().all(|b| b.is_ascii_hexdigit()) {
                return err(n, "code point is not 1 to 6 hex digits");
            }
            let cp = u32::from_str_radix(cp, 16).unwrap_or(u32::MAX);
            if char::from_u32(cp).is_none() {
                return err(n, format!("U+{cp:04X} is not a character"));
            }
            if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return err(n, "glyph data is not hexadecimal");
            }

            let h = cell_h as usize;
            let single = 2 * cell_w.div_ceil(8) as usize;
            let double = 2 * (2 * cell_w).div_ceil(8) as usize;
            let can_double = double > single && 2 * cell_w <= 32;
            let (per_row, width) = if digits.len() == single * h {
                (single, cell_w)
            } else if can_double && digits.len() == double * h {
                (double, 2 * cell_w)
            } else if can_double {
                return err(
                    n,
                    format!(
                        "glyph data is {} digits; a {cell_w} by {cell_h} glyph needs {} or {}",
                        digits.len(),
                        single * h,
                        double * h
                    ),
                );
            } else {
                return err(
                    n,
                    format!("glyph data is {} digits; a {cell_w} by {cell_h} glyph needs {}", digits.len(), single * h),
                );
            };

            rows.clear();
            for r in 0..h {
                let v = u32::from_str_radix(&digits[r * per_row..(r + 1) * per_row], 16).unwrap_or(0);
                rows.push(v << (32 - per_row as u32 * 4));
            }
            font.insert(cp, width, &rows);
        }

        match font {
            Some(f) => Ok(f.finish(None)),
            None => err(0, "no glyphs found"),
        }
    }

    /// Unpacks the table the build script makes from `gallant.hex`: a header,
    /// runs of consecutive code points, then every row at exactly the glyph's
    /// width in bits. See `build.rs` for the layout.
    pub(crate) fn from_packed(name: &str, data: &[u8]) -> Result<BitmapFont, FontError> {
        let short = || FontError { line: 0, what: "packed face is cut short".to_string() };
        if data.get(..4) != Some(b"OKF1") {
            return err(0, "not a packed face");
        }
        let byte = |at: usize| data.get(at).copied().ok_or_else(short);
        let (cell_w, cell_h, ascent) = (byte(4)? as u32, byte(5)? as u32, byte(6)? as u32);
        if !(1..=16).contains(&cell_w) || !(1..=MAX_CELL_H).contains(&cell_h) {
            return err(0, format!("unsupported cell size {cell_w} by {cell_h}"));
        }
        let runs = u16::from_le_bytes([byte(8)?, byte(9)?]) as usize;
        let mut at = 12usize;
        let mut bits_at = at.checked_add(runs.checked_mul(6).ok_or_else(short)?).ok_or_else(short)?;
        let mut font = BitmapFont::empty(name, cell_w, cell_h);
        let mut rows = vec![0u32; cell_h as usize];

        for _ in 0..runs {
            let head = data.get(at..at + 6).ok_or_else(short)?;
            at += 6;
            let first = u32::from_le_bytes([head[0], head[1], head[2], 0]);
            let wide = head[3] != 0;
            let count = u16::from_le_bytes([head[4], head[5]]) as u32;
            let width = if wide { cell_w * 2 } else { cell_w };
            let glyph_bytes = (width * cell_h).div_ceil(8) as usize;
            for k in 0..count {
                let packed = data.get(bits_at..bits_at + glyph_bytes).ok_or_else(short)?;
                bits_at += glyph_bytes;
                let mut bit = 0usize;
                for row in rows.iter_mut() {
                    let mut v = 0u32;
                    for _ in 0..width {
                        v = v << 1 | (packed[bit / 8] >> (7 - bit % 8) & 1) as u32;
                        bit += 1;
                    }
                    *row = v << (32 - width);
                }
                let cp = first + k;
                if char::from_u32(cp).is_none() {
                    return err(0, format!("U+{cp:04X} is not a character"));
                }
                font.insert(cp, width, &rows);
            }
        }
        if font.is_empty() {
            return err(0, "no glyphs found");
        }
        Ok(font.finish(Some(ascent)))
    }

    /// Reads a face from the bytes of a file of unknown kind: PSF by its
    /// magic number, BDF by its first word, otherwise `.hex`.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<BitmapFont, FontError> {
        if crate::psf::looks_like_psf(bytes) || bytes.starts_with(&[0x1F, 0x8B]) {
            return BitmapFont::from_psf(name, bytes);
        }
        let text = String::from_utf8_lossy(bytes);
        if text.trim_start_matches('\u{FEFF}').trim_start().starts_with("STARTFONT") {
            BitmapFont::from_bdf(name, &text)
        } else {
            BitmapFont::from_hex(name, &text)
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Cell width and height in font pixels.
    pub fn cell(&self) -> (u32, u32) {
        (self.cell_w, self.cell_h)
    }

    /// Rows above the baseline, counting the row the letters stand on.
    /// Gallant is 17 of 22. Two faces are lined up by this number.
    pub fn ascent(&self) -> u32 {
        self.ascent
    }

    /// Rows below the baseline, where descenders go.
    pub fn descent(&self) -> u32 {
        self.cell_h - self.ascent
    }

    /// The first row a lower-case x reaches. Marks above a letter are told
    /// from marks below it by which side of this row they fall on.
    pub fn x_top(&self) -> u32 {
        self.x_top
    }

    /// The first row and the thickness of the face's underscore, which is
    /// where an underline goes.
    pub fn underline_rows(&self) -> (u32, u32) {
        self.underline
    }

    /// The first row and the thickness of a strike through the middle of the
    /// lower-case letters.
    pub fn strike_rows(&self) -> (u32, u32) {
        self.strike
    }

    /// How many code points have a glyph.
    pub fn len(&self) -> usize {
        self.mapped
    }

    pub fn is_empty(&self) -> bool {
        self.mapped == 0
    }

    /// How many code points have a double-width glyph.
    pub fn wide_len(&self) -> usize {
        self.wide
    }

    /// Every code point that has a glyph, in order. For cross-checks against
    /// a width table, not for drawing.
    pub fn code_points(&self) -> Vec<u32> {
        let mut all: Vec<u32> = (0..FLAT as u32).filter(|&cp| self.flat[cp as usize] != 0).collect();
        all.extend(self.rest.keys().copied());
        all.sort_unstable();
        all
    }

    pub fn has(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    fn slot(&self, id: u32) -> Option<&Slot> {
        self.slots.get((id as usize).checked_sub(1)?)
    }

    fn id_of(&self, cp: u32) -> u32 {
        match self.flat.get(cp as usize) {
            Some(&id) => id,
            None => self.rest.get(&cp).copied().unwrap_or(0),
        }
    }

    /// The picture stored for a code point, exactly as the file has it. This
    /// is the only way to reach the pictures at U+0000 to U+001F and U+007F
    /// (in Gallant, DEC's special graphics and control pictures), which a
    /// reveal mode may want and running text must never show.
    pub fn raw_glyph(&self, cp: u32) -> Option<Glyph<'_>> {
        let s = self.slot(self.id_of(cp))?;
        let start = s.start as usize;
        let rows = self.rows.get(start..start + self.cell_h as usize)?;
        Some(Glyph { width: s.width, rows })
    }

    /// The picture stored for a character, unless it is an empty picture
    /// for a character that ought to show something. A file can hold such a
    /// picture (a row of zeros in `.hex`, a BDF glyph whose box puts all of
    /// its ink outside the cell), and drawn as it stands it would make that
    /// letter vanish from the page. Treated as lacking, the letter is
    /// borrowed from the fallback or boxed instead.
    fn shown(&self, ch: char) -> Option<Glyph<'_>> {
        let cp = ch as u32;
        let s = self.slot(self.id_of(cp))?;
        if s.empty && !cluster::is_blank(ch) {
            return None;
        }
        self.raw_glyph(cp)
    }

    /// The glyph to draw for a character of text.
    ///
    /// Control codes have none, whatever the file stores in their slots. A
    /// no-break space and a narrow no-break space are a blank cell in every
    /// face. A non-breaking hyphen is drawn as the hyphen. A character whose
    /// stored picture is empty has none either, unless it is a space or
    /// some other character that is rightly blank.
    pub fn glyph(&self, ch: char) -> Option<Glyph<'_>> {
        match ch as u32 {
            0x00..=0x1F | 0x7F => None,
            0xA0 | 0x202F => Some(Glyph { width: self.cell_w, rows: &self.blank }),
            0x2011 => ['\u{2010}', '\u{2011}', '-'].into_iter().find_map(|c| self.shown(c)),
            _ => self.shown(ch),
        }
    }

    /// Draws `ch` with its cell's top-left corner at (`x`, `y`). A character
    /// the face lacks is drawn as a box holding its hex digits so it is
    /// never invisible. Returns the width drawn in screen pixels.
    pub fn draw(&self, frame: &mut Frame, ch: char, x: i32, y: i32, pen: Pen) -> i32 {
        let s = pen.step();
        match self.glyph(ch) {
            Some(g) => {
                draw_glyph(frame, g, x, y, pen);
                g.width as i32 * s
            }
            None => {
                hexbox::draw(frame, ch as u32, x, y, (self.cell_w, self.cell_h), self.ascent, pen);
                self.cell_w as i32 * s
            }
        }
    }
}

/// The part of a rectangle that lies inside `clip`. The rectangle is given
/// in 64-bit numbers because a position times a scale can pass what an
/// `i32` holds, and `Rect` adds its corner to its size without checking.
pub(crate) fn clipped(clip: Rect, x: i64, y: i64, w: i64, h: i64) -> Rect {
    let x0 = x.max(clip.x as i64);
    let y0 = y.max(clip.y as i64);
    let x1 = x.saturating_add(w).min(clip.x as i64 + clip.w.max(0) as i64);
    let y1 = y.saturating_add(h).min(clip.y as i64 + clip.h.max(0) as i64);
    if x0 >= x1 || y0 >= y1 {
        return Rect::default();
    }
    Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
}

/// Draws a bitmap glyph with its top-left corner at (`x`, `y`).
pub fn draw_glyph(frame: &mut Frame, g: Glyph<'_>, x: i32, y: i32, pen: Pen) {
    if pen.alpha == 0 {
        return;
    }
    let s = pen.step();
    let clip = frame.clip();
    let width = g.width.min(32);
    let (gw, gh) = (width as i64 * s as i64, g.rows.len() as i64 * s as i64);
    if x >= clip.right() || y >= clip.bottom() || x as i64 + gw <= clip.x as i64 || y as i64 + gh <= clip.y as i64 {
        return;
    }
    let keep = if width >= 32 { u32::MAX } else { !(u32::MAX >> width) };
    let px = pen.color.px();

    for (r, &row) in g.rows.iter().enumerate() {
        let y0 = y as i64 + r as i64 * s as i64;
        if y0 >= clip.bottom() as i64 {
            break;
        }
        if y0 + s as i64 <= clip.y as i64 {
            continue;
        }
        let y0 = y0 as i32;
        let mut rest = if pen.bold { (row | row >> 1) & keep } else { row & keep };
        let mut col = 0u32;
        // Walk the row as runs of set pixels so each run is one span.
        while rest != 0 {
            let gap = rest.leading_zeros();
            rest <<= gap;
            col += gap;
            let run = (!rest).leading_zeros();
            let x0 = x.saturating_add(col as i32 * s);
            for dy in 0..s {
                frame.span(x0, y0 + dy, run as i32 * s, px, pen.alpha);
            }
            if run == 32 {
                break;
            }
            rest <<= run;
            col += run;
        }
    }
}

/// Lays down a block of 8-bit coverage, `w` by `h`, with its top-left corner
/// at (`x`, `y`), each coverage pixel becoming a `scale` by `scale` block.
/// Runs of equal coverage go down as one span. Coverage shorter than
/// `w * h` draws nothing.
pub(crate) fn draw_coverage(frame: &mut Frame, cov: &[u8], (w, h): (usize, usize), (x, y): (i32, i32), scale: i32, color: Rgb, alpha: u8) {
    let Some(n) = w.checked_mul(h) else { return };
    if alpha == 0 || n == 0 || cov.len() < n || w > i32::MAX as usize / 128 || h > i32::MAX as usize / 128 {
        return;
    }
    let s = scale.clamp(1, MAX_SCALE as i32) as i64;
    let clip = frame.clip();
    let (x, y) = (x as i64, y as i64);
    if x >= clip.right() as i64 || y >= clip.bottom() as i64 || x + w as i64 * s <= clip.x as i64 || y + h as i64 * s <= clip.y as i64 {
        return;
    }
    let px = color.px();
    for (r, line) in cov[..n].chunks_exact(w).enumerate() {
        let y0 = y + r as i64 * s;
        if y0 >= clip.bottom() as i64 {
            break;
        }
        if y0 + s <= clip.y as i64 {
            continue;
        }
        let mut c = 0usize;
        while c < w {
            let v = line[c];
            let start = c;
            while c < w && line[c] == v {
                c += 1;
            }
            if v == 0 {
                continue;
            }
            let x0 = x + start as i64 * s;
            let len = (c - start) as i64 * s;
            if x0 >= clip.right() as i64 || x0 + len <= clip.x as i64 {
                continue;
            }
            let a = ((v as u32 * alpha as u32 + 127) / 255) as u8;
            for dy in 0..s {
                frame.span(x0 as i32, (y0 + dy) as i32, len as i32, px, a);
            }
        }
    }
}

/// Draws a coverage glyph for a cell whose top-left corner is at (`x`, `y`).
pub fn draw_alpha_glyph(frame: &mut Frame, g: &AlphaGlyph, x: i32, y: i32, color: Rgb, alpha: u8) {
    if !g.is_sound() {
        return;
    }
    let (x, y) = (x.saturating_add(g.left), y.saturating_add(g.top));
    draw_coverage(frame, &g.coverage, (g.width as usize, g.height as usize), (x, y), 1, color, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &str = "# Width: 12\n# Height: 2\n0041:F0000000\n0042:90100000\n4E00:FF00000F0000\n";

    #[test]
    fn reads_header_single_and_double_width() {
        let f = BitmapFont::from_hex("tiny", TINY).unwrap();
        assert_eq!(f.cell(), (12, 2));
        assert_eq!(f.len(), 3);
        assert_eq!(f.wide_len(), 1);
        let a = f.glyph('A').unwrap();
        assert_eq!(a.width, 12);
        assert_eq!(a.rows, &[0xF000_0000, 0x0000_0000]);
        let b = f.glyph('B').unwrap();
        assert_eq!(b.rows, &[0x9010_0000, 0x0000_0000]);
        let wide = f.glyph('\u{4E00}').unwrap();
        assert_eq!(wide.width, 24);
        assert_eq!(wide.rows, &[0xFF00_0000, 0x0F00_0000]);
        assert!(f.glyph('C').is_none());
        assert!(f.glyph('\u{10FFFF}').is_none());
        assert_eq!(f.code_points(), vec![0x41, 0x42, 0x4E00]);
    }

    #[test]
    fn unifont_default_cell_is_8_by_16() {
        let line = format!("0041:{}\n", "18".repeat(16));
        let f = BitmapFont::from_hex("u", &line).unwrap();
        assert_eq!(f.cell(), (8, 16));
        assert_eq!(f.glyph('A').unwrap().rows[0], 0x1800_0000);
        let wide = format!("0041:{}\n4E00:{}\n", "18".repeat(16), "0180".repeat(16));
        let f = BitmapFont::from_hex("u", &wide).unwrap();
        assert_eq!(f.glyph('\u{4E00}').unwrap().width, 16);
    }

    #[test]
    fn code_points_take_one_to_six_digits_and_need_the_colon() {
        let rows = "F000".repeat(2);
        for cp in ["41", "041", "0041", "00041", "000041"] {
            let f = BitmapFont::from_hex("x", &format!("# Width: 12\n# Height: 2\n{cp}:{rows}\n")).unwrap();
            assert!(f.has('A'), "{cp}");
        }
        let f = BitmapFont::from_hex("x", &format!("# Width: 12\n# Height: 2\n1F600:{rows}\r\n10FFFF:{rows}\r\n")).unwrap();
        assert!(f.has('\u{1F600}') && f.has('\u{10FFFF}'));
        for bad in ["0000041", "", "+41", "41 ", "4 1", "0x41", "U+41", "110000", "D800"] {
            let src = format!("# Width: 12\n# Height: 2\n{bad}:{rows}\n");
            assert_eq!(BitmapFont::from_hex("x", &src).unwrap_err().line, 3, "{bad:?}");
        }
        // A line of hex digits with no colon is the input that crashes vtfontcvt.
        let src = format!("# Width: 12\n# Height: 2\n0041{rows}\n");
        assert_eq!(BitmapFont::from_hex("x", &src).unwrap_err().line, 3);
    }

    #[test]
    fn pad_bits_are_masked() {
        let f = BitmapFont::from_hex("x", "# Width: 12\n# Height: 2\n0041:FFFFFFFF\n4E00:FFFFFFFFFFFF\n").unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0xFFF0_0000, 0xFFF0_0000]);
        assert_eq!(f.glyph('\u{4E00}').unwrap().rows, &[0xFFFF_FF00, 0xFFFF_FF00]);
        let f = BitmapFont::from_hex("x", "# Width: 5\n# Height: 1\n0041:FF\n").unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0xF800_0000]);
    }

    #[test]
    fn rejects_malformed_input_with_a_line_number() {
        assert!(BitmapFont::from_hex("x", "").is_err());
        assert!(BitmapFont::from_hex("x", "# Width: 12\n# Height: 2\n").is_err());
        let line = |src: &str| BitmapFont::from_hex("x", src).unwrap_err().line;
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:F\n"), 3);
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:F000F0\n"), 3);
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:F000F000F0\n"), 3);
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:ZZZZZZZZ\n"), 3);
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:éééé\n"), 3);
        assert_eq!(line("# Width: 40\n# Height: 2\n0041:F0\n"), 3);
        assert_eq!(line("# Width: 0\n# Height: 2\n0041:F0\n"), 3);
        assert_eq!(line("# Width: 12\n# Height: 65\n0041:F0\n"), 3);
        assert_eq!(line("# Width: twelve\n0041:F0\n"), 1);
        assert_eq!(line("# Width: 99999999999999999999\n0041:F0\n"), 2);
        assert_eq!(line("# Width: 12\n# Height: 2\n0041:F000F000\n# Height: 3\n"), 4);
        assert_eq!(line("# Width: 12\n# Height: 2\njunk\n"), 3);
        // Cells of 4 pixels or fewer cannot tell a wide glyph from a narrow one.
        assert_eq!(line("# Width: 4\n# Height: 2\n4E00:FF0F0000\n"), 3);
    }

    #[test]
    fn other_comments_and_blank_lines_are_skipped() {
        let f = BitmapFont::from_hex("x", "# a face\n\n# Width: 12\n#\n# Height: 2\n\n0041:F000F000\n# done\n").unwrap();
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn a_later_line_for_the_same_code_point_replaces_the_earlier() {
        let f = BitmapFont::from_hex("x", "# Width: 12\n# Height: 1\n4E00:FFFFFF\n4E00:8000\n0041:8000\n").unwrap();
        assert_eq!((f.len(), f.wide_len()), (2, 0));
        assert_eq!(f.glyph('\u{4E00}').unwrap().width, 12);
    }

    #[test]
    fn text_never_sees_control_pictures() {
        let rows = "FFF0".repeat(2);
        let mut src = String::from("# Width: 12\n# Height: 2\n");
        for cp in [0x00, 0x09, 0x0A, 0x0D, 0x1F, 0x7F, 0x41, 0x2010] {
            src.push_str(&format!("{cp:04X}:{rows}\n"));
        }
        let f = BitmapFont::from_hex("x", &src).unwrap();
        assert_eq!(f.len(), 8);
        for cp in [0x00u32, 0x09, 0x0A, 0x0D, 0x1F, 0x7F] {
            let ch = char::from_u32(cp).unwrap();
            assert!(f.glyph(ch).is_none() && !f.has(ch), "{cp:02X}");
            assert_eq!(f.raw_glyph(cp).unwrap().rows, &[0xFFF0_0000, 0xFFF0_0000]);
        }
        assert!(f.raw_glyph(0x80).is_none());
        assert!(f.raw_glyph(u32::MAX).is_none());
    }

    #[test]
    fn same_picture_equivalents() {
        let f = BitmapFont::from_hex("x", "# Width: 12\n# Height: 2\n0041:FFF0FFF0\n00A0:FFF0FFF0\n2010:0F000000\n").unwrap();
        for nbsp in ['\u{00A0}', '\u{202F}'] {
            let g = f.glyph(nbsp).unwrap();
            assert_eq!((g.width, g.rows), (12, &[0u32, 0][..]));
        }
        assert_eq!(f.glyph('\u{2011}').unwrap().rows, f.glyph('\u{2010}').unwrap().rows);
        // With no hyphen of its own the face falls back to the ASCII one.
        let f = BitmapFont::from_hex("x", "# Width: 12\n# Height: 2\n002D:0F000000\n").unwrap();
        assert_eq!(f.glyph('\u{2011}').unwrap().rows, &[0x0F00_0000, 0]);
        let f = BitmapFont::from_hex("x", "# Width: 12\n# Height: 2\n0041:0F000000\n").unwrap();
        assert!(f.glyph('\u{2011}').is_none());
    }

    #[test]
    fn an_empty_picture_is_a_glyph_only_for_a_character_that_is_rightly_blank() {
        let zeros = "0000".repeat(2);
        let src = format!(
            "# Width: 12\n# Height: 2\n0020:{zeros}\n0041:{zeros}\n0042:FFF00000\n2003:{zeros}\n2010:{zeros}\n002D:0F000000\n2800:{zeros}\n"
        );
        let f = BitmapFont::from_hex("x", &src).unwrap();
        assert_eq!(f.len(), 7);
        for blank in [' ', '\u{2003}', '\u{2800}'] {
            assert_eq!(f.glyph(blank).map(|g| g.rows.to_vec()), Some(vec![0, 0]), "U+{:04X}", blank as u32);
        }
        // The A is stored and has nothing in it: text is told the face lacks it.
        assert!(f.glyph('A').is_none() && !f.has('A') && f.has('B'));
        assert_eq!(f.raw_glyph(0x41).map(|g| g.rows.to_vec()), Some(vec![0, 0]));
        // An empty hyphen is passed over for the one that has ink.
        assert_eq!(f.glyph('\u{2011}').unwrap().rows, &[0x0F00_0000, 0]);
        assert!(f.glyph('\u{2010}').is_none());
    }

    #[test]
    fn metrics_are_read_off_the_letters() {
        // H stands on row 5 of 8, x starts on row 3, the underscore is row 7.
        let glyph = |cp: u32, rows: [u8; 8]| format!("{cp:04X}:{}\n", rows.map(|r| format!("{r:02X}")).concat());
        let mut src = String::from("# Width: 8\n# Height: 8\n");
        src.push_str(&glyph(0x48, [0, 0x42, 0x42, 0x7E, 0x42, 0x42, 0, 0]));
        src.push_str(&glyph(0x78, [0, 0, 0, 0x42, 0x24, 0x42, 0, 0]));
        src.push_str(&glyph(0x5F, [0, 0, 0, 0, 0, 0, 0, 0xFF]));
        let f = BitmapFont::from_hex("m", &src).unwrap();
        assert_eq!((f.ascent(), f.descent(), f.x_top()), (6, 2, 3));
        assert_eq!(f.underline_rows(), (7, 1));
        assert_eq!(f.strike_rows(), (4, 1));
        // A face with no letters still gets a baseline inside its cell.
        let f = BitmapFont::from_hex("m", &format!("# Width: 8\n# Height: 8\n{}", glyph(0x2588, [0xFF; 8]))).unwrap();
        assert!(f.ascent() >= 1 && f.ascent() <= 8 && f.x_top() < f.ascent());
        let (row, thick) = f.underline_rows();
        assert!(row + thick <= 8);
    }

    #[test]
    fn draws_runs_at_scale_and_clips() {
        let f = BitmapFont::from_hex("tiny", TINY).unwrap();
        let mut frame = Frame::new(26, 6);
        frame.clear(Rgb(0, 0, 0));
        // 'B' is 1001 then 0000 0001 on its first row; at scale 2 each is a 2x2 block.
        let w = f.draw(&mut frame, 'B', 1, 1, Pen::new(Rgb(255, 255, 255)).scale(2));
        assert_eq!(w, 24);
        let lit: Vec<(i32, i32)> = (0..6)
            .flat_map(|y| (0..26).map(move |x| (x, y)))
            .filter(|&(x, y)| frame.get(x, y) == Some(Rgb(255, 255, 255)))
            .collect();
        assert_eq!(
            lit,
            vec![(1, 1), (2, 1), (7, 1), (8, 1), (23, 1), (24, 1), (1, 2), (2, 2), (7, 2), (8, 2), (23, 2), (24, 2)]
        );

        // Partly and wholly off the frame: no panic, nothing out of bounds.
        f.draw(&mut frame, 'A', -3, -1, Pen::new(Rgb(255, 0, 0)).scale(3));
        f.draw(&mut frame, 'A', 500, 500, Pen::new(Rgb(255, 0, 0)));
        for &(x, y) in &[(i32::MAX, i32::MAX), (i32::MIN, i32::MIN), (i32::MAX, 0), (0, i32::MIN), (i32::MAX - 5, 2)] {
            for scale in [0, 1, 3, 64, u32::MAX] {
                f.draw(&mut frame, 'A', x, y, Pen::new(Rgb(255, 0, 0)).scale(scale).bold(true));
                f.draw(&mut frame, '\u{2603}', x, y, Pen::new(Rgb(255, 0, 0)).scale(scale));
            }
        }
    }

    #[test]
    fn bold_thickens_right_without_leaving_the_cell() {
        let f = BitmapFont::from_hex("tiny", TINY).unwrap();
        let mut frame = Frame::new(14, 2);
        frame.clear(Rgb(0, 0, 0));
        f.draw(&mut frame, 'B', 0, 0, Pen::new(Rgb(255, 255, 255)).bold(true));
        let row: Vec<bool> = (0..14).map(|x| frame.get(x, 0) == Some(Rgb(255, 255, 255))).collect();
        let lit: Vec<usize> = row.iter().enumerate().filter(|(_, &on)| on).map(|(i, _)| i).collect();
        assert_eq!(lit, vec![0, 1, 3, 4, 11]);
    }

    #[test]
    fn a_missing_character_still_takes_a_cell() {
        let f = BitmapFont::from_hex("u", &format!("0041:{}\n", "18".repeat(16))).unwrap();
        let mut frame = Frame::new(16, 16);
        frame.clear(Rgb(0, 0, 0));
        let w = f.draw(&mut frame, '\u{2603}', 0, 0, Pen::new(Rgb(255, 255, 255)));
        assert_eq!(w, 8);
        assert!(frame.pixels().contains(&0xFFFFFF));
        // Nothing of the box leaves the cell.
        assert!((0..16).all(|y| (8..16).all(|x| frame.get(x, y) == Some(Rgb(0, 0, 0)))));
    }

    #[test]
    fn coverage_draws_scaled_and_refuses_a_short_buffer() {
        let mut frame = Frame::new(8, 4);
        frame.clear(Rgb(0, 0, 0));
        let g = AlphaGlyph { width: 2, height: 1, left: 1, top: 1, coverage: vec![255, 128] };
        draw_alpha_glyph(&mut frame, &g, 0, 0, Rgb(255, 255, 255), 255);
        assert_eq!(frame.get(1, 1), Some(Rgb(255, 255, 255)));
        assert_eq!(frame.get(2, 1), Some(Rgb(128, 128, 128)));
        draw_coverage(&mut frame, &[255], (1, 1), (4, 0), 2, Rgb(255, 0, 0), 255);
        assert_eq!(frame.get(5, 1), Some(Rgb(255, 0, 0)));
        assert_eq!(frame.get(6, 1), Some(Rgb(0, 0, 0)));

        let before = frame.pixels().to_vec();
        let short = AlphaGlyph { width: 4, height: 4, left: 0, top: 0, coverage: vec![255; 3] };
        draw_alpha_glyph(&mut frame, &short, 0, 0, Rgb(9, 9, 9), 255);
        let huge = AlphaGlyph { width: u32::MAX, height: u32::MAX, left: i32::MAX, top: i32::MIN, coverage: vec![255; 8] };
        draw_alpha_glyph(&mut frame, &huge, i32::MAX, i32::MIN, Rgb(9, 9, 9), 255);
        assert!(!short.is_sound() && !huge.is_sound());
        assert_eq!(frame.pixels(), &before[..]);
    }
}
