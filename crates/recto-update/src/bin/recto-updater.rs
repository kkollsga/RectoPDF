//! The installer, on its own in the bundle: `recto-updater install <plan>`, started by the app after a good hand-off. It outlives the app. `recto-updater source` says what this build would use, which is how a release is checked to read no override. `recto-updater check-zip <zip> <folder>` unpacks a zip with the reader an update uses, so a release can be checked to be one that can be installed.

use recto_update::install::{self, Ended};
use recto_update::plan::Plan;
use recto_update::source;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    // In a test's broken release, this program stands where the app should be: it does what a broken new version might, to the real record, and ends without coming up. Nothing in a release build.
    if !recto_update::RELEASE_BUILD
        && std::env::current_exe().is_ok_and(|exe| exe.file_name().is_some_and(|name| name == "RectoPDF"))
        && words.first() != Some(&"test-app")
    {
        if let Some(state) = std::env::var_os("RECTO_STATE_DIR") {
            let _ = std::fs::write(Path::new(&state).join("session/record.a.json"), "damaged by the new version");
            let _ = std::fs::write(Path::new(&state).join("session/record.b.json"), "damaged by the new version");
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::process::exit(1);
    }
    std::process::exit(match words.as_slice() {
        ["install" | "back", file] => {
            let Some(plan) = Plan::read(Path::new(file)) else {
                eprintln!("recto-updater: that's not a plan");
                std::process::exit(2);
            };
            let ended = if words[0] == "install" {
                install::run(&plan)
            } else {
                install::run_back(&plan)
            };
            i32::from(ended != Ended::Updated)
        }
        // A stand-in for the app, in a test's fake bundle, so the installer's watching a real process at a real path. Nothing in a release build.
        ["test-app", version, behaviour, work, session] if !recto_update::RELEASE_BUILD => test_app(version, behaviour, Path::new(work), Path::new(session)),
        ["source"] => {
            let source = source::from_env(false);
            let takes_test_hooks = !recto_update::RELEASE_BUILD;
            println!(
                "{{\"mode\":\"{}\",\"repo\":\"{}\",\"release_build\":{},\"trust\":\"{}\",\"takes_test_hooks\":{takes_test_hooks}}}",
                source.mode_name(),
                source.repo,
                recto_update::RELEASE_BUILD,
                source.trust_name()
            );
            0
        }
        ["check-zip", zip, into] => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            match recto_update::zip::unpack(Path::new(zip), recto_update::APP_BUNDLE, Path::new(into), deadline) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("recto-updater: that zip wouldn't install: {error:?}");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: recto-updater install <plan> | back <plan> | source | check-zip <zip> <folder>");
            2
        }
    });
}

/// How a test's stand-in app behaves: it reaches its first frame, or it does what a broken new version might, damaging the record and then ending or hanging.
fn test_app(version: &str, behaviour: &str, work: &Path, session: &Path) -> i32 {
    // The installer opens whatever's at the app's path with the same words, so the old version coming back has to know it isn't the one that's broken.
    let own = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.parent()?.to_path_buf()))
        .and_then(|app| recto_update::stage::app_version(&app))
        .unwrap_or_default();
    let _ = std::fs::write(work.join(format!("ran-{own}")), "");
    let behaviour = if own == version { behaviour } else { "ok" };
    match behaviour {
        "ok" => {
            let _ = recto_platform::local::write_private_file(&work.join("launch-ok"), format!("{own} {} 0", std::process::id()).as_bytes());
            std::thread::sleep(std::time::Duration::from_secs(4));
            0
        }
        "exits" => {
            let _ = std::fs::write(session.join("record.a.json"), "damaged by the new version");
            1
        }
        _ => {
            let _ = std::fs::write(session.join("record.a.json"), "damaged by the new version");
            std::thread::sleep(std::time::Duration::from_secs(120));
            1
        }
    }
}
