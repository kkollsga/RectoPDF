//! **One install.** Owner, 2026-11-11: "Im seeing two rectopdf installs in macos. Can you check? We should only ever have one." Every copy of the app the system has registered is in Open With, and the system can open a PDF with any of them. The one install is `/Applications/RectoPDF.app`. The app's and the update's tests start copies, which the system registers. Each test ends its copy and tells the system once to forget it, with no look; this stage, which runs last, settles them all together: forgets every copy this tree's tests made, looks until two looks in a row are clean, and fails only for what it can show a test left (a process still running, a folder still on disk, a registration that won't stay gone).
//!
//! It runs on its own, after those tests (`tools/check.sh --full`, its own stage), as it's about what they left. Skipped where the app's tests are (`RECTO_SKIP_APP`).

use recto_platform::registry::{self, Registered};
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "common/bed_folder.rs"]
mod bed_folder;

/// This checkout, wherever it is.
fn checkout() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The main checkout, which the worktrees share, for the old dev copy in its `target/dev` until the coordinator retires it.
fn main_checkout() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(checkout())
        .output()
        .ok()?;
    let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    common.parent()?.canonicalize().ok()
}

/// Whose a registered copy is, for what's done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whose {
    /// Inside this checkout, as its build folders are, and not another worktree's inside it.
    ThisTree,
    /// A test's, in a `recto-` folder of its own in temp, whichever tree's test made it.
    ATest,
    /// Anywhere else: `/Applications`, `~/Applications`, another volume, another worktree. Said, never changed.
    Elsewhere,
}

fn whose(copy: &Registered, checkout: &Path, temp: &Path) -> Whose {
    if let Ok(rest) = copy.path.strip_prefix(checkout)
        && rest.components().next().is_none_or(|first| first.as_os_str() != ".worktrees")
    {
        return Whose::ThisTree;
    }
    if let Ok(rest) = copy.path.strip_prefix(temp)
        && rest
            .components()
            .next()
            .is_some_and(|first| first.as_os_str().to_string_lossy().starts_with("recto-"))
    {
        return Whose::ATest;
    }
    Whose::Elsewhere
}

/// How long after it was made another tree's ended test's copy is left to that tree's own stage, which forgets it and answers for it. After this it is stale: its tree is gone, or never looked.
const OTHERS_GRACE_MS: u64 = 60 * 60 * 1000;

/// What the stage does with one registered copy that is not an allowed install.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// A test of some tree is running in it still. Not looked at.
    InUse,
    /// Left by this tree's own tests in a way no test is allowed to: forgotten, and the stage fails.
    Blame,
    /// A copy this tree's own test ended and removed, or should have (an `own-*` copy beside the build, or a bed in temp carrying this checkout's mark), whose test is gone: settled with the rest, once, at the end of the stage, and not a failure by itself. It fails if a process of it is running, if it won't stay forgotten, or if `folder`, the test's own, is still on disk: a test that didn't remove its copy.
    Ended { folder: PathBuf },
    /// Forgotten, and the stage doesn't fail, with why: an older tree's before folders carried a tree, or another tree's, stale.
    Forget(&'static str),
    /// This checkout's own build folder, `app/build.noindex/RectoPDF.app` and no other path: the one every app test launches, so no test can be named for it. Forgotten until it stays gone, said in one line, and not a failure unless it won't stay gone or a process of it is running.
    BuildFolder,
    /// Said, and left to whoever it belongs to.
    Leave(&'static str),
}

/// This checkout's build folder, the one path `Verdict::BuildFolder` is for.
fn build_folder(checkout: &Path) -> PathBuf {
    checkout.join("app/build.noindex/RectoPDF.app")
}

/// Which `own-*` folder beside the build a path is in, and the process that made it: a test's own copy of the app (`OwnCopy`).
fn own_copy_of(path: &Path, checkout: &Path) -> Option<(PathBuf, u32)> {
    let beside = build_folder(checkout).parent()?.to_path_buf();
    let name = path.strip_prefix(&beside).ok()?.components().next()?.as_os_str().to_string_lossy().into_owned();
    Some((beside.join(&name), bed_folder::parse_own(&name)?))
}

/// The process that made a folder of a test's copy, if this tree's: an `own-*` beside the build (named for the tree by where it is), or a bed in temp carrying this checkout's mark.
fn maker_of_this_trees_folder(name: &str, in_build_folder: bool, checkout: &Path) -> Option<u32> {
    if in_build_folder {
        return bed_folder::parse_own(name);
    }
    let made = bed_folder::parse(name)?;
    (name.ends_with(".noindex") && made.tree == Some(bed_folder::tree_of(checkout))).then_some(made.pid)
}

/// Folders of this tree's tests' copies still on disk whose test has ended, registered or not: a test that did not remove its copy.
fn folders_left_by_ended_tests(checkout: &Path, temp: &Path, alive: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let beside = build_folder(checkout).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut left = Vec::new();
    for (parent, in_build_folder) in [(&beside, true), (&temp.to_path_buf(), false)] {
        let Ok(entries) = std::fs::read_dir(parent) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if maker_of_this_trees_folder(&name, in_build_folder, checkout).is_some_and(|pid| !alive(pid)) {
                left.push(entry.path());
            }
        }
    }
    left
}

