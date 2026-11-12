//! **Generated contents:** headings distillPDF found in a document, offered when the file's own contents are missing or look bad, and never in their place (owner, 2026-11-07: "dont modify the bad toc, but offer a generated toc using distillpdf").
//!
//! The helper `recto-headings` makes them and says them as one line in [`Generated::encode`]'s form. The session reads that line with [`Generated::decode`], the same reader as for the file kept on disk, so there's one parser to fuzz, and it treats both as hostile: what's in them came from a PDF, and the file sits in a folder something else may have touched.
//!
//! ```text
//! <root>/<path hash>.<key>.json   One document's, written once, small and capped
//! ```
//!
//! **Keyed to what the file is,** by [`CorpusKey`], the same identity a search corpus has, so a replaced file never gets its old headings. The path's hash in the name is only so that a new version's write takes the old version's headings away. Nothing else in the name or the file says which file it was.
//!
//! **For the drive's sake** (SPEC §2's disk row): written once per version of a file, when someone asks, at most [`MAX_FILE_BYTES`], and never touched by reading. All of them together are held to [`MAX_FILES`] files, [`MAX_TOTAL_BYTES`] and [`MAX_AGE`], the oldest first, looked at whenever one's written.

use crate::corpus::{Cleared, CorpusKey};
use recto_core::{NONE, Toc, TocLine, clean_line};
use recto_platform::local;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What the file says it is.
pub const FORMAT: &str = "recto-generated-contents";
/// Bumped when a kept file would mean something else. An older one is then thrown away and made again when asked.
pub const VERSION: u64 = 1;
/// Most entries kept for one document. A real document's headings are a few hundred at most; past this it's a flood, and the list says it was cut.
pub const MAX_ENTRIES: usize = 4000;
/// Longest title, in bytes. A heading is a line, and anything longer is a paragraph taken for one.
pub const MAX_TITLE_BYTES: usize = 240;
/// Deepest nesting. distillPDF's levels go to six.
pub const MAX_DEPTH: u8 = 5;
/// A place on a page, as thousandths of its height from the top.
pub const PLACE_SCALE: u16 = 1000;
/// Largest file or helper answer read, which [`MAX_ENTRIES`] of [`MAX_TITLE_BYTES`] fit in with their JSON.
pub const MAX_FILE_BYTES: u64 = 1536 * 1024;
/// How many documents' contents are kept, how much room they take together, and how long one is kept since it was written.
pub const MAX_FILES: usize = 512;
pub const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// Most pages a document may claim. The same limit as the search corpus's.
pub const MAX_PAGES: u32 = crate::corpus::MAX_PAGES;

/// One heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedEntry {
    /// 0 is the top level.
    pub depth: u8,
    /// Zero-based page in the file.
    pub page: u32,
    /// Where on its page, in thousandths of the page's height from the top.
    pub place: u16,
    /// Cleaned, never empty, and still the file's text: shown as text, never acted on.
    pub title: String,
}

/// A document's generated contents, and what was left out making them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Generated {
    /// The file's pages when they were made.
    pub pages: u32,
    pub entries: Vec<GeneratedEntry>,
    /// More headings were found than [`MAX_ENTRIES`].
    pub truncated: bool,
    /// Headings left out because their text looked hidden, by the render worker's check that draws the page.
    pub hidden_dropped: u32,
    /// Pages whose headings were left out because the check for hidden text couldn't run on them.
    pub pages_not_checked: u32,
    /// Pages distillPDF couldn't read, whose headings aren't here.
    pub pages_failed: u32,
}

/// Why a file or an answer wasn't believed. Only ever said in Rust's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bad(pub &'static str);

