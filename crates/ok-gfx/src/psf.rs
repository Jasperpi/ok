//! Reading PSF, the Linux console's binary font format, in both versions.
//!
//! These files come from anywhere, so every offset is checked before it is
//! used and every size is multiplied with overflow in mind: a file cut
//! short, or a gzip file renamed to `.psf`, is an error and never a panic.

use crate::font::{err, BitmapFont, FontError, MAX_CELL_H, MAX_CELL_W};

const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];
const PSF2_MAGIC: [u8; 4] = [0x72, 0xB5, 0x4A, 0x86];

pub(crate) fn looks_like_psf(bytes: &[u8]) -> bool {
    bytes.starts_with(&PSF1_MAGIC) || bytes.starts_with(&PSF2_MAGIC)
}

fn cut_short<T>() -> Result<T, FontError> {
    err(0, "the file is cut short")
}

fn le32(bytes: &[u8], at: usize) -> Result<u32, FontError> {
    match bytes.get(at..at + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => cut_short(),
    }
}

/// One UTF-8 character from the front of `bytes`, with its length.
fn utf8(bytes: &[u8]) -> Option<(char, usize)> {
    let len = match *bytes.first()? {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => return None,
    };
    let ch = std::str::from_utf8(bytes.get(..len)?).ok()?.chars().next()?;
    Some((ch, len))
}

impl BitmapFont {
    /// Reads a PC Screen Font, version 1 or 2.
    ///
    /// Glyphs are numbered, not named: the Unicode table at the end of the
    /// file says which characters each one draws. A file with no table can
    /// only be trusted for ASCII, since its upper half follows whatever code
    /// page it was made for, so nothing above U+007F is taken from it.
    /// Entries for sequences of several characters are skipped.
    ///
    /// Distributions ship these gzip-compressed; such a file is refused with
    /// a message saying so.
    pub fn from_psf(name: &str, bytes: &[u8]) -> Result<BitmapFont, FontError> {
        if bytes.starts_with(&[0x1F, 0x8B]) {
            return err(0, "this file is gzip-compressed; unpack it first");
        }
        if bytes.starts_with(&PSF2_MAGIC) {
            psf2(name, bytes)
        } else if bytes.starts_with(&PSF1_MAGIC) {
            psf1(name, bytes)
        } else {
            err(0, "not a PSF face")
        }
    }
}

/// Copies glyph `index` out of the bitmap block and returns its picture
/// number. `at` is where the block starts; the caller has checked that the
/// whole block is inside the file.
fn picture(font: &mut BitmapFont, bytes: &[u8], at: usize, index: usize, charsize: usize, rows: &mut [u32]) -> Result<u32, FontError> {
    let (width, height) = font.cell();
    let stride = width.div_ceil(8) as usize;
    let start = at + index * charsize;
    let Some(data) = bytes.get(start..start + charsize) else {
        return cut_short();
    };
    for (r, row) in rows.iter_mut().enumerate().take(height as usize) {
        let mut v = 0u32;
        for (b, &byte) in data[r * stride..(r + 1) * stride].iter().enumerate() {
            v |= (byte as u32) << (24 - 8 * b as u32);
        }
        *row = v;
    }
    Ok(font.add(width, rows))
}

fn psf2(name: &str, bytes: &[u8]) -> Result<BitmapFont, FontError> {
    if bytes.len() < 32 {
        return cut_short();
    }
    let header_size = le32(bytes, 8)? as usize;
    let flags = le32(bytes, 12)?;
    let length = le32(bytes, 16)? as usize;
    let charsize = le32(bytes, 20)? as usize;
    let height = le32(bytes, 24)?;
    let width = le32(bytes, 28)?;

    if !(1..=MAX_CELL_W).contains(&width) || !(1..=MAX_CELL_H).contains(&height) {
        return err(0, format!("unsupported cell size {width} by {height}"));
    }
    if charsize != (height * width.div_ceil(8)) as usize {
        return err(0, format!("{charsize} bytes per glyph does not fit a {width} by {height} cell"));
    }
    if header_size < 32 {
        return err(0, "the header claims to be shorter than a header");
    }
    if length == 0 {
        return err(0, "no glyphs found");
    }
    let table = length.checked_mul(charsize).and_then(|n| n.checked_add(header_size));
    let Some(table) = table.filter(|&end| end <= bytes.len()) else {
        return cut_short();
    };

    let mut font = BitmapFont::empty(name, width, height);
    let mut rows = vec![0u32; height as usize];

    if flags & 1 == 0 {
        for index in 0..length.min(0x80) {
            let id = picture(&mut font, bytes, header_size, index, charsize, &mut rows)?;
            font.map(index as u32, id);
        }
    } else {
        let mut at = table;
        for index in 0..length {
            let id = picture(&mut font, bytes, header_size, index, charsize, &mut rows)?;
            // Single characters first, then sequences introduced by 0xFE,
            // and 0xFF ends the entry.
            let mut in_sequence = false;
            loop {
                let Some(&b) = bytes.get(at) else {
                    return cut_short();
                };
                match b {
                    0xFF => {
                        at += 1;
                        break;
                    }
                    0xFE => {
                        in_sequence = true;
                        at += 1;
                    }
                    _ => {
                        let Some((ch, len)) = bytes.get(at..).and_then(utf8) else {
                            return err(0, format!("the Unicode table is damaged at byte {at}"));
                        };
                        if !in_sequence {
                            font.map(ch as u32, id);
                        }
                        at += len;
                    }
                }
            }
        }
    }

    if font.is_empty() {
        return err(0, "no glyphs found");
    }
    Ok(font.finish(None))
}

