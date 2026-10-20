//! Settings: every option `set` takes, what each can be, and the settings
//! text they are kept in.
//!
//! An option is a name and a value, both plain words. The line tries a
//! value with [`apply`], which changes the screen at once; [`snapshot`] and
//! [`put_back`] are how a value that was only tried is taken back. The
//! settings text is the same words again, one `name value` to a line, read
//! from top to bottom exactly as if each line had been typed after `set`:
//! [`written`] makes it and [`load`] reads it.
//!
//! Two options are not the interface's to keep. `limit` belongs to the open
//! text and `folder` to the session, so both are asked of the session and
//! neither is written in the settings text.

use std::path::Path;

use ok_gfx::Rgb;
use ok_host::Host;
use ok_text::str_width;

use crate::app::{App, Prefs};
use crate::grid::MEASURE;
use crate::look::{Look, Shade, CUSTOM, TOKENS};

/// What an option's value can be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The name of a look: a built-in one, one of his own, or a new name to
    /// keep the present colours under.
    Look,
    /// The name of a face: a built-in one or a fixed-pitch face of the
    /// system's.
    Face,
    /// A whole number between two bounds, typed or nudged.
    Number { least: usize, most: usize },
    /// A colour, typed as hex.
    Colour,
    /// A colour, or `ink` to follow the ink of whatever it falls on.
    Mark,
    /// A share of the way from ground to ink, or a colour.
    Shade,
    /// One of two words. The first is the default.
    Switch(&'static str, &'static str),
    /// A number of words, or `off`.
    Limit,
    /// The path of a folder.
    Folder,
}

/// One option.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Opt {
    pub name: &'static str,
    pub kind: Kind,
}

const fn opt(name: &'static str, kind: Kind) -> Opt {
    Opt { name, kind }
}

/// Every option, in the order DESIGN.md's "Settings" lists them, which is
/// the order of the table.
pub const OPTIONS: [Opt; 20] = [
    opt("look", Kind::Look),
    opt("face", Kind::Face),
    opt("scale", Kind::Number { least: 1, most: 3 }),
    opt("paper", Kind::Colour),
    opt("ink", Kind::Colour),
    opt("desk", Kind::Colour),
    opt("desk-ink", Kind::Colour),
    opt("mark", Kind::Mark),
    opt("half", Kind::Shade),
    opt("faint", Kind::Shade),
    opt("measure", Kind::Number { least: 40, most: 100 }),
    opt("platen", Kind::Switch("on", "off")),
    opt("scroll", Kind::Switch("smooth", "jump")),
    opt("cursor", Kind::Switch("block", "underline")),
    opt("focus", Kind::Switch("on", "off")),
    opt("echo", Kind::Switch("on", "off")),
    opt("enter", Kind::Switch("paragraph", "line")),
    opt("limit", Kind::Limit),
    opt("window", Kind::Switch("full", "framed")),
    opt("folder", Kind::Folder),
];

/// The faces inside the program, by the words the settings take for them.
pub const FACES: [&str; 3] = ["gallant", "spleen", "terminus"];

/// The word that asks for an option's default.
const DEFAULT: &str = "default";

/// The longest name a look of his own may have.
const LONGEST_NAME: usize = 24;

/// How far a word limit moves when it is nudged.
const LIMIT_STEP: usize = 50;

/// The largest word limit that can be set.
const LIMIT_MOST: usize = 10_000_000;

/// The option of exactly this name, whatever its case.
pub fn find(name: &str) -> Option<&'static Opt> {
    OPTIONS.iter().find(|opt| opt.name.eq_ignore_ascii_case(name.trim()))
}

/// The option a name being typed is heading for: the one of that name, or
/// the first whose name starts with it.
pub fn starting(typed: &str) -> Option<&'static Opt> {
    let typed = typed.trim().to_ascii_lowercase();
    find(&typed).or_else(|| OPTIONS.iter().find(|opt| !typed.is_empty() && opt.name.starts_with(&typed)))
}

/// How many letters must be put in, taken out or exchanged to turn one
/// word into another.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &from) in a.iter().enumerate() {
        let mut corner = row[0];
        row[0] = i + 1;
        for (j, &to) in b.iter().enumerate() {
            let exchanged = corner + usize::from(from != to);
            corner = row[j + 1];
            row[j + 1] = exchanged.min(row[j] + 1).min(corner + 1);
        }
    }
    row[b.len()]
}

