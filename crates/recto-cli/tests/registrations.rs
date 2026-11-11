//! **One install.** Owner, 2026-11-11: "Im seeing two rectopdf installs in macos. Can you check? We should only ever have one." Every copy of the app the system has registered is in Open With, and the system can open a PDF with any of them. The one install is `/Applications/RectoPDF.app`. The app's and the update's tests start copies, which the system registers, and forget them when they're done; this checks they did.
//!
//! It runs on its own, after those tests (`tools/check.sh --full`, its own stage), as it's about what they left. Skipped where the app's tests are (`RECTO_SKIP_APP`).

use recto_platform::registry::{self, Registered};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A temp folder of a test's that's still in use: named with its test's process, which is still running. Another worktree's tests may be under way.
fn in_use(path: &Path, temp: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(temp) else { return false };
    let Some(first) = rest.components().next() else { return false };
    let name = first.as_os_str().to_string_lossy();
    name.split('-')
        .find_map(|part| part.parse::<u32>().ok())
        .is_some_and(recto_platform::local::process_exists)
}

/// Whether this tree's own tests would have made it: anything inside this checkout, such as its build folders, or a test's `.noindex` folder in temp whose test has ended. What older trees leave is forgotten too, and said, but it isn't this tree's to answer for.
fn this_trees(copy: &Registered, checkout: &Path, temp: &Path) -> bool {
    copy.path.starts_with(checkout)
        || (copy.path.starts_with(temp)
            && copy.path.components().any(|part| part.as_os_str().to_string_lossy().ends_with(".noindex"))
            && !in_use(&copy.path, temp))
}

/// Failure: a copy of the app that isn't the one install is left registered after the tests, so a second RectoPDF shows in Open With: a build folder (`app/build.noindex`, or a worktree's), a test's install in temp (`recto-update-e2e-*`), or a kept copy's renderer helper, which is how about 40 were found on 2026-11-11. Each found is forgotten here, so they don't pile up whichever tree left them.
#[test]
fn no_copy_but_the_one_install_is_left_registered() {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        eprintln!("skipped: it's about what the app's tests left, and they're skipped");
        return;
    }
    let checkout = checkout();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    let old_dev_copy = main_checkout().map(|main| main.join("target/dev/RectoPDF.app"));
    let mut allowed = vec![PathBuf::from("/Applications/RectoPDF.app")];
    allowed.extend(old_dev_copy);
    let allowed: Vec<&Path> = allowed.iter().map(PathBuf::as_path).collect();
    let strays = registry::strays(&registry::registered(), &allowed);
    let mut this_tree = Vec::new();
    for copy in &strays {
        if in_use(&copy.path, &temp) {
            continue;
        }
        eprintln!("registered, and forgotten now: {} ({})", copy.path.display(), copy.identifier);
        if this_trees(copy, &checkout, &temp) {
            this_tree.push(copy.path.clone());
        }
        registry::forget(&copy.path);
    }
    assert!(
        this_tree.is_empty(),
        "this tree's tests left these registered, each a second RectoPDF in Open With: {this_tree:#?}"
    );
}

/// Failure: what's this tree's to answer for is told wrongly: another worktree's running test is blamed, or this checkout's build folder isn't.
#[test]
fn what_counts_as_this_trees() {
    let checkout = Path::new("/w/tree");
    let temp = Path::new("/t");
    let copy = |path: &str| Registered {
        path: PathBuf::from(path),
        identifier: "io.github.kkollsga.RectoPDF".into(),
    };
    assert!(this_trees(&copy("/w/tree/app/build.noindex/RectoPDF.app"), checkout, temp));
    assert!(this_trees(
        &copy("/t/recto-update-e2e-999999999-1.noindex/install/RectoPDF.app"),
        checkout,
        temp
    ));
    let running = format!("/t/recto-update-e2e-{}-1.noindex/install/RectoPDF.app", std::process::id());
    assert!(!this_trees(&copy(&running), checkout, temp), "a test still running");
    assert!(
        !this_trees(&copy("/t/recto-update-e2e-999999999-1/install/RectoPDF.app"), checkout, temp),
        "an older tree's"
    );
    assert!(!this_trees(&copy("/w/other/app/build/RectoPDF.app"), checkout, temp), "another worktree's");
}
