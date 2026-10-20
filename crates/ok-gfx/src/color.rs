/// An sRGB colour.
///
/// A frame stores each pixel as `0x00RRGGBB`. In memory on a little-endian
/// machine that is the byte order B, G, R, X, which is exactly what a 32-bit
/// Win32 DIB expects, so a frame is blitted to the window with no conversion.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn px(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }

    pub const fn from_px(p: u32) -> Rgb {
        Rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
    }

    /// Reads `#rrggbb`, `rrggbb`, `#rgb` or `rgb`.
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim();
        let s = s.strip_prefix('#').unwrap_or(s);
        if !s.is_ascii() {
            return None;
        }
        let nib = |i: usize| (s.as_bytes()[i] as char).to_digit(16).map(|d| d as u8);
        match s.len() {
            6 => Some(Rgb(
                nib(0)? << 4 | nib(1)?,
                nib(2)? << 4 | nib(3)?,
                nib(4)? << 4 | nib(5)?,
            )),
            3 => Some(Rgb(nib(0)? * 17, nib(1)? * 17, nib(2)? * 17)),
            _ => None,
        }
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// `t` = 0 gives `self`, 255 gives `other`.
    pub fn mix(self, other: Rgb, t: u8) -> Rgb {
        Rgb::from_px(blend_px(self.px(), other.px(), t as u32))
    }

    /// WCAG 2 relative luminance.
    pub fn luminance(self) -> f32 {
        fn lin(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }

    /// WCAG 2 contrast ratio, from 1.0 to 21.0.
    pub fn contrast(self, other: Rgb) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// Lays `src` over `dst` with coverage `a` in 0..=255. Both are `0x00RRGGBB`.
#[inline]
pub fn blend_px(dst: u32, src: u32, a: u32) -> u32 {
    let ia = 255 - a;
    let rb = (dst & 0x00FF_00FF) * ia + (src & 0x00FF_00FF) * a + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
    let g = (dst & 0x0000_FF00) * ia + (src & 0x0000_FF00) * a + 0x0000_8000;
    let g = ((g + ((g >> 8) & 0x0000_FF00)) >> 8) & 0x0000_FF00;
    rb | g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(Rgb::parse("#E7C9A9"), Some(Rgb(0xE7, 0xC9, 0xA9)));
        assert_eq!(Rgb::parse("e7c9a9"), Some(Rgb(0xE7, 0xC9, 0xA9)));
        assert_eq!(Rgb::parse(" #fa0 "), Some(Rgb(0xFF, 0xAA, 0x00)));
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("#gggggg"), None);
        assert_eq!(Rgb::parse("#ééé"), None);
    }

    #[test]
    fn blend_hits_both_ends_exactly() {
        for &(d, s) in &[(0x000000u32, 0xFFFFFFu32), (0x123456, 0xFEDCBA), (0xFFFFFF, 0x000000)] {
            assert_eq!(blend_px(d, s, 0), d);
            assert_eq!(blend_px(d, s, 255), s);
        }
        assert_eq!(blend_px(0x000000, 0xFFFFFF, 128), 0x808080);
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let c = Rgb(0, 0, 0).contrast(Rgb(255, 255, 255));
        assert!((c - 21.0).abs() < 0.01);
    }
}
