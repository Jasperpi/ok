//! Links between texts: finding `[[name]]` in a line, comparing names, and
//! an index of which text links to which.
//!
//! A text's name is its path relative to the writing folder, with forward
//! slashes and no extension, for example `essays/common app`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::markup::{self, LineKind};

/// One `[[...]]` in a line.
///
/// `start..end` is the whole link, brackets included. `target` is trimmed
/// and has neither the `#heading` nor the `|shown` part: `[[a#b|c]]` has
/// target `a`, heading `b` and shown `c`. A part that is missing or empty is
/// `None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LinkRef<'a> {
    pub start: usize,
    pub end: usize,
    pub target: &'a str,
    pub shown: Option<&'a str>,
    pub heading: Option<&'a str>,
}

/// The links in one line, in order.
///
/// A link needs a target, so `[[]]` and `[[ ]]` are not links. Links do not
/// nest: of `[[a [[b]]` only `[[b]]` is one. Nothing inside a backtick code
/// span is a link, nor is a `[[` whose first bracket is escaped with a
/// backslash, nor anything on a line that is a fence or a rule.
///
/// The line is read as if no fence were open above it; whoever walks a whole
/// text must leave out the lines inside fenced code, as [`LinkIndex::set`]
/// does.
pub fn links_in(line: &str) -> Vec<LinkRef<'_>> {
    if !line.contains("[[") {
        return Vec::new();
    }
    let kind = markup::classify_line(line, false);
    markup::wikis(line, kind).iter().map(|wiki| wiki.link(line)).collect()
}

/// The link with `start <= col < end`, if the column is on one.
pub fn link_at(line: &str, col: usize) -> Option<LinkRef<'_>> {
    links_in(line).into_iter().find(|link| link.start <= col && col < link.end)
}

/// The comparison form of a name, under which two spellings of the same
/// text are equal: backslashes become forward slashes, whitespace and
/// slashes at either end go, a trailing `.md`, `.markdown` or `.txt` in any
/// case goes, each run of whitespace becomes one space, and everything is
/// lowercased.
///
/// Characters that look the same on screen also compare the same, so a link
/// pasted from another program cannot quietly start a twin of an existing
/// text: curly and modifier apostrophes become `'`, curly double quotes become
/// `"`, and zero-width characters and soft hyphens are dropped. (No-break
/// spaces already count as whitespace.)
pub fn key(name: &str) -> String {
    // Look-alikes are folded first, so that dropping an invisible character
    // can never leave a space or a slash stranded at either end.
    let plain: String = name
        .chars()
        .filter(|c| !matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}'))
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            c => c,
        })
        .collect();
    let edge = |c: char| c.is_whitespace() || c == '/' || c == '\\';
    let mut name = plain.trim_matches(edge);
    for ext in [".markdown", ".md", ".txt"] {
        let Some(cut) = name.len().checked_sub(ext.len()) else { continue };
        if name.get(cut..).is_some_and(|tail| tail.eq_ignore_ascii_case(ext)) {
            name = name.get(..cut).unwrap_or(name).trim_end_matches(edge);
            break;
        }
    }
    let mut folded = String::with_capacity(name.len());
    let mut gap = false;
    for c in name.chars() {
        if c.is_whitespace() {
            gap = true;
            continue;
        }
        if gap {
            folded.push(' ');
            gap = false;
        }
        folded.push(if c == '\\' { '/' } else { c });
    }
    folded.to_lowercase()
}

/// Where the parts of one `[[...]]` sit in its line, as byte ranges.
pub(crate) struct Wiki {
    pub(crate) start: usize,
    pub(crate) end: usize,
    target: (usize, usize),
    heading: Option<(usize, usize)>,
    shown: Option<(usize, usize)>,
}

impl Wiki {
    /// Reads the link whose `[[` begins at byte `at`.
    ///
    /// The search for `]]` gives up at another `[[`, which both stops links
    /// nesting and keeps a line full of unclosed openers from being read to
    /// its end once for each of them. Of three opening brackets the last two
    /// are the opener.
    pub(crate) fn at(line: &str, at: usize) -> Option<Wiki> {
        let bytes = line.as_bytes();
        let open = at + 2;
        if bytes.get(at..open) != Some(b"[[".as_slice()) || bytes.get(open) == Some(&b'[') {
            return None;
        }
        let mut close = open;
        loop {
            match (*bytes.get(close)?, bytes.get(close + 1)) {
                (b']', Some(b']')) => break,
                (b'[', Some(b'[')) => return None,
                _ => close += 1,
            }
        }
        let inner = line.get(open..close)?;
        let (left, shown) = match inner.split_once('|') {
            Some((left, shown)) => (left, Some(trimmed(shown, open + left.len() + 1))),
            None => (inner, None),
        };
        let (name, heading) = match left.split_once('#') {
            Some((name, heading)) => (name, Some(trimmed(heading, open + name.len() + 1))),
            None => (left, None),
        };
        let filled = |range: &(usize, usize)| range.0 < range.1;
        let target = trimmed(name, open);
        filled(&target).then_some(Wiki {
            start: at,
            end: close + 2,
            target,
            heading: heading.filter(filled),
            shown: shown.filter(filled),
        })
    }

    /// The stretch a reader sees as the link: the shown text if there is
    /// any, otherwise the target with its heading.
    pub(crate) fn visible(&self) -> (usize, usize) {
        self.shown.unwrap_or((self.target.0, self.heading.map_or(self.target.1, |heading| heading.1)))
    }

