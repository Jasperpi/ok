//! The two things the shell has to read out of a TrueType or OpenType file
//! itself, because GDI will not say: a face's family name before it is
//! loaded, and whether it has a glyph for a character above U+FFFF.
//!
//! Font files come from the user's `faces` folder, so every read is bounds
//! checked and a malformed file yields `None`, never a panic.

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Finds a table in a font file and returns its bytes. For a collection
/// (`.ttc`) the first face is used.
fn table<'a>(file: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let mut base = 0usize;
    if file.get(0..4)? == b"ttcf" {
        if u32_at(file, 8)? == 0 {
            return None;
        }
        base = u32_at(file, 12)? as usize;
    }
    let count = u16_at(file, base.checked_add(4)?)? as usize;
    for i in 0..count {
        let rec = base.checked_add(12)?.checked_add(i.checked_mul(16)?)?;
        if file.get(rec..rec.checked_add(4)?)? == tag {
            let offset = u32_at(file, rec + 8)? as usize;
            let length = u32_at(file, rec + 12)? as usize;
            return file.get(offset..offset.checked_add(length)?);
        }
    }
    None
}

/// The family name GDI will know a font file by: name 1 of its `name` table,
/// in the Windows English form when there is one.
pub(crate) fn family_name(file: &[u8]) -> Option<String> {
    family_from_name_table(table(file, b"name")?)
}

fn family_from_name_table(name: &[u8]) -> Option<String> {
    let count = u16_at(name, 2)? as usize;
    let strings = u16_at(name, 4)? as usize;
    // Best first: Windows Unicode in US English, any Windows Unicode, then the
    // old Macintosh Roman record that some faces carry alone.
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let rec = 6usize.checked_add(i.checked_mul(12)?)?;
        let platform = u16_at(name, rec)?;
        let encoding = u16_at(name, rec + 2)?;
        let language = u16_at(name, rec + 4)?;
        let name_id = u16_at(name, rec + 6)?;
        let length = u16_at(name, rec + 8)? as usize;
        let offset = u16_at(name, rec + 10)? as usize;
        if name_id != 1 {
            continue;
        }
        let rank = match (platform, encoding, language) {
            (3, 0 | 1 | 10, 0x0409) => 3,
            (3, 0 | 1 | 10, _) => 2,
            (1, 0, _) => 1,
            _ => continue,
        };
        if best.as_ref().is_some_and(|(r, _)| *r >= rank) {
            continue;
        }
        let start = strings.checked_add(offset)?;
        let Some(raw) = name.get(start..start.checked_add(length)?) else { continue };
        let text = if platform == 3 {
            let units: Vec<u16> = raw.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
            String::from_utf16_lossy(&units)
        } else {
            raw.iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect()
        };
        let text = text.trim().to_string();
        if !text.is_empty() {
            best = Some((rank, text));
        }
    }
    best.map(|(_, s)| s)
}

