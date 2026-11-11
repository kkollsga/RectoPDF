//! The long-running side of the agent interface: a session held by one process, reached over a local socket, with levels the user sets and a log of what agents did.
//!
//! Built so a host can carry it. `recto serve` runs [`server::Server`] on its main thread; the Mac app will run the same thing on a background thread with its own [`host::Host`], so `recto` works the
//! same against either. See `docs/ENGINEERING.md` for why it's one blocking `poll` and no async runtime.

pub mod client;
pub mod finding;
pub mod generating;
pub mod host;
pub mod indexing;
pub mod levels;
pub mod local;
pub mod log;
pub mod record_store;
pub mod server;
pub mod state;
pub mod terminal;

pub use state::StatePaths;

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

/// Finds one of the install's own programs beside this one, or one folder up for a test binary in `deps/`. This program's path is followed through links first, so `recto` linked
/// onto `PATH` finds the helpers beside the real one, not beside the link. A file of the install, like the render worker: nothing in the environment can point elsewhere.
pub(crate) fn program_beside(this: &std::path::Path, stem: &str) -> Option<std::path::PathBuf> {
    let name = format!("{stem}{}", std::env::consts::EXE_SUFFIX);
    let this = std::fs::canonicalize(this).unwrap_or_else(|_| this.to_path_buf());
    let here = this.parent()?;
    [Some(here), here.parent()]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

#[cfg(all(test, unix))]
mod tests {
    use super::program_beside;

    /// Failure: `recto` linked onto `PATH` looks for `recto-indexer` and `recto-headings` beside the link, finds nothing, and every index and generated contents fails with
    /// `indexer_missing`, while the app beside the real `recto` works.
    #[test]
    fn a_program_reached_through_a_link_finds_the_helpers_beside_the_real_one() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (real, linked) = (root.join("App/MacOS"), root.join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(real.join("recto"), b"").unwrap();
        std::fs::write(real.join("recto-indexer"), b"").unwrap();
        std::os::unix::fs::symlink(real.join("recto"), linked.join("recto")).unwrap();
        let found = program_beside(&linked.join("recto"), "recto-indexer");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(found, Some(real.join("recto-indexer")));
    }
}
