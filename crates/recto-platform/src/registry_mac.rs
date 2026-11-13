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

/// **Forgets a bundle, and those inside it, until the system's dump no longer has any of them,** within `limit`: whether it did. One `-u` isn't
/// enough when the copy was launched a moment ago, as the system registers a copy it launches as that launch goes on, and a forgetting that came
/// first was undone (CI run 43101058944: a test's own copy, opened and then forgotten, was still registered after). Each look is a whole dump,
/// seconds, so it's for a test and a tool, never the app. The bundle should still be there, so the copies inside it can be found.
pub fn forget_until_gone(bundle: &Path, limit: Duration) -> bool {
    let began = Instant::now();
    loop {
        forget_all_under(bundle);
        forget(bundle);
        if !registered().iter().any(|copy| copy.path.starts_with(bundle)) {
            return true;
        }
        if began.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// **How a forgetting is settled: by looks, not by the clock.** A registration still on its way can land after one clean look, so a copy is gone when two
/// looks in a row say so; and each look is a whole dump of the registry, which takes a few seconds on a quiet machine and has taken over 30 on a starved
/// one (CI, 2026-11-13: a bound of 20 s gave up after one or two looks, and the copy was called left). So the bound is a number of looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settling {
    clean: u32,
    looks: u32,
}

/// What a look came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Look again.
    NotYet,
    /// Clean twice in a row.
    Gone,
    /// Every look allowed has been made, and it never stayed gone.
    GaveUp,
}

impl Settling {
    /// Clean looks in a row that mean gone.
    pub const CLEAN: u32 = 2;
    /// The most looks made.
    pub const MAX_LOOKS: u32 = 12;

    /// A look was made: `clean` if nothing of it was registered or running.
    pub fn after_look(&mut self, clean: bool) -> Settled {
        self.looks += 1;
        self.clean = if clean { self.clean + 1 } else { 0 };
        if self.clean >= Self::CLEAN {
            Settled::Gone
        } else if self.looks >= Self::MAX_LOOKS {
            Settled::GaveUp
        } else {
            Settled::NotYet
        }
    }

    /// How many looks have been made.
    #[must_use]
    pub fn looks(self) -> u32 {
        self.looks
    }
}

/// One look: forgets `bundle` and everything in it, then says whether the dump has none of it.
pub fn forget_and_look(bundle: &Path) -> bool {
    forget_all_and_look(std::slice::from_ref(&bundle.to_path_buf()))
}

/// One look for a whole set: forgets every bundle, and everything in each, then reads the dump once and says whether it has none of them. A set costs the
/// one dump a single bundle does, which is why a run's ended copies are settled together.
pub fn forget_all_and_look(bundles: &[PathBuf]) -> bool {
    for bundle in bundles {
        forget_all_under(bundle);
        forget(bundle);
    }
    none_registered_under(&registered(), bundles)
}