impl Generated {
    /// One line of JSON. Titles go through [`clean_line`] first, so nothing a file wrote can make a second line.
    pub fn encode(&self, key: CorpusKey) -> String {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|entry| json!([entry.depth, entry.page, entry.place, clean_line(&entry.title)]))
            .collect();
        json!({
            "format": FORMAT,
            "version": VERSION,
            "key": key.hex(),
            "pages": self.pages,
            "truncated": self.truncated,
            "hidden_dropped": self.hidden_dropped,
            "pages_not_checked": self.pages_not_checked,
            "pages_failed": self.pages_failed,
            "entries": entries,
        })
        .to_string()
    }

    /// Reads what [`Self::encode`] wrote, for `key` and nothing else, believing nothing it hasn't checked. A title that cleans to nothing goes, and so does an entry about a page the file doesn't have.
    pub fn decode(bytes: &[u8], key: CorpusKey) -> Result<Self, Bad> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(Bad("too_large"));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Bad("not_json"))?;
        if value["format"] != FORMAT {
            return Err(Bad("not_this_format"));
        }
        if value["version"].as_u64() != Some(VERSION) {
            return Err(Bad("other_version"));
        }
        if value["key"].as_str() != Some(key.hex().as_str()) {
            return Err(Bad("other_file"));
        }
        let count = |name: &str| value[name].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(Bad("bad_count"));
        let pages = count("pages")?;
        if pages == 0 || pages > MAX_PAGES {
            return Err(Bad("bad_pages"));
        }
        let Some(list) = value["entries"].as_array() else {
            return Err(Bad("no_entries"));
        };
        if list.len() > MAX_ENTRIES {
            return Err(Bad("too_many_entries"));
        }
        let mut entries = Vec::with_capacity(list.len());
        for item in list {
            let Some([depth, page, place, title]) = item.as_array().map(Vec::as_slice) else {
                return Err(Bad("bad_entry"));
            };
            let (Some(depth), Some(page), Some(place), Some(title)) = (depth.as_u64(), page.as_u64(), place.as_u64(), title.as_str()) else {
                return Err(Bad("bad_entry"));
            };
            if depth > u64::from(MAX_DEPTH) || place > u64::from(PLACE_SCALE) {
                return Err(Bad("bad_entry"));
            }
            if page >= u64::from(pages) {
                return Err(Bad("page_out_of_range"));
            }
            let title = short_title(title);
            if title.is_empty() {
                continue;
            }
            entries.push(GeneratedEntry {
                depth: depth as u8,
                page: page as u32,
                place: place as u16,
                title,
            });
        }
        Ok(Self {
            pages,
            entries,
            truncated: value["truncated"] == true,
            hidden_dropped: count("hidden_dropped").unwrap_or(0),
            pages_not_checked: count("pages_not_checked").unwrap_or(0),
            pages_failed: count("pages_failed").unwrap_or(0),
        })
    }

    /// As contents the sidebar and `recto contents` already know how to show. Depth never jumps by more than one below its parent, so the list nests the way it reads.
    pub fn toc(&self) -> Toc {
        let mut lines = Vec::with_capacity(self.entries.len());
        // The open parents: their depth in the file, their depth as shown, and where they are.
        let mut open: Vec<(u8, u8, u32)> = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            while open.last().is_some_and(|(raw, ..)| *raw >= entry.depth) {
                open.pop();
            }
            let depth = open.last().map_or(0, |(_, shown, _)| shown + 1);
            let parent = open.last().map_or(NONE, |(.., at)| *at);
            lines.push(TocLine {
                title: &entry.title,
                page: entry.page,
                parent,
                depth,
            });
            open.push((entry.depth, depth, u32::try_from(index).unwrap_or(NONE)));
        }
        Toc::from_lines(&lines, self.truncated)
    }
}

/// A title cleaned and cut to [`MAX_TITLE_BYTES`] without splitting a character.
pub fn short_title(text: &str) -> String {
    let mut title = clean_line(text);
    if title.len() > MAX_TITLE_BYTES {
        let mut end = MAX_TITLE_BYTES;
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
    }
    title
}

