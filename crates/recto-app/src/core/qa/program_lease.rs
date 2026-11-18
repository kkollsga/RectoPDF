//! A quiet-run cancellation lease, never executable identity or authority.
use std::ffi::OsString;
use std::path::PathBuf;

pub fn program(current: Option<PathBuf>, argv0: Option<OsString>) -> Option<PathBuf> {
    current.or_else(|| argv0.map(PathBuf::from).filter(|path| path.is_absolute()))
}
