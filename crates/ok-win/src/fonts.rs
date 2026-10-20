//! Faces Windows knows, drawn through GDI into coverage the grid can use.
//!
//! Two routes. An outline face gives each glyph's coverage straight from
//! `GetGlyphOutlineW`. A raster face (Terminal, Fixedsys) has no outlines, so
//! its text is drawn with `ExtTextOutW` into a private bitmap and read back.
//!
//! GDI never says no: asked for a face that is missing it selects another,
//! and asked to draw a character the face lacks it borrows one from a linked
//! face. Both are checked here before anything is drawn, so a `None` from
//! this module always means "this face does not have that".

use std::collections::HashMap;
use std::path::Path;
use std::ptr::null_mut;

use ok_gfx::AlphaGlyph;
use ok_host::FaceCell;

use crate::sfnt;
use crate::win32::*;

/// Heights outside this range are refused: nothing on a cell grid is drawn
/// smaller or larger, and it keeps a wild setting from allocating a huge bitmap.
const HEIGHTS: std::ops::RangeInclusive<u32> = 4..=400;

/// Glyphs kept before the store is emptied and refilled. A page of prose uses
/// a few hundred.
const GLYPHS_KEPT: usize = 8192;

struct Face {
    font: HFONT,
    cell: FaceCell,
    /// False for a raster face, where `GetGlyphOutlineW` always fails.
    outlines: bool,
    /// The face's own character map, read the first time a character above
    /// U+FFFF is asked for. `Some(None)` means it was tried and there is none.
    cmap: Option<Option<Vec<u8>>>,
}

pub(crate) struct Faces {
    dc: HDC,
    by_name: HashMap<(String, u32), Option<usize>>,
    faces: Vec<Face>,
    glyphs: HashMap<(usize, char), Option<AlphaGlyph>>,
    raster: Option<TextRaster>,
    /// Font files added for this process only, to be removed again on drop.
    files: Vec<Vec<u16>>,
}

impl Faces {
    pub(crate) fn new() -> Faces {
        Faces { dc: 0, by_name: HashMap::new(), faces: Vec::new(), glyphs: HashMap::new(), raster: None, files: Vec::new() }
    }

    /// Family names of the installed fixed-pitch faces, raster ones included,
    /// together with any loaded from a file.
    pub(crate) fn system_faces(&mut self) -> Vec<String> {
        fixed_pitch_families().into_iter().map(|f| f.name).collect()
    }

    /// Adds a font file for this process alone and returns the family name GDI
    /// now knows it by.
    pub(crate) fn load_file(&mut self, path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let name = sfnt::family_name(&bytes)?;
        // A LOGFONT holds 31 characters, so that is all GDI keeps of a name.
        let units: Vec<u16> = name.encode_utf16().take(LF_FACESIZE - 1).collect();
        let name = String::from_utf16_lossy(&units);
        let wide = wide_path(path);
        if !self.files.contains(&wide) {
            let added = unsafe { AddFontResourceExW(wide.as_ptr(), FR_PRIVATE, null_mut()) };
            if added <= 0 {
                return None;
            }
            self.files.push(wide);
            // A family that was missing a moment ago may exist now.
            self.by_name.retain(|_, found| found.is_some());
        }
        Some(name)
    }

    pub(crate) fn cell(&mut self, family: &str, height: u32) -> Option<FaceCell> {
        let id = self.face(family, height)?;
        Some(self.faces[id].cell)
    }

    /// One character as coverage inside its cell (or two cells, for a
    /// character the face draws double width). `None` when the face has no
    /// glyph of its own for it.
    pub(crate) fn rasterize(&mut self, family: &str, height: u32, ch: char) -> Option<AlphaGlyph> {
        let id = self.face(family, height)?;
        if let Some(known) = self.glyphs.get(&(id, ch)) {
            return known.clone();
        }
        let drawn = self.draw(id, ch);
        if self.glyphs.len() >= GLYPHS_KEPT {
            self.glyphs.clear();
        }
        self.glyphs.insert((id, ch), drawn.clone());
        drawn
    }

    fn memory_dc(&mut self) -> Option<HDC> {
        if self.dc == 0 {
            self.dc = unsafe { CreateCompatibleDC(0) };
        }
        (self.dc != 0).then_some(self.dc)
    }

    fn face(&mut self, family: &str, height: u32) -> Option<usize> {
        let family = family.trim();
        let too_long = family.encode_utf16().count() >= LF_FACESIZE;
        if family.is_empty() || too_long || family.contains('\0') || !HEIGHTS.contains(&height) {
            return None;
        }
        let key = (family.to_lowercase(), height);
        if let Some(&found) = self.by_name.get(&key) {
            return found;
        }
        let made = self.make_face(family, height).map(|face| {
            self.faces.push(face);
            self.faces.len() - 1
        });
        self.by_name.insert(key, made);
        made
    }