/// The programs of a copy that could be running: the app and its updater.
fn programs_of(bundle: &Path) -> [PathBuf; 2] {
    ["RectoPDF", "recto-updater"].map(|name| bundle.join("Contents/MacOS").join(name))
}

/// Forgets all of these bundles until two looks in a row (one dump each, however many bundles) find none registered, bounded by looks and not by the clock
/// (`registry::Settling`); fails if a process of any is running, naming it, or if they won't stay gone. Returns how long it took.
fn settle(paths: &[PathBuf]) -> Result<std::time::Duration, String> {
    let began = std::time::Instant::now();
    let mut settling = registry::Settling::default();
    loop {
        let mut running = Vec::new();
        for path in paths {
            for program in programs_of(path) {
                for pid in recto_platform::local::processes_running(&program) {
                    running.push(format!("{} is still running as process {pid}", program.display()));
                }
            }
        }
        if !running.is_empty() {
            return Err(running.join("; "));
        }
        match settling.after_look(registry::forget_all_and_look(paths)) {
            registry::Settled::Gone => return Ok(began.elapsed()),
            registry::Settled::GaveUp => {
                return Err(format!(
                    "{paths:?} would not stay forgotten in {} looks ({:.1} s)",
                    settling.looks(),
                    began.elapsed().as_secs_f64()
                ));
            }
            registry::Settled::NotYet => {}
        }
    }
}

