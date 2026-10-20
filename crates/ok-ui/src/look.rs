//! Looks: the seven values that colour the screen, the two surfaces they
//! colour, and the strengths of ink on each.
//!
//! Nothing outside this file names a colour. A painter says which surface it
//! is drawing on and how strongly, and the look answers with the pixel.

use ok_gfx::Rgb;

/// The two grounds the screen is made of.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Surface {
    /// The page and the machine's two lines.
    Sheet,
    /// Everything round the sheet.
    Desk,
}

/// How strongly something is printed, from bare ground to full ink.
///
/// There are three strengths a thing can rest at: ink, half and faint. A
/// strength is a point on the line through them, so that a fade from one to
/// another passes through real values and a painter never has to know that a
/// fade is going on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Strength(u16);

impl Strength {
    /// Nothing printed at all.
    pub const GROUND: Strength = Strength(0);
    /// For what is not meant to be read just then.
    pub const FAINT: Strength = Strength(256);
    /// Readable, and out of the way.
    pub const HALF: Strength = Strength(512);
    pub const INK: Strength = Strength(768);

    /// Part of the way from this strength to `to`: 0 is this one, 255 is `to`.
    pub fn toward(self, to: Strength, t: u8) -> Strength {
        let (from, to) = (self.0 as i32, to.0 as i32);
        Strength((from + (to - from) * t as i32 / 255) as u16)
    }

    /// True when nothing would be printed.
    pub fn is_ground(self) -> bool {
        self.0 == 0
    }
}

/// Where half or faint sits in a look: a share of the way from the ground to
/// the ink, or a colour given outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shade {
    /// 0 is the ground, 100 the ink.
    Percent(u8),
    Colour(Rgb),
}

impl Shade {
    /// Reads a shade as it is typed: up to three digits are a share of the
    /// way from ground to ink (`65`, or `65%`), anything else a colour in
    /// hex. A colour of three digits therefore needs its `#`.
    pub fn parse(text: &str) -> Option<Shade> {
        let text = text.trim();
        let digits = text.strip_suffix('%').unwrap_or(text);
        if !digits.is_empty() && digits.len() <= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
            return digits.parse::<u8>().ok().filter(|&percent| percent <= 100).map(Shade::Percent);
        }
        if text.ends_with('%') {
            return None;
        }
        Rgb::parse(text).map(Shade::Colour)
    }

    /// The shade as the settings write it.
    pub fn written(self) -> String {
        match self {
            Shade::Percent(percent) => percent.to_string(),
            Shade::Colour(colour) => hex(colour),
        }
    }
}

/// A colour as the settings write it: six hex digits in capitals, the way
/// DESIGN.md's table of looks has them.
pub fn hex(colour: Rgb) -> String {
    format!("{:02X}{:02X}{:02X}", colour.0, colour.1, colour.2)
}

/// The share of the way from ground to ink that half and faint take unless a
/// look says otherwise.
pub const HALF_PERCENT: u8 = 65;
pub const FAINT_PERCENT: u8 = 35;

/// The seven values of a look by the words the line and the settings text
/// take for them, in DESIGN.md's order.
pub const TOKENS: [&str; 7] = ["paper", "ink", "desk", "desk-ink", "mark", "half", "faint"];

/// What a look is called once one of its colours has been set by hand.
pub const CUSTOM: &str = "custom";

/// A look: seven values, each named for the thing it is taken from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Look {
    pub name: String,
    /// The sheet.
    pub paper: Rgb,
    /// Text on the sheet.
    pub ink: Rgb,
    /// Everything round the sheet.
    pub desk: Rgb,
    /// Text on the desk.
    pub desk_ink: Rgb,
    /// The cursor, the notch and the change bars. `None` is the ink of
    /// whichever surface they fall on, which keeps a look one-bit.
    pub mark: Option<Rgb>,
    pub half: Shade,
    pub faint: Shade,
}

fn rgb(hex: u32) -> Rgb {
    Rgb::from_px(hex)
}

impl Look {
    /// A look with a sheet on a desk of another colour.
    fn on_desk(name: &str, paper: u32, ink: u32, desk: u32, desk_ink: u32) -> Look {
        Look {
            name: name.to_string(),
            paper: rgb(paper),
            ink: rgb(ink),
            desk: rgb(desk),
            desk_ink: rgb(desk_ink),
            mark: None,
            half: Shade::Percent(HALF_PERCENT),
            faint: Shade::Percent(FAINT_PERCENT),
        }
    }