    /// Asks GDI for the face and keeps it only if GDI really selected that
    /// family and it is fixed pitch.
    fn make_face(&mut self, family: &str, height: u32) -> Option<Face> {
        let dc = self.memory_dc()?;
        unsafe {
            let font = CreateFontW(
                height as i32, 0, 0, 0, FW_NORMAL, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY, FIXED_PITCH | FF_DONTCARE, wide(family).as_ptr(),
            );
            if font == 0 {
                return None;
            }
            let old = SelectObject(dc, font);
            let selected = font_face_name(dc);
            let metrics = text_metrics(dc);
            let (mut wide_m, mut narrow_i) = (0i32, 0i32);
            let measured = GetCharWidth32W(dc, 'M' as u32, 'M' as u32, &mut wide_m) != 0
                && GetCharWidth32W(dc, 'i' as u32, 'i' as u32, &mut narrow_i) != 0;
            SelectObject(dc, old);

            let face = metrics.filter(|_| selected.to_lowercase() == family.to_lowercase()).and_then(|tm| {
                // The bit is named for the opposite of what it means: set is variable pitch.
                let fixed = tm.tmPitchAndFamily & TMPF_FIXED_PITCH == 0 && measured && wide_m == narrow_i;
                if !fixed || wide_m <= 0 || tm.tmHeight <= 0 || tm.tmAscent < 0 {
                    return None;
                }
                Some(Face {
                    font,
                    cell: FaceCell { width: wide_m as u32, height: tm.tmHeight as u32, ascent: tm.tmAscent as u32 },
                    outlines: tm.tmPitchAndFamily & (TMPF_VECTOR | TMPF_TRUETYPE) != 0,
                    cmap: None,
                })
            });
            if face.is_none() {
                DeleteObject(font);
            }
            face
        }
    }

    fn draw(&mut self, id: usize, ch: char) -> Option<AlphaGlyph> {
        let code = ch as u32;
        if !is_text_char(ch) {
            return None;
        }
        let dc = self.memory_dc()?;
        let (font, cell, outlines) = {
            let f = &self.faces[id];
            (f.font, f.cell, f.outlines)
        };
        let mut pair = [0u16; 2];
        let units: &[u16] = ch.encode_utf16(&mut pair);

        let old = unsafe { SelectObject(dc, font) };
        // Which glyph, and whether the face has one at all.
        let outline = if code <= 0xFFFF {
            if font_has_glyph(dc, code as u16) {
                Some(if outlines { glyph_outline(dc, code, GGO_GRAY8_BITMAP) } else { None })
            } else {
                None
            }
        } else {
            let face = &mut self.faces[id];
            let cmap = face.cmap.get_or_insert_with(|| read_cmap(dc));
            cmap.as_deref().and_then(|table| sfnt::cmap_glyph(table, ch)).map(|glyph| {
                if outlines { glyph_outline(dc, glyph, GGO_GRAY8_BITMAP | GGO_GLYPH_INDEX) } else { None }
            })
        };
        let mut advance = SIZE::default();
        unsafe {
            GetTextExtentPoint32W(dc, units.as_ptr(), units.len() as i32, &mut advance);
            SelectObject(dc, old);
        }

        match outline? {
            Some(bitmap) => Some(place_outline(&bitmap, cell)),
            None => self.draw_as_text(font, units, cell, advance.cx),
        }
    }

    /// The raster route: GDI draws the character and the coverage is read
    /// back out of the bitmap.
    fn draw_as_text(&mut self, font: HFONT, units: &[u16], cell: FaceCell, advance: i32) -> Option<AlphaGlyph> {
        let (need_w, need_h) = (cell.width as i32 * 2, cell.height as i32);
        if self.raster.as_ref().is_none_or(|r| r.w < need_w || r.h < need_h) {
            let (w, h) = self.raster.as_ref().map_or((0, 0), |r| (r.w, r.h));
            self.raster = None;
            self.raster = TextRaster::new(w.max(need_w), h.max(need_h));
        }
        let raster = self.raster.as_mut()?;
        let coverage = raster.draw(font, units);
        let stride = raster.w as usize;
        let box_w = (cell.width as usize * cells_for(advance, cell)).min(stride);
        let box_h = (cell.height as usize).min(raster.h as usize);
        Some(crop(box_w, box_h, |x, y| coverage.get(y * stride + x).copied().unwrap_or(0)))
    }
}

impl Drop for Faces {
    fn drop(&mut self) {
        self.raster = None;
        unsafe {
            for face in &self.faces {
                DeleteObject(face.font);
            }
            if self.dc != 0 {
                DeleteDC(self.dc);
            }
            for file in &self.files {
                RemoveFontResourceExW(file.as_ptr(), FR_PRIVATE, null_mut());
            }
        }
    }
}

