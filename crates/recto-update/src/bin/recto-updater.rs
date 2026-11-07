//! The installer, on its own in the bundle: `recto-updater install <plan>`, started by the app after a good hand-off. It outlives the app. `recto-updater source` says what this build would use, which is how a release is checked to read no override.

use recto_update::install::{self, Ended};
use recto_update::plan::Plan;
use recto_update::source;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    std::process::exit(match words.as_slice() {
        ["install" | "back", file] => {
            let Some(plan) = Plan::read(Path::new(file)) else {
                eprintln!("recto-updater: that's not a plan");
                std::process::exit(2);
            };
            let ended = if words[0] == "install" { install::run(&plan) } else { install::run_back(&plan) };
            i32::from(ended != Ended::Updated)
        }
        // A stand-in for the app, in a test's fake bundle, so the installer's watching a real process at a real path. Nothing in a release build.
        ["test-app", version, behaviour, work, session] if !recto_update::RELEASE_BUILD => test_app(version, behaviour, Path::new(work), Path::new(session)),
        ["source"] => {
            let source = source::from_env(false);
            println!("{{\"mode\":\"{}\",\"repo\":\"{}\",\"release_build\":{}}}", source.mode_name(), source.repo, recto_update::RELEASE_BUILD);
            0
        }
        _ => {
            eprintln!("usage: recto-updater install <plan> | back <plan> | source");
            2
        }
    });
}

/// How a test's stand-in app behaves: it reaches its first frame, or it does what a broken new version might, damaging the record and then ending or hanging.
fn test_app(version: &str, behaviour: &str, work: &Path, session: &Path) -> i32 {
    // The installer opens whatever's at the app's path with the same words, so the old version coming back has to know it isn't the one that's broken.
    let own = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.parent()?.to_path_buf())).and_then(|app| recto_update::stage::app_version(&app)).unwrap_or_default();
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
