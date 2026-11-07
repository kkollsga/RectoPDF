//! The long-running side of the agent interface: a session held by one process, reached over a local socket, with levels the user sets and a log of what agents did.
//!
//! Built so a host can carry it. `recto serve` runs [`server::Server`] on its main thread; the Mac app will run the same thing on a background thread with its own [`host::Host`], so `recto` works the
//! same against either. See `docs/ENGINEERING.md` for why it's one blocking `poll` and no async runtime.

pub mod client;
pub mod host;
pub mod indexing;
pub mod levels;
pub mod local;
pub mod log;
pub mod server;
pub mod state;
pub mod terminal;

pub use state::StatePaths;
