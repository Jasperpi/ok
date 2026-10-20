//! The three faces inside the binary. Each is read on first use and kept.

use std::sync::OnceLock;

use crate::font::BitmapFont;

/// `gallant.hex`, checked and packed by the build script.
static GALLANT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gallant.bin"));

static SPLEEN: &str = include_str!("../../../assets/fonts/spleen/spleen-12x24.bdf");

/// Terminus is kept as the exact bytes of its BDF and read at run time. Its
/// licence reserves the name "Terminus Font" for unmodified copies, and a
/// table converted at build time would be a modified one.
static TERMINUS: &str = include_str!("../../../assets/fonts/terminus/ter-u22n.bdf");

/// A face that failed to load is replaced by one with no glyphs, which
/// draws every character as its hex box: plainly wrong on screen, and the
/// program keeps running. The embedded data is checked by the build and by
/// the tests, so this is not expected to happen.
fn or_empty(name: &str, cell: (u32, u32), loaded: Result<BitmapFont, crate::FontError>) -> BitmapFont {
    loaded.unwrap_or_else(|_| BitmapFont::empty(name, cell.0, cell.1).finish(None))
}

/// Gallant 12 by 22, the Sun console face and the default: 5,320 glyphs.
/// It is also what every other face borrows from for a character it lacks.
pub fn gallant() -> &'static BitmapFont {
    static FACE: OnceLock<BitmapFont> = OnceLock::new();
    FACE.get_or_init(|| or_empty("Gallant", (12, 22), BitmapFont::from_packed("Gallant", GALLANT)))
}

/// Spleen 12 by 24, OpenBSD's console face: 950 glyphs.
pub fn spleen() -> &'static BitmapFont {
    static FACE: OnceLock<BitmapFont> = OnceLock::new();
    FACE.get_or_init(|| or_empty("Spleen", (12, 24), BitmapFont::from_bdf("Spleen", SPLEEN)))
}

/// Terminus 11 by 22: 1,356 glyphs.
pub fn terminus() -> &'static BitmapFont {
    static FACE: OnceLock<BitmapFont> = OnceLock::new();
    FACE.get_or_init(|| or_empty("Terminus", (11, 22), BitmapFont::from_bdf("Terminus", TERMINUS)))
}

/// The licence of every face in the binary, as (heading, full text). Each
/// licence asks that its notice travel with every copy, and the copy that
/// travels is a lone .exe, so the notices screen prints these.
pub fn face_notices() -> &'static [(&'static str, &'static str)] {
    &[
        ("Gallant, Unicode extension", include_str!("../../../assets/fonts/gallant/LICENSE")),
        ("Gallant, as shipped in FreeBSD", include_str!("../../../assets/fonts/gallant/LICENSE.FreeBSD")),
        ("Gallant, the original Sun console face in 4.4BSD", include_str!("../../../assets/fonts/gallant/LICENSE.Berkeley")),
        ("Spleen", include_str!("../../../assets/fonts/spleen/LICENSE")),
        ("Terminus Font", include_str!("../../../assets/fonts/terminus/OFL.TXT")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminus_is_embedded_byte_for_byte() {
        let on_disk = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/terminus/ter-u22n.bdf")).unwrap();
        assert_eq!(TERMINUS.as_bytes(), &on_disk[..]);
        assert_eq!(on_disk.len(), 267_801);
    }

    #[test]
    fn every_embedded_face_carries_its_notice() {
        let notices = face_notices();
        let all: String = notices.iter().map(|(_, text)| *text).collect();
        for holder in ["Jens Schweikhardt", "The FreeBSD Project", "Regents of the University of California", "Frederic Cambus", "Dimitar Toshkov Zhekov"] {
            assert!(all.contains(holder), "{holder}");
        }
        assert!(all.contains("Reserved Font Name"));
    }
}
