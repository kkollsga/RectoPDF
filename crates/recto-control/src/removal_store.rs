//! Read-only surveys of what the app has kept about a document, for safe removal (`docs/design/removal.md` §4). One module per store; each
//! lists its own folder and adds what it finds to a `recto_core::removal::Survey`. Nothing here deletes, writes or opens a file for its
//! content, and nothing is wired to a command yet.

mod backups;
/// The record store writes in place by descriptor, which only the Mac has (the folder helper is a stand-in elsewhere).
#[cfg(unix)]
pub mod barrier;
#[cfg(test)]
mod copy_fixtures;
#[cfg(test)]
mod destinations_tests;
mod journals;
pub mod ledger;
pub mod leftovers;
mod legacy_session;
mod placing;
mod record;
mod update_copy;
mod writer_temps;

pub use backups::survey_backups;
pub use journals::survey_journals;
pub use legacy_session::survey_legacy_sessions;
pub use record::survey_record;
pub use update_copy::survey_update_copy;
pub use writer_temps::survey_writer_temps;
#[cfg(test)]
mod testing;
