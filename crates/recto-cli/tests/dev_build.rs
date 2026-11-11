//! `tools/dev-build.sh`'s rules that need no build: how the next development build is numbered, and which process `--first-install` may end. And that the installer's idea of the user's state folder is the app's. SPEC decision 28.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/dev-build.sh")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("recto-dev-build-{}-{name}.noindex", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Cargo.toml's version, which a development build's number builds on.
fn release() -> String {
    let manifest = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
    let package = &manifest[manifest.find("[workspace.package]").unwrap()..];
    let line = package.lines().find(|line| line.trim_start().starts_with("version")).unwrap();
    line.split('"').nth(1).unwrap().to_owned()
}

fn next_version(counter: &Path, installed: &Path) -> String {
    let output = Command::new(tool())
        .arg("--next-version")
        .env("RECTO_DEV_COUNTER", counter)
        .env("RECTO_DEV_INSTALLED", installed)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn installed_as(app: &Path, dev: Option<&str>) {
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    let key = dev.map_or_else(String::new, |dev| format!("<key>RectoDevVersion</key><string>{dev}</string>"));
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>{}</string>{key}</dict></plist>"#,
            release()
        ),
    )
    .unwrap();
}

/// Failure (review, 2026-11-11): two worktrees each count from their own folder and hand the owner two different builds of one number, or a build numbered below what's installed, which the app then takes as nothing new. One count, kept by the repository's own git folder, and never below what's installed.
#[test]
fn the_next_build_is_one_count_for_every_worktree() {
    let dir = temp("number");
    let (counter, app) = (dir.join("counter"), dir.join("RectoPDF.app"));
    let release = release();
    assert_eq!(next_version(&counter, &app), format!("{release}.1"), "nothing counted, nothing installed");
    std::fs::write(&counter, format!("{release} 4\n")).unwrap();
    assert_eq!(next_version(&counter, &app), format!("{release}.5"));
    installed_as(&app, Some(&format!("{release}.7")));
    assert_eq!(
        next_version(&counter, &app),
        format!("{release}.8"),
        "after what's installed, if that's further on"
    );
    installed_as(&app, Some(&format!("{release}.2")));
    assert_eq!(next_version(&counter, &app), format!("{release}.5"), "after the count, if that is");
    std::fs::write(&counter, "0.0.1 40\n").unwrap();
    installed_as(&app, Some("0.0.1.41"));
    assert_eq!(next_version(&counter, &app), format!("{release}.1"), "another release's count starts again");
    installed_as(&app, None);
    assert_eq!(next_version(&counter, &app), format!("{release}.1"), "a release installed counts nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Failure (review, 2026-11-11): `--first-install` ends a RectoPDF it doesn't know, such as a worktree's or another copy the owner keeps, where it may end only the owner's old dev copy in the main checkout's `target/dev`.
#[test]
fn the_first_install_ends_only_the_old_dev_copy() {
    let dir = temp("old-copy");
    let program = |at: &Path| {
        let macos = at.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::copy("/bin/sleep", macos.join("RectoPDF")).unwrap();
        // A system program copied out of its place won't run as it was signed: ad hoc, it's an ordinary program.
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(macos.join("RectoPDF"))
                .output()
                .unwrap()
                .status
                .success()
        );
        macos.join("RectoPDF")
    };
    let old = dir.join("target/dev/RectoPDF.app");
    let other = dir.join("elsewhere/RectoPDF.app");
    let mut old_run = Command::new(program(&old)).arg("30").spawn().unwrap();
    let mut other_run = Command::new(program(&other)).arg("30").spawn().unwrap();
    let output = Command::new(tool()).arg("--old-copy-pids").env("RECTO_DEV_OLD_COPY", &old).output().unwrap();
    let _ = (old_run.kill(), other_run.kill(), old_run.wait(), other_run.wait());
    let pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    assert_eq!(pids, [old_run.id()], "only the old dev copy, not {}", other_run.id());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Failure: the installer reads the owner's switch from a folder that isn't the app's own, so it's never on where it's set, or is on where it isn't: the two are one folder.
#[test]
fn the_installers_state_folder_is_the_apps() {
    assert_eq!(recto_update::real_state_root(), recto_control::state::default_root().ok());
}

/// Runs `--place` on a zip into a folder standing for /Applications, with nothing of the owner's ever matched: its old dev copy is named somewhere that isn't.
fn place(applications: &Path, zip: &Path, extra: &[&str], team: Option<&str>) -> std::process::Output {
    let mut command = Command::new(tool());
    command
        .arg("--place")
        .arg(zip)
        .args(extra)
        .env("RECTO_DEV_APPLICATIONS", applications)
        .env("RECTO_DEV_OLD_COPY", applications.join("no-such-old-copy.app"))
        .env("RECTO_DEV_NO_REOPEN", "1");
    match team {
        Some(team) => command.env("RECTO_DEV_TEAM_OF", team),
        None => command.env_remove("RECTO_DEV_TEAM_OF"),
    };
    command.output().unwrap()
}

/// Failure (review round 2): the first dev build goes over a release signed by the team without being told to, or over a dev build already there, or a release it replaces is lost. Only `--replace-release` puts one over a team-signed release, and that release is kept aside where the updater keeps the way back, not as an app.
#[test]
fn the_first_install_replaces_a_release_only_when_told_and_keeps_it() {
    let dir = temp("place");
    let build = dir.join("built");
    installed_as(&build.join("RectoPDF.app"), Some(&format!("{}.3", release())));
    let zip = dir.join(format!("RectoPDF-{}.3.zip", release()));
    assert!(
        Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(build.join("RectoPDF.app"))
            .arg(&zip)
            .status()
            .unwrap()
            .success()
    );
    let applications = dir.join("Applications");
    std::fs::create_dir_all(&applications).unwrap();
    let target = applications.join("RectoPDF.app");
    assert!(
        !place(&applications, &zip, &["--replace-release"], None).status.success(),
        "nothing there to replace"
    );
    assert!(place(&applications, &zip, &[], None).status.success(), "the first, where there's none");
    assert!(target.join("Contents/Info.plist").is_file());
    let again = place(&applications, &zip, &[], None);
    assert!(!again.status.success(), "a dev build's already there");
    assert!(String::from_utf8_lossy(&again.stderr).contains("recto update install"));
    // A release signed by the team, standing in: never replaced unasked, and kept when it is.
    std::fs::write(target.join("release"), "the release").unwrap();
    let unasked = place(&applications, &zip, &[], Some("6GND8Z2C7Z"));
    assert!(!unasked.status.success());
    assert!(String::from_utf8_lossy(&unasked.stderr).contains("--replace-release"));
    assert!(target.join("release").exists(), "untouched");
    assert!(place(&applications, &zip, &["--replace-release"], Some("6GND8Z2C7Z")).status.success());
    let kept = applications.join(".RectoPDF-update/previous/RectoPDF.kept");
    assert_eq!(std::fs::read_to_string(kept.join("release")).unwrap(), "the release", "kept aside");
    assert!(!target.join("release").exists(), "and the build in its place");
    let _ = std::fs::remove_dir_all(&dir);
}
