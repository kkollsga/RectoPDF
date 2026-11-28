//! The Find text's folders: the corpus (`StatePaths.corpus`), its version notes and agent folders, and the generated contents (`StatePaths.generated`), for
//! `Store::{CorpusText, CorpusAgentPages, CorpusAliases, GeneratedContents}` (`docs/design/removal.md` §3, §4 and §10 "D2d as built").
//!
//! **Read-only, and nothing in the cache is opened for its content.** A corpus's `app/` and `agent/` and a contents file hold the document's words and
//! headings: they are never opened. A version note holds keys and nothing else, and is read for at most 2,080 bytes. Every folder is reached by descriptor
//! (`recto_platform::folder`: no link followed at any step, this user's), so a path swapped for a link after the check changes nothing. No `recto_index`
//! function is called here (`keys_of_file` follows paths and opens the document, `generated::read` deletes what doesn't decode): the names are restated in
//! [`names`] and held against the real writers by `pins`.
//!
//! **Placing is the app's own per-file clear** (`index clear <file>`): a corpus is the target's if its key is one the target has now or one a version note
//! named for the target's path or inode lists (`T`); a kept contents file if its tag is the current path's or its key is in `T`. Nothing else can be placed, and
//! there is no `Unrelated`: the cache holds no path, so another document's can't be proved to be another's by anything but its absence from `T`. Everything not
//! placed is `Unknown`, and a plan refuses. A note planted by another local process can only add keys to `T`, so the app clears more cache: that is the clear's own rule,
//! and it costs a rebuild.
//!
//! **Marked complete** only after every folder was listed to the end and nothing failed: a root, `by-file` or contents folder that is a link, a file or another
//! user's, a listing error, or an entry that vanishes between the listing and its opening leaves its stores unmarked. A missing folder is complete and empty.

mod names;
#[cfg(test)]
mod pins;
/// The fixtures are shared with the coverage gate (`removal_store/coverage_tests.rs`).
#[cfg(test)]
pub(crate) mod tests;

use super::backups::{self, Finder};
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Side, Sides, Store, Survey, TargetMode};
use recto_index::corpus::CorpusKey;
use recto_platform::folder::{Entry, Folder, Kind, Opened};
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::testing::Hook;

/// What the caller knows of the target's file now, from the descriptor it holds: the survey never opens the document or looks at a path.
pub struct CacheTarget {
    /// The file's key as `CorpusKey::of_open` makes it; `None` if it couldn't be made.
    pub key: Option<CorpusKey>,
    /// The device and inode of that descriptor, as an inode note is named; `None` if unknown.
    pub device_inode: Option<(u64, u64)>,
}

/// The caps (design limits, not measurements), so a test can make them small. One over any is `TooLarge`, and nothing is marked.
#[derive(Clone)]
pub(crate) struct CacheCaps {
    /// Entries of the corpus root, every one counted.
    pub root_entries: usize,
    /// Entries of one corpus's folder, and the format folders opened in it.
    pub key_entries: usize,
    pub subfolders: usize,
    /// Entries of a format folder.
    pub sub_entries: usize,
    /// Entries of `by-file`, and of the contents folder.
    pub alias_entries: usize,
    pub contents_entries: usize,
    /// The keys the target's notes may name in all: its own and 32 under each of two notes.
    pub target_keys: usize,
    /// Called with a path just before it is opened: a test's way to change the folder under the survey.
    #[cfg(test)]
    pub before_open: Option<Hook>,
    /// Called with a folder's path once it is checked and before it is listed.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for CacheCaps {
    fn default() -> Self {
        Self {
            root_entries: 8_192,
            key_entries: 64,
            subfolders: 8,
            sub_entries: 256,
            alias_entries: 8_192,
            contents_entries: 8_192,
            target_keys: 1 + 2 * names::MAX_ALIAS_KEYS,
            #[cfg(test)]
            before_open: None,
            #[cfg(test)]
            before_list: None,
        }
    }
}

