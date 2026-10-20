//! The writing folder as a tree of folders and texts, read from directory
//! listings alone: no file is opened to build it.

use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::fsio;
use crate::names;
use crate::sys;

/// Folders nested deeper than this are not walked.
const MAX_DEPTH: usize = 16;

/// One text in the folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Text {
    /// The file's name with its ending.
    pub name: String,
    /// Its path inside the writing folder.
    pub rel: PathBuf,
    /// Size on disk.
    pub bytes: u64,
    /// Last written, in milliseconds since 1970, UTC.
    pub modified_ms: u64,
    /// Made, in milliseconds since 1970, UTC.
    pub created_ms: u64,
    /// The file's bytes are not on this disk (a cloud placeholder). Such a
    /// file is listed but never opened by a scan.
    pub placeholder: bool,
    /// The file carries the read-only attribute.
    pub read_only: bool,
}

/// One folder: its sub-folders, then its texts, each in alphabetical order
/// whatever the case.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Folder {
    /// The folder's own name; empty for the writing folder itself.
    pub name: String,
    /// Its path inside the writing folder; empty for the writing folder.
    pub rel: PathBuf,
    /// The folders directly inside this one.
    pub folders: Vec<Folder>,
    /// The texts directly inside this one.
    pub texts: Vec<Text>,
}

/// Everything listed in the writing folder: texts ending in `.md`,
/// `.markdown` or `.txt` and the folders that hold them. Names starting with
/// a dot are left out.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tree {
    /// The writing folder itself.
    pub root: Folder,
}

impl Folder {
    fn collect<'a>(&'a self, out: &mut Vec<&'a Text>) {
        for folder in &self.folders {
            folder.collect(out);
        }
        out.extend(self.texts.iter());
    }
}

impl Tree {
    /// Every text, folder by folder.
    pub fn texts(&self) -> Vec<&Text> {
        let mut out = Vec::new();
        self.root.collect(&mut out);
        out
    }

    /// The text at a path inside the folder, whatever the case of the path.
    pub fn find(&self, rel: &Path) -> Option<&Text> {
        let key = names::key(rel);
        self.texts().into_iter().find(|text| names::key(&text.rel) == key)
    }
}

/// What one walk of the folder found.
pub(crate) struct Walk {
    pub tree: Tree,
    /// Leftover save temp files, with their last-write time in ms since 1970.
    pub temps: Vec<(PathBuf, u64)>,
}

fn walk_folder(dir: &Path, rel: &Path, name: String, depth: usize, temps: &mut Vec<(PathBuf, u64)>) -> io::Result<Folder> {
    let mut folder = Folder { name, rel: rel.to_path_buf(), folders: Vec::new(), texts: Vec::new() };
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let attrs = meta.file_attributes();
        if attrs & sys::ATTR_DIRECTORY != 0 {
            // A link to a folder is listed but not followed: it could lead back here.
            if name.starts_with('.') {
                continue;
            }
            let sub = if attrs & sys::ATTR_REPARSE != 0 || depth >= MAX_DEPTH {
                Folder { name, rel: rel.join(entry.file_name()), folders: Vec::new(), texts: Vec::new() }
            } else {
                match walk_folder(&entry.path(), &rel.join(&name), name.clone(), depth + 1, temps) {
                    Ok(sub) => sub,
                    Err(_) => Folder { name, rel: rel.join(entry.file_name()), folders: Vec::new(), texts: Vec::new() },
                }
            };
            folder.folders.push(sub);
        } else if fsio::is_temp_name(&name) {
            temps.push((entry.path(), sys::filetime_to_unix_ms(meta.last_write_time())));
        } else if !name.starts_with('.') && names::is_text_name(&name) {
            folder.texts.push(Text {
                rel: rel.join(&name),
                name,
                bytes: meta.file_size(),
                modified_ms: sys::filetime_to_unix_ms(meta.last_write_time()),
                created_ms: sys::filetime_to_unix_ms(meta.creation_time()),
                placeholder: attrs & sys::ATTR_PLACEHOLDER != 0,
                read_only: attrs & sys::ATTR_READONLY != 0,
            });
        }
    }
    folder.folders.sort_by(|a, b| (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name)));
    folder.texts.sort_by(|a, b| (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name)));
    Ok(folder)
}

/// Lists the writing folder. Fails only if the folder itself cannot be
/// listed; a sub-folder that cannot be read is shown empty.
pub(crate) fn walk(root: &Path) -> io::Result<Walk> {
    let mut temps = Vec::new();
    let root = walk_folder(root, Path::new(""), String::new(), 0, &mut temps)?;
    Ok(Walk { tree: Tree { root }, temps })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    #[test]
    fn lists_texts_and_folders_in_order_and_leaves_the_rest_out() {
        let dir = Scratch::new("tree");
        let root = dir.path();
        for sub in ["College", "college notes", ".discarded", "Empty", r"College\Drafts"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        for (file, text) in [
            ("b.md", "bb"),
            ("A.txt", "a"),
            ("c.markdown", "ccc"),
            (".hidden.md", "x"),
            ("notes.docx", "x"),
            ("~b.md.0a1b2c3d.tmp", "half"),
            (r"College\The orchard.md", "orchard"),
            (r"College\Drafts\old.md", "old"),
            (r".discarded\gone.md", "gone"),
        ] {
            fs::write(root.join(file), text).unwrap();
        }
        let walk = walk(root).unwrap();
        let tree = walk.tree;
        let names: Vec<&str> = tree.root.texts.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["A.txt", "b.md", "c.markdown"]);
        let folders: Vec<&str> = tree.root.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(folders, ["College", "college notes", "Empty"]);
        let college = &tree.root.folders[0];
        assert_eq!(college.texts[0].rel, PathBuf::from(r"College\The orchard.md"));
        assert_eq!(college.folders[0].texts[0].rel, PathBuf::from(r"College\Drafts\old.md"));
        assert_eq!(tree.texts().len(), 5);
        let b = tree.find(Path::new("B.MD")).unwrap();
        assert_eq!((b.bytes, b.placeholder, b.read_only), (2, false, false));
        assert!(b.modified_ms > 1_600_000_000_000 && b.created_ms > 1_600_000_000_000);
        assert!(tree.find(Path::new(r".discarded\gone.md")).is_none());
        assert_eq!(walk.temps.len(), 1);
        assert!(walk.temps[0].0.ends_with("~b.md.0a1b2c3d.tmp"));
        assert!(super::walk(&root.join("no such folder")).is_err());
    }
}
