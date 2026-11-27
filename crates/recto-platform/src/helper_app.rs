//! The structure helper's place in the app, and what a program in it owes before it reads a document. Pure paths and numbers, so every platform builds it and the host and the helper agree.
//!
//! `RectoStructure.app` is `recto-headings` in a sandboxed helper of its own, in the main app's `Contents/Helpers`, with the renderer's walls (`docs/engineering/sandbox.md`). It's the
//! renderer's layout again (`recto-render-service`'s `bundle`), and the one name it can't share, since the App Sandbox takes a helper's identity from its own bundle.

use std::path::{Path, PathBuf};

/// The helper app, in the main app's `Contents/Helpers`.
pub const STRUCTURE_APP: &str = "RectoStructure.app";
/// The helper's file name, in its `Contents/MacOS` and, in development, in `target/`.
pub const STRUCTURE_EXE: &str = "recto-headings";

/// The structure helper's program for a program in an app's `Contents/MacOS`: the app itself, `recto` and the other helpers there.
pub fn structure_helper(macos_dir: &Path) -> Option<PathBuf> {
    Some(
        macos_dir
            .parent()?
            .join("Helpers")
            .join(STRUCTURE_APP)
            .join("Contents/MacOS")
            .join(format!("{STRUCTURE_EXE}{}", std::env::consts::EXE_SUFFIX)),
    )
}

/// Whether a program at this path is the structure helper laid out in an app: it has to put its walls up before it reads a document. Run bare, as the headings helper is from
/// `target/` in development and today, it isn't, and nothing changes for it. Not knowing where it is counts as the helper (it fails closed): a wall that was asked for and can't go up
/// exits, and one that wasn't asked for never starts.
pub fn in_structure_helper(exe: Option<&Path>) -> bool {
    let Some(exe) = exe else { return true };
    let folders: Vec<_> = exe
        .ancestors()
        .take(5)
        .map(|part| part.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let exe_name = format!("{STRUCTURE_EXE}{}", std::env::consts::EXE_SUFFIX);
    matches!(folders.as_slice(), [file, macos, contents, app, helpers] if *file == exe_name && macos == "MacOS" && contents == "Contents" && app == STRUCTURE_APP && helpers == "Helpers")
}

/// Whether the structure helper must put its walls up before it reads a document: in a release build always, and in any build when it is laid out in an app. The renderer's rule again
/// (`recto-render-service`'s): a debug build run bare from `target/` has no bundle to take walls from, so it runs without, and the host reads that as `walled: false` and refuses it.
pub fn walls_required(exe: Option<&Path>, release: bool) -> bool {
    release || in_structure_helper(exe)
}

/// Exit codes a helper uses when its walls can't go up, the same numbers as the renderer's. The host reads them off the exit status; none is a crash. Each means no document was read.
pub mod exit {
    /// The App Sandbox isn't on: the helper is signed wrong.
    pub const NO_WALL: i32 = 77;
    /// macOS gave it no Mach rights to drop.
    pub const NO_RIGHTS_TO_DROP: i32 = 78;
    /// Launchd still answered after the drop.
    pub const LAUNCHD_REACHABLE: i32 = 79;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the structure helper in the shipped bundle skips its walls, so a helper signed without the sandbox reads a hostile file; or the unwalled helper beside `target/` demands a
    /// sandbox it can't have and never starts; or an unknown place isn't treated as the bundled one.
    #[test]
    fn only_the_helper_laid_out_in_an_app_needs_its_walls() {
        let bundled = Path::new("/Applications/RectoPDF.app/Contents/Helpers/RectoStructure.app/Contents/MacOS/recto-headings");
        assert!(in_structure_helper(Some(bundled)));
        assert!(in_structure_helper(Some(Path::new(
            "/tmp/Host.app/Contents/Helpers/RectoStructure.app/Contents/MacOS/recto-headings"
        ))));
        for bare in [
            "/repo/crates/recto-headings/target/debug/recto-headings",
            "/Applications/RectoPDF.app/Contents/MacOS/recto-headings",
            "/Applications/RectoPDF.app/Contents/Helpers/RectoRender.app/Contents/MacOS/recto-headings",
            "/Applications/RectoPDF.app/Contents/Helpers/RectoStructure.app/Contents/recto-headings",
            "/Applications/RectoPDF.app/Contents/Helpers/RectoStructure.app/Contents/MacOS/recto-render-service",
            "/Applications/RectoPDF.app/Elsewhere/RectoStructure.app/Contents/MacOS/recto-headings",
        ] {
            assert!(!in_structure_helper(Some(Path::new(bare))), "{bare}");
        }
        assert!(in_structure_helper(None), "not knowing where it is fails closed");
        let bare = Some(Path::new("/repo/target/debug/recto-headings"));
        assert!(walls_required(bare, true), "a release build always needs its walls, wherever it is");
        assert!(!walls_required(bare, false), "a bare debug build has none to put up");
        assert!(walls_required(Some(bundled), false), "laid out in an app, any build needs them");
        assert_eq!(
            structure_helper(Path::new("/A.app/Contents/MacOS")).unwrap(),
            Path::new("/A.app/Contents/Helpers/RectoStructure.app/Contents/MacOS").join(format!("recto-headings{}", std::env::consts::EXE_SUFFIX))
        );
    }

    /// Failure: a code is changed on one side only, and the host reads the renderer's walls as the helper's or the other way round. The three are the renderer's, since one host reads both.
    #[test]
    fn the_wall_exit_codes_are_the_renderers_and_distinct() {
        assert_eq!([exit::NO_WALL, exit::NO_RIGHTS_TO_DROP, exit::LAUNCHD_REACHABLE], [77, 78, 79]);
    }
}
