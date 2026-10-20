//! Paths and names: the one place a path is put into the form a rename by
//! handle needs, the rules a new file name must pass, and the names given to
//! the copies kept beside a text when two versions both have to survive.

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

use crate::hash;
use crate::sys;

/// The endings that make a file a text.
pub const TEXT_ENDINGS: [&str; 3] = ["md", "markdown", "txt"];

/// Why a name was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameError {
    /// Nothing left once spaces were set aside.
    Empty,
    /// A device name Windows keeps for itself: CON, PRN, AUX, NUL, COM1 to
    /// COM9, LPT1 to LPT9, whatever ending follows.
    Reserved,
    /// A character Windows does not allow in a name.
    Forbidden(char),
    /// Ends in a dot or a space, which Windows silently strips.
    TrailingDotOrSpace,
    /// Too long for one path component.
    TooLong,
    /// Starts with a dot, so it would never be listed.
    Hidden,
    /// Does not end in `.md`, `.markdown` or `.txt`.
    NotAText,
    /// Leaves the writing folder (an absolute path, or `..`).
    Outside,
    /// Something by this name is already there.
    Taken,
}

/// Longest name accepted for a new file, in UTF-16 units. Windows allows 255;
/// the rest is room for the temp-file prefix and for " (mine 2026-10-11
/// 143205)-2" style suffixes.
const NAME_LIMIT: usize = 200;

/// True for names ending in one of [`TEXT_ENDINGS`], whatever the case.
pub fn is_text_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ending)) => !stem.is_empty() && TEXT_ENDINGS.iter().any(|e| ending.eq_ignore_ascii_case(e)),
        None => false,
    }
}

fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = upper.chars();
    let head: String = chars.by_ref().take(3).collect();
    let (Some(digit), None) = (chars.next(), chars.next()) else { return false };
    (head == "COM" || head == "LPT") && matches!(digit, '1'..='9' | '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
}

/// Checks one path component against the rules for a name Windows, Explorer
/// and sync tools will all accept: not empty, not a device name, none of
/// `< > : " / \ | ? *` or a control character, no trailing dot or space, not
/// over-long, and not starting with a dot.
pub fn check_name(name: &str) -> Result<(), NameError> {
    if name.trim().is_empty() {
        return Err(NameError::Empty);
    }
    if let Some(bad) = name.chars().find(|&c| (c as u32) < 32 || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')) {
        return Err(NameError::Forbidden(bad));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(NameError::TrailingDotOrSpace);
    }
    if name.starts_with('.') {
        return Err(NameError::Hidden);
    }
    if is_reserved(name) {
        return Err(NameError::Reserved);
    }
    if name.encode_utf16().count() > NAME_LIMIT {
        return Err(NameError::TooLong);
    }
    Ok(())
}