    /// A look whose desk is the sheet's own colour: one flat ground.
    fn flat(name: &str, paper: u32, ink: u32) -> Look {
        Look::on_desk(name, paper, ink, paper, ink)
    }

    /// The same look with half set at `percent` of the way to the ink. The
    /// phosphor looks and WordPerfect's grey on blue start from less contrast
    /// than ink on paper, and 65 percent of it does not reach the 4.5 to 1
    /// that anything carrying words must have; each is given the least share
    /// that does.
    fn half_at(mut self, percent: u8) -> Look {
        self.half = Shade::Percent(percent);
        self
    }

    /// A cream typing sheet on a stained walnut desk, struck through a black
    /// fabric ribbon. The default.
    pub fn manila() -> Look {
        Look::on_desk("manila", 0xEEE3C8, 0x221F1B, 0x35251A, 0xE6D9BA)
    }

    /// Every built-in look, in the order DESIGN.md lists them.
    pub fn builtin() -> Vec<Look> {
        // Grey on blue leaves no room for a lighter grey that still reads,
        // so half is VGA palette entry 3, cyan: another colour, not another
        // grey, at 4.6 to 1 on the blue.
        let mut wordperfect = Look::flat("wordperfect", 0x0000AA, 0xAAAAAA);
        wordperfect.half = Shade::Colour(rgb(0x00AAAA));
        vec![
            Look::manila(),
            Look::flat("sun", 0xFFFFFF, 0x000000),
            Look::flat("acme", 0xFFFFEA, 0x000000),
            Look::flat("register", 0xF4F5F1, 0x1E1F1C),
            Look::flat("register night", 0x20211F, 0xE9EAE4),
            Look::flat("amber", 0x191310, 0xFFB000),
            Look::flat("vt100", 0x151819, 0x9BB7CB).half_at(67),
            Look::flat("5151", 0x101913, 0x00CB75).half_at(69),
            wordperfect,
        ]
    }

    /// The built-in look of that name, compared without case.
    pub fn named(name: &str) -> Option<Look> {
        let name = name.trim();
        Look::builtin().into_iter().find(|look| look.name.eq_ignore_ascii_case(name))
    }

    /// True when the desk is the sheet's own colour and the screen is one
    /// flat ground.
    pub fn is_flat(&self) -> bool {
        self.desk == self.paper
    }

    /// One of the seven values as the settings write it: a colour as six
    /// hex digits, a mark that follows the ink as `ink`, a shade as its
    /// share or its colour. `None` for a word that is not one of
    /// [`TOKENS`].
    pub fn token(&self, token: &str) -> Option<String> {
        Some(match token {
            "paper" => hex(self.paper),
            "ink" => hex(self.ink),
            "desk" => hex(self.desk),
            "desk-ink" => hex(self.desk_ink),
            "mark" => self.mark.map_or_else(|| String::from("ink"), hex),
            "half" => self.half.written(),
            "faint" => self.faint.written(),
            _ => return None,
        })
    }

    /// Sets one of the seven values from what was typed for it. False, and
    /// nothing changed, when the word is not one of [`TOKENS`] or the value
    /// is not one it can take. The name is left alone; whoever sets a
    /// colour by hand says what the look is called afterwards.
    pub fn set_token(&mut self, token: &str, value: &str) -> bool {
        let value = value.trim();
        let colour = Rgb::parse(value);
        match (token, colour) {
            ("paper", Some(colour)) => self.paper = colour,
            ("ink", Some(colour)) => self.ink = colour,
            ("desk", Some(colour)) => self.desk = colour,
            ("desk-ink", Some(colour)) => self.desk_ink = colour,
            ("mark", _) if value.eq_ignore_ascii_case("ink") => self.mark = None,
            ("mark", Some(colour)) => self.mark = Some(colour),
            ("half", _) | ("faint", _) => match Shade::parse(value) {
                Some(shade) if token == "half" => self.half = shade,
                Some(shade) => self.faint = shade,
                None => return false,
            },
            _ => return false,
        }
        true
    }