/// Whether none of these bundles, nor anything inside one, is in the dump.
#[must_use]
pub fn none_registered_under(all: &[Registered], bundles: &[PathBuf]) -> bool {
    !all.iter().any(|copy| bundles.iter().any(|bundle| copy.path.starts_with(bundle)))
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

/// What a Launch Services file in a failure's evidence starts with when the dump answered.
pub const DESCRIBED: &str = "registered in this checkout";

/// The text of a failure's Launch Services file for a dump: the copies in this checkout and in temp by path, and only a count of every other copy, so no other tree's path is named.
#[must_use]
pub fn describe(all: &[Registered], checkout: &Path, temp: &Path) -> String {
    let (mine, elsewhere): (Vec<_>, Vec<_>) = all.iter().partition(|copy| copy.path.starts_with(checkout) || copy.path.starts_with(temp));
    let mut text = format!("{DESCRIBED} ({}) or temp ({}):\n", checkout.display(), temp.display());
    for copy in &mine {
        text.push_str(&format!("  {} ({})\n", copy.path.display(), copy.identifier));
    }
    if mine.is_empty() {
        text.push_str("  none\n");
    }
    text.push_str(&format!("registered elsewhere, not named: {}\n", elsewhere.len()));
    text
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

    /// Failure: the Launch Services file names a copy from another tree (the owner's `/Applications`, another checkout), or leaves out one in this tree or in temp, or loses the count. In-process, so it doesn't depend on what the machine has registered or on the dump answering in time (CI, 2026-11-13).
    #[test]
    fn the_evidence_file_names_this_tree_and_counts_the_rest() {
        let copy = |path: &str| Registered {
            path: PathBuf::from(path),
            identifier: ID_PREFIX.into(),
        };
        let (checkout, temp) = (Path::new("/work/tree"), Path::new("/var/tmp1"));
        let all = [
            copy("/work/tree/target/RectoPDF.app"),
            copy("/var/tmp1/a/RectoPDF.app"),
            copy("/Applications/RectoPDF.app"),
            copy("/work/other/RectoPDF.app"),
        ];
        let text = describe(&all, checkout, temp);
        assert!(text.starts_with(DESCRIBED), "{text}");
        assert!(
            text.contains("/work/tree/target/RectoPDF.app") && text.contains("/var/tmp1/a/RectoPDF.app"),
            "{text}"
        );
        assert!(!text.contains("/Applications") && !text.contains("/work/other"), "{text}");
        assert!(text.contains("registered elsewhere, not named: 2"), "{text}");
        let none = describe(&[copy("/Applications/RectoPDF.app")], checkout, temp);
        assert!(
            none.contains("  none\n") && none.contains("not named: 1") && !none.contains("/Applications"),
            "{none}"
        );
    }

    /// Failure: a set settled together is called gone while one of its bundles, or something inside one, is still in the dump, or a sibling folder
    /// with a longer name is taken for it (`own-1-1` for `own-1-10`).
    #[test]
    fn a_set_is_gone_only_when_none_of_it_is_registered() {
        let copy = |path: &str| Registered {
            path: PathBuf::from(path),
            identifier: ID_PREFIX.into(),
        };
        let set = [PathBuf::from("/b/own-1-1/RectoPDF.app"), PathBuf::from("/t/bed/install/RectoPDF.app")];
        assert!(none_registered_under(&[], &set));
        assert!(none_registered_under(
            &[copy("/b/own-1-10/RectoPDF.app"), copy("/Applications/RectoPDF.app")],
            &set
        ));
        assert!(!none_registered_under(&[copy("/t/bed/install/RectoPDF.app")], &set), "the second of the set");
        assert!(
            !none_registered_under(&[copy("/b/own-1-1/RectoPDF.app/Contents/Helpers/RectoRender.app")], &set),
            "a helper inside the first"
        );
    }

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
        let dir = recto_testpdf::TempDir::with_suffix(&std::env::temp_dir(), "recto-registry-", ".noindex");
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

    /// Failure (CI run 43392356580, 2026-11-13): a copy was called left after 20 s, in which a starved runner had made one or two looks. Settling goes by
    /// looks: gone is two clean in a row whatever the time, a dirty look starts the count again, and it gives up only after every look allowed.
    #[test]
    fn a_forgetting_settles_by_looks_and_not_by_the_clock() {
        let mut settling = Settling::default();
        assert_eq!(
            settling.after_look(true),
            Settled::NotYet,
            "one clean look isn't gone: a registration can still land"
        );
        assert_eq!(settling.after_look(false), Settled::NotYet, "it landed");
        assert_eq!(settling.after_look(true), Settled::NotYet, "the count began again");
        assert_eq!(settling.after_look(true), Settled::Gone);
        let mut never = Settling::default();
        for look in 1..Settling::MAX_LOOKS {
            assert_eq!(never.after_look(look % 2 == 0), Settled::NotYet, "look {look}");
        }
        assert_eq!(never.after_look(false), Settled::GaveUp);
        assert_eq!(never.looks(), Settling::MAX_LOOKS);
    }
}