/// Turns a title someone typed into a file name stem that [`check_name`]
/// accepts, with or without a text's ending added to it: forbidden characters
/// become spaces, runs of spaces collapse, leading dots and spaces and
/// trailing dots and spaces go, a device name gains an underscore, and
/// nothing at all becomes `Untitled`. No ending is added.
///
/// Windows judges a device name by what stands before the first dot, so the
/// underscore goes there: `Con. law outline` becomes `Con_. law outline`.
pub fn safe_file_name(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut gap = false;
    for c in title.chars() {
        let bad = (c as u32) < 32 || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_whitespace();
        if bad {
            gap = !out.is_empty();
        } else {
            if gap {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    let mut name: String = out.trim_start_matches(['.', ' ']).to_string();
    while name.encode_utf16().count() > 120 {
        name.pop();
    }
    let mut name = name.trim_end_matches(['.', ' ']).to_string();
    if is_reserved(&name) {
        // What stands before the first dot, less the spaces that end it, is
        // the device name; the underscore goes straight after it.
        let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ').len();
        name.insert(stem, '_');
    }
    // Whatever was typed, what is handed back is a name that will be taken.
    if check_name(&name).is_err() || check_name(&format!("{name}.md")).is_err() {
        return "Untitled".to_string();
    }
    name
}

/// Checks a path given relative to the writing folder and returns it with
/// plain backslash separators. With `strict` (a name being made, not one that
/// already exists) every component must pass [`check_name`] and the last must
/// be a text name.
pub(crate) fn check_rel(rel: &Path, strict: bool) -> Result<PathBuf, NameError> {
    // `a:b.md` reads to Windows as a path on drive A. It is a name with a
    // colon in it, and is refused as that rather than as a path elsewhere.
    if !rel.has_root() && rel.to_string_lossy().contains(':') {
        return Err(NameError::Forbidden(':'));
    }
    let mut out = PathBuf::new();
    let mut last = String::new();
    for part in rel.components() {
        match part {
            Component::Normal(name) => {
                let Some(name) = name.to_str() else { return Err(NameError::Forbidden('\u{FFFD}')) };
                if strict {
                    check_name(name)?;
                }
                last = name.to_string();
                out.push(name);
            }
            Component::CurDir => {}
            _ => return Err(NameError::Outside),
        }
    }
    if last.is_empty() {
        return Err(NameError::Empty);
    }
    if !is_text_name(&last) {
        return Err(NameError::NotAText);
    }
    Ok(out)
}

/// The key a relative path is looked up by: lower case, so that two spellings
/// of one file on a case-blind file system meet.
pub(crate) fn key(rel: &Path) -> String {
    rel.to_string_lossy().to_lowercase()
}

/// Puts an absolute path into the verbatim (`\\?\`) form a rename by handle
/// needs, or says no. A path already in that form comes back unchanged, so
/// the prefix can never be doubled; `\\server\share` becomes
/// `\\?\UNC\server\share`. Relative paths, forward slashes and `.` or `..`
/// components are refused, because in verbatim form Windows would take them
/// literally and fail with error 123.
#[cfg(windows)]
pub(crate) fn verbatim(path: &Path) -> Option<PathBuf> {
    const SEP: u16 = b'\\' as u16;
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let full: Vec<u16> = if wide.starts_with(&prefix) {
        wide
    } else if wide.starts_with(&[SEP, SEP]) {
        let mut v: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        v.extend_from_slice(&wide[2..]);
        v
    } else if wide.len() >= 3 && wide[1] == b':' as u16 && wide[2] == SEP && (wide[0] as u8 as char).is_ascii_alphabetic() && wide[0] < 128 {
        let mut v = prefix.clone();
        v.extend_from_slice(&wide);
        v
    } else {
        return None;
    };
    let body = &full[prefix.len()..];
    if body.is_empty() || body.contains(&(b'/' as u16)) {
        return None;
    }
    let parts: Vec<&[u16]> = body.split(|&c| c == SEP).collect();
    for (i, part) in parts.iter().enumerate() {
        let dots = !part.is_empty() && part.len() <= 2 && part.iter().all(|&c| c == b'.' as u16);
        let empty_inside = part.is_empty() && i + 1 != parts.len();
        if dots || empty_inside {
            return None;
        }
    }
    Some(PathBuf::from(OsString::from_wide(&full)))
}

/// An absolute path with no `.` or `..` in it, unchanged; anything else is
/// refused, as on Windows.
#[cfg(unix)]
pub(crate) fn verbatim(path: &Path) -> Option<PathBuf> {
    let plain = path.is_absolute() && path.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    plain.then(|| path.to_path_buf())
}

/// True when a file is there under this name in any capitals. Texts are
/// told apart without case everywhere, so that a folder means the same on
/// every system: on one where `Notes.md` and `notes.md` can both exist, the
/// second is refused all the same.
pub(crate) fn taken(path: &Path) -> bool {
    if sys::stat(path).is_ok() {
        return true;
    }
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return false };
    let name = name.to_string_lossy().to_lowercase();
    std::fs::read_dir(folder).map(|listing| listing.flatten().any(|entry| entry.file_name().to_string_lossy().to_lowercase() == name)).unwrap_or(false)
}

/// The lock and index key of a writing folder: FNV-1a of its canonical path
/// in lower case, as 16 hex digits.
pub(crate) fn folder_key(root: &Path) -> String {
    format!("{:016x}", hash::fnv1a(root.to_string_lossy().to_lowercase().as_bytes()))
}

/// True when two folder paths name the same place, ignoring case.
pub(crate) fn same_folder(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Splits a file name into stem and ending (`"essay"`, `"md"`). A name with
/// no dot has an empty ending.
pub(crate) fn split_ending(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ending)) if !stem.is_empty() => (stem, ending),
        _ => (name, ""),
    }
}

/// The ending a text's history copies carry: the text's own, or `md`.
pub(crate) fn ending_of(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (_, ending) = split_ending(&name);
    if ending.is_empty() || !ending.chars().all(|c| c.is_ascii_alphanumeric()) {
        "md".to_string()
    } else {
        ending.to_lowercase()
    }
}