/// The start of a kept file's name for this file: a hash of where it is, so a new version replaces the old. Not the path itself.
fn path_tag(path: &Path) -> String {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let digest = Sha256::digest(real.as_os_str().as_encoded_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn file_name(path: &Path, key: CorpusKey) -> String {
    format!("{}.{}.json", path_tag(path), key.hex())
}

/// Whether a name in the folder is one of ours, so nothing else is ever removed.
fn is_ours(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else { return false };
    let Some((tag, key)) = stem.split_once('.') else { return false };
    tag.len() == 16 && tag.bytes().all(|byte| byte.is_ascii_hexdigit()) && CorpusKey::from_hex(key).is_some()
}

/// The kept contents for this version of this file, if they're there and hold up. One that doesn't is removed, which only costs making it again.
pub fn read(root: &Path, path: &Path, key: CorpusKey) -> Option<Generated> {
    if local::check_private_dir(root).is_err() {
        return None;
    }
    let file = root.join(file_name(path, key));
    match local::read_private_file(&file, MAX_FILE_BYTES) {
        Ok(Some(bytes)) => match Generated::decode(&bytes, key) {
            Ok(generated) => Some(generated),
            Err(_) => {
                let _ = fs::remove_file(&file);
                None
            }
        },
        Ok(None) => None,
        Err(_) => {
            let _ = fs::remove_file(&file);
            None
        }
    }
}

/// Keeps a document's contents, in one step, and takes away any kept for an older version of the same file. Then the folder's held to its limits.
pub fn write(root: &Path, path: &Path, key: CorpusKey, generated: &Generated) -> io::Result<()> {
    local::ensure_private_dir(root)?;
    let _ = local::exclude_from_backups(root);
    let line = generated.encode(key);
    if line.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "larger than it may be"));
    }
    let name = file_name(path, key);
    local::write_private_file(&root.join(&name), line.as_bytes())?;
    let tag = path_tag(path);
    for entry in fs::read_dir(root)?.flatten() {
        let Some(other) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if other != name && is_ours(&other) && other.starts_with(&format!("{tag}.")) {
            let _ = fs::remove_file(entry.path());
        }
    }
    evict(root, Some(&name));
    Ok(())
}

/// Takes away everything kept for a file, every version that's still there, and says how many went and how many couldn't be removed, whose headings are still on disk.
pub fn clear(root: &Path, path: &Path) -> Cleared {
    let tag = format!("{}.", path_tag(path));
    remove_ours(root, |name| name.starts_with(&tag))
}

/// Takes away every document's kept contents: `recto index clear --all`. Only files named as ours, so nothing else in the folder is touched.
pub fn clear_all(root: &Path) -> Cleared {
    remove_ours(root, |_| true)
}

fn remove_ours(root: &Path, which: impl Fn(&str) -> bool) -> Cleared {
    let mut report = Cleared::default();
    let listing = match fs::read_dir(root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return report,
        Err(error) => {
            report.note(Err(error));
            return report;
        }
    };
    for entry in listing.flatten() {
        if entry.file_name().to_str().is_some_and(|name| is_ours(name) && which(name)) {
            report.note(fs::remove_file(entry.path()));
        }
    }
    report
}