    fn link<'a>(&self, line: &'a str) -> LinkRef<'a> {
        let part = |range: (usize, usize)| line.get(range.0..range.1).unwrap_or("");
        LinkRef {
            start: self.start,
            end: self.end,
            target: part(self.target),
            shown: self.shown.map(part),
            heading: self.heading.map(part),
        }
    }
}

/// The byte range of `part` without the whitespace at its ends, given that
/// `part` starts at byte `base` of its line.
fn trimmed(part: &str, base: usize) -> (usize, usize) {
    let start = base + part.len() - part.trim_start().len();
    (start, start + part.trim().len())
}

/// The lines of a whole file, each with the byte it starts at. Lines end at
/// `\n`, `\r\n` or a lone `\r`, and a byte-order mark at the top is not part
/// of the first line.
fn lines_of(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let bytes = text.as_bytes();
    let mut next = Some(if text.starts_with('\u{FEFF}') { '\u{FEFF}'.len_utf8() } else { 0 });
    std::iter::from_fn(move || {
        let start = next?;
        let len = bytes.get(start..)?.iter().take_while(|&&b| b != b'\n' && b != b'\r').count();
        let end = start + len;
        next = match (bytes.get(end), bytes.get(end + 1)) {
            (Some(b'\r'), Some(b'\n')) => Some(end + 2),
            (Some(_), _) => Some(end + 1),
            (None, _) => None,
        };
        Some((start, text.get(start..end).unwrap_or("")))
    })
}

/// One link out of a text: the target as it was first written there, and
/// the name of the text it leads to, if any.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outgoing {
    pub target: String,
    pub resolved: Option<String>,
}

/// A link target as a text wrote it, with its key.
#[derive(Clone, Debug)]
struct Target {
    written: String,
    key: String,
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    /// One per distinct target key, in order of first appearance.
    links: Vec<Target>,
}

/// Which text links to which, across the writing folder.
///
/// Texts are told apart by the [`key`] of their name, so two spellings of
/// one name are one text and the latest spelling is the one handed back. A
/// name whose key is empty names nothing and is ignored.
///
/// Links are stored as written and resolved when asked, because what a link
/// leads to changes as texts come and go. A link whose target has an empty
/// key, such as `[[/]]`, is a link like any other: it leads nowhere, and is
/// listed among a text's outgoing links and among the dangling ones. Every
/// question is answered from ordered maps without walking the folder.
#[derive(Clone, Debug, Default)]
pub struct LinkIndex {
    /// Every text, by the key of its name.
    texts: BTreeMap<String, Entry>,
    /// The key of a last path component, to the texts whose name ends with
    /// it: shallowest first, then alphabetically.
    leaves: BTreeMap<String, BTreeSet<(usize, String)>>,
    /// The key of a link target, to the texts that link to it: each by its
    /// own key, with where the link sits among that text's links.
    sources: BTreeMap<String, BTreeMap<String, usize>>,
}

impl LinkIndex {
    pub fn new() -> LinkIndex {
        LinkIndex::default()
    }