impl CacheCaps {
    #[cfg(test)]
    fn opening(&self, at: impl FnOnce() -> PathBuf) {
        if let Some(hook) = &self.before_open {
            hook(&at());
        }
    }

    #[cfg(not(test))]
    fn opening(&self, _: impl FnOnce() -> PathBuf) {}

    #[cfg(test)]
    fn listing(&self, at: impl FnOnce() -> PathBuf) {
        if let Some(hook) = &self.before_list {
            hook(&at());
        }
    }

    #[cfg(not(test))]
    fn listing(&self, _: impl FnOnce() -> PathBuf) {}
}

/// Adds what the corpus, its notes and the generated contents hold to `survey`, and marks the four stores complete only after everything was listed to the end.
///
/// `lineage.paths().next()` is the current path (D5 passes the canonical one), and the only path used: an older path's caches can't be proved and stay `Unknown`. The
/// adapter does not call `Survey::history`; the caller does. A missing folder is complete and empty. `add`'s refusal passes through, and so does a folder over a cap.
pub fn survey_caches(
    corpus_root: &Path,
    contents_root: &Path,
    lineage: &Lineage<'_>,
    target: &CacheTarget,
    mode: TargetMode,
    survey: &mut Survey,
) -> Result<(), Refusal> {
    survey_caches_with(corpus_root, contents_root, lineage, target, mode, survey, &CacheCaps::default())
}

pub(crate) fn survey_caches_with(
    corpus_root: &Path,
    contents_root: &Path,
    lineage: &Lineage<'_>,
    target: &CacheTarget,
    mode: TargetMode,
    survey: &mut Survey,
    caps: &CacheCaps,
) -> Result<(), Refusal> {
    let Some((current, _)) = lineage.paths().next() else { return Ok(()) };
    let mut run = Run {
        caps,
        mode,
        path_alias: names::path_alias(current.as_os_str().as_encoded_bytes()),
        inode_alias: target.device_inode.map(|(device, inode)| names::inode_alias(device, inode)),
        tag: names::contents_tag(current.as_os_str().as_encoded_bytes()),
        keys: target.key.into_iter().collect(),
        marks: Marks::default(),
        survey,
    };
    run.corpus(corpus_root)?;
    run.contents(contents_root)?;
    run.finish();
    Ok(())
}

/// Which stores stay complete. Each starts so, and a problem takes one away.
struct Marks {
    text: bool,
    pages: bool,
    aliases: bool,
    contents: bool,
}

impl Default for Marks {
    fn default() -> Self {
        Self {
            text: true,
            pages: true,
            aliases: true,
            contents: true,
        }
    }
}

struct Run<'a> {
    caps: &'a CacheCaps,
    mode: TargetMode,
    /// The names of the notes that are the target's: by its current path and by its inode.
    path_alias: String,
    inode_alias: Option<String>,
    /// The start of a contents file's name for the current path.
    tag: String,
    /// `T`: the target's key now, and every key those notes list.
    keys: Vec<CorpusKey>,
    marks: Marks,
    survey: &'a mut Survey,
}

/// Whether an opened file is the one that was listed: the entry's device and inode, and the link count the entry had (a name made since the listing is a
/// change; a note that already had two names is `Shared`, and stays one).
#[cfg(unix)]
fn is_the_listed(file: &std::fs::File, entry: &Entry) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    file.metadata()
        .is_ok_and(|opened| opened.dev() == entry.dev && opened.ino() == entry.ino && opened.nlink() == entry.links)
}

#[cfg(not(unix))]
fn is_the_listed(_file: &std::fs::File, _entry: &Entry) -> bool {
    false
}

/// A file that went away between the listing and the opening: the folder changed under the survey.
struct Gone;

