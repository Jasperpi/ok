//! The text core of ok. A document is a list of lines; everything that
//! changes it goes through one primitive, [`Doc::replace`], which hands back a
//! [`Change`] that is enough to undo the edit, to journal it, and to keep any
//! per-line side table in step.
//!
//! No I/O, no drawing, no dependencies. Positions are byte offsets that always
//! sit on a character boundary.

pub mod count;
pub mod doc;
pub mod fuzzy;
pub mod history;
pub mod links;
pub mod markup;
pub mod motion;
pub mod search;
pub mod width;
pub mod wrap;

pub use doc::{Change, Doc, Pos, Span};
pub use width::{cell_width, is_combining, str_width};
