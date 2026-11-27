//! **Whose a crashed save is, for the person's confirmation** (the owner's rule of 2026-11-26: other documents' crashed saves are "listed by name in the
//! confirmation"). A writer's temporary is named from its document's file name by a hash ([`recto_core::save::writer_temp_prefix`]), so the document is the
//! file in the temporary's folder whose own prefix is the same. A save of the document asked about is skipped without reading anything (the sheet already
//! says it is beside that document); a save of another document is named by the file found, or counted as one whose document isn't there, never shown by the
//! temporary's own name. Reads only, and bounded: the saves, folders and entries looked at are capped, and past the cap the answer says it isn't the whole.

use super::pin::TempPin;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The most temporaries looked into: more are not read, and the answer says there are more.
pub const MAX_SAVES: usize = 64;
const MAX_FOLDERS: usize = 16;
const MAX_ENTRIES: usize = 4096;
/// `.append-` and 16 hex digits and `-`: what a writer's temporary has in common with its document.
const PREFIX_LEN: usize = ".append-".len() + 16 + 1;

/// What the saves of other documents come to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Others {
    /// The cleaned names of the other documents found, sorted, none twice.
    pub names: Vec<String>,
    /// Saves of a document that isn't in the folder.
    pub missing: u32,
    /// There were more saves than [`MAX_SAVES`].
    pub more: bool,
}

/// The other documents the unfinished saves are of, `document`'s own left out.
pub fn saves_of(temps: &[TempPin], document: &Path) -> Others {
    let own = recto_core::save::writer_temp_prefix(document);
    let mut folders: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    let mut names = BTreeSet::new();
    let mut missing = 0;
    for temp in temps.iter().take(MAX_SAVES) {
        let text = temp.name().to_string_lossy();
        let prefix = text.get(..PREFIX_LEN).unwrap_or(&text);
        if prefix == own {
            continue;
        }
        let known = folders.len() < MAX_FOLDERS || folders.contains_key(temp.folder());
        let found = known
            .then(|| folders.entry(temp.folder().to_owned()).or_insert_with(|| documents_in(temp.folder())))
            .and_then(|documents| documents.get(prefix));
        match found {
            Some(name) => {
                names.insert(name.clone());
            }
            None => missing += 1,
        }
    }
    Others {
        names: names.into_iter().collect(),
        missing,
        more: temps.len() > MAX_SAVES,
    }
}

/// The files in `folder` by the prefix their writer's temporaries carry, to their cleaned names. Capped; a name that isn't text is left out.
fn documents_in(folder: &Path) -> HashMap<String, String> {
    let mut found = HashMap::new();
    let Ok(entries) = std::fs::read_dir(folder) else { return found };
    for entry in entries.take(MAX_ENTRIES).flatten() {
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        if text.starts_with(".append-") {
            continue;
        }
        found
            .entry(recto_core::save::writer_temp_prefix(Path::new(&name)))
            .or_insert_with(|| recto_core::clean_line(text));
    }
    found
}