/// The option whose name is nearest a word that names none.
pub fn nearest(name: &str) -> &'static str {
    let name: String = name.trim().to_lowercase().chars().take(32).collect();
    OPTIONS.iter().min_by_key(|opt| distance(&name, opt.name)).map_or("look", |opt| opt.name)
}

/// The first of `choices` that is `typed` whatever its case, or failing
/// that the first that starts with it.
fn chosen(choices: &[String], typed: &str) -> Option<String> {
    let typed = typed.to_lowercase();
    let exact = choices.iter().find(|choice| choice.to_lowercase() == typed);
    exact.or_else(|| choices.iter().find(|choice| choice.to_lowercase().starts_with(&typed))).cloned()
}

/// Which of a switch's two words an option is at: true for the first.
fn switched(app: &App, name: &str) -> bool {
    let prefs = app.prefs();
    match name {
        "platen" => prefs.platen,
        "scroll" => prefs.smooth,
        "cursor" => !prefs.underline,
        "focus" => prefs.focus,
        "echo" => prefs.echo,
        "enter" => !prefs.enter_line,
        _ => app.fullscreen(),
    }
}

/// The value an option has now, as the settings write it.
pub fn value(app: &App, opt: &Opt) -> String {
    match opt.kind {
        Kind::Look => app.look().name.clone(),
        Kind::Face => app.face_name().to_string(),
        Kind::Number { .. } if opt.name == "scale" => app.prefs().scale.to_string(),
        Kind::Number { .. } => app.prefs().measure.to_string(),
        Kind::Colour | Kind::Mark | Kind::Shade => app.look().token(opt.name).unwrap_or_default(),
        Kind::Switch(first, second) => String::from(if switched(app, opt.name) { first } else { second }),
        Kind::Limit => app.session().limit().map_or_else(|| String::from("off"), |limit| limit.to_string()),
        Kind::Folder => app.session().folder().map(|folder| folder.display().to_string()).unwrap_or_default(),
    }
}

/// The value an option has when nothing has been set.
pub fn default(opt: &Opt, host: &dyn Host) -> String {
    match opt.kind {
        Kind::Look => Look::manila().name,
        Kind::Face => String::from(FACES[0]),
        Kind::Number { .. } if opt.name == "scale" => String::from("1"),
        Kind::Number { .. } => MEASURE.to_string(),
        Kind::Colour | Kind::Mark | Kind::Shade => Look::manila().token(opt.name).unwrap_or_default(),
        Kind::Switch(first, _) => String::from(first),
        Kind::Limit => String::from("off"),
        Kind::Folder => host.documents_dir().join("ok").display().to_string(),
    }
}

/// Every look there is: the built-in ones, then his own.
fn looks(app: &App) -> Vec<Look> {
    let mut all = Look::builtin();
    all.extend(app.looks().iter().filter(|kept| Look::named(&kept.name).is_none()).cloned());
    all
}

/// The values a list option is walked through, in order. `None` for an
/// option that is typed instead.
pub fn choices(app: &App, opt: &Opt, host: &mut dyn Host) -> Option<Vec<String>> {
    match opt.kind {
        Kind::Look => {
            let mut names: Vec<String> = looks(app).into_iter().map(|look| look.name).collect();
            if app.look().name == CUSTOM {
                names.push(String::from(CUSTOM));
            }
            Some(names)
        }
        Kind::Face => {
            let mut names: Vec<String> = FACES.iter().map(|face| face.to_string()).collect();
            for family in host.system_faces() {
                if !names.iter().any(|name| name.eq_ignore_ascii_case(&family)) {
                    names.push(family);
                }
            }
            Some(names)
        }
        Kind::Switch(first, second) => Some(vec![String::from(first), String::from(second)]),
        _ => None,
    }
}