/// What is done with a copy. **Another tree's ended test's copy is never this tree's to change, nor to answer for:** its tree's own stage forgets it and fails on it (2026-11-13: this stage forgot a copy another tree's update test had made 3 s before, blamed its own tree, and hid it from the tree whose test had left it).
fn judge(copy: &Registered, checkout: &Path, temp: &Path, now_ms: u64, alive: impl Fn(u32) -> bool) -> Verdict {
    if copy.path == build_folder(checkout) {
        return Verdict::BuildFolder;
    }
    match whose(copy, checkout, temp) {
        Whose::Elsewhere => Verdict::Leave("not this check's to change"),
        Whose::ThisTree => match own_copy_of(&copy.path, checkout) {
            Some((_, pid)) if alive(pid) => Verdict::InUse,
            Some((folder, _)) => Verdict::Ended { folder },
            None => Verdict::Blame,
        },
        Whose::ATest => {
            let folder = copy.path.strip_prefix(temp).ok().and_then(|rest| rest.components().next());
            let name = folder.map(|part| part.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            if !name.ends_with(".noindex") {
                return Verdict::Forget("an older tree's test folder");
            }
            let Some(made) = bed_folder::parse(&name) else {
                return Verdict::Forget("an unnamed test's folder");
            };
            if alive(made.pid) {
                return Verdict::InUse;
            }
            match made.tree {
                None => Verdict::Forget("an older tree's bed, from before folders named their tree"),
                Some(tree) if tree == bed_folder::tree_of(checkout) => Verdict::Ended { folder: temp.join(&name) },
                Some(_) if now_ms.saturating_sub(made.at_ms) > OTHERS_GRACE_MS => Verdict::Forget("another tree's, stale"),
                Some(_) => Verdict::Leave("another tree's test left it; its own stage forgets it"),
            }
        }
    }
}

/// For each stray, what to read in a failure: when its folder was made, whether its process is alive, whose tree, and whether the folder is still on disk.
fn describe(copy: &Registered, checkout: &Path, temp: &Path, now_ms: u64, alive: impl Fn(u32) -> bool) -> String {
    let on_disk = if copy.path.exists() { "on disk" } else { "not on disk" };
    let Some(made) = copy
        .path
        .strip_prefix(temp)
        .ok()
        .and_then(|rest| rest.components().next())
        .and_then(|part| bed_folder::parse(&part.as_os_str().to_string_lossy()))
    else {
        return format!("not a bed's folder; {on_disk}");
    };
    let tree = match &made.tree {
        Some(tree) if *tree == bed_folder::tree_of(checkout) => "this tree's".to_owned(),
        Some(tree) => format!("another tree's ({tree}; this one is {})", bed_folder::tree_of(checkout)),
        None => "of no known tree".to_owned(),
    };
    let process = if alive(made.pid) { "alive" } else { "gone" };
    format!(
        "made {} s ago (wall ms {}), by process {} ({process}), {tree}; {on_disk}",
        now_ms.saturating_sub(made.at_ms) / 1000,
        made.at_ms,
        made.pid
    )
}

/// Failure: a copy of the app that isn't the one install is left registered after the tests, so a second RectoPDF shows in Open With: this checkout's build folder (`app/build.noindex`), a test's install in temp (`recto-update-e2e-*`), or a kept copy's renderer helper, which is how about 40 were found on 2026-11-11. This tree's and the tests' are forgotten here; anything else is only said, as it's not this check's to change. Another tree's update test's copy is its own tree's to answer for: said, and left unless it is over an hour old.
///
/// **This checkout's own build folder (`app/build.noindex/RectoPDF.app`, that path only) is forgotten here until it stays gone, said in one line, and is not a failure.** Owner, 2026-11-13: "the check ... reports and removes a stray and fails only when it can show which test left it". Nearly every app test launches that bundle and forgets it once, with no look, so a late registration (CI run 43369527265: `registered, and forgotten now, this tree's: .../app/build.noindex/RectoPDF.app`, "not a bed's folder; on disk", the four reruns clean) can come from any of about 180 tests, and none can be named. A direct run of the built binary that ends at once left it registered 20 of 20 times by hand, which is why no test may do that (`app_launch_guard`). It still fails if a process of that app is running, or it won't stay forgotten within 12 looks.
///
/// **So is every copy this tree's tests made and ended** (2026-11-13, the same go-ahead; ending each copy with a settling of its own cost CI about a minute: `end_and_forget` per copy, a whole dump per look, about twenty settlings a run). A test's `OwnCopy` and a bed end by telling the system once to forget the copy and removing it, with no look. Here all of them, and the build folder, are forgotten together and looked at until two looks in a row are clean: two or three dumps a run, one line each saying what was found. It fails for a process of any of them still running, a registration that won't stay gone, and a copy's folder still on disk after its test ended, registered or not, named with when it was made and by which process. Any other path in this checkout still fails the stage. Alone, it settles what it finds the same way.
#[test]
fn no_copy_but_the_one_install_is_left_registered() {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        eprintln!("skipped: it's about what the app's tests left, and they're skipped");
        return;
    }
    let checkout = checkout();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    let old_dev_copy = main_checkout().map(|main| main.join("target/dev/RectoPDF.app"));
    // The one install, and what its updater keeps beside it: `RectoPDF.kept` and `RectoPDF.staged` aren't apps to a scan, but the background-only renderer helper inside is found, and is never in Open With.
    let mut allowed = vec![PathBuf::from("/Applications/RectoPDF.app"), PathBuf::from("/Applications/.RectoPDF-update")];
    allowed.extend(old_dev_copy);
    let allowed: Vec<&Path> = allowed.iter().map(PathBuf::as_path).collect();
    let strays = registry::strays(&registry::registered(), &allowed);
    let now_ms = recto_update::wall_ms();
    let alive = recto_platform::local::process_exists;
    let mut this_tree = Vec::new();
    // What is settled together at the end: the build folder and every ended copy of this tree's.
    let mut to_settle: Vec<PathBuf> = Vec::new();
    let mut folders: Vec<(PathBuf, String)> = Vec::new();
    for copy in &strays {
        let what = describe(copy, &checkout, &temp, now_ms, alive);
        match judge(copy, &checkout, &temp, now_ms, alive) {
            Verdict::InUse => {}
            Verdict::Leave(why) => eprintln!("registered, and left ({why}): {} ({}): {what}", copy.path.display(), copy.identifier),
            Verdict::Forget(why) => {
                eprintln!("registered, and forgotten now ({why}): {} ({}): {what}", copy.path.display(), copy.identifier);
                registry::forget(&copy.path);
            }
            Verdict::BuildFolder => {
                eprintln!(
                    "registered, and forgotten with the rest below, this checkout's build folder, which every app test launches and none can be named for: {} ({})",
                    copy.path.display(),
                    copy.identifier
                );
                to_settle.push(copy.path.clone());
            }
            Verdict::Ended { folder } => {
                eprintln!(
                    "registered, and forgotten with the rest below, a copy this tree's test ended: {} ({}): {what}",
                    copy.path.display(),
                    copy.identifier
                );
                to_settle.push(copy.path.clone());
                if !folders.iter().any(|(seen, _)| *seen == folder) {
                    folders.push((folder, what));
                }
            }
            Verdict::Blame => {
                eprintln!(
                    "registered, and forgotten now, this tree's: {} ({}): {what}",
                    copy.path.display(),
                    copy.identifier
                );
                this_tree.push(copy.path.clone());
                registry::forget(&copy.path);
            }
        }
    }
    let mut unsettled = Vec::new();
    if !to_settle.is_empty() {
        match settle(&to_settle) {
            Ok(took) => eprintln!(
                "settled {} registered bundle(s) of this tree's ended copies and its build folder together: forgotten until they stayed gone, in {} ms",
                to_settle.len(),
                took.as_millis()
            ),
            Err(why) => unsettled.push(why),
        }
    }
    // A test that ended its copy has removed its folder. One still there is a test's bug, and is named; removed here unless a process is still in it.
    let mut left_on_disk = Vec::new();
    for folder in folders_left_by_ended_tests(&checkout, &temp, alive) {
        let what = folders
            .iter()
            .find(|(registered, _)| *registered == folder)
            .map_or_else(|| "not registered".to_owned(), |(_, what)| what.clone());
        left_on_disk.push(format!("{} ({what})", folder.display()));
        if unsettled.is_empty() {
            let _ = std::fs::remove_dir_all(&folder);
        }
    }
    assert!(
        unsettled.is_empty(),
        "a copy of this tree's, or its build folder, could not be made to stay forgotten, or a process of it is running: {unsettled:?}"
    );
    assert!(
        left_on_disk.is_empty(),
        "these folders of this tree's tests' copies were still on disk after their tests ended, so the test did not remove its copy (removed now): {left_on_disk:#?}"
    );
    assert!(
        this_tree.is_empty(),
        "this tree's tests left these registered, each a second RectoPDF in Open With: {this_tree:#?}"
    );
}