fn psf1(name: &str, bytes: &[u8]) -> Result<BitmapFont, FontError> {
    let (Some(&mode), Some(&charsize)) = (bytes.get(2), bytes.get(3)) else {
        return cut_short();
    };
    let height = charsize as u32;
    if !(1..=MAX_CELL_H).contains(&height) {
        return err(0, format!("unsupported cell size 8 by {height}"));
    }
    let charsize = charsize as usize;
    let length: usize = if mode & 0x01 != 0 { 512 } else { 256 };
    let table = 4 + length * charsize;
    if table > bytes.len() {
        return cut_short();
    }

    let mut font = BitmapFont::empty(name, 8, height);
    let mut rows = vec![0u32; height as usize];

    // Either flag means a table follows: 0x02 says so directly and 0x04
    // says the table also holds sequences.
    if mode & 0x06 == 0 {
        for index in 0..0x80 {
            let id = picture(&mut font, bytes, 4, index, charsize, &mut rows)?;
            font.map(index as u32, id);
        }
    } else {
        let mut at = table;
        for index in 0..length {
            let id = picture(&mut font, bytes, 4, index, charsize, &mut rows)?;
            let mut in_sequence = false;
            loop {
                let Some(pair) = bytes.get(at..at + 2) else {
                    return cut_short();
                };
                at += 2;
                match u16::from_le_bytes([pair[0], pair[1]]) {
                    0xFFFF => break,
                    0xFFFE => in_sequence = true,
                    unit => {
                        // A lone surrogate is not a character; such an entry is left out.
                        if let (false, Some(ch)) = (in_sequence, char::from_u32(unit as u32)) {
                            font.map(ch as u32, id);
                        }
                    }
                }
            }
        }
    }

    if font.is_empty() {
        return err(0, "no glyphs found");
    }
    Ok(font.finish(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PSF2 file: `glyphs` are 12 by 2, two bytes per row.
    fn psf2_file(glyphs: &[[u8; 4]], table: Option<&[u8]>) -> Vec<u8> {
        let mut f = PSF2_MAGIC.to_vec();
        for v in [0u32, 32, table.is_some() as u32, glyphs.len() as u32, 4, 2, 12] {
            f.extend(v.to_le_bytes());
        }
        for g in glyphs {
            f.extend(g);
        }
        f.extend(table.unwrap_or(&[]));
        f
    }

    #[test]
    fn psf2_with_a_table_maps_every_listed_character() {
        // Glyph 0 draws A; glyph 1 draws both apostrophes; glyph 2 draws
        // e-acute and, as a sequence, e followed by a combining acute.
        let table = [
            &b"A\xFF"[..],
            &b"'\xE2\x80\x99\xFF"[..],
            &b"\xC3\xA9\xFEe\xCC\x81\xFF"[..],
        ]
        .concat();
        let file = psf2_file(&[[0xFF, 0xFF, 0, 0], [0x80, 0x10, 0, 0], [0, 0, 0x12, 0x34]], Some(&table));
        let f = BitmapFont::from_psf("p", &file).unwrap();
        assert_eq!((f.cell(), f.len()), ((12, 2), 4));
        assert_eq!(f.glyph('A').unwrap().rows, &[0xFFF0_0000, 0]);
        assert_eq!(f.glyph('\'').unwrap().rows, &[0x8010_0000, 0]);
        assert_eq!(f.glyph('\u{2019}').unwrap().rows, &[0x8010_0000, 0]);
        assert_eq!(f.glyph('\u{00E9}').unwrap().rows, &[0, 0x1230_0000]);
        // The sequence did not make e or the bare accent draw as e-acute.
        assert!(!f.has('e') && !f.has('\u{0301}'));
        assert_eq!(BitmapFont::from_bytes("p", &file).unwrap().len(), 4);
    }

    #[test]
    fn psf2_without_a_table_is_ascii_only() {
        let glyphs: Vec<[u8; 4]> = (0..256u32).map(|i| [i as u8, 0, 0, 0]).collect();
        let f = BitmapFont::from_psf("p", &psf2_file(&glyphs, None)).unwrap();
        assert_eq!(f.len(), 128);
        assert_eq!(f.glyph('A').unwrap().rows[0], 0x4100_0000);
        assert!(!f.has('\u{00E9}'));
        assert!(f.glyph('\n').is_none() && f.raw_glyph(0x0A).is_some());
    }

    #[test]
    fn psf2_header_may_be_longer_than_32_bytes() {
        let mut file = psf2_file(&[[0xFF, 0xF0, 0, 0]], Some(b"A\xFF"));
        file[8] = 40;
        file.splice(32..32, [0u8; 8]);
        let f = BitmapFont::from_psf("p", &file).unwrap();
        assert_eq!(f.glyph('A').unwrap().rows, &[0xFFF0_0000, 0]);
    }

    fn psf1_file(mode: u8, table: &[u16]) -> Vec<u8> {
        let count = if mode & 1 != 0 { 512 } else { 256 };
        let mut f = vec![0x36, 0x04, mode, 3];
        for i in 0..count {
            f.extend([i as u8, (i >> 8) as u8, 0xFF]);
        }
        for unit in table {
            f.extend(unit.to_le_bytes());
        }
        f
    }

    #[test]
    fn psf1_reads_256_or_512_glyphs() {
        let f = BitmapFont::from_psf("p", &psf1_file(0, &[])).unwrap();
        assert_eq!((f.cell(), f.len()), ((8, 3), 128));
        assert_eq!(f.glyph('A').unwrap().rows, &[0x4100_0000, 0, 0xFF00_0000]);

        // Glyph 0 draws A and B, glyph 1 a sequence only, glyph 2 a lone surrogate.
        let mut table = vec![0x41, 0x42, 0xFFFF, 0xFFFE, 0x43, 0x301, 0xFFFF, 0xD800, 0xFFFF];
        table.extend(std::iter::repeat_n(0xFFFF, 509));
        for mode in [0x03, 0x05] {
            let f = BitmapFont::from_psf("p", &psf1_file(mode, &table)).unwrap();
            assert_eq!(f.code_points(), vec![0x41, 0x42], "mode {mode}");
            assert_eq!(f.glyph('B').unwrap().rows, &[0, 0, 0xFF00_0000]);
        }
    }

    #[test]
    fn gzip_is_refused_by_name() {
        let e = BitmapFont::from_psf("p", &[0x1F, 0x8B, 8, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(e.what.contains("gzip"), "{e}");
        assert!(BitmapFont::from_bytes("p", &[0x1F, 0x8B, 8, 0]).unwrap_err().what.contains("gzip"));
    }

    #[test]
    fn every_truncation_is_an_error() {
        let table = [&b"A\xFF"[..], &b"B\xFEe\xCC\x81\xFF"[..]].concat();
        let with_table = psf2_file(&[[0xFF; 4], [0x0F; 4]], Some(&table));
        let without = psf2_file(&[[0xFF; 4]; 3], None);
        let mut units = vec![0x41u16, 0xFFFF];
        units.extend(std::iter::repeat_n(0xFFFF, 255));
        let one = psf1_file(0x02, &units);
        let bare = psf1_file(0, &[]);
        for file in [&with_table, &without, &one, &bare] {
            assert!(BitmapFont::from_psf("p", file).is_ok());
            for cut in 0..file.len() {
                assert!(BitmapFont::from_psf("p", &file[..cut]).is_err(), "cut at {cut} of {}", file.len());
            }
        }
    }

    #[test]
    fn lying_headers_are_refused() {
        let good = psf2_file(&[[0xFF; 4]; 2], Some(b"A\xFFB\xFF"));
        let set = |at: usize, v: u32| {
            let mut f = good.clone();
            f[at..at + 4].copy_from_slice(&v.to_le_bytes());
            f
        };
        for (at, v) in [
            (8, 0),
            (8, 31),
            (8, u32::MAX),
            (16, 0),
            (16, u32::MAX),
            (16, 0x4000_0000),
            (20, 0),
            (20, 5),
            (20, u32::MAX),
            (24, 0),
            (24, 65),
            (24, u32::MAX),
            (28, 0),
            (28, 33),
            (28, u32::MAX),
        ] {
            assert!(BitmapFont::from_psf("p", &set(at, v)).is_err(), "field at {at} set to {v}");
        }
        // A table byte that is neither UTF-8 nor a marker.
        let mut f = good.clone();
        let last = f.len() - 4;
        f[last] = 0x80;
        assert!(BitmapFont::from_psf("p", &f).is_err());
        assert!(BitmapFont::from_psf("p", &[]).is_err());
        assert!(BitmapFont::from_psf("p", b"STARTFONT 2.1").is_err());
        assert!(BitmapFont::from_psf("p", &[0x36, 0x04, 0, 0]).is_err());
        assert!(BitmapFont::from_psf("p", &[0x36, 0x04, 0, 200]).is_err());
    }
}
