//! **What a quiet run may use:** the temp folders its state and its test's files live in.

use recto_platform::local;
use std::path::PathBuf;

/// **The temp folders a quiet run may use,** for its state and its test's files: the system's own (from the OS, never `$TMPDIR`, which the caller can
/// point anywhere) and `/tmp`, each as the folder it really is. One list, so the state rule and the QA verbs' file wall can't disagree.
pub fn temp_folders() -> Vec<PathBuf> {
    [local::system_temp_dir(), Some(PathBuf::from("/tmp"))]
        .into_iter()
        .flatten()
        .filter_map(|temp| temp.canonicalize().ok())
        .collect()
}