/// Holds the folder to [`MAX_FILES`], [`MAX_TOTAL_BYTES`] and [`MAX_AGE`], the oldest first, never removing `keep`. Only files named as ours are looked at.
pub fn evict(root: &Path, keep: Option<&str>) -> u32 {
    let Ok(listing) = fs::read_dir(root) else { return 0 };
    let mut files: Vec<(SystemTime, u64, PathBuf, bool)> = Vec::new();
    for entry in listing.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_ours(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        files.push((
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            meta.len(),
            entry.path(),
            keep == Some(name.as_str()),
        ));
        // A folder past any sane count is looked at no further: what's here is enough to go on.
        if files.len() > MAX_FILES * 8 {
            break;
        }
    }
    files.sort_by_key(|(when, ..)| *when);
    let now = SystemTime::now();
    let mut total: u64 = files.iter().map(|(_, bytes, ..)| *bytes).sum();
    let mut count = files.len();
    let mut removed = 0;
    for (when, bytes, path, kept) in &files {
        let old = now.duration_since(*when).is_ok_and(|age| age > MAX_AGE);
        if *kept || !(old || count > MAX_FILES || total > MAX_TOTAL_BYTES) {
            continue;
        }
        if fs::remove_file(path).is_ok() {
            removed += 1;
            count -= 1;
            total = total.saturating_sub(*bytes);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> CorpusKey {
        CorpusKey::from_hex(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    fn sample() -> Generated {
        Generated {
            pages: 10,
            entries: vec![
                GeneratedEntry {
                    depth: 0,
                    page: 0,
                    place: 100,
                    title: "Introduction".into(),
                },
                GeneratedEntry {
                    depth: 1,
                    page: 1,
                    place: 0,
                    title: "Background".into(),
                },
                GeneratedEntry {
                    depth: 3,
                    page: 2,
                    place: 500,
                    title: "A deep one".into(),
                },
                GeneratedEntry {
                    depth: 0,
                    page: 9,
                    place: 1000,
                    title: "Appendix".into(),
                },
            ],
            ..Generated::default()
        }
    }

    /// Failure: what's written can't be read back the same, so every look makes them again, or a kept file answers for another version of the file.
    #[test]
    fn what_is_written_reads_back_for_its_own_key_only() {
        let line = sample().encode(key(1));
        assert!(!line.contains('\n'));
        assert_eq!(Generated::decode(line.as_bytes(), key(1)), Ok(sample()));
        assert_eq!(Generated::decode(line.as_bytes(), key(2)), Err(Bad("other_file")));
    }

    /// Failure: a forged or damaged file is believed: a page past the end sends the reader nowhere, a huge depth or count sizes something, or a title carries a second line or invisible characters into the sidebar.
    #[test]
    fn a_forged_file_is_refused_or_cleaned() {
        let good: Value = serde_json::from_str(&sample().encode(key(1))).unwrap();
        let with = |field: &str, value: Value| {
            let mut forged = good.clone();
            forged[field] = value;
            Generated::decode(forged.to_string().as_bytes(), key(1))
        };
        assert!(with("pages", json!(0)).is_err());
        assert!(with("pages", json!(u64::MAX)).is_err());
        assert!(with("version", json!(VERSION + 1)).is_err());
        assert!(with("entries", json!([[0, 10, 0, "past the end"]])).is_err());
        assert!(with("entries", json!([[200, 1, 0, "too deep"]])).is_err());
        assert!(with("entries", json!([[0, 1, 1001, "below the page"]])).is_err());
        assert!(with("entries", json!([[0, 1, 0]])).is_err());
        assert!(with("entries", json!(vec![json!([0, 1, 0, "x"]); MAX_ENTRIES + 1])).is_err());
        let cleaned = with("entries", json!([[0, 1, 0, "Line one\nline two\u{200B}\u{E0041}"], [0, 2, 0, " \u{200B} "]])).unwrap();
        assert_eq!(cleaned.entries.len(), 1, "a title that cleans to nothing goes");
        assert_eq!(cleaned.entries[0].title, "Line one line two");
        let long = with("entries", json!([[0, 1, 0, "é".repeat(400)]])).unwrap();
        assert!(long.entries[0].title.len() <= MAX_TITLE_BYTES);
        assert!(Generated::decode(&vec![b' '; MAX_FILE_BYTES as usize + 1], key(1)).is_err());
        assert!(Generated::decode(b"{\"format\":", key(1)).is_err());
    }

    /// Failure: a heading two levels under its parent shows as a jump in the sidebar, or hangs under the wrong section.
    #[test]
    fn depth_never_jumps_and_parents_are_the_heading_above() {
        let toc = sample().toc();
        let depths: Vec<u8> = toc.entries().iter().map(|entry| entry.depth).collect();
        assert_eq!(depths, [0, 1, 2, 0]);
        let parents: Vec<u32> = toc.entries().iter().map(|entry| entry.parent).collect();
        assert_eq!(parents, [NONE, 0, 1, NONE]);
        // A list that starts deep starts at the top.
        let deep = Generated {
            pages: 3,
            entries: vec![
                GeneratedEntry {
                    depth: 4,
                    page: 0,
                    place: 0,
                    title: "Deep".into(),
                },
                GeneratedEntry {
                    depth: 2,
                    page: 1,
                    place: 0,
                    title: "Less deep".into(),
                },
            ],
            ..Generated::default()
        };
        let depths: Vec<u8> = deep.toc().entries().iter().map(|entry| entry.depth).collect();
        assert_eq!(depths, [0, 0]);
    }

    /// Failure: a new version of a file leaves the old version's headings on disk, a forged file in the folder is used, or the folder grows past its limits, or something not ours is removed.
    #[test]
    fn a_new_version_replaces_the_old_and_the_folder_keeps_to_its_limits() {
        let temp = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-generated-");
        local::ensure_private_dir(&temp).unwrap();
        let root = temp.join("contents.noindex");
        let pdf = temp.join("a.pdf");
        fs::write(&pdf, b"%PDF-1.7").unwrap();
        write(&root, &pdf, key(1), &sample()).unwrap();
        assert_eq!(read(&root, &pdf, key(1)), Some(sample()));
        write(&root, &pdf, key(2), &sample()).unwrap();
        assert_eq!(read(&root, &pdf, key(1)), None, "the old version's headings went");
        assert_eq!(read(&root, &pdf, key(2)), Some(sample()));
        // Forged in place: thrown away, and not believed.
        let name = root.join(file_name(&pdf, key(2)));
        local::write_private_file(&name, b"{\"format\":\"recto-generated-contents\"}").unwrap();
        assert_eq!(read(&root, &pdf, key(2)), None);
        assert!(!name.exists());
        // Something of someone else's is never looked at. The folder's filled past its count by placing files as `write` names them, oldest first, so the
        // one `write` after that holds it to its limit: a `write` for each cost 517 listings of a growing folder (3.7 s on CI).
        fs::write(root.join("notes.txt"), b"keep me").unwrap();
        let line = sample().encode(key(3));
        let placed: Vec<PathBuf> = (0..(MAX_FILES + 5))
            .map(|n| {
                let placed = root.join(file_name(&temp.join(format!("{n}.pdf")), key((n % 250) as u8)));
                fs::write(&placed, line.as_bytes()).unwrap();
                placed
            })
            .collect();
        let newest = temp.join("newest.pdf");
        fs::write(&newest, b"%PDF").unwrap();
        write(&root, &newest, key(4), &sample()).unwrap();
        let ours = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(is_ours))
            .count();
        assert!(ours <= MAX_FILES, "{ours} kept");
        assert!(!placed[0].exists(), "the oldest goes first");
        assert_eq!(read(&root, &newest, key(4)), Some(sample()), "and never the one just written");
        assert!(root.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&temp);
    }

    /// Failure: `recto index clear` leaves a document's generated contents on disk, heading text from the file, though it says it removes the copy of documents' text; or clearing one file takes another's, or something in the folder that isn't ours.
    #[test]
    fn clearing_takes_one_files_contents_or_all_and_nothing_else() {
        let temp = recto_testpdf::TempDir::new("recto-generated-clear-");
        let root = temp.join("generated");
        let (one, two) = (temp.join("one.pdf"), temp.join("two.pdf"));
        local::ensure_private_dir(&temp).unwrap();
        fs::write(&one, b"%PDF").unwrap();
        fs::write(&two, b"%PDF").unwrap();
        write(&root, &one, key(1), &sample()).unwrap();
        write(&root, &two, key(2), &sample()).unwrap();
        fs::write(root.join("notes.txt"), b"not ours").unwrap();
        assert_eq!(clear(&root, &one).cleared, 1);
        assert!(read(&root, &one, key(1)).is_none());
        assert!(read(&root, &two, key(2)).is_some(), "another file's stay");
        assert_eq!(clear(&root, &one), Cleared::default(), "nothing left to take");
        let all = clear_all(&root);
        assert_eq!((all.cleared, all.failed), (1, 0));
        assert!(read(&root, &two, key(2)).is_none());
        assert!(root.join("notes.txt").exists(), "only ours are removed");
        assert_eq!(clear_all(&temp.join("never made")), Cleared::default());
        let _ = fs::remove_dir_all(&temp);
    }
}
