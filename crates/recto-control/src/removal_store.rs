//! Read-only surveys of what the app has kept about a document, for safe removal (`docs/design/removal.md` §4). One module per store; each
//! lists its own folder and adds what it finds to a `recto_core::removal::Survey`. Nothing here deletes, writes or opens a file for its
//! content, and nothing is wired to a command yet.

mod backups;

pub use backups::survey_backups;