/// Failure: what's this tree's to answer for, or this check's to change, is told wrongly: another worktree's running test or build folder is blamed or changed, a copy in Applications or on another volume is touched, or this checkout's build folder isn't caught.
#[test]
fn what_counts_as_this_trees() {
    let checkout = Path::new("/w/tree");
    let temp = Path::new("/t");
    let copy = |path: &str| Registered {
        path: PathBuf::from(path),
        identifier: "io.github.kkollsga.RectoPDF".into(),
    };
    assert_eq!(whose(&copy("/w/tree/app/build.noindex/RectoPDF.app"), checkout, temp), Whose::ThisTree);
    assert_eq!(
        whose(&copy("/w/tree/.worktrees/other/app/build.noindex/RectoPDF.app"), checkout, temp),
        Whose::Elsewhere,
        "another worktree's, inside the main checkout"
    );
    for elsewhere in [
        "/Applications/Old RectoPDF.app",
        "/Users/me/Applications/RectoPDF.app",
        "/Volumes/Backup/RectoPDF.app",
        "/w/other/app/build/RectoPDF.app",
        "/t/someone-else/RectoPDF.app",
    ] {
        assert_eq!(whose(&copy(elsewhere), checkout, temp), Whose::Elsewhere, "{elsewhere}");
        assert!(
            matches!(judge(&copy(elsewhere), checkout, temp, 0, |_| false), Verdict::Leave(_)),
            "{elsewhere}"
        );
    }
    assert_eq!(whose(&copy("/t/recto-update-e2e-1-2/install/RectoPDF.app"), checkout, temp), Whose::ATest);
    let none_alive = |_: u32| false;
    let now = 1_794_531_500_000;
    let bed = |pid: u32, at: u64, tree: &Path| format!("/t/{}/install/RectoPDF.app", bed_folder::name(pid, at, tree));
    let judged = |path: &str, alive: &dyn Fn(u32) -> bool| judge(&copy(path), checkout, temp, now, alive);
    assert_eq!(
        judged("/w/tree/app/build.noindex/RectoPDF.app", &none_alive),
        Verdict::BuildFolder,
        "the one path every app test launches"
    );
    assert_eq!(
        judged("/w/tree/app/build.noindex/own-1-0/RectoPDF.app", &none_alive),
        Verdict::Ended {
            folder: PathBuf::from("/w/tree/app/build.noindex/own-1-0")
        },
        "a test's copy beside it, its test gone: settled with the rest"
    );
    assert_eq!(
        judged("/w/tree/app/build.noindex/own-1-0/RectoPDF.app/Contents/Helpers/RectoRender.app", &none_alive),
        Verdict::Ended {
            folder: PathBuf::from("/w/tree/app/build.noindex/own-1-0")
        },
        "its renderer helper, the same folder"
    );
    assert_eq!(
        judged("/w/tree/app/build.noindex/own-1-0/RectoPDF.app", &|pid| pid == 1),
        Verdict::InUse,
        "a test of this tree still running in it"
    );
    assert_eq!(
        judged("/w/tree/app/build.noindex/own-x-0/RectoPDF.app", &none_alive),
        Verdict::Blame,
        "a folder that is not an own copy's name is not assumed to be a test's"
    );
    assert_eq!(
        judged("/w/tree/app/build.noindex/RectoPDF.app/Contents/Helpers/RectoRender.app", &none_alive),
        Verdict::Blame,
        "inside it"
    );
    assert_eq!(
        judged("/w/tree/target/dev/RectoPDF.app", &none_alive),
        Verdict::Blame,
        "anything else of this checkout's"
    );
    assert!(
        matches!(
            judged("/w/tree/.worktrees/other/app/build.noindex/RectoPDF.app", &none_alive),
            Verdict::Leave(_)
        ),
        "another tree's build folder is left alone"
    );
    for elsewhere in ["/Applications/Old RectoPDF.app", "/t/someone-else/RectoPDF.app"] {
        assert!(matches!(judged(elsewhere, &none_alive), Verdict::Leave(_)), "{elsewhere}");
    }

    // This tree's own ended test: settled with the rest, and its folder is what the stage checks is gone.
    assert_eq!(
        judged(&bed(999_999_999, now - 5_000, checkout), &none_alive),
        Verdict::Ended {
            folder: PathBuf::from(format!("/t/{}", bed_folder::name(999_999_999, now - 5_000, checkout)))
        }
    );
    // A test still running, this tree's or another's: not looked at.
    assert_eq!(judged(&bed(42, now - 5_000, checkout), &|pid| pid == 42), Verdict::InUse);
    assert_eq!(judged(&bed(42, now - 5_000, Path::new("/w/other")), &|pid| pid == 42), Verdict::InUse);
    // **The evidence of 2026-11-13:** another worktree's update test, ended 5 s ago, found by this tree's stage. Not this tree's to answer for, nor to change: the old rule blamed it and forgot it.
    let others = bed(17_821, now - 5_000, Path::new("/w/tree/.worktrees/other"));
    assert!(matches!(judged(&others, &none_alive), Verdict::Leave(_)), "another tree's, just ended");
    // Another tree's from long ago, whose tree never looked: forgotten, but not blamed.
    assert!(matches!(
        judged(&bed(17_821, now - 2 * OTHERS_GRACE_MS, Path::new("/w/other")), &none_alive),
        Verdict::Forget(_)
    ));
    // Before names carried a tree, and what isn't a bed: forgotten, not blamed.
    assert!(matches!(
        judged("/t/recto-update-e2e-999999999-1.noindex/install/RectoPDF.app", &none_alive),
        Verdict::Forget(_)
    ));
    assert!(matches!(
        judged("/t/recto-update-e2e-999999999-1/install/RectoPDF.app", &none_alive),
        Verdict::Forget(_)
    ));
    // Two checkouts never share a tag, and a name reads back.
    assert_ne!(
        bed_folder::tree_of(Path::new("/w/tree")),
        bed_folder::tree_of(Path::new("/w/tree/.worktrees/other"))
    );
    let made = bed_folder::parse(&bed_folder::name(7, 8, checkout)).unwrap();
    assert_eq!((made.pid, made.at_ms, made.tree), (7, 8, Some(bed_folder::tree_of(checkout))));
    assert_eq!(bed_folder::parse("someone-elses"), None);
    // Which folders on disk are this tree's tests' copies, for the stage to find the ones a test left.
    let mine = bed_folder::name(77, 5, checkout);
    assert_eq!(maker_of_this_trees_folder("own-12-3", true, checkout), Some(12));
    assert_eq!(maker_of_this_trees_folder("own-12-3", false, checkout), None, "own copies are beside the build only");
    assert_eq!(maker_of_this_trees_folder(&mine, false, checkout), Some(77));
    assert_eq!(maker_of_this_trees_folder(&mine, true, checkout), None, "beds are in temp only");
    assert_eq!(
        maker_of_this_trees_folder(&bed_folder::name(77, 5, Path::new("/w/other")), false, checkout),
        None,
        "another tree's"
    );
    assert_eq!(maker_of_this_trees_folder("recto-update-e2e-77-5", false, checkout), None, "from before folders named their tree");
    assert_eq!(bed_folder::parse_own(&bed_folder::own_name(12, 3)), Some(12));
    for not_own in ["own-12", "own-12-3-4", "own-a-3", "RectoPDF.app"] {
        assert_eq!(bed_folder::parse_own(not_own), None, "{not_own}");
    }
}

/// Failure: a stray's line leaves out when its folder was made, whether its process is alive or whose tree it is, so a failure needs inference from the clock and `ps`.
#[test]
fn a_stray_is_described_by_when_it_was_made_and_whose() {
    let checkout = Path::new("/w/tree");
    let temp = Path::new("/t");
    let path = format!("/t/{}/install/RectoPDF.app", bed_folder::name(17_821, 1_000_000, Path::new("/w/other")));
    let copy = Registered {
        path: PathBuf::from(path),
        identifier: "io.github.kkollsga.RectoPDF.e2e".into(),
    };
    let said = describe(&copy, checkout, temp, 1_007_000, |_| false);
    for part in ["made 7 s ago", "process 17821 (gone)", "another tree's", "not on disk"] {
        assert!(said.contains(part), "{part} in {said}");
    }
    assert!(describe(&copy, checkout, temp, 1_007_000, |_| true).contains("(alive)"));
}
