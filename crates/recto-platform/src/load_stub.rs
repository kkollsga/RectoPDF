//! The load average is the Mac's: elsewhere it isn't read.

/// Always `None` here.
#[must_use]
pub fn load_average_1m() -> Option<f64> {
    None
}
