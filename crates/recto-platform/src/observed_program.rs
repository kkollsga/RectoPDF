//! Process-path observation only. A missing executable name can still belong to a live mapped program.
use std::io;
use std::path::{Path, PathBuf};

pub fn path(executable: &Path) -> io::Result<PathBuf> {
    select(executable, executable.canonicalize(), Path::canonicalize)
}

pub fn select(executable: &Path, found: io::Result<PathBuf>, parent: impl FnOnce(&Path) -> io::Result<PathBuf>) -> io::Result<PathBuf> {
    match found {
        Err(error) if error.kind() == io::ErrorKind::NotFound && executable.is_absolute() => {
            let Some(name) = executable.file_name() else { return Err(error) };
            let Some(folder) = executable.parent() else { return Err(error) };
            parent(folder).map(|folder| folder.join(name))
        }
        other => other,
    }
}
