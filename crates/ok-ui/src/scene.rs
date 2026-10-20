//! One frame: everything a painter may look at, and the order the regions
//! are painted in.

use crate::canvas::Canvas;
use crate::flow::Flow;
use crate::line::Line;
use crate::page::{Day, Page};
use crate::platen::View;
use crate::session::Session;
use crate::{contents, machine, map, sheet};

/// What is to be drawn. Painters read this and draw; none of them changes
/// anything.
pub struct Scene<'a> {
    pub page: &'a Page,
    pub view: &'a View,
    pub session: &'a dyn Session,
    pub line: &'a dyn Line,
    pub flow: &'a dyn Flow,
    /// The host's clock at this frame, for the flow's fades.
    pub now_ms: u64,
    /// The writing day it is, for change bars and the map's ink.
    pub today: Day,
    /// False while another window has the keyboard.
    pub focused: bool,
    /// The sentence on the say row, when the line is not supplying one.
    pub said: Option<&'a str>,
}

/// Paints the whole screen onto a canvas that has just been started.
pub fn paint(c: &mut Canvas, s: &Scene) {
    sheet::paint(c, s);
    machine::paint(c, s);
    contents::paint(c, s);
    map::paint(c, s);
    contents::rise(c, s);
}
