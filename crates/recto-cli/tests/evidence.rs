//! **A failure carries its evidence** (PLAN R0.7a): a test that started the app and fails leaves one folder, made by the harness (`common::evidence`), with everything a rerun used to be needed for. Proved here by failing a throwaway app test on purpose, in a child process, and reading what it left.
//!
//! The throwaway test is ignored, and does nothing unless [`ON_PURPOSE`] is set, which only the test here sets for its child: a normal run, `--run-ignored` included, can't see it fail.

mod common;

use common::State;
use common::app::{Run, app_path, numbered};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Set by the test below for its child alone.
const ON_PURPOSE: &str = "RECTO_EVIDENCE_FAIL_ON_PURPOSE";

/// Failure: a failing app test leaves nothing to go on but its message, as on CI on 2026-11-12, when each failure cost a rerun to learn anything: no folder, more than one, or one missing an item (every launch's messages, `qa state` as each launch was last read, the threads by name, a sample, the slow seams and seed, the system pressure, the machine, the settings and the runner's command line, Launch Services, and the command that runs it again); or one that holds a path from the owner's own folders.
#[test]
fn a_failing_app_test_leaves_one_folder_with_its_evidence() {
    if app_path().is_none() {
        return;
    }
    let root = common::free_folder();
    std::fs::create_dir_all(&root).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fails_on_purpose", "--include-ignored", "--nocapture"])
        .env(ON_PURPOSE, "1")
        .env(common::evidence::ROOT_ENV, &root)
        .env_remove("NEXTEST_TEST_NAME")
        .env_remove("NEXTEST_BINARY_ID")
        .output()
        .unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let result = std::panic::catch_unwind(|| check_the_folder(&root, &output.status, &said));
    let _ = std::fs::remove_dir_all(&root);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn check_the_folder(root: &Path, status: &std::process::ExitStatus, said: &str) {
    assert!(!status.success(), "the throwaway test passed: {said}");
    assert!(said.contains("failing on purpose"), "{said}");
    let folders: Vec<PathBuf> = std::fs::read_dir(root).unwrap().flatten().map(|entry| entry.path()).collect();
    assert_eq!(folders.len(), 1, "one folder for the one failed test: {folders:?}\n{said}");
    let folder = &folders[0];
    let name = folder.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("recto-cli--evidence--") && name.ends_with("fails_on_purpose"), "{name}");
    let read = |file: &str| std::fs::read_to_string(folder.join(file)).unwrap_or_else(|error| panic!("{file}: {error}; there: {:?}", listing(folder)));
    // Both launches' messages: the first's as the second began, and the second's.
    let messages: Vec<String> = listing(folder).into_iter().filter(|file| file.starts_with("messages/")).collect();
    assert!(messages.iter().any(|file| file.ends_with("/before-launch-2.txt")), "{messages:?}");
    assert!(messages.iter().any(|file| file.ends_with("/server-messages.txt")), "{messages:?}");
    // The before and after: each launch's last `qa state`.
    for launch in [1, 2] {
        let state: serde_json::Value = serde_json::from_str(&read(&format!("launch-{launch}-qa-state.json"))).unwrap();
        assert!(state["session"].is_object(), "launch {launch}: {state}");
    }
    assert!(read("launch-2-threads.txt").contains("recto-session"), "{}", read("launch-2-threads.txt"));
    let sample = read("launch-2-sample.txt");
    assert!(
        sample.contains("Call graph") || sample.starts_with("sample failed"),
        "{}",
        &sample[..sample.len().min(400)]
    );
    serde_json::from_str::<serde_json::Value>(&read("launch-2-qa-state-at-failure.json")).unwrap();
    let slow = read("slow.json");
    assert!(slow.contains("\"worker\"") && slow.contains("\"jitter\": 42"), "{slow}");
    let launches = read("launches.json");
    assert!(launches.contains("system_pressure"), "{launches}");
    let machine = read("machine.txt");
    for part in ["ProductVersion", "load", "hw.ncpu", "memory pressure"] {
        assert!(machine.contains(part), "{part}: {machine}");
    }
    let settings = read("settings.txt");
    assert!(settings.contains(&format!("RECTO_EVIDENCE={}", root.display())), "{settings}");
    assert!(settings.contains("the commands this ran under"), "{settings}");
    assert!(read("launch-services.txt").contains("registered elsewhere, not named"));
    let again = read("rerun.txt");
    for part in [
        "tools/check.sh --like-ci",
        "binary_id(=recto-cli::evidence) & test(=fails_on_purpose)",
        "worker=",
        "jitter=42",
        "--profile",
    ] {
        assert!(again.contains(part), "{part}: {again}");
    }
    assert!(said.contains(again.trim()), "the rerun line is printed with the failure: {said}");
    assert!(read("summary.txt").contains("failing on purpose"));
    // Nothing from the owner's own folders: the documents are the test's, made in temp, and no path under the home folder is named but this checkout's, temp's and the toolchain's (on CI the checkout is in the home folder).
    let home = std::env::var("HOME").unwrap_or_default();
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let allowed = [
        checkout.display().to_string(),
        std::env::temp_dir().canonicalize().unwrap().display().to_string(),
        format!("{home}/.cargo/"),
        format!("{home}/.rustup/"),
    ];
    for file in listing(folder) {
        let text = String::from_utf8_lossy(&std::fs::read(folder.join(&file)).unwrap()).to_string();
        for (at, _) in text.match_indices(&format!("{home}/")) {
            let path = &text[at..];
            assert!(
                home.is_empty() || allowed.iter().any(|ok| path.starts_with(ok.as_str())),
                "{file} names {}",
                path.lines().next().unwrap_or("")
            );
        }
    }
    let size: u64 = listing(folder).iter().map(|file| std::fs::metadata(folder.join(file)).unwrap().len()).sum();
    assert!(size <= common::evidence::FOLDER_CAP, "{size} bytes");
}

/// Every file under a folder, relative to it.
fn listing(folder: &Path) -> Vec<String> {
    fn walk(base: &Path, at: &Path, into: &mut Vec<String>) {
        for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                walk(base, &entry.path(), into);
            } else {
                into.push(entry.path().strip_prefix(base).unwrap().to_string_lossy().to_string());
            }
        }
    }
    let mut files = Vec::new();
    walk(folder, folder, &mut files);
    files.sort();
    files
}

/// The throwaway app test: two launches on one state folder, a document open and the slow mode on with a seed, then a failure while the second is still running. Only ever run by the test above.
#[test]
#[ignore = "fails on purpose; only a_failing_app_test_leaves_one_folder_with_its_evidence runs it, in a child"]
fn fails_on_purpose() {
    if std::env::var_os(ON_PURPOSE).is_none() {
        return;
    }
    let state = State::new();
    let slow = std::ffi::OsStr::new("worker=1,jitter=42");
    let document = numbered(2);
    let first = Run::on_with_env(&state, &[("RECTO_QA_SLOW", slow)]).unwrap();
    first.ok(&["open", document.0.to_str().unwrap()]);
    first.settled();
    first.report();
    first.quit();
    let second = Run::on_with_env(&state, &[("RECTO_QA_SLOW", slow)]).unwrap();
    second.settled();
    second.report();
    panic!("failing on purpose, to prove the evidence folder");
}