/// Looks a character up in a `cmap` table and returns its glyph index, or
/// `None` when the face has no glyph for it. Only the full-Unicode subtable
/// formats (12 and 13) are read, since this exists for characters above
/// U+FFFF; GDI answers for the rest.
pub(crate) fn cmap_glyph(cmap: &[u8], ch: char) -> Option<u32> {
    let cp = ch as u32;
    let count = u16_at(cmap, 2)? as usize;
    for i in 0..count {
        let rec = 4usize.checked_add(i.checked_mul(8)?)?;
        let platform = u16_at(cmap, rec)?;
        let encoding = u16_at(cmap, rec + 2)?;
        let full_unicode = matches!((platform, encoding), (3, 10) | (0, 4) | (0, 6));
        if !full_unicode {
            continue;
        }
        let Some(sub) = cmap.get(u32_at(cmap, rec + 4)? as usize..) else { continue };
        let format = u16_at(sub, 0)?;
        if format != 12 && format != 13 {
            continue;
        }
        let groups = u32_at(sub, 12)? as usize;
        // Groups are sorted by their first character: binary search.
        let (mut lo, mut hi) = (0usize, groups);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let g = 16usize.checked_add(mid.checked_mul(12)?)?;
            let first = u32_at(sub, g)?;
            let last = u32_at(sub, g + 4)?;
            if cp < first {
                hi = mid;
            } else if cp > last {
                lo = mid + 1;
            } else {
                let start = u32_at(sub, g + 8)?;
                let glyph = if format == 12 { start.checked_add(cp - first)? } else { start };
                return if glyph == 0 { None } else { Some(glyph) };
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }
    fn be32(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }

    /// A font file holding nothing but the one table given.
    fn file_with(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&be32(0x0001_0000));
        f.extend_from_slice(&be16(1));
        f.extend_from_slice(&[0; 6]);
        f.extend_from_slice(tag);
        f.extend_from_slice(&be32(0));
        f.extend_from_slice(&be32(28));
        f.extend_from_slice(&be32(body.len() as u32));
        f.extend_from_slice(body);
        f
    }

    fn name_table(records: &[(u16, u16, u16, u16, &[u8])]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(&be16(0));
        t.extend_from_slice(&be16(records.len() as u16));
        t.extend_from_slice(&be16(6 + 12 * records.len() as u16));
        let mut offset = 0u16;
        for (platform, encoding, language, id, text) in records {
            for v in [*platform, *encoding, *language, *id, text.len() as u16, offset] {
                t.extend_from_slice(&be16(v));
            }
            offset += text.len() as u16;
        }
        for (.., text) in records {
            t.extend_from_slice(text);
        }
        t
    }

    fn utf16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    #[test]
    fn prefers_the_windows_english_family_name() {
        let german = utf16be("Schreibmaschine");
        let english = utf16be("Typewriter");
        let style = utf16be("Regular");
        let table = name_table(&[
            (1, 0, 0, 1, b"Mac Name"),
            (3, 1, 0x0407, 1, &german),
            (3, 1, 0x0409, 2, &style),
            (3, 1, 0x0409, 1, &english),
        ]);
        assert_eq!(family_name(&file_with(b"name", &table)).as_deref(), Some("Typewriter"));
    }

    #[test]
    fn falls_back_to_other_languages_and_to_the_macintosh_record() {
        let german = utf16be("Schreibmaschine");
        let only_german = name_table(&[(3, 1, 0x0407, 1, &german)]);
        assert_eq!(family_name(&file_with(b"name", &only_german)).as_deref(), Some("Schreibmaschine"));
        let only_mac = name_table(&[(1, 0, 0, 1, b"Mac Name")]);
        assert_eq!(family_name(&file_with(b"name", &only_mac)).as_deref(), Some("Mac Name"));
    }

    #[test]
    fn damaged_files_give_none() {
        assert_eq!(family_name(&[]), None);
        assert_eq!(family_name(b"ttcf"), None);
        assert_eq!(family_name(&[0xFF; 64]), None);
        let english = utf16be("Typewriter");
        let good = file_with(b"name", &name_table(&[(3, 1, 0x0409, 1, &english)]));
        for cut in 0..good.len() {
            let _ = family_name(&good[..cut]);
        }
        assert_eq!(family_name(&file_with(b"glyf", &[1, 2, 3])), None);
    }

    fn cmap12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(&be16(0));
        t.extend_from_slice(&be16(1));
        t.extend_from_slice(&be16(3));
        t.extend_from_slice(&be16(10));
        t.extend_from_slice(&be32(12));
        t.extend_from_slice(&be16(12));
        t.extend_from_slice(&be16(0));
        t.extend_from_slice(&be32(16 + 12 * groups.len() as u32));
        t.extend_from_slice(&be32(0));
        t.extend_from_slice(&be32(groups.len() as u32));
        for (first, last, glyph) in groups {
            t.extend_from_slice(&be32(*first));
            t.extend_from_slice(&be32(*last));
            t.extend_from_slice(&be32(*glyph));
        }
        t
    }

    #[test]
    fn finds_characters_above_the_basic_plane() {
        let t = cmap12(&[(0x41, 0x5A, 10), (0x1F600, 0x1F64F, 500), (0x20000, 0x2A6DF, 9000)]);
        assert_eq!(cmap_glyph(&t, 'A'), Some(10));
        assert_eq!(cmap_glyph(&t, 'Z'), Some(35));
        assert_eq!(cmap_glyph(&t, '\u{1F600}'), Some(500));
        assert_eq!(cmap_glyph(&t, '\u{1F64F}'), Some(579));
        assert_eq!(cmap_glyph(&t, '\u{1F650}'), None);
        assert_eq!(cmap_glyph(&t, '\u{20001}'), Some(9001));
        assert_eq!(cmap_glyph(&t, 'a'), None);
    }

    #[test]
    fn a_group_that_maps_to_glyph_zero_is_a_missing_character() {
        let t = cmap12(&[(0x1F600, 0x1F600, 0)]);
        assert_eq!(cmap_glyph(&t, '\u{1F600}'), None);
    }

    #[test]
    fn a_damaged_cmap_gives_none() {
        let good = cmap12(&[(0x1F600, 0x1F64F, 500)]);
        for cut in 0..good.len() {
            let _ = cmap_glyph(&good[..cut], '\u{1F600}');
        }
        assert_eq!(cmap_glyph(&[], 'A'), None);
        assert_eq!(cmap_glyph(&[0xFF; 40], '\u{1F600}'), None);
    }
}