/// How many cells a glyph with this advance takes: two for the wide
/// characters of a face such as MS Gothic, otherwise one.
fn cells_for(advance: i32, cell: FaceCell) -> usize {
    if advance > cell.width as i32 {
        2
    } else {
        1
    }
}

/// The `cmap` table of the face selected into `dc`, when it is an sfnt face.
fn read_cmap(dc: HDC) -> Option<Vec<u8>> {
    unsafe {
        let size = GetFontData(dc, TAG_CMAP, 0, null_mut(), 0);
        if size == GDI_ERROR || size == 0 || size > 16 << 20 {
            return None;
        }
        let mut table = vec![0u8; size as usize];
        let read = GetFontData(dc, TAG_CMAP, 0, table.as_mut_ptr() as *mut _, size);
        (read == size).then_some(table)
    }
}

/// Puts an outline glyph where it belongs in its cell and cuts off anything
/// that falls outside, so a glyph never bleeds into its neighbours.
fn place_outline(g: &GlyphBitmap, cell: FaceCell) -> AlphaGlyph {
    let box_w = (cell.width as usize * cells_for(g.advance, cell)) as i64;
    let box_h = cell.height as i64;
    let (left, top) = (g.left as i64, cell.ascent as i64 - g.top as i64);
    let (x0, y0) = (left.max(0), top.max(0));
    let (x1, y1) = ((left + g.w as i64).min(box_w), (top + g.h as i64).min(box_h));
    if x1 <= x0 || y1 <= y0 {
        return AlphaGlyph::default();
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut coverage = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = (y as i64 + y0 - top) as usize * g.pitch;
        for x in 0..w {
            // GDI's grey levels run 0 to 64.
            let v = g.data.get(row + (x as i64 + x0 - left) as usize).copied().unwrap_or(0).min(64) as u32;
            coverage.push(((v * 255 + 32) / 64) as u8);
        }
    }
    AlphaGlyph { width: w as u32, height: h as u32, left: x0 as i32, top: y0 as i32, coverage }
}

