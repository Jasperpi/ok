//! The interface of ok: one sheet of Gallant on a desk, drawn on a grid of
//! cells and driven by the events of a window it never sees.
//!
//! [`App`] is the program the shell runs. Everything it draws goes through
//! a [`canvas::Canvas`] onto a [`grid::Grid`] in a [`look::Look`]. The text
//! and its keys are in [`page`], the paper's position in [`platen`], the
//! regions of the screen in [`sheet`], [`contents`], [`map`] and
//! [`machine`].
//!
//! Three modules are seams that other work fills in, each a trait whose
//! methods all come with a body that does nothing: [`session`] (where the
//! text comes from and goes to), [`mod@line`] (the ok line) and [`flow`] (what
//! fades while he writes). Each opens with a brief for whoever fills it,
//! and each hands the program its implementation from a `start` function,
//! so filling one touches one file.
//!
//! [`harness`] drives the whole thing with no window: a host with a clock
//! that only moves when told, a small script language, the screen read back
//! as text, and frames written as PNG.

pub mod app;
pub mod canvas;
pub mod contents;
pub mod flow;
pub mod folder;
pub mod grid;
pub mod harness;
pub mod keys;
pub mod line;
pub mod look;
pub mod machine;
pub mod map;
pub mod page;
pub mod platen;
pub mod scene;
pub mod session;
pub mod sheet;

pub use app::{App, Prefs};
pub use session::Options;