/// What an option takes, as a sentence for the say row. Each is short
/// enough to follow `Enter does nothing yet.` on a row of 66 cells.
pub fn takes(opt: &Opt) -> String {
    // The option's name opens the sentence, so it takes a capital there.
    let mut letters = opt.name.chars();
    let name: String = letters.next().map(|first| first.to_ascii_uppercase()).into_iter().chain(letters).collect();
    match opt.kind {
        Kind::Look => format!("{name} takes a name of {LONGEST_NAME} letters at most."),
        Kind::Face => format!("{name} takes one of the faces listed."),
        Kind::Number { least, most } => format!("{name} takes {least} to {most}."),
        Kind::Colour => format!("{name} takes a colour as six hex digits."),
        Kind::Mark => format!("{name} takes six hex digits, or ink."),
        Kind::Shade => format!("{name} takes 0 to 100, or six hex digits."),
        // Said after "Enter does nothing yet.", the option called enter
        // would read as the key.
        Kind::Switch(first, second) if opt.name == "enter" => format!("The enter setting takes {first} or {second}."),
        Kind::Switch(first, second) => format!("{name} takes {first} or {second}."),
        Kind::Limit => format!("{name} takes a number of words, or off."),
        Kind::Folder => format!("{name} takes the path of a folder."),
    }
}

/// What was typed for an option, read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Read {
    /// A value the option can take, as the settings write it.
    Value(String),
    /// For `look` alone: a name no look has yet, under which the present
    /// colours can be kept.
    NewLook(String),
    /// Nothing the option can take.
    No,
}

/// Reads what was typed for an option. `default` stands for the option's
/// default, a list's value may be shortened to its first letters, and
/// nothing typed is nothing the option can take.
pub fn read(app: &App, opt: &Opt, typed: &str, host: &mut dyn Host) -> Read {
    let typed = typed.trim();
    if typed.is_empty() {
        return Read::No;
    }
    if typed.eq_ignore_ascii_case(DEFAULT) {
        return Read::Value(default(opt, host));
    }
    let value = match opt.kind {
        Kind::Look => {
            let found = choices(app, opt, host).and_then(|names| chosen(&names, typed));
            // `custom` is what colours set by hand are called, never a
            // name to keep them under.
            let nameable = typed.chars().count() <= LONGEST_NAME && !typed.chars().any(char::is_control) && !typed.eq_ignore_ascii_case(CUSTOM);
            return match found {
                Some(name) => Read::Value(name),
                None if nameable => Read::NewLook(typed.to_string()),
                None => Read::No,
            };
        }
        Kind::Face | Kind::Switch(..) => choices(app, opt, host).and_then(|names| chosen(&names, typed)),
        Kind::Number { least, most } => typed.parse::<usize>().ok().filter(|n| (least..=most).contains(n)).map(|n| n.to_string()),
        Kind::Colour => Rgb::parse(typed).map(crate::look::hex),
        Kind::Mark if typed.eq_ignore_ascii_case("ink") => Some(String::from("ink")),
        Kind::Mark => Rgb::parse(typed).map(crate::look::hex),
        Kind::Shade => Shade::parse(typed).map(Shade::written),
        Kind::Limit if "off".starts_with(&typed.to_ascii_lowercase()) || typed == "0" => Some(String::from("off")),
        Kind::Limit => typed.parse::<usize>().ok().filter(|n| (1..=LIMIT_MOST).contains(n)).map(|n| n.to_string()),
        Kind::Folder => Some(typed.to_string()),
    };
    value.map_or(Read::No, Read::Value)
}

/// What a look is called once one of its values has been set by hand: the
/// name it had if that look still has these values, the name of any other
/// look that has them, and `custom` when none does.
fn name_for(app: &App, look: &Look) -> String {
    let all = looks(app);
    let same = |known: &&Look| known.same_values(look);
    let kept_name = all.iter().filter(same).find(|known| known.name == look.name);
    kept_name.or_else(|| all.iter().find(same)).map_or_else(|| String::from(CUSTOM), |known| known.name.clone())
}

/// Sets an option to a value [`read`] handed back, and the screen changes
/// with it. False when it could not be done: a face that has gone, or the
/// folder, which is never only tried (see [`App::use_folder`]).
pub fn apply(app: &mut App, opt: &Opt, value: &str, host: &mut dyn Host) -> bool {
    match opt.kind {
        Kind::Look => match looks(app).into_iter().find(|look| look.name.eq_ignore_ascii_case(value)) {
            Some(look) => app.use_look(look),
            // The present colours are what `custom` already means.
            None => return value == CUSTOM && app.look().name == CUSTOM,
        },
        Kind::Face => return app.use_face(value, host),
        Kind::Number { .. } => match value.parse::<usize>() {
            Ok(number) if opt.name == "scale" => app.set_scale(number.min(3) as u32),
            Ok(number) => app.set_measure(number),
            Err(_) => return false,
        },
        Kind::Colour | Kind::Mark | Kind::Shade => {
            let mut look = app.look().clone();
            if !look.set_token(opt.name, value) {
                return false;
            }
            look.name = name_for(app, &look);
            app.use_look(look);
        }
        Kind::Switch(first, _) => {
            let on = value == first;
            match opt.name {
                "platen" => app.prefs_mut().platen = on,
                "scroll" => app.prefs_mut().smooth = on,
                "cursor" => app.prefs_mut().underline = !on,
                "focus" => app.prefs_mut().focus = on,
                "echo" => app.prefs_mut().echo = on,
                "enter" => app.prefs_mut().enter_line = !on,
                _ => app.set_fullscreen(on),
            }
            app.redraw();
        }
        Kind::Limit => {
            app.session_mut().set_limit(value.parse::<usize>().ok());
            app.redraw();
        }
        Kind::Folder => return false,
    }
    true
}