/// Cuts a `box_w` by `box_h` field of coverage down to the rectangle that
/// holds ink. A field with no ink (a space) gives an empty glyph.
fn crop(box_w: usize, box_h: usize, at: impl Fn(usize, usize) -> u8) -> AlphaGlyph {
    let (mut x0, mut y0, mut x1, mut y1) = (box_w, box_h, 0usize, 0usize);
    for y in 0..box_h {
        for x in 0..box_w {
            if at(x, y) != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return AlphaGlyph::default();
    }
    let mut coverage = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        for x in x0..x1 {
            coverage.push(at(x, y));
        }
    }
    AlphaGlyph { width: (x1 - x0) as u32, height: (y1 - y0) as u32, left: x0 as i32, top: y0 as i32, coverage }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside_cell(g: &AlphaGlyph, cell: FaceCell, cells: u32) -> bool {
        g.left >= 0
            && g.top >= 0
            && g.left as u32 + g.width <= cell.width * cells
            && g.top as u32 + g.height <= cell.height
            && g.coverage.len() == (g.width * g.height) as usize
    }

    /// The reason every glyph is checked before it is drawn: left to itself,
    /// GDI's text output fills in a missing character from some other face.
    #[test]
    fn gdi_text_output_borrows_glyphs_which_is_why_it_is_never_asked_first() {
        let mut faces = Faces::new();
        let id = faces.face("Consolas", 22).expect("Consolas is on every Windows");
        let (font, cell) = (faces.faces[id].font, faces.faces[id].cell);
        let borrowed = faces.draw_as_text(font, &[0x2713], cell, cell.width as i32).expect("a bitmap");
        assert!(borrowed.width > 0, "GDI drew a check mark Consolas does not have");
        assert!(faces.rasterize("Consolas", 22, '\u{2713}').is_none());
    }

    #[test]
    fn a_raster_face_goes_through_the_text_route() {
        let mut faces = Faces::new();
        let Some(cell) = faces.cell("Terminal", 12) else { return };
        let id = faces.face("Terminal", 12).expect("just measured");
        assert!(!faces.faces[id].outlines);
        let g = faces.rasterize("Terminal", 12, 'g').expect("g");
        assert!(inside_cell(&g, cell, 1), "{g:?}");
        assert!(g.width > 0 && g.coverage.iter().all(|&v| v == 0 || v == 255), "a raster face is one bit deep");
        assert!(faces.rasterize("Terminal", 12, '\u{4E2D}').is_none(), "Terminal has no CJK and must not borrow one");
        assert_eq!(faces.rasterize("Terminal", 12, ' ').map(|g| g.width), Some(0));
    }

    #[test]
    fn a_character_above_ffff_is_found_through_the_faces_own_map() {
        let mut faces = Faces::new();
        let Some(cell) = faces.cell("SimSun-ExtB", 22) else { return };
        let g = faces.rasterize("SimSun-ExtB", 22, '\u{20000}').expect("CJK extension B is what this face is for");
        assert!(g.width > cell.width, "an ideograph is two cells wide");
        assert!(inside_cell(&g, cell, 2), "{g:?}");
        assert!(faces.rasterize("SimSun-ExtB", 22, '\u{1F600}').is_none());
    }

    #[test]
    fn consolas_has_the_cell_the_probe_measured() {
        let mut faces = Faces::new();
        assert_eq!(faces.cell("Consolas", 22), Some(FaceCell { width: 10, height: 22, ascent: 17 }));
        assert_eq!(faces.cell("consolas", 22), faces.cell("Consolas", 22), "family names are not case sensitive");
    }

    #[test]
    fn a_missing_or_proportional_face_has_no_cell() {
        let mut faces = Faces::new();
        assert_eq!(faces.cell("No Such Face d13c71", 22), None, "GDI would have substituted Courier New");
        assert_eq!(faces.cell("Arial", 22), None);
        assert_eq!(faces.cell("", 22), None);
        assert_eq!(faces.cell("Consolas", 0), None);
        assert_eq!(faces.cell("Consolas", 100_000), None);
        assert_eq!(faces.cell(&"x".repeat(200), 22), None);
        assert_eq!(faces.cell("Con\0solas", 22), None);
    }

    #[test]
    fn an_outline_glyph_is_coverage_inside_its_cell() {
        let mut faces = Faces::new();
        let cell = faces.cell("Consolas", 22).expect("Consolas is on every Windows");
        let g = faces.rasterize("Consolas", 22, 'g').expect("g");
        assert!(inside_cell(&g, cell, 1), "{g:?}");
        assert!(g.coverage.contains(&255), "full coverage reaches 255");
        assert!(g.coverage.iter().any(|&v| v > 0 && v < 255), "edges are anti-aliased");
        // A descender: ink below the baseline.
        assert!(g.top as u32 + g.height > cell.ascent);
        let again = faces.rasterize("Consolas", 22, 'g').expect("g again");
        assert_eq!(again.coverage, g.coverage);
    }

    #[test]
    fn a_space_is_a_glyph_with_no_ink() {
        let mut faces = Faces::new();
        let g = faces.rasterize("Consolas", 22, ' ').expect("a space is in the face");
        assert_eq!((g.width, g.height), (0, 0));
        assert!(g.coverage.is_empty());
    }

    #[test]
    fn a_character_the_face_lacks_is_none_not_a_borrowed_glyph() {
        let mut faces = Faces::new();
        assert!(faces.rasterize("Consolas", 22, '\u{2713}').is_none(), "Consolas has no check mark");
        assert!(faces.rasterize("Consolas", 22, '\u{4E2D}').is_none(), "nor CJK");
        assert!(faces.rasterize("Consolas", 22, '\u{1F600}').is_none(), "nor emoji");
        assert!(faces.rasterize("Consolas", 22, '\u{0007}').is_none(), "control characters have no glyph");
        assert!(faces.rasterize("No Such Face d13c71", 22, 'g').is_none());
    }

    #[test]
    fn crop_finds_the_ink() {
        let field = [0u8, 0, 0, 0, 0, 9, 7, 0, 0, 0, 5, 0];
        let g = crop(4, 3, |x, y| field[y * 4 + x]);
        assert_eq!((g.left, g.top, g.width, g.height), (1, 1, 2, 2));
        assert_eq!(g.coverage, vec![9, 7, 0, 5]);
        let none = crop(4, 3, |_, _| 0);
        assert_eq!((none.width, none.height), (0, 0));
    }

    #[test]
    fn an_outline_that_overhangs_is_cut_to_the_cell() {
        let cell = FaceCell { width: 4, height: 6, ascent: 4 };
        // A 6 by 8 glyph starting one pixel left of the cell and two above it.
        let g = GlyphBitmap { w: 6, h: 8, pitch: 8, left: -1, top: 6, advance: 4, data: vec![64; 64] };
        let placed = place_outline(&g, cell);
        assert_eq!((placed.left, placed.top, placed.width, placed.height), (0, 0, 4, 6));
        assert!(placed.coverage.iter().all(|&v| v == 255));
        // Wholly outside: nothing.
        let off = GlyphBitmap { w: 2, h: 2, pitch: 4, left: 40, top: 0, advance: 4, data: vec![64; 8] };
        assert_eq!(place_outline(&off, cell).width, 0);
        // A short buffer does not panic.
        let short = GlyphBitmap { w: 4, h: 4, pitch: 4, left: 0, top: 4, advance: 4, data: vec![64; 3] };
        assert_eq!(place_outline(&short, cell).coverage.len(), 16);
    }
}
