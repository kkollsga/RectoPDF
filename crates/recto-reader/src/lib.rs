//! The page view, with no toolkit in sight: where pages go, which tiles are wanted, what stays in memory and what the host is told to draw. Rust decides, the host draws.
//!
//! - [`layout`]: where pages sit. Costs nothing per page for a document of one page size.
//! - [`tiles`]: the policy. No threads or system calls beyond a surface's own, so the UI thread can call it and a test can drive it.
//! - [`engine`]: one thread, parked at rest, that runs what [`tiles`] asks for through a `DocRenderer`.
//! - [`paper`]: dark paper, as a change to a tile's pixels.

pub mod engine;
pub mod layout;
pub mod paper;
pub mod tiles;

pub use engine::{Engine, EngineConfig};
pub use layout::{PageBox, PageLayout, PageRect};
pub use paper::PaperKind;
pub use tiles::{Op, Reader, ReaderReport, Viewport};