/// The value one step up or down from what is typed for an option, or from
/// the value it has when that is no number. `None` for an option that is
/// not nudged.
pub fn nudge(app: &App, opt: &Opt, typed: &str, up: bool) -> Option<String> {
    let typed = typed.trim();
    let from = |text: &str| text.trim_end_matches('%').parse::<usize>().ok();
    let now = value(app, opt);
    let (least, most, step) = match opt.kind {
        Kind::Number { least, most } => (least, most, 1),
        Kind::Shade => (0, 100, 1),
        Kind::Limit => (0, LIMIT_MOST, LIMIT_STEP),
        _ => return None,
    };
    let at = match from(typed).or_else(|| from(&now)) {
        Some(at) => at,
        // A limit that is off counts up from nothing.
        None if opt.kind == Kind::Limit => 0,
        // A shade given as a colour has no step up or down.
        None => return None,
    };
    let next = if up { at.saturating_add(step) } else { at.saturating_sub(step) }.clamp(least, most);
    Some(if opt.kind == Kind::Limit && next == 0 { String::from("off") } else { next.to_string() })
}

/// Everything `set` can change, as it stood at one moment.
#[derive(Clone, PartialEq, Debug)]
pub struct Snapshot {
    look: Look,
    looks: Vec<Look>,
    prefs: Prefs,
    face: String,
    fullscreen: bool,
    limit: Option<usize>,
}

impl Snapshot {
    /// The look that was in force.
    pub fn look(&self) -> &Look {
        &self.look
    }
}

/// Everything `set` can change, as it stands now.
pub fn snapshot(app: &App) -> Snapshot {
    Snapshot { look: app.look().clone(), looks: app.looks().to_vec(), prefs: app.prefs().clone(), face: app.face_name().to_string(), fullscreen: app.fullscreen(), limit: app.session().limit() }
}

/// Puts everything back as a snapshot has it. Only what differs is
/// touched, so a face that was never changed is not loaded again.
pub fn put_back(app: &mut App, was: &Snapshot, host: &mut dyn Host) {
    if app.look() != &was.look {
        app.use_look(was.look.clone());
    }
    if app.looks() != was.looks.as_slice() {
        app.set_looks(was.looks.clone());
    }
    if !app.face_name().eq_ignore_ascii_case(&was.face) {
        app.use_face(&was.face, host);
    }
    if app.prefs().scale != was.prefs.scale {
        app.set_scale(was.prefs.scale);
    }
    if app.prefs().measure != was.prefs.measure {
        app.set_measure(was.prefs.measure);
    }
    *app.prefs_mut() = was.prefs.clone();
    app.set_fullscreen(was.fullscreen);
    if app.session().limit() != was.limit {
        app.session_mut().set_limit(was.limit);
    }
    app.redraw();
}

/// The seven values of a look as seven lines of the settings text.
fn seven(look: &Look, out: &mut String) {
    for token in TOKENS {
        out.push_str(&format!("{token} {}\n", look.token(token).unwrap_or_default()));
    }
}

/// The settings text for the interface as it stands: the same words the
/// line takes, one `name value` to a line. Each look of his own comes first
/// as its seven values and then its name, which is how the line makes one;
/// then the look in force, by name, or by its seven values when it has no
/// name; then every other option.
pub fn written(app: &App) -> String {
    let mut out = String::new();
    for kept in app.looks() {
        seven(kept, &mut out);
        out.push_str(&format!("look {}\n", kept.name));
    }
    let look = app.look();
    if looks(app).iter().any(|known| known.name == look.name && known.same_values(look)) {
        out.push_str(&format!("look {}\n", look.name));
    } else {
        seven(look, &mut out);
    }
    for opt in OPTIONS.iter().filter(|opt| matches!(opt.kind, Kind::Face | Kind::Number { .. } | Kind::Switch(..))) {
        out.push_str(&format!("{} {}\n", opt.name, value(app, opt)));
    }
    out
}