    /// Indexes one text, or indexes it afresh. `text` is the whole file;
    /// links inside fenced code are left out.
    pub fn set(&mut self, name: &str, text: &str) {
        let name_key = key(name);
        if !name_key.is_empty() {
            self.put(name_key, name.to_string(), targets_in(text));
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.take(&key(name));
    }

    /// Gives a text a new name; it keeps its outgoing links. A text already
    /// under the new name is replaced, as the file would be. Nothing happens
    /// when `old` is not known. Links to the old name in other texts are
    /// left as they are: see [`retarget`].
    pub fn rename(&mut self, old: &str, new: &str) {
        let new_key = key(new);
        if new_key.is_empty() {
            return;
        }
        if let Some(entry) = self.take(&key(old)) {
            self.put(new_key, new.to_string(), entry.links);
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.texts.contains_key(&key(name))
    }

    /// Every name, sorted by key.
    pub fn names(&self) -> Vec<&str> {
        self.texts.values().map(|entry| entry.name.as_str()).collect()
    }

    /// The text a link target leads to: the name whose key equals
    /// `key(target)`; failing that, a name whose last path component has
    /// that key, so that `[[common app]]` finds `essays/common app`. When
    /// several names end that way, the one with the fewest path components
    /// wins, then the first alphabetically by key.
    pub fn resolve(&self, target: &str) -> Option<&str> {
        self.find(&key(target)).map(|(_, entry)| entry.name.as_str())
    }

    /// What a text links to, in order of first appearance, one entry for
    /// each distinct target key.
    pub fn outgoing(&self, name: &str) -> Vec<Outgoing> {
        let Some(entry) = self.texts.get(&key(name)) else { return Vec::new() };
        entry
            .links
            .iter()
            .map(|link| Outgoing {
                target: link.written.clone(),
                resolved: self.find(&link.key).map(|(_, entry)| entry.name.clone()),
            })
            .collect()
    }

    /// The names of the other texts holding a link that resolves to this
    /// one, sorted by key.
    pub fn incoming(&self, name: &str) -> Vec<&str> {
        let Some((name_key, _)) = self.texts.get_key_value(&key(name)) else { return Vec::new() };
        // Only two targets can lead here: the full name, and the last
        // component when this text is the one that component resolves to.
        let mut from: BTreeSet<&str> = BTreeSet::new();
        let (leaf, _) = leaf_of(name_key);
        for target in [name_key.as_str(), leaf.as_str()] {
            if self.find(target).is_some_and(|(found, _)| found == name_key) {
                from.extend(self.sources.get(target).into_iter().flat_map(BTreeMap::keys).map(String::as_str));
            }
        }
        from.remove(name_key.as_str());
        from.into_iter().filter_map(|source| self.texts.get(source)).map(|entry| entry.name.as_str()).collect()
    }

    /// The distinct targets that resolve to nothing, sorted by key. Each is
    /// spelled as it is first written in the first text, by key, that has it.
    pub fn dangling(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter(|(target, _)| self.find(target).is_none())
            .filter_map(|(_, from)| {
                let (source, &at) = from.first_key_value()?;
                Some(self.texts.get(source)?.links.get(at)?.written.clone())
            })
            .collect()
    }

    /// The text a target key leads to, with that text's own key.
    fn find(&self, target: &str) -> Option<(&String, &Entry)> {
        if let Some(found) = self.texts.get_key_value(target) {
            return Some(found);
        }
        let (_, best) = self.leaves.get(target)?.first()?;
        self.texts.get_key_value(best)
    }

    fn put(&mut self, name_key: String, name: String, links: Vec<Target>) {
        self.take(&name_key);
        for (at, link) in links.iter().enumerate() {
            self.sources.entry(link.key.clone()).or_default().insert(name_key.clone(), at);
        }
        let (leaf, depth) = leaf_of(&name_key);
        self.leaves.entry(leaf).or_default().insert((depth, name_key.clone()));
        self.texts.insert(name_key, Entry { name, links });
    }

    /// Takes a text out, along with every trace of it in the other maps.
    fn take(&mut self, name_key: &str) -> Option<Entry> {
        let entry = self.texts.remove(name_key)?;
        for link in &entry.links {
            if let Some(from) = self.sources.get_mut(&link.key) {
                from.remove(name_key);
                if from.is_empty() {
                    self.sources.remove(&link.key);
                }
            }
        }
        let (leaf, depth) = leaf_of(name_key);
        if let Some(ending) = self.leaves.get_mut(&leaf) {
            ending.remove(&(depth, name_key.to_string()));
            if ending.is_empty() {
                self.leaves.remove(&leaf);
            }
        }
        Some(entry)
    }
}

/// The last path component of a name's key, and how many components the key
/// has.
fn leaf_of(name_key: &str) -> (String, usize) {
    let mut parts = name_key.split('/').map(str::trim).filter(|part| !part.is_empty());
    let depth = parts.clone().count();
    (parts.next_back().unwrap_or("").to_string(), depth)
}

/// Every link of a whole file in order, leaving out fenced code: the byte
/// its line starts at, the line, and where in the line the link sits.
fn links_of(text: &str) -> impl Iterator<Item = (usize, &str, Wiki)> {
    let mut in_fence = false;
    lines_of(text).flat_map(move |(at, line)| {
        let kind = markup::classify_line(line, in_fence);
        in_fence ^= kind == LineKind::Fence;
        let wikis = if line.contains("[[") { markup::wikis(line, kind) } else { Vec::new() };
        wikis.into_iter().map(move |wiki| (at, line, wiki))
    })
}

/// The distinct link targets of a whole file in order of first appearance.
fn targets_in(text: &str) -> Vec<Target> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for (_, line, wiki) in links_of(text) {
        let written = wiki.link(line).target;
        let target_key = key(written);
        if seen.insert(target_key.clone()) {
            found.push(Target { written: written.to_string(), key: target_key });
        }
    }
    found
}

/// For renames: rewrites every link in `text` whose target key equals
/// `key(old)` so that it points at `new`, written as given. Any `#heading`
/// and `|shown` parts stay, and so does every other byte, line endings
/// included. Links inside fenced code are not links and are left alone.
///
/// `None` when nothing changed. That includes an `old` or `new` whose key
/// is empty, and a `new` that cannot be written here as a link. A name
/// holding `#`, `|`, `[[`, `]]` or a line break never can: between the
/// brackets it reads as a link to something else, or as none. One holding a
/// backtick cannot on a line with another backtick for it to pair with. The
/// whole text is then left pointing at `old`, which is better than having
/// it quietly point somewhere new.
pub fn retarget(text: &str, old: &str, new: &str) -> Option<String> {
    let old_key = key(old);
    if old_key.is_empty() || key(new).is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut copied = 0;
    // What each link of the text is to point at when this is done.
    let mut wanted = Vec::new();
    let mut changed = false;
    for (at, line, wiki) in links_of(text) {
        let written = wiki.link(line).target;
        if written == new || key(written) != old_key {
            wanted.push(written);
            continue;
        }
        if !changed {
            out.reserve(text.len() + new.len());
            changed = true;
        }
        out.push_str(text.get(copied..at + wiki.target.0)?);
        out.push_str(new);
        copied = at + wiki.target.1;
        wanted.push(new.trim());
    }
    if !changed {
        return None;
    }
    out.push_str(text.get(copied..)?);
    // The proof that `new` could be written: read again, the text holds the
    // same links as before, each pointing where it should.
    let reads_back = links_of(&out).map(|(_, line, wiki)| wiki.link(line).target).eq(wanted);
    reads_back.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u32);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as usize % n.max(1)
        }

        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[self.below(from.len())]
        }
    }

    fn link<'a>(
        start: usize,
        end: usize,
        target: &'a str,
        shown: Option<&'a str>,
        heading: Option<&'a str>,
    ) -> LinkRef<'a> {
        LinkRef { start, end, target, shown, heading }
    }

    #[test]
    fn finds_links_and_their_parts() {
        assert_eq!(links_in("see [[a]] and [[b c]]."), [link(4, 9, "a", None, None), link(14, 21, "b c", None, None)]);
        assert_eq!(links_in("[[a#b|c]]"), [link(0, 9, "a", Some("c"), Some("b"))]);
        assert_eq!(links_in("[[ a # b | c ]]"), [link(0, 15, "a", Some("c"), Some("b"))]);
        assert_eq!(links_in("[[a|c#d]]"), [link(0, 9, "a", Some("c#d"), None)]);
        assert_eq!(links_in("[[a#b#c]]"), [link(0, 9, "a", None, Some("b#c"))]);
        assert_eq!(links_in("[[a#|]]"), [link(0, 7, "a", None, None)]);
        assert_eq!(links_in("[[essays/common app]]"), [link(0, 21, "essays/common app", None, None)]);
        assert_eq!(links_in("\u{00E9}[[\u{4E00}|\u{00E9}]]"), [link(2, 12, "\u{4E00}", Some("\u{00E9}"), None)]);
    }

    #[test]
    fn things_that_are_not_links() {
        for line in ["", "[[]]", "[[ ]]", "[[|x]]", "[[#h]]", "[[a", "a]]", "[a]", "`[[a]]`", "``[[a]] ` ``", "\\[[a]]"] {
            assert_eq!(links_in(line), [], "{line:?}");
        }
        // No nesting: the inner pair is the link.
        assert_eq!(links_in("[[a [[b]] c]]"), [link(4, 9, "b", None, None)]);
        assert_eq!(links_in("[[[a]]]"), [link(1, 6, "a", None, None)]);
        // An unclosed backtick is not code.
        assert_eq!(links_in("` [[a]]"), [link(2, 7, "a", None, None)]);
        assert_eq!(links_in("`x` [[a]] `y`"), [link(4, 9, "a", None, None)]);
        // Fence and rule lines hold no inline text.
        assert_eq!(links_in("``` [[a]]"), []);
        // Block markers do not get in the way.
        assert_eq!(links_in("- [ ] [[a]]"), [link(6, 11, "a", None, None)]);
        assert_eq!(links_in("## [[a]] ##"), [link(3, 8, "a", None, None)]);
        assert_eq!(links_in("> *[[a]]*"), [link(3, 8, "a", None, None)]);
    }

    #[test]
    fn link_at_takes_any_column_inside_the_brackets() {
        let line = "ab [[cd]] ef";
        assert_eq!(link_at(line, 2), None);
        assert_eq!(link_at(line, 3).map(|l| l.target), Some("cd"));
        assert_eq!(link_at(line, 8).map(|l| l.target), Some("cd"));
        assert_eq!(link_at(line, 9), None);
        assert_eq!(link_at(line, 999), None);
        assert_eq!(link_at("", 0), None);
    }

    #[test]
    fn key_is_the_comparison_form() {
        assert_eq!(key("Essays\\Common App.MD"), "essays/common app");
        assert_eq!(key("  /notes/ideas/  "), "notes/ideas");
        assert_eq!(key("a   \t b.txt"), "a b");
        assert_eq!(key("Draft.Markdown"), "draft");
        assert_eq!(key("notes.md.txt"), "notes.md");
        assert_eq!(key("read me .md"), "read me");
        assert_eq!(key("\u{00C9}t\u{00E9}"), "\u{00E9}t\u{00E9}");
        assert_eq!(key("md"), "md");
        assert_eq!(key("a.mdx"), "a.mdx");
        assert_eq!(key("\u{4E00}md"), "\u{4E00}md");
        assert_eq!(key(""), "");
        assert_eq!(key(" / \\ "), "");
        assert_eq!(key(".md"), "");
    }

    #[test]
    fn resolves_by_full_name_then_by_last_component() {
        let mut index = LinkIndex::new();
        for name in ["essays/common app", "drafts/old/common app", "drafts/notes", "Notes", "zebra/x/leaf", "apple/y/leaf"] {
            index.set(name, "");
        }
        assert_eq!(index.resolve("Essays/Common App.md"), Some("essays/common app"));
        assert_eq!(index.resolve("common  app"), Some("essays/common app"));
        assert_eq!(index.resolve("notes"), Some("Notes"));
        assert_eq!(index.resolve("drafts\\notes"), Some("drafts/notes"));
        assert_eq!(index.resolve("leaf"), Some("apple/y/leaf"));
        assert_eq!(index.resolve("old/common app"), None);
        assert_eq!(index.resolve("missing"), None);
        assert_eq!(index.resolve(""), None);
        assert_eq!(index.names(), ["apple/y/leaf", "drafts/notes", "drafts/old/common app", "essays/common app", "Notes", "zebra/x/leaf"]);
        index.remove("NOTES");
        assert_eq!(index.resolve("notes"), Some("drafts/notes"));
        assert!(index.contains("Drafts/Notes.txt") && !index.contains("notes"));
    }

    #[test]
    fn outgoing_incoming_and_dangling() {
        let mut index = LinkIndex::new();
        index.set("a", "to [[b]], [[B|again]], [[Ghost]] and [[c#part]]\n```\n[[fenced]]\n```\n[[a]]");
        index.set("b", "back to [[a]]\r\n`[[coded]]` and [[ghost]] and [[Other Ghost]]");
        index.set("dir/c", "[[b]]");
        let out = |target: &str, resolved: Option<&str>| Outgoing {
            target: target.to_string(),
            resolved: resolved.map(str::to_string),
        };
        assert_eq!(index.outgoing("a"), [out("b", Some("b")), out("Ghost", None), out("c", Some("dir/c")), out("a", Some("a"))]);
        assert_eq!(index.outgoing("nobody"), []);
        assert_eq!(index.incoming("a"), ["b"]);
        assert_eq!(index.incoming("B"), ["a", "dir/c"]);
        assert_eq!(index.incoming("dir/c"), ["a"]);
        assert_eq!(index.incoming("nobody"), [] as [&str; 0]);
        assert_eq!(index.dangling(), ["Ghost", "Other Ghost"]);

        // A new text can take a link away from the one that had it by its
        // last component, and make a dangling link whole.
        index.set("c", "");
        index.set("ghost", "");
        assert_eq!(index.incoming("dir/c"), [] as [&str; 0]);
        assert_eq!(index.incoming("c"), ["a"]);
        assert_eq!(index.incoming("ghost"), ["a", "b"]);
        assert_eq!(index.dangling(), ["Other Ghost"]);

        index.rename("b", "letters/bee");
        assert_eq!(index.names(), ["a", "c", "dir/c", "ghost", "letters/bee"]);
        assert_eq!(index.outgoing("letters/bee").len(), 3);
        assert_eq!(index.incoming("ghost"), ["a", "letters/bee"]);
        assert_eq!(index.dangling(), ["b", "Other Ghost"]);
        index.rename("nobody", "somebody");
        assert!(!index.contains("somebody"));
    }

    /// A target such as `/` has an empty key: no text can have that name,
    /// so the link dangles, and says so.
    #[test]
    fn a_link_that_names_nothing_is_still_a_link_that_dangles() {
        let mut index = LinkIndex::new();
        index.set("a", "[[b]] [[/]] [[.md]] [[ \\ ]] [[missing]] [[]]");
        index.set("b", "[[.txt]]");
        let out = |target: &str, resolved: Option<&str>| Outgoing {
            target: target.to_string(),
            resolved: resolved.map(str::to_string),
        };
        assert_eq!(index.outgoing("a"), [out("b", Some("b")), out("/", None), out("missing", None)]);
        assert_eq!(index.outgoing("b"), [out(".txt", None)]);
        assert_eq!(index.dangling(), ["/", "missing"]);
        assert_eq!(index.resolve("/"), None);
        assert_eq!(index.incoming("b"), ["a"]);
        index.remove("a");
        assert_eq!(index.dangling(), [".txt"]);
        index.remove("b");
        assert_eq!(index.dangling(), [] as [&str; 0]);
    }

    #[test]
    fn retarget_rewrites_only_the_target() {
        let text = "\u{FEFF}# [[Old]] and [[ old.md #sec | shown ]]\r\nkeep [[older]] `[[old]]`\r```\n[[old]]\n```\n[[OLD|x]]";
        let want = "\u{FEFF}# [[new/name]] and [[ new/name #sec | shown ]]\r\nkeep [[older]] `[[old]]`\r```\n[[old]]\n```\n[[new/name|x]]";
        assert_eq!(retarget(text, "old", "new/name").as_deref(), Some(want));
        assert_eq!(retarget(text, "missing", "new"), None);
        assert_eq!(retarget("[[Old]]", "old", "Old"), None);
        assert_eq!(retarget("[[old]]", "old", "Old").as_deref(), Some("[[Old]]"));
        assert_eq!(retarget("[[old]]", "", "new"), None);
        assert_eq!(retarget("[[old]]", "old", " "), None);
        assert_eq!(retarget("", "old", "new"), None);
    }

    #[test]
    fn retarget_writes_only_names_a_link_can_hold() {
        // Written between the brackets, each of these would be a link to
        // something else, or no link at all.
        for new in ["x]]y", "C# notes", "a|b", "a#", "a|", "[a", "a]", "a[[b", "two\nlines", "two\rlines", "#", "|"] {
            assert_eq!(retarget("see [[old]] and [[old|o]]", "old", new), None, "{new:?}");
        }
        // Anything else is written as given, and reads back as itself.
        for new in ["x]y", "a [b] c", "back\\slash", " padded ", "tick`s", "notes/2024 (old).md", "\u{00E9}\u{4E00}"] {
            let after = retarget("see [[old]].", "old", new).unwrap_or_default();
            assert_eq!(after, format!("see [[{new}]]."), "{new:?}");
            let found = links_in(&after);
            assert_eq!(found.len(), 1, "{new:?}");
            assert_eq!(key(found[0].target), key(new), "{new:?}");
            assert_eq!((found[0].heading, found[0].shown), (None, None), "{new:?}");
        }
        // A backtick in a name is fine until the line has another for it to
        // close, which would turn the link into code.
        assert_eq!(retarget("a ` and [[old]]", "old", "tick`s"), None);
        assert_eq!(retarget("a `b` and [[old]]", "old", "tick`s").as_deref(), Some("a `b` and [[tick`s]]"));
        // One link that cannot be written leaves the whole text alone.
        assert_eq!(retarget("[[old]]\n` [[old]]", "old", "tick`s"), None);
    }

    #[test]
    fn lines_of_understands_every_line_ending() {
        let all = |text| lines_of(text).collect::<Vec<_>>();
        assert_eq!(all(""), [(0, "")]);
        assert_eq!(all("a\nb\r\nc\rd"), [(0, "a"), (2, "b"), (5, "c"), (7, "d")]);
        assert_eq!(all("a\n"), [(0, "a"), (2, "")]);
        assert_eq!(all("\r\r\n"), [(0, ""), (1, ""), (3, "")]);
        assert_eq!(all("\u{FEFF}```"), [(3, "```")]);
    }

    const NAMES: &[&str] = &["a", "b", "notes/a", "notes/b", "deep/notes/a", "A", "Notes\\B.md", "c d", "x/c  d", "x/y/z", "z", ""];

    const TEXTS: &[&str] = &[
        "[[a]]",
        " and [[notes/a|this]] then [[B#head]] ",
        "[[missing]] [[Missing]] [[z]]",
        "\n```\n[[a]] [[hidden]]\n```\n",
        "\r\n",
        "`[[a]]` [[c  d]]",
        "[[x/c d.md]] [[A.txt]]",
        "plain words",
        "[[deep/notes/a]] [[notes\\b]]",
        "\n~~~\n[[z]]\n",
        "- [ ] [[lost|shown]] *[[b]]*",
        "[[/]] [[]]",
    ];

    /// A slow index that answers every question by reading every text.
    #[derive(Default)]
    struct Model {
        texts: Vec<(String, String)>,
    }

    impl Model {
        fn at(&self, name_key: &str) -> Option<usize> {
            self.texts.iter().position(|(name, _)| key(name) == name_key)
        }

        fn set(&mut self, name: &str, text: &str) {
            if key(name).is_empty() {
                return;
            }
            self.remove(name);
            self.texts.push((name.to_string(), text.to_string()));
        }

        fn remove(&mut self, name: &str) -> Option<String> {
            let at = self.at(&key(name))?;
            Some(self.texts.remove(at).1)
        }

        fn rename(&mut self, old: &str, new: &str) {
            if key(new).is_empty() {
                return;
            }
            if let Some(text) = self.remove(old) {
                self.set(new, &text);
            }
        }

        fn resolve(&self, target: &str) -> Option<String> {
            let target = key(target);
            if target.is_empty() {
                return None;
            }
            if let Some(at) = self.at(&target) {
                return Some(self.texts[at].0.clone());
            }
            self.texts
                .iter()
                .filter(|(name, _)| key(name).rsplit('/').next().map(str::trim) == Some(target.as_str()))
                .min_by_key(|(name, _)| (key(name).split('/').count(), key(name)))
                .map(|(name, _)| name.clone())
        }

        /// The targets of a text as written, one per key, in order.
        fn targets(text: &str) -> Vec<String> {
            let mut found: Vec<String> = Vec::new();
            let mut in_fence = false;
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            for line in text.split('\n') {
                let kind = markup::classify_line(line, in_fence);
                in_fence ^= kind == LineKind::Fence;
                if matches!(kind, LineKind::Fence | LineKind::Code) {
                    continue;
                }
                for link in links_in(line) {
                    let new = key(link.target);
                    if found.iter().all(|seen| key(seen) != new) {
                        found.push(link.target.to_string());
                    }
                }
            }
            found
        }

        fn sorted(&self) -> Vec<&(String, String)> {
            let mut all: Vec<_> = self.texts.iter().collect();
            all.sort_by_key(|(name, _)| key(name));
            all
        }

        fn incoming(&self, name: &str) -> Vec<String> {
            let Some(at) = self.at(&key(name)) else { return Vec::new() };
            let me = &self.texts[at].0;
            self.sorted()
                .into_iter()
                .filter(|(other, text)| other != me && Model::targets(text).iter().any(|t| self.resolve(t).as_ref() == Some(me)))
                .map(|(other, _)| other.clone())
                .collect()
        }

        fn dangling(&self) -> Vec<String> {
            let mut found: Vec<String> = Vec::new();
            for (_, text) in self.sorted() {
                for target in Model::targets(text) {
                    if self.resolve(&target).is_none() && found.iter().all(|seen| key(seen) != key(&target)) {
                        found.push(target);
                    }
                }
            }
            found.sort_by_key(|target| key(target));
            found
        }
    }

    #[test]
    fn random_edits_match_a_slow_model() {
        let mut rng = Rng(0x2545_F491);
        let mut index = LinkIndex::new();
        let mut model = Model::default();
        let (mut by_leaf, mut linked_from, mut dangled) = (0, 0, 0);
        for step in 0..4000 {
            match rng.below(10) {
                0..=5 => {
                    let name = rng.pick(NAMES);
                    let text: String = (0..rng.below(5)).map(|_| rng.pick(TEXTS)).collect();
                    index.set(name, &text);
                    model.set(name, &text);
                }
                6..=7 => {
                    let name = rng.pick(NAMES);
                    index.remove(name);
                    model.remove(name);
                }
                _ => {
                    let (old, new) = (rng.pick(NAMES), rng.pick(NAMES));
                    index.rename(old, new);
                    model.rename(old, new);
                }
            }

            let names: Vec<&str> = model.sorted().into_iter().map(|(name, _)| name.as_str()).collect();
            assert_eq!(index.names(), names, "step {step}");
            for name in NAMES {
                assert_eq!(index.contains(name), model.at(&key(name)).is_some());
                assert_eq!(index.resolve(name).map(str::to_string), model.resolve(name), "step {step} {name:?}");
                assert_eq!(index.incoming(name), model.incoming(name), "step {step} {name:?}");
                let outgoing: Vec<Outgoing> = match model.at(&key(name)) {
                    Some(at) => Model::targets(&model.texts[at].1)
                        .into_iter()
                        .map(|target| Outgoing { resolved: model.resolve(&target), target })
                        .collect(),
                    None => Vec::new(),
                };
                assert_eq!(index.outgoing(name), outgoing, "step {step} {name:?}");
            }
            for target in ["missing", "notes/a.md", "B", "c d", "z", "hidden", "lost", "/"] {
                assert_eq!(index.resolve(target).map(str::to_string), model.resolve(target), "step {step} {target:?}");
            }
            assert_eq!(index.dangling(), model.dangling(), "step {step}");

            // Nothing is left behind in the side maps once texts go.
            let linked: usize = index.texts.values().map(|entry| entry.links.len()).sum();
            assert_eq!(index.sources.values().map(BTreeMap::len).sum::<usize>(), linked);
            assert_eq!(index.leaves.values().map(BTreeSet::len).sum::<usize>(), index.texts.len());

            by_leaf += usize::from(index.resolve("c d").is_some_and(|name| key(name) != "c d"));
            linked_from += usize::from(NAMES.iter().any(|name| index.incoming(name).len() > 1));
            dangled += usize::from(index.dangling().len() > 1);
        }
        // The run was busy enough to mean something.
        assert!(by_leaf > 200 && linked_from > 200 && dangled > 200, "{by_leaf} {linked_from} {dangled}");
    }

    /// The text with every link target cut out. Two texts that differ only
    /// in where their links point have the same skeleton.
    fn skeleton(text: &str) -> String {
        let mut out = String::new();
        let mut copied = 0;
        let mut in_fence = false;
        for (at, line) in lines_of(text) {
            let kind = markup::classify_line(line, in_fence);
            in_fence ^= kind == LineKind::Fence;
            for wiki in markup::wikis(line, kind) {
                out.push_str(&text[copied..at + wiki.target.0]);
                copied = at + wiki.target.1;
            }
        }
        out + &text[copied..]
    }

    fn targets_of(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut in_fence = false;
        for (_, line) in lines_of(text) {
            let kind = markup::classify_line(line, in_fence);
            in_fence ^= kind == LineKind::Fence;
            found.extend(markup::wikis(line, kind).iter().map(|wiki| wiki.link(line).target.to_string()));
        }
        found
    }

    #[test]
    fn random_retargets_touch_nothing_but_matching_targets() {
        let parts = [
            "[[old]]", "[[ Old.md ]]", "[[OLD#h|s]]", "[[dir/old]]", "[[older]]", "[[new/name]]", "[[old\\]]", "word ",
            "\n", "\r\n", "\r", "```\n", "~~~", "`[[old]]`", "`", "\u{00E9}", "[[", "]]", "- [ ] ", "# ", "*", "\u{FEFF}",
        ];
        let mut rng = Rng(0x00C0_FFEE);
        let mut rewritten = 0;
        for _ in 0..4000 {
            let text: String = (0..rng.below(14)).map(|_| rng.pick(&parts)).collect();
            let (old, new) = (rng.pick(&["old", "Old.md", "dir/old", "nothing"]), rng.pick(&["new/name", "old", "Old", "x"]));
            let before = targets_of(&text);
            let hit = before.iter().any(|target| key(target) == key(old) && target != new);
            let Some(after) = retarget(&text, old, new) else {
                assert!(!hit, "{text:?} {old:?} {new:?}");
                continue;
            };
            rewritten += 1;
            assert!(hit && after != text, "{text:?} {old:?} {new:?}");
            assert_eq!(skeleton(&after), skeleton(&text), "{text:?} {old:?} {new:?}");
            let expected: Vec<String> =
                before.iter().map(|t| if key(t) == key(old) { new.to_string() } else { t.clone() }).collect();
            assert_eq!(targets_of(&after), expected, "{text:?} {old:?} {new:?}");
            if key(old) != key(new) {
                assert_eq!(retarget(&after, old, new), None);
            }
        }
        assert!(rewritten > 500, "{rewritten}");
    }

    /// New names made of everything a link cannot hold. A text comes back
    /// rewritten only with the links it had, the matching ones now pointing
    /// at the new name; and a name of plain words is never turned away.
    #[test]
    fn random_hostile_names_are_written_only_when_they_read_back() {
        let parts = ["[[old]]", "[[ Old.md #h | s ]]", "[[other]]", "`", "``", "word ", "\n", "- ", "> ", "[[", "]]", "\\", "*"];
        let names = ["[", "]", "[[", "]]", "|", "#", "`", "\n", "\r", "a", "b c", " ", "\\", "/", ".md", "*", "\u{00E9}", "\t"];
        let mut rng = Rng(0x0D15_EA5E);
        let (mut written, mut refused) = (0, 0);
        for _ in 0..20_000 {
            let text: String = (0..rng.below(10)).map(|_| rng.pick(&parts)).collect();
            let new: String = (0..1 + rng.below(4)).map(|_| rng.pick(&names)).collect();
            let before = targets_of(&text);
            let hit = before.iter().any(|target| key(target) == "old" && *target != new);
            let plain = !key(&new).is_empty() && new.chars().all(|c| c.is_alphanumeric() || c == ' ');
            let Some(after) = retarget(&text, "old", &new) else {
                assert!(!(hit && plain), "{text:?} {new:?}");
                refused += usize::from(hit);
                continue;
            };
            written += 1;
            assert!(hit, "{text:?} {new:?}");
            // Whitespace round the new name is written too, and then reads
            // as padding, which is the one way the rest can seem to change.
            if new == new.trim() {
                assert_eq!(skeleton(&after), skeleton(&text), "{text:?} {new:?}");
            }
            let expected: Vec<String> =
                before.iter().map(|target| if key(target) == "old" { new.trim().to_string() } else { target.clone() }).collect();
            assert_eq!(targets_of(&after), expected, "{text:?} {new:?}");
        }
        assert!(written > 1000 && refused > 1000, "{written} {refused}");
    }

    /// Where the links of a line are, found by plain string search. Right
    /// only for lines with no backticks and no backslashes, where nothing
    /// can hide a link.
    fn plain_search(line: &str) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        let mut at = 0;
        while let Some(open) = line[at..].find("[[").map(|i| at + i) {
            at = open + 1;
            let rest = &line[open + 2..];
            let close = match (rest.find("]]"), rest.find("[[")) {
                _ if rest.starts_with('[') => continue,
                (Some(close), Some(again)) if again < close => continue,
                (Some(close), _) => close,
                (None, _) => continue,
            };
            let target = rest[..close].split('|').next().unwrap_or("").split('#').next().unwrap_or("");
            if !target.trim().is_empty() {
                at = open + close + 4;
                found.push((open, at));
            }
        }
        found
    }

    #[test]
    fn random_lines_hold_exactly_the_links_a_plain_search_finds() {
        let parts = [
            "[[", "]]", "[[", "]]", "[", "]", "|", "#", " ", "a", "B c", "\u{00E9}", "\u{4E00}", "*", "- ", "> ", "(", ")", "# ",
            "http://x", "_", "[t](u)", "1. ", "[ ] ", "[[n]]", "[[n#h|s]]",
        ];
        let mut rng = Rng(0xACE0_FBA5);
        let mut total = 0;
        for _ in 0..20_000 {
            let line: String = (0..rng.below(16)).map(|_| rng.pick(&parts)).collect();
            let links: Vec<(usize, usize)> = links_in(&line).iter().map(|link| (link.start, link.end)).collect();
            assert_eq!(links, plain_search(&line), "{line:?}");
            total += links.len();
        }
        assert!(total > 5000, "{total}");
    }

    /// Characters from every corner of Unicode, thick with the ones that
    /// matter here, so that a slice at the wrong byte would be found out.
    fn soup(rng: &mut Rng, most: usize) -> String {
        const MARKS: &[char] = &['[', ']', '|', '#', '`', '\\', '/', ' ', '\t', '\n', '\r', '.', 'm', 'd', 'M', '*', '-', '~'];
        (0..rng.below(most + 1))
            .map(|_| match rng.below(8) {
                0..=3 => MARKS[rng.below(MARKS.len())],
                4 => char::from_u32(rng.below(0x80) as u32).unwrap_or('a'),
                5 => char::from_u32(0x80 + rng.below(0x780) as u32).unwrap_or('\u{00E9}'),
                6 => char::from_u32(0x800 + rng.below(0xF800) as u32).unwrap_or('\u{4E00}'),
                _ => char::from_u32(0x1_0000 + rng.below(0x10_0000) as u32).unwrap_or('\u{1F600}'),
            })
            .collect()
    }

    #[test]
    fn random_unicode_never_breaks_a_character() {
        let mut rng = Rng(0xFEED_F00D);
        let mut index = LinkIndex::new();
        for _ in 0..10_000 {
            let (text, name, other) = (soup(&mut rng, 40), soup(&mut rng, 8), soup(&mut rng, 8));

            let name_key = key(&name);
            assert_eq!(name_key, name_key.to_lowercase(), "{name:?}");
            assert_eq!(name_key, name_key.split_whitespace().collect::<Vec<_>>().join(" "), "{name:?}");
            assert!(!name_key.contains('\\') && !name_key.starts_with('/') && !name_key.ends_with('/'), "{name:?}");

            for (at, line) in lines_of(&text) {
                assert_eq!(text.get(at..at + line.len()), Some(line), "{text:?}");
                assert!(!line.contains(['\n', '\r']), "{text:?}");
                for link in links_in(line) {
                    assert!(line[link.start..link.end].contains(link.target), "{line:?}");
                }
            }
            if let Some(after) = retarget(&text, &name, &other) {
                assert_eq!(skeleton(&after), skeleton(&text), "{text:?} {name:?} {other:?}");
            }

            index.set(&name, &text);
            index.rename(&other, &name);
            assert_eq!(index.contains(&name), !name_key.is_empty());
            let _ = (index.outgoing(&name), index.incoming(&name), index.dangling(), index.resolve(&other));
            if rng.below(4) == 0 {
                index.remove(&name);
            }
        }
    }

    #[test]
    fn random_lines_give_sound_links() {
        let parts = ["[[", "]]", "[", "]", "|", "#", "`", "\\", " ", "a", "B c", "\u{00E9}", "\u{4E00}", "*", "- ", "> ", "(", ")"];
        let mut rng = Rng(0x5EED_1234);
        for _ in 0..8000 {
            let line: String = (0..rng.below(14)).map(|_| rng.pick(&parts)).collect();
            let links = links_in(&line);
            let mut last_end = 0;
            for link in &links {
                assert!(last_end <= link.start && link.start + 4 < link.end && link.end <= line.len(), "{line:?}");
                last_end = link.end;
                let whole = &line[link.start..link.end];
                assert!(whole.starts_with("[[") && whole.ends_with("]]"), "{line:?}");
                let inner = &whole[2..whole.len() - 2];
                assert!(!inner.contains("[[") && !inner.contains("]]"), "{line:?}");
                assert!(!link.target.is_empty() && link.target == link.target.trim(), "{line:?}");
                assert!(!link.target.contains(['|', '#']), "{line:?}");
                for part in [link.shown, link.heading].into_iter().flatten() {
                    assert!(!part.is_empty() && part == part.trim() && inner.contains(part), "{line:?}");
                }
                assert_eq!(link.shown.is_some(), inner.split_once('|').is_some_and(|(_, s)| !s.trim().is_empty()));
                for col in link.start..link.end {
                    assert_eq!(link_at(&line, col), Some(*link), "{line:?}");
                }
            }
            for col in 0..=line.len() {
                let inside = links.iter().any(|l| l.start <= col && col < l.end);
                assert_eq!(link_at(&line, col).is_some(), inside, "{line:?}");
            }
        }
    }
}
