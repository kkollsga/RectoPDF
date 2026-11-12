//! **Every thread the code starts has a name.** The app's thread counts go by name (`common::threads`): the system's libdispatch pool is unnamed, and its size follows the system's load, not the app's (CI, 2026-11-12: "12 then 15 threads" with a thousand tabs, from the pool). A thread of ours with no name would hide among the pool's and never be counted.

use std::path::{Path, PathBuf};

/// The crates whose code runs in the app or `recto`. Not the fuzz targets, nor the headings helper, a workspace of its own whose process exits when it's done.
fn sources() -> Vec<PathBuf> {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "recto-fuzz" || name == "recto-headings" {
            continue;
        }
        collect(&entry.path().join("src"), &mut files);
    }
    files
}

fn collect(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// `#[cfg(test)]`, or a `cfg` that needs `test` among others, as `#[cfg(all(test, target_os = "macos"))]`.
fn is_test_only(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("#[cfg(") && (line == "#[cfg(test)]" || (line.starts_with("#[cfg(all(") && line.contains("test,")))
}

/// The lines of a file outside its tests: an inline `#[cfg(test)] mod … {` up to its closing brace at the start of a line, and the files such a `mod …;` names, are left out.
fn production_lines(text: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((at, line)) = lines.next() {
        if is_test_only(line)
            && let Some((_, next)) = lines.peek()
            && ["mod ", "pub mod ", "pub(crate) mod "].iter().any(|start| next.trim_start().starts_with(start))
            && next.trim_end().ends_with('{')
        {
            let indent = next.len() - next.trim_start().len();
            let close = format!("{}}}", " ".repeat(indent));
            for (_, inner) in lines.by_ref() {
                if inner.trim_end() == close {
                    break;
                }
            }
            continue;
        }
        kept.push((at + 1, line));
    }
    kept
}

/// The modules a file declares as tests only, `#[cfg(test)] mod name;`, whose files hold no production code.
fn test_modules(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .windows(2)
        .filter(|pair| is_test_only(pair[0]))
        .filter_map(|pair| {
            let declared = pair[1].trim().strip_prefix("mod ")?.strip_suffix(';')?;
            Some(declared.trim().to_string())
        })
        .collect()
}

/// Failure: a thread the app's or `recto`'s code starts has no name (`std::thread::spawn`, a scope's own `spawn`, or a `Builder` without `.name(…)`), so a count of the app's threads by name misses it, and a thread per tab of it would pass every thread test. Each start outside tests is through a named `Builder`.
#[test]
fn every_thread_the_code_starts_is_named() {
    let files = sources();
    assert!(files.len() > 100, "the sources weren't found: {} files", files.len());
    let skipped: Vec<PathBuf> = files
        .iter()
        .flat_map(|file| {
            let text = std::fs::read_to_string(file).unwrap();
            let folder = if file.file_name().is_some_and(|name| name == "lib.rs" || name == "main.rs" || name == "mod.rs") {
                file.parent().unwrap().to_path_buf()
            } else {
                file.with_extension("")
            };
            test_modules(&text)
                .into_iter()
                .flat_map(move |name| [folder.join(format!("{name}.rs")), folder.join(&name).join("mod.rs")])
        })
        .collect();
    let mut unnamed = Vec::new();
    for file in &files {
        if file.file_name().is_some_and(|name| name == "tests.rs") || skipped.contains(file) {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap();
        let lines = production_lines(&text);
        for (index, (at, line)) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            // A `Builder` names its thread within the next few lines of its chain, or not at all.
            let named_builder = || lines[index..lines.len().min(index + 4)].iter().any(|(_, next)| next.contains(".name("));
            let unnamed_start =
                line.contains("thread::spawn(") || line.contains("scope.spawn(") || (line.contains("thread::Builder::new()") && !named_builder());
            if unnamed_start {
                unnamed.push(format!("{}:{at}: {}", file.display(), line.trim()));
            }
        }
    }
    assert!(
        unnamed.is_empty(),
        "threads started with no name, which a count by name can't see:\n{}",
        unnamed.join("\n")
    );
}

/// Failure: the guard above reads test code as production (and fails on every test's helper thread) or production as test code (and passes on an unnamed thread of the app's).
#[test]
fn the_guard_tells_tests_from_production() {
    let text = "fn real() {\n    std::thread::spawn(|| {});\n}\n\n#[cfg(test)]\nmod tests {\n    fn helper() {\n        std::thread::spawn(|| {});\n    }\n}\n\nfn after() {}\n";
    let kept: Vec<usize> = production_lines(text)
        .into_iter()
        .filter(|(_, line)| line.contains("thread::spawn("))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(kept, vec![2], "only the production one");
    assert!(
        production_lines(text).iter().any(|(_, line)| line.contains("fn after")),
        "what follows the tests is read again"
    );
    assert_eq!(
        test_modules("#[cfg(test)]\nmod event_agreement;\npub mod event;\n"),
        vec!["event_agreement".to_string()]
    );
}