impl Run<'_> {
    fn finish(&mut self) {
        let marks = [
            (Store::CorpusText, self.marks.text),
            (Store::CorpusAgentPages, self.marks.pages),
            (Store::CorpusAliases, self.marks.aliases),
            (Store::GeneratedContents, self.marks.contents),
        ];
        for (store, complete) in marks {
            if complete {
                self.survey.mark_complete(store);
            }
        }
    }

    fn add(&mut self, store: Store, key: &[u8], owner: Owner, format: Format) -> Result<(), Refusal> {
        self.survey.add(Found {
            store,
            key: ArtifactKey::of(store, key),
            owner,
            format,
        })
    }

    /// The owner of something proven the target's: the target's for a Save, the source's for a Save As; a file with other names is shared.
    fn placed(&self, links: u64) -> Owner {
        if links != 1 {
            return Owner::Shared;
        }
        Sides::default().with(Side::Target).owner(self.mode)
    }

    fn is_target_alias(&self, stem: &str) -> bool {
        stem == self.path_alias || self.inode_alias.as_deref() == Some(stem)
    }

    /// The folder's entries, at most `cap`; one more is `TooLarge`. `None` if it can't be read.
    fn list(&self, folder: &Folder, cap: usize, at: impl FnOnce() -> PathBuf) -> Result<Option<Vec<Entry>>, Refusal> {
        self.caps.listing(at);
        let Ok(entries) = folder.entries() else { return Ok(None) };
        let mut listed = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else { return Ok(None) };
            if listed.len() >= cap {
                return Err(Refusal::TooLarge);
            }
            listed.push(entry);
        }
        Ok(Some(listed))
    }

    /// Steps 1 to 4: the corpus root, its version notes (so `T` is known), its folders, and the older build's folder.
    fn corpus(&mut self, root_path: &Path) -> Result<(), Refusal> {
        let root = match Folder::open(root_path) {
            Opened::Missing => None,
            Opened::Other => {
                self.marks.text = false;
                self.marks.pages = false;
                self.marks.aliases = false;
                return Ok(());
            }
            Opened::Folder(root) => Some(root),
        };
        if let Some(root) = &root {
            self.aliases(root, root_path)?;
            self.folders(root, root_path)?;
        }
        self.old_folder(root_path)
    }

    /// `by-file`: `T` from the target's notes, then every note placed by its name or its keys.
    fn aliases(&mut self, root: &Folder, root_path: &Path) -> Result<(), Refusal> {
        let at = || root_path.join(names::VERSIONS);
        let folder = match root.open_in(OsStr::new(names::VERSIONS)) {
            Opened::Missing => return Ok(()),
            Opened::Other => {
                self.marks.aliases = false;
                return Ok(());
            }
            Opened::Folder(folder) => folder,
        };
        let Some(listed) = self.list(&folder, self.caps.alias_entries, at)? else {
            self.marks.aliases = false;
            return Ok(());
        };
        let mut buffer = Vec::new();
        for entry in listed.iter().filter(|entry| entry.is_file) {
            if !names::alias_stem(&entry.name).is_some_and(|stem| self.is_target_alias(stem)) {
                continue;
            }
            match self.read_note(&folder, root_path, entry, &mut buffer) {
                Err(Gone) => {
                    self.marks.aliases = false;
                    return Ok(());
                }
                Ok(false) => {}
                Ok(true) => {
                    for key in names::keys_the_app_reads(&buffer) {
                        if !self.keys.contains(&key) {
                            if self.keys.len() >= self.caps.target_keys {
                                return Err(Refusal::TooLarge);
                            }
                            self.keys.push(key);
                        }
                    }
                }
            }
        }
        for entry in &listed {
            let finder = backups::finder_file(entry.name.as_encoded_bytes(), entry.is_file, entry.links);
            if finder == Some(Finder::Skip) {
                continue;
            }
            let (mut owner, mut format) = (Owner::Unknown, Format::Current);
            if let Some(Finder::Owner(found)) = finder {
                owner = found;
            } else if let Some(stem) = names::alias_stem(&entry.name).filter(|_| entry.is_file) {
                match self.read_note(&folder, root_path, entry, &mut buffer) {
                    Err(Gone) => {
                        self.marks.aliases = false;
                        return Ok(());
                    }
                    Ok(false) => {}
                    Ok(true) => {
                        format = if names::keys_strictly(&buffer).is_some() {
                            Format::Current
                        } else {
                            Format::Foreign
                        };
                        let listed_here = names::keys_the_app_reads(&buffer).iter().any(|key| self.keys.contains(key));
                        if self.is_target_alias(stem) || listed_here {
                            owner = self.placed(entry.links);
                        }
                    }
                }
            }
            let mut key = names::VERSIONS.as_bytes().to_vec();
            key.push(b'/');
            key.extend_from_slice(entry.name.as_encoded_bytes());
            self.add(Store::CorpusAliases, &key, owner, format)?;
        }
        Ok(())
    }

    /// A note's first bytes into `buffer`, if it is a private plain file; `Gone` if it isn't there any more; `false` if it can't be trusted (a pipe, another
    /// user's, group-writable, unreadable).
    fn read_note(&self, folder: &Folder, root_path: &Path, entry: &Entry, buffer: &mut Vec<u8>) -> Result<bool, Gone> {
        self.caps.opening(|| root_path.join(names::VERSIONS).join(&entry.name));
        let file = match folder.open_private_in(&entry.name) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Gone),
            Err(_) => return Ok(false),
        };
        // Not the file that was listed (linked or swapped between the listing and the opening): its bytes would place one file and `entry`'s facts describe another.
        if !is_the_listed(&file, entry) {
            return Ok(false);
        }
        buffer.clear();
        // One byte more than the most a note may be, so a note over the cap is told from one at it.
        Ok(file.take(names::ALIAS_BYTES as u64 + 1).read_to_end(buffer).is_ok())
    }

    /// The corpus root's entries: each folder named as a key, and below it each agent's folder.
    fn folders(&mut self, root: &Folder, root_path: &Path) -> Result<(), Refusal> {
        let Some(listed) = self.list(root, self.caps.root_entries, || root_path.to_path_buf())? else {
            self.marks.text = false;
            self.marks.pages = false;
            return Ok(());
        };
        for entry in &listed {
            let name = entry.name.as_encoded_bytes();
            let finder = backups::finder_file(name, entry.is_file, entry.links);
            let owner = match names::root_name(&entry.name) {
                _ if finder == Some(Finder::Skip) => continue,
                _ if let Some(Finder::Owner(owner)) = finder => owner,
                names::RootName::Versions => continue,
                names::RootName::Marker if entry.is_file && entry.links == 1 => continue,
                names::RootName::Key(key) if entry.kind == Kind::Folder => {
                    self.key_folder(root, root_path, entry, key)?;
                    continue;
                }
                _ => Owner::Unknown,
            };
            self.add(Store::CorpusText, name, owner, Format::Current)?;
        }
        Ok(())
    }

    /// One corpus's folder: placed by its key, opened by the root's descriptor, and its agent folders listed.
    fn key_folder(&mut self, root: &Folder, root_path: &Path, entry: &Entry, key: CorpusKey) -> Result<(), Refusal> {
        let name = entry.name.as_encoded_bytes();
        self.caps.opening(|| root_path.join(&entry.name));
        let folder = match root.open_in(&entry.name) {
            // Gone between the listing and the opening: what is left can't be said to be everything.
            Opened::Missing => {
                self.marks.text = false;
                self.marks.pages = false;
                return Ok(());
            }
            Opened::Other => return self.add(Store::CorpusText, name, Owner::Unknown, Format::Current),
            Opened::Folder(folder) => folder,
        };
        let owner = if self.keys.contains(&key) { self.placed(1) } else { Owner::Unknown };
        self.add(Store::CorpusText, name, owner, Format::Current)?;
        self.agent_pages(&folder, root_path, entry, owner)
    }

    /// Each format folder of a corpus (`v4`, and `v1` to `v3` from before the text was sealed): its `agent` and `agent.gone.*` entries, never opened.
    fn agent_pages(&mut self, key_folder: &Folder, root_path: &Path, key_entry: &Entry, owner: Owner) -> Result<(), Refusal> {
        let at = root_path.join(&key_entry.name);
        let Some(listed) = self.list(key_folder, self.caps.key_entries, || at.clone())? else {
            self.marks.pages = false;
            return Ok(());
        };
        let mut opened = 0;
        for sub in listed.iter().filter(|sub| sub.kind == Kind::Folder) {
            opened += 1;
            if opened > self.caps.subfolders {
                return Err(Refusal::TooLarge);
            }
            self.caps.opening(|| at.join(&sub.name));
            let folder = match key_folder.open_in(&sub.name) {
                Opened::Missing => {
                    self.marks.text = false;
                    self.marks.pages = false;
                    return Ok(());
                }
                // Listed as a folder and now something else: swapped.
                Opened::Other => {
                    self.marks.pages = false;
                    continue;
                }
                Opened::Folder(folder) => folder,
            };
            let Some(inner) = self.list(&folder, self.caps.sub_entries, || at.join(&sub.name))? else {
                self.marks.pages = false;
                continue;
            };
            for agent in inner.iter().filter(|agent| names::is_agent_folder(&agent.name)) {
                let mut key = key_entry.name.as_encoded_bytes().to_vec();
                for part in [&sub.name, &agent.name] {
                    key.push(b'/');
                    key.extend_from_slice(part.as_encoded_bytes());
                }
                let owner = if agent.kind == Kind::Folder { owner } else { Owner::Unknown };
                self.add(Store::CorpusAgentPages, &key, owner, Format::Current)?;
            }
        }
        Ok(())
    }

    /// An older build's folder of plain text beside the corpus root: whatever is there, of any kind, is one `Unknown` artifact.
    fn old_folder(&mut self, root_path: &Path) -> Result<(), Refusal> {
        if root_path.file_name() != Some(OsStr::new(names::CORPUS_ROOT_NAME)) {
            return Ok(());
        }
        let Some(parent) = root_path.parent() else { return Ok(()) };
        match Folder::open(parent) {
            Opened::Missing => Ok(()),
            Opened::Other => {
                self.marks.text = false;
                Ok(())
            }
            Opened::Folder(parent) => match parent.look(OsStr::new(names::OLD_FOLDER)) {
                Ok(_) => self.add(Store::CorpusText, b"old/corpus", Owner::Unknown, Format::Current),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => {
                    self.marks.text = false;
                    Ok(())
                }
            },
        }
    }

    /// Step 5: the generated contents, each classified by its name alone.
    fn contents(&mut self, root_path: &Path) -> Result<(), Refusal> {
        let folder = match Folder::open(root_path) {
            Opened::Missing => return Ok(()),
            Opened::Other => {
                self.marks.contents = false;
                return Ok(());
            }
            Opened::Folder(folder) => folder,
        };
        let Some(listed) = self.list(&folder, self.caps.contents_entries, || root_path.to_path_buf())? else {
            self.marks.contents = false;
            return Ok(());
        };
        for entry in &listed {
            let name = entry.name.as_encoded_bytes();
            let finder = backups::finder_file(name, entry.is_file, entry.links);
            let owner = match names::contents_name(&entry.name) {
                _ if finder == Some(Finder::Skip) => continue,
                _ if let Some(Finder::Owner(owner)) = finder => owner,
                Some((tag, key)) if entry.is_file && (tag == self.tag || self.keys.contains(&key)) => self.placed(entry.links),
                _ => Owner::Unknown,
            };
            self.add(Store::GeneratedContents, name, owner, Format::Current)?;
        }
        Ok(())
    }
}
