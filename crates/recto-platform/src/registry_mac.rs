//! **What Launch Services knows of the app's copies.** Owner, 2026-11-11: "Im seeing two rectopdf installs in macos. Can you check? We should only ever have one." A copy of the app is registered when it's launched, by any means, or found by a scan, and then it's in Open With and the system can open a PDF with it, whichever version it is. Build folders end in `.noindex`, so Spotlight leaves them alone, and whatever starts a copy that isn't the one install forgets it again when it's done with it, here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The system's own tool for its registrations.
pub const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// The app's identifiers all start so: the app, its renderer helper, and a test's copies.
pub const ID_PREFIX: &str = "io.github.kkollsga.RectoPDF";

/// How long a forgetting may take before it's left.
const FORGET_LIMIT: Duration = Duration::from_secs(10);

/// One copy the system has registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub path: PathBuf,
    pub identifier: String,
}

/// Tells the system to forget a bundle, there or already gone. Waits for it, within a limit. What it says is nothing to anyone.
pub fn forget(bundle: &Path) {
    let Ok(mut child) = Command::new(LSREGISTER)
        .arg("-u")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let began = Instant::now();
    while began.elapsed() < FORGET_LIMIT {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The same without waiting, for a process on its way out: the tool outlives it.
pub fn forget_soon(bundle: &Path) {
    let _ = Command::new(LSREGISTER)
        .arg("-u")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Forgets every bundle in a folder that's about to go, nested ones included (a renderer helper inside a kept copy is registered by a scan too), a few levels down. Bounded by the levels, and only folders are looked in.
pub fn forget_all_under(folder: &Path) {
    let mut bundles = Vec::new();
    find_bundles(folder, 8, &mut bundles);
    for bundle in bundles {
        forget(&bundle);
    }
}

fn find_bundles(folder: &Path, levels: u32, found: &mut Vec<PathBuf>) {
    if levels == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("Contents/Info.plist").is_file() {
            found.push(path.clone());
        }
        find_bundles(&path, levels - 1, found);
    }
}

/// Every copy of the app the system has registered, by its dump. Slow, seconds, as the dump is of everything: for a test and a tool, never the app.
#[must_use]
pub fn registered() -> Vec<Registered> {
    let Ok(output) = Command::new(LSREGISTER).arg("-dump").stdin(Stdio::null()).stderr(Stdio::null()).output() else {
        return Vec::new();
    };
    parse_dump(&String::from_utf8_lossy(&output.stdout))
}

/// The copies of the app in a dump: each record's path and identifier, for identifiers of ours. Records are separated by lines of dashes.
#[must_use]
pub fn parse_dump(dump: &str) -> Vec<Registered> {
    let mut found = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut identifier: Option<String> = None;
    let mut finish = |path: &mut Option<PathBuf>, identifier: &mut Option<String>| {
        if let (Some(path), Some(identifier)) = (path.take(), identifier.take())
            && (identifier == ID_PREFIX || identifier.starts_with(&format!("{ID_PREFIX}.")))
        {
            found.push(Registered { path, identifier });
        }
    };
    for line in dump.lines() {
        if line.starts_with("--------") {
            finish(&mut path, &mut identifier);
            continue;
        }
        if let Some(rest) = line.strip_prefix("path:") {
            // `path:   /a/b/RectoPDF.app (0x3b1f4)`
            let rest = rest.trim();
            let rest = rest.rsplit_once(" (0x").map_or(rest, |(before, _)| before);
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("identifier:") {
            identifier = Some(rest.trim().to_owned());
        }
    }
    finish(&mut path, &mut identifier);
    found
}

/// What's registered and shouldn't be: anything that isn't one of the copies allowed, or inside one (the one install's own renderer helper is registered too).
#[must_use]
pub fn strays(registered: &[Registered], allowed: &[&Path]) -> Vec<Registered> {
    registered
        .iter()
        .filter(|copy| !allowed.iter().any(|allowed| copy.path.starts_with(allowed)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
--------------------------------------------------------------------------------
bundle id:                  1234
path:                       /Applications/RectoPDF.app (0x3b1f4)
name:                       RectoPDF
identifier:                 io.github.kkollsga.RectoPDF
--------------------------------------------------------------------------------
path:                       /Applications/RectoPDF.app/Contents/Helpers/RectoRender.app (0x3b1f8)
identifier:                 io.github.kkollsga.RectoPDF.Render.adhoc
--------------------------------------------------------------------------------
path:                       /private/var/folders/x/T/recto-update-e2e-1-2.noindex/install/RectoPDF.app (0x3a7b8)
identifier:                 io.github.kkollsga.RectoPDF.e2e
--------------------------------------------------------------------------------
path:                       /Applications/Preview.app (0x1)
identifier:                 com.apple.Preview
--------------------------------------------------------------------------------
path:                       /Users/me/RectoPDFish.app (0x2)
identifier:                 io.github.kkollsga.RectoPDFish
--------------------------------------------------------------------------------
path:                       /Users/me/build dir/app/build/RectoPDF.app (0x3)
identifier:                 io.github.kkollsga.RectoPDF
";

    /// Failure: a copy of ours is missed in the dump, as a path with a space or the trailing number would hide it, or another app is taken for one of ours.
    #[test]
    fn the_dump_gives_our_copies_and_nothing_else() {
        let found = parse_dump(DUMP);
        let paths: Vec<&str> = found.iter().map(|copy| copy.path.to_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "/Applications/RectoPDF.app",
                "/Applications/RectoPDF.app/Contents/Helpers/RectoRender.app",
                "/private/var/folders/x/T/recto-update-e2e-1-2.noindex/install/RectoPDF.app",
                "/Users/me/build dir/app/build/RectoPDF.app",
            ]
        );
        let stray = strays(&found, &[Path::new("/Applications/RectoPDF.app")]);
        assert_eq!(stray.len(), 2, "the one install and its helper are fine: {stray:?}");
    }

    /// Failure: a test's copy stays registered after the test, as forgetting only the top bundle left a kept copy's renderer helper in the system's list (2026-11-11).
    #[test]
    fn every_bundle_in_a_folder_is_found_to_forget() {
        let dir = std::env::temp_dir().join(format!("recto-registry-{}.noindex", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for bundle in [
            "install/RectoPDF.app",
            "install/.RectoPDF-update/previous/RectoPDF.kept",
            "install/.RectoPDF-update/previous/RectoPDF.kept/Contents/Helpers/RectoRender.app",
        ] {
            std::fs::create_dir_all(dir.join(bundle).join("Contents")).unwrap();
            std::fs::write(dir.join(bundle).join("Contents/Info.plist"), "x").unwrap();
        }
        std::fs::create_dir_all(dir.join("not-a-bundle/Contents")).unwrap();
        let mut found = Vec::new();
        find_bundles(&dir, 8, &mut found);
        found.sort();
        let names: Vec<String> = found.iter().map(|path| path.strip_prefix(&dir).unwrap().display().to_string()).collect();
        assert_eq!(
            names,
            [
                "install/.RectoPDF-update/previous/RectoPDF.kept",
                "install/.RectoPDF-update/previous/RectoPDF.kept/Contents/Helpers/RectoRender.app",
                "install/RectoPDF.app",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