/// Reads a settings text, line by line, as if each line had been typed
/// after `set` and kept. A line that names no option, or gives one a value
/// it cannot take, is passed over: a slip in the file must never keep the
/// program from starting.
pub fn load(app: &mut App, text: &str, host: &mut dyn Host) {
    for line in text.lines() {
        let line = line.trim();
        let (name, typed) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let Some(opt) = find(name).filter(|opt| !matches!(opt.kind, Kind::Limit | Kind::Folder)) else { continue };
        match read(app, opt, typed, host) {
            Read::Value(value) => {
                apply(app, opt, &value, host);
            }
            Read::NewLook(name) => app.keep_look(&name),
            Read::No => {}
        }
    }
}

/// A value cut down to what a row of the table has room for. A path keeps
/// its end, which is the part that tells one folder from another.
pub fn shown(value: &str, cells: usize) -> String {
    if str_width(value) <= cells {
        return value.to_string();
    }
    let parts: Vec<&str> = value.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut wide = 0;
    for part in parts.iter().rev() {
        let more = str_width(part) + usize::from(!kept.is_empty());
        if wide + more > cells {
            break;
        }
        wide += more;
        kept.push(part);
    }
    if kept.is_empty() {
        return crate::canvas::fit(value, cells).to_string();
    }
    kept.reverse();
    kept.join("\\")
}

/// The folder a path names, for a sentence: its last part.
pub fn leaf_of(path: &str) -> String {
    Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_found_by_name_by_their_first_letters_and_by_nearness() {
        assert_eq!(find("Measure").map(|opt| opt.name), Some("measure"));
        assert_eq!(find("mesure"), None);
        assert_eq!(starting("me").map(|opt| opt.name), Some("measure"));
        assert_eq!(starting("desk").map(|opt| opt.name), Some("desk"), "the whole name before a longer one that starts with it");
        assert_eq!(starting("desk-").map(|opt| opt.name), Some("desk-ink"));
        assert_eq!(starting(""), None);
        assert_eq!(starting("zz"), None);
        assert_eq!(nearest("mesure"), "measure");
        assert_eq!(nearest("platten"), "platen");
        assert_eq!(nearest("desk-nk"), "desk-ink");
        assert_eq!(nearest("fase"), "face");
    }

    #[test]
    fn the_distance_between_words_counts_letters() {
        assert_eq!(distance("measure", "measure"), 0);
        assert_eq!(distance("mesure", "measure"), 1);
        assert_eq!(distance("", "ink"), 3);
        assert_eq!(distance("ink", ""), 3);
        assert_eq!(distance("scale", "scroll"), 3);
    }

    #[test]
    fn every_option_of_the_design_is_in_the_table_once() {
        let names: Vec<&str> = OPTIONS.iter().map(|opt| opt.name).collect();
        assert_eq!(names, ["look", "face", "scale", "paper", "ink", "desk", "desk-ink", "mark", "half", "faint", "measure", "platen", "scroll", "cursor", "focus", "echo", "enter", "limit", "window", "folder"]);
        for opt in &OPTIONS {
            let sentence = takes(opt);
            assert!(str_width(&sentence) <= 42 && sentence.ends_with('.') && !sentence.contains('!'), "{sentence}");
            assert!(sentence.starts_with(char::is_uppercase), "{sentence}");
        }
    }

    #[test]
    #[cfg(windows)] // Windows paths: drive letters and `\`.
    fn a_long_value_keeps_its_end() {
        assert_eq!(shown("manila", 22), "manila");
        assert_eq!(shown("C:\\Users\\jaspe\\Documents\\ok", 22), "jaspe\\Documents\\ok");
        assert_eq!(shown("C:\\Users\\jaspe\\Documents\\ok", 12), "Documents\\ok");
        assert_eq!(shown("C:/Users/jaspe/Documents/ok", 4), "ok");
        assert_eq!(shown("Antidisestablishmentarianism", 6), "Antidi");
        assert_eq!(leaf_of("C:\\Users\\jaspe\\Documents\\essays"), "essays");
    }
}
