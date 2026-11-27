//! **Whose a crashed save is, for the person's confirmation** (the owner's rule of 2026-11-26: other documents' crashed saves are "listed by name in the
//! confirmation"). A writer's temporary is named from its document's file name by a hash ([`recto_core::save::writer_temp_prefix`]), so the document is the
//! file in the temporary's folder whose own prefix is the same; with none there, the temporary's own cleaned name is all there is to say. Reads only, and bounded:
//! the folders and the entries it looks at are capped, as everything that comes from outside is.

use super::pin::TempPin;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The most temporaries looked into, folders and entries: a longer list is cut here and the sheet says "and N more" of what it has.
pub const MAX_SAVES: usize = 64;
const MAX_FOLDERS: usize = 16;
const MAX_ENTRIES: usize = 4096;
/// `.append-` and 16 hex digits and `-`: what a writer's temporary has in common with its document.
const PREFIX_LEN: usize = ".append-".len() + 16 + 1;

/// The cleaned names of the documents the unfinished saves are of, sorted and with none twice: the document's own name where it is in the temporary's folder,
/// else the temporary's.
pub fn saves_of(temps: &[TempPin]) -> Vec<String> {
    let mut folders: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    let mut names = BTreeSet::new();
    for temp in temps.iter().take(MAX_SAVES) {
        let own = temp.name().to_string_lossy();
        let prefix = own.get(..PREFIX_LEN).unwrap_or(&own).to_owned();
        let known = folders.len() < MAX_FOLDERS || folders.contains_key(temp.folder());
        let documents = known.then(|| folders.entry(temp.folder().to_owned()).or_insert_with(|| documents_in(temp.folder())));
        let name = documents
            .and_then(|documents| documents.get(&prefix))
            .cloned()
            .unwrap_or_else(|| recto_core::clean_line(&own));
        names.insert(name);
    }
    names.into_iter().collect()
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
