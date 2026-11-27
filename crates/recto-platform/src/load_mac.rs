//! The one-minute load average, from `getloadavg`: no process is started to read it.

/// The system's one-minute load average, or `None` when the system won't say.
#[must_use]
pub fn load_average_1m() -> Option<f64> {
    let mut average = [0f64; 1];
    // SAFETY: `getloadavg` writes at most the one double the array holds.
    let read = unsafe { libc::getloadavg(average.as_mut_ptr(), 1) };
    (read == 1).then_some(average[0])
}