/// `2026-10-11 143205` in local time: the date and time written into the name
/// of a copy kept beside a text.
pub(crate) fn name_stamp(unix_ms: u64) -> String {
    let (y, mo, d, h, mi, s) = sys::local_fields(unix_ms);
    format!("{y:04}-{mo:02}-{d:02} {h:02}{mi:02}{s:02}")
}

/// The name of a copy kept beside `path`: `essay (mine 2026-10-11 143205).md`,
/// and with `-2`, `-3` before the ending when `n` is above 1.
pub(crate) fn sibling(path: &Path, tag: &str, unix_ms: u64, n: u32) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ending) = split_ending(&name);
    let mut stem: String = stem.to_string();
    while stem.encode_utf16().count() > 160 {
        stem.pop();
    }
    let count = if n > 1 { format!("-{n}") } else { String::new() };
    let dot = if ending.is_empty() { String::new() } else { format!(".{ending}") };
    path.with_file_name(format!("{stem} ({tag} {}){count}{dot}", name_stamp(unix_ms)))
}

/// `name` with `-2`, `-3` before its ending, for a destination that must not
/// replace anything.
pub(crate) fn numbered(path: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return path.to_path_buf();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ending) = split_ending(&name);
    let dot = if ending.is_empty() { String::new() } else { format!(".{ending}") };
    path.with_file_name(format!("{stem}-{n}{dot}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn verbatim_prefixes_once_and_only_absolute_paths() {
        let v = verbatim(Path::new(r"C:\Users\x\essay.md")).unwrap();
        assert_eq!(v, PathBuf::from(r"\\?\C:\Users\x\essay.md"));
        // Already verbatim: unchanged, so a doubled prefix cannot be built.
        assert_eq!(verbatim(&v).unwrap(), v);
        assert_eq!(verbatim(&verbatim(&v).unwrap()).unwrap(), v);
        assert_eq!(verbatim(Path::new(r"\\server\share\a.md")).unwrap(), PathBuf::from(r"\\?\UNC\server\share\a.md"));
        assert_eq!(verbatim(Path::new(r"\\?\UNC\server\share\a.md")).unwrap(), PathBuf::from(r"\\?\UNC\server\share\a.md"));
    }

    #[test]
    #[cfg(windows)]
    fn verbatim_refuses_what_windows_would_take_literally() {
        for bad in [r"essay.md", r"sub\essay.md", r"C:essay.md", r"C:\a\.\b.md", r"C:\a\..\b.md", r"C:/a/b.md", r"\\?\C:\a\.\b.md", r"\\?\C:\a\\b.md", r"\a\b.md", r""] {
            assert!(verbatim(Path::new(bad)).is_none(), "{bad} should be refused");
        }
        // A trailing separator is harmless.
        assert!(verbatim(Path::new(r"C:\a\")).is_some());
    }

    #[test]
    fn device_names_are_refused_whatever_follows() {
        for bad in ["CON", "con", "PRN.md", "aux.txt", "NUL", "nul.tar.md", "COM1", "com9.md", "LPT1.txt", "lpt5", "COM\u{00B9}.md", "CON .md"] {
            assert_eq!(check_name(bad), Err(NameError::Reserved), "{bad}");
        }
        for fine in ["CONSOLE.md", "COM0.md", "COM10.md", "LPT.md", "console", "auxiliary.txt", "nulls.md", "COMA.md"] {
            assert_eq!(check_name(fine), Ok(()), "{fine}");
        }
    }

    #[test]
    fn forbidden_characters_and_trailing_dots_are_refused() {
        for (bad, c) in [("a<b", '<'), ("a>b", '>'), ("a:b", ':'), ("a\"b", '"'), ("a/b", '/'), ("a\\b", '\\'), ("a|b", '|'), ("a?b", '?'), ("a*b", '*'), ("a\tb", '\t'), ("a\u{0}b", '\u{0}')] {
            assert_eq!(check_name(bad), Err(NameError::Forbidden(c)), "{bad:?}");
        }
        assert_eq!(check_name("essay."), Err(NameError::TrailingDotOrSpace));
        assert_eq!(check_name("essay "), Err(NameError::TrailingDotOrSpace));
        assert_eq!(check_name(""), Err(NameError::Empty));
        assert_eq!(check_name("   "), Err(NameError::Empty));
        assert_eq!(check_name(".hidden.md"), Err(NameError::Hidden));
        assert_eq!(check_name(&"x".repeat(201)), Err(NameError::TooLong));
        assert_eq!(check_name(&"x".repeat(200)), Ok(()));
        assert_eq!(check_name("mitsqanaqan\u{0313}.md"), Ok(()));
        assert_eq!(check_name("\u{6587}\u{7AE0}.md"), Ok(()));
    }

    #[test]
    fn titles_become_names_that_pass() {
        let cases = [
            ("Rebuttal notes", "Rebuttal notes"),
            ("  What is \"justice\"?  ", "What is justice"),
            ("a/b\\c:d", "a b c d"),
            ("...", "Untitled"),
            ("", "Untitled"),
            ("CON", "CON_"),
            ("lpt3", "lpt3_"),
            ("trailing dots...", "trailing dots"),
            (".env notes", "env notes"),
            ("tabs\tand\nbreaks", "tabs and breaks"),
            ("many     spaces", "many spaces"),
        ];
        for (title, want) in cases {
            let got = safe_file_name(title);
            assert_eq!(got, want, "{title:?}");
            assert_eq!(check_name(&got), Ok(()), "{got:?}");
            assert_eq!(check_name(&format!("{got}.md")), Ok(()), "{got:?}");
        }
        let long = safe_file_name(&"word ".repeat(100));
        assert!(long.encode_utf16().count() <= 120);
        assert_eq!(check_name(&long), Ok(()));
    }

    #[test]
    #[cfg(windows)]
    fn relative_paths_stay_inside_the_folder() {
        assert_eq!(check_rel(Path::new("essay.md"), true), Ok(PathBuf::from("essay.md")));
        assert_eq!(check_rel(Path::new("College/The orchard.md"), true), Ok(PathBuf::from(r"College\The orchard.md")));
        assert_eq!(check_rel(Path::new(r".\a.txt"), true), Ok(PathBuf::from("a.txt")));
        assert_eq!(check_rel(Path::new(r"..\a.md"), false), Err(NameError::Outside));
        assert_eq!(check_rel(Path::new(r"C:\a.md"), false), Err(NameError::Outside));
        assert_eq!(check_rel(Path::new(r"\a.md"), false), Err(NameError::Outside));
        assert_eq!(check_rel(Path::new("a:b.md"), true), Err(NameError::Forbidden(':')));
        assert_eq!(check_rel(Path::new(r"sub\C:x.md"), false), Err(NameError::Forbidden(':')));
        assert_eq!(check_rel(Path::new("notes.docx"), false), Err(NameError::NotAText));
        assert_eq!(check_rel(Path::new("md"), false), Err(NameError::NotAText));
        assert_eq!(check_rel(Path::new(""), false), Err(NameError::Empty));
        assert_eq!(check_rel(Path::new(r"CON\a.md"), true), Err(NameError::Reserved));
        // An existing file with an awkward but legal name can still be opened.
        assert!(check_rel(Path::new(".drafts.md"), false).is_ok());
        assert_eq!(check_rel(Path::new(".drafts.md"), true), Err(NameError::Hidden));
    }

    #[test]
    fn text_names_are_recognised_by_ending() {
        for yes in ["a.md", "A.MD", "notes.markdown", "x.y.txt", "The orchard.Md"] {
            assert!(is_text_name(yes), "{yes}");
        }
        for no in ["a.mdx", "md", ".md", "a.doc", "a", "a.md.bak"] {
            assert!(!is_text_name(no), "{no}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn sibling_and_numbered_names() {
        let p = Path::new(r"C:\w\essay.md");
        let s = sibling(p, "mine", 0, 1);
        let name = s.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("essay (mine ") && name.ends_with(").md"), "{name}");
        let s2 = sibling(p, "mine", 0, 2);
        assert!(s2.file_name().unwrap().to_string_lossy().ends_with(")-2.md"));
        assert_eq!(check_name(&name), Ok(()));
        assert_eq!(numbered(p, 1), p);
        assert_eq!(numbered(p, 3), Path::new(r"C:\w\essay-3.md"));
        assert_eq!(ending_of(p), "md");
        assert_eq!(ending_of(Path::new("X.TXT")), "txt");
        assert_eq!(ending_of(Path::new("noending")), "md");
        assert_eq!(key(Path::new(r"College\The Orchard.MD")), r"college\the orchard.md");
    }
}
