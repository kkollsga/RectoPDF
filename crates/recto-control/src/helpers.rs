//! How the session starts its helper processes, the indexer and the headings helper: at what level, and with what environment.

/// The level the helpers run at, the indexer and the headings helper, and the render workers they start. Every job they do is one somebody asked for and
/// is waiting on: a search or Find waiting on the index's totals, Generate contents, or an agent's `wait`. Nothing starts them on its own. So every job is
/// at utility, below whatever the person is looking at and never starved. Not Darwin background, which on GitHub's virtual Macs starved both for
/// minutes (2026-11-11): work nobody waits on, should there ever be any, may go lower than this but never there.
pub const HELPER_PRIORITY: recto_platform::helper::Priority = recto_platform::helper::Priority::Utility;

/// Starts a helper at `priority` with an environment of `extra` and nothing of this process's but the two names that point a development build's helpers
/// at its render worker and PDFium, as the worker's own rules say.
pub(crate) fn spawn_helper(
    exe: &std::path::Path,
    extra: &[(&str, &std::ffi::OsStr)],
    priority: recto_platform::helper::Priority,
) -> std::io::Result<recto_platform::helper::Spawned> {
    let passed: Vec<(&str, std::ffi::OsString)> = ["RECTO_RENDER_SERVICE", "RECTO_PDFIUM_LIB"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect();
    let env: Vec<(&str, &std::ffi::OsStr)> = extra
        .iter()
        .copied()
        .chain(passed.iter().map(|(name, value)| (*name, value.as_os_str())))
        .collect();
    recto_platform::helper::spawn(exe, &env, priority)
}