    /// True when the seven values are the same, whatever the two are
    /// called.
    pub fn same_values(&self, other: &Look) -> bool {
        (self.paper, self.ink, self.desk, self.desk_ink, self.mark, self.half, self.faint) == (other.paper, other.ink, other.desk, other.desk_ink, other.mark, other.half, other.faint)
    }

    /// The bare colour of a surface.
    pub fn ground(&self, surface: Surface) -> Rgb {
        match surface {
            Surface::Sheet => self.paper,
            Surface::Desk => self.desk,
        }
    }

    /// The colour of text at full strength on a surface.
    pub fn ink_on(&self, surface: Surface) -> Rgb {
        match surface {
            Surface::Sheet => self.ink,
            Surface::Desk => self.desk_ink,
        }
    }

    /// The colour of the cursor, the notch and the change bars on a surface.
    pub fn mark_on(&self, surface: Surface) -> Rgb {
        self.mark.unwrap_or(self.ink_on(surface))
    }

    /// The colour of text printed on `surface` at `strength`.
    pub fn shade(&self, surface: Surface, strength: Strength) -> Rgb {
        let (ground, ink) = (self.ground(surface), self.ink_on(surface));
        let stop = |shade: Shade| match shade {
            Shade::Percent(percent) => ground.mix(ink, ((percent.min(100) as u32 * 255 + 50) / 100) as u8),
            Shade::Colour(colour) => colour,
        };
        let at = strength.0.min(Strength::INK.0);
        let (from, to, part) = match at {
            0..=255 => (ground, stop(self.faint), at),
            256..=511 => (stop(self.faint), stop(self.half), at - 256),
            _ => (stop(self.half), ink, at - 512),
        };
        if part >= 256 {
            to
        } else {
            from.mix(to, part as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_look_is_manila_with_the_values_the_design_gives() {
        let look = Look::manila();
        assert_eq!((look.paper, look.ink), (Rgb(0xEE, 0xE3, 0xC8), Rgb(0x22, 0x1F, 0x1B)));
        assert_eq!((look.desk, look.desk_ink), (Rgb(0x35, 0x25, 0x1A), Rgb(0xE6, 0xD9, 0xBA)));
        assert!(!look.is_flat());
        assert_eq!(Look::builtin()[0], look);
    }

    #[test]
    fn every_built_in_look_is_found_by_name() {
        let names: Vec<String> = Look::builtin().into_iter().map(|look| look.name).collect();
        assert_eq!(names, ["manila", "sun", "acme", "register", "register night", "amber", "vt100", "5151", "wordperfect"]);
        for name in &names {
            assert_eq!(Look::named(name).map(|look| look.name), Some(name.clone()));
        }
        assert_eq!(Look::named(" Register Night ").map(|look| look.paper), Some(Rgb(0x20, 0x21, 0x1F)));
        assert!(Look::named("solarized").is_none());
    }

    #[test]
    fn the_strengths_run_from_ground_to_ink_through_faint_and_half() {
        let look = Look::named("sun").unwrap();
        for surface in [Surface::Sheet, Surface::Desk] {
            assert_eq!(look.shade(surface, Strength::GROUND), Rgb(255, 255, 255));
            assert_eq!(look.shade(surface, Strength::INK), Rgb(0, 0, 0));
            // 65 and 35 percent of the way from white to black.
            assert_eq!(look.shade(surface, Strength::HALF), Rgb(89, 89, 89));
            assert_eq!(look.shade(surface, Strength::FAINT), Rgb(166, 166, 166));
        }
        let mut last = 256;
        for step in 0..=255u8 {
            let grey = look.shade(Surface::Sheet, Strength::GROUND.toward(Strength::INK, step)).0 as i32;
            assert!(grey <= last, "a fade never turns back");
            last = grey;
        }
        assert_eq!(Strength::HALF.toward(Strength::FAINT, 0), Strength::HALF);
        assert_eq!(Strength::HALF.toward(Strength::FAINT, 255), Strength::FAINT);
        assert!(Strength::GROUND.is_ground() && !Strength::FAINT.is_ground());
    }

    /// DESIGN.md: "Body text is at least 4.5 to 1 on its ground in every
    /// built-in look, and so is half strength wherever it carries words."
    #[test]
    fn ink_and_half_carry_words_at_four_and_a_half_to_one_in_every_look() {
        for look in Look::builtin() {
            for surface in [Surface::Sheet, Surface::Desk] {
                let ground = look.ground(surface);
                let ink = look.shade(surface, Strength::INK).contrast(ground);
                let half = look.shade(surface, Strength::HALF).contrast(ground);
                let faint = look.shade(surface, Strength::FAINT).contrast(ground);
                assert!(ink >= 4.5, "{}: ink at {ink:.2}", look.name);
                assert!(half >= 4.5, "{}: half at {half:.2}", look.name);
                assert!(faint < half && faint > 1.2, "{}: faint at {faint:.2}", look.name);
            }
        }
    }

    #[test]
    fn a_mark_is_the_ink_of_its_surface_unless_the_look_names_one() {
        let manila = Look::manila();
        assert_eq!(manila.mark_on(Surface::Sheet), manila.ink);
        assert_eq!(manila.mark_on(Surface::Desk), manila.desk_ink);
        let wordperfect = Look::named("wordperfect").unwrap();
        assert_eq!(wordperfect.mark_on(Surface::Sheet), wordperfect.ink, "no white stripe down the change bar");
    }

    #[test]
    fn the_seven_values_are_read_and_written_by_the_words_the_line_takes() {
        let mut look = Look::manila();
        let written: Vec<String> = TOKENS.iter().filter_map(|token| look.token(token)).collect();
        assert_eq!(written, ["EEE3C8", "221F1B", "35251A", "E6D9BA", "ink", "65", "35"]);
        assert!(look.set_token("paper", "#fff") && look.set_token("ink", "000000") && look.set_token("desk", "#101010") && look.set_token("desk-ink", "c0C0c0"));
        assert!(look.set_token("mark", "ff0000") && look.set_token("half", "70%") && look.set_token("faint", "#888"));
        let written: Vec<String> = TOKENS.iter().filter_map(|token| look.token(token)).collect();
        assert_eq!(written, ["FFFFFF", "000000", "101010", "C0C0C0", "FF0000", "70", "888888"]);
        assert!(look.set_token("mark", "Ink") && look.mark.is_none());
        // What a value cannot be leaves it as it was.
        for (token, value) in [("paper", "cream"), ("ink", "12345"), ("half", "101"), ("faint", "12%%"), ("half", ""), ("mark", "none"), ("name", "x"), ("look", "sun")] {
            let before = look.clone();
            assert!(!look.set_token(token, value), "{token} {value}");
            assert_eq!(look, before);
        }
        assert!(look.token("look").is_none());
    }

    #[test]
    fn a_shade_is_a_share_or_a_colour() {
        assert_eq!(Shade::parse("65"), Some(Shade::Percent(65)));
        assert_eq!(Shade::parse(" 0% "), Some(Shade::Percent(0)));
        assert_eq!(Shade::parse("100"), Some(Shade::Percent(100)));
        assert_eq!(Shade::parse("123"), None, "three digits are a share, and this one is too large");
        assert_eq!(Shade::parse("#123"), Some(Shade::Colour(Rgb(0x11, 0x22, 0x33))));
        assert_eq!(Shade::parse("59595A"), Some(Shade::Colour(Rgb(0x59, 0x59, 0x5A))));
        assert_eq!(Shade::parse("59595A%"), None);
        assert_eq!(Shade::parse(""), None);
        assert_eq!((Shade::Percent(65).written(), Shade::Colour(Rgb(1, 2, 255)).written()), (String::from("65"), String::from("0102FF")));
    }

    #[test]
    fn two_looks_are_the_same_when_their_seven_values_are() {
        let mut other = Look::manila();
        other.name = String::from("mine");
        assert!(other.same_values(&Look::manila()));
        other.faint = Shade::Percent(30);
        assert!(!other.same_values(&Look::manila()));
    }

    #[test]
    fn a_shade_given_as_a_colour_is_used_as_it_is() {
        let mut look = Look::manila();
        look.half = Shade::Colour(Rgb(1, 2, 3));
        assert_eq!(look.shade(Surface::Sheet, Strength::HALF), Rgb(1, 2, 3));
        assert_eq!(look.shade(Surface::Desk, Strength::HALF), Rgb(1, 2, 3));
    }
}
