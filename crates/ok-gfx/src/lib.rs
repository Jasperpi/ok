//! Everything ok puts on screen goes through this crate: a plain `Vec<u32>`
//! framebuffer, faces drawn on a cell grid at integer scale, and a PNG
//! writer so a frame can be saved and looked at without a window.
//!
//! A face is a bitmap ([`BitmapFont`], read from `.hex`, BDF or PSF) or an
//! outline rasterized by the host ([`OutlineFace`], filled through
//! [`FaceSet::fill`]). The interface holds a [`FaceSet`], the chosen face
//! with Gallant behind it, and draws every cell on screen with
//! [`FaceSet::draw_cluster_in`].

mod bdf;
mod boxes;
mod builtin;
mod cluster;
mod color;
mod compose;
mod face;
mod font;
mod frame;
mod hexbox;
mod outline;
mod png;
mod psf;

pub use builtin::{face_notices, gallant, spleen, terminus};
pub use cluster::{clusters, Clusters};
pub use color::{blend_px, Rgb};
pub use face::{Face, FaceSet, MAX_MARKS};
pub use font::{draw_alpha_glyph, draw_glyph, AlphaGlyph, BitmapFont, FontError, Glyph, Pen};
pub use frame::{Frame, Rect};
pub use outline::OutlineFace;
pub use png::encode_png;
