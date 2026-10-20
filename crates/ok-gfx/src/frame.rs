use crate::color::{blend_px, Rgb};

/// A rectangle in pixels. Empty when either side is zero or negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The x just past the right edge. Saturates, so a rectangle at the far
    /// end of the number line cannot overflow.
    pub const fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// The y just past the bottom edge. Saturates like [`Rect::right`].
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn intersect(&self, o: Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect { x, y, w: r.saturating_sub(x).max(0), h: b.saturating_sub(y).max(0) }
    }
}

/// The picture: `width * height` pixels, top row first, each `0x00RRGGBB`.
///
/// Every drawing call is clipped to the current clip rectangle, which is
/// itself always inside the frame, so callers may draw at any coordinates.
pub struct Frame {
    w: i32,
    h: i32,
    px: Vec<u32>,
    clip: Rect,
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        let mut f = Frame { w: 0, h: 0, px: Vec::new(), clip: Rect::default() };
        f.resize(width, height);
        f
    }

    /// Changes the size. Contents are undefined afterwards; clip is reset.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.w = width.min(i32::MAX as usize / 2) as i32;
        self.h = height.min(i32::MAX as usize / 2) as i32;
        self.px.resize(self.w as usize * self.h as usize, 0);
        self.clip = self.bounds();
    }

    pub fn width(&self) -> usize {
        self.w as usize
    }

    pub fn height(&self) -> usize {
        self.h as usize
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    pub fn pixels(&self) -> &[u32] {
        &self.px
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.px
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Narrows drawing to `r` (cut down to the frame). Returns the previous
    /// clip so the caller can put it back.
    pub fn set_clip(&mut self, r: Rect) -> Rect {
        let old = self.clip;
        self.clip = r.intersect(self.bounds());
        old
    }

    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Rgb> {
        if self.bounds().contains(x, y) {
            Some(Rgb::from_px(self.px[(y * self.w + x) as usize]))
        } else {
            None
        }
    }

    /// Fills the whole frame, ignoring the clip.
    pub fn clear(&mut self, c: Rgb) {
        self.px.fill(c.px());
    }

    pub fn fill(&mut self, r: Rect, c: Rgb) {
        self.blend(r, c, 255);
    }

    /// Lays `c` over `r` with coverage `alpha` (255 is opaque).
    pub fn blend(&mut self, r: Rect, c: Rgb, alpha: u8) {
        let r = r.intersect(self.clip);
        if r.is_empty() || alpha == 0 {
            return;
        }
        let px = c.px();
        for y in r.y..r.bottom() {
            self.span(r.x, y, r.w, px, alpha);
        }
    }

    /// One horizontal run of `len` pixels starting at (`x`, `y`). This is the
    /// primitive every glyph and rectangle is built from.
    #[inline]
    pub fn span(&mut self, x: i32, y: i32, len: i32, px: u32, alpha: u8) {
        let c = self.clip;
        if y < c.y || y >= c.bottom() {
            return;
        }
        let x0 = x.max(c.x);
        let x1 = x.saturating_add(len).min(c.right());
        if x0 >= x1 {
            return;
        }
        let row = y as usize * self.w as usize;
        let run = &mut self.px[row + x0 as usize..row + x1 as usize];
        if alpha == 255 {
            run.fill(px);
        } else if alpha != 0 {
            let a = alpha as u32;
            for p in run {
                *p = blend_px(*p, px, a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_outside_the_frame_is_harmless() {
        let mut f = Frame::new(8, 4);
        f.clear(Rgb(0, 0, 0));
        f.fill(Rect::new(-100, -100, 1000, 1000), Rgb(1, 2, 3));
        assert!(f.pixels().iter().all(|&p| p == 0x010203));
        f.span(i32::MAX - 2, 1, 10, 0xFFFFFF, 255);
        f.span(-50, 1, 10, 0xFFFFFF, 255);
        f.span(0, 99, 10, 0xFFFFFF, 255);
        assert!(f.pixels().iter().all(|&p| p == 0x010203));
    }

    #[test]
    fn clip_limits_fills() {
        let mut f = Frame::new(8, 4);
        f.clear(Rgb(0, 0, 0));
        let old = f.set_clip(Rect::new(2, 1, 3, 2));
        f.fill(Rect::new(0, 0, 8, 4), Rgb(255, 255, 255));
        f.set_clip(old);
        let lit = f.pixels().iter().filter(|&&p| p == 0xFFFFFF).count();
        assert_eq!(lit, 6);
        assert_eq!(f.get(2, 1), Some(Rgb(255, 255, 255)));
        assert_eq!(f.get(1, 1), Some(Rgb(0, 0, 0)));
        assert_eq!(f.get(8, 0), None);
    }

    #[test]
    fn zero_sized_frame_does_not_panic() {
        let mut f = Frame::new(0, 0);
        f.clear(Rgb(9, 9, 9));
        f.fill(Rect::new(0, 0, 5, 5), Rgb(1, 1, 1));
        assert_eq!(f.pixels().len(), 0);
    }
}
