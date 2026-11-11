//! The third-party notices that ship in the app: `THIRD-PARTY-NOTICES.txt`, written by `tools/notices.py`, and its copy in the bundle.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn notices() -> String {
    std::fs::read_to_string(repo().join("THIRD-PARTY-NOTICES.txt")).expect("THIRD-PARTY-NOTICES.txt is missing: run tools/notices.py")
}

/// `name version` of every package in a lockfile that comes from outside the repo.
fn locked(lock: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let (mut name, mut version) = (None, None);
    for line in lock.lines() {
        let quoted = |line: &str| line.split('"').nth(1).map(str::to_string);
        if line == "[[package]]" {
            (name, version) = (None, None);
        } else if line.starts_with("name = \"") && name.is_none() {
            name = quoted(line);
        } else if line.starts_with("version = \"") && version.is_none() {
            version = quoted(line);
        } else if line.starts_with("source = ")
            && let (Some(name), Some(version)) = (&name, &version)
        {
            found.insert(format!("{name} {version}"));
        }
    }
    found
}

/// The inventory's `ships` and `not-shipped` lines, as `name version`, with each shipped package's licence expression.
fn inventory(text: &str) -> (BTreeSet<String>, Vec<(String, String)>) {
    let (mut all, mut shipped) = (BTreeSet::new(), Vec::new());
    for line in text.lines() {
        let words: Vec<&str> = line.splitn(4, ' ').collect();
        match words.as_slice() {
            ["ships", name, version, licence] => {
                all.insert(format!("{name} {version}"));
                shipped.push((format!("{name} {version}"), licence.to_string()));
            }
            ["not-shipped", name, version, ..] => {
                all.insert(format!("{name} {version}"));
            }
            _ => {}
        }
    }
    (all, shipped)
}

/// Failure: a dependency is added, bumped or dropped and the notices aren't written again, so the app ships a component whose
/// licence it doesn't reproduce, or lists one it no longer has. Every outside package in both lockfiles must be in the inventory,
/// shipped or not, and nothing else.
#[test]
fn the_notices_account_for_every_locked_package() {
    let mut locks = locked(&std::fs::read_to_string(repo().join("Cargo.lock")).unwrap());
    locks.extend(locked(&std::fs::read_to_string(repo().join("crates/recto-headings/Cargo.lock")).unwrap()));
    let (listed, shipped) = inventory(&notices());
    assert!(shipped.len() > 50, "the inventory has only {} shipped packages", shipped.len());
    let missing: Vec<_> = locks.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(&locks).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "THIRD-PARTY-NOTICES.txt is out of date: run tools/notices.py. In a lockfile and not listed: {missing:?}. Listed and in no lockfile: {extra:?}"
    );
}

/// Failure: PDFium is moved to another release and its notices still describe the old one, whose third-party code may differ.
#[test]
fn the_notices_describe_the_pinned_pdfium() {
    let script = std::fs::read_to_string(repo().join("tools/pdfium.sh")).unwrap();
    let pin = script
        .split("releases/download/")
        .nth(1)
        .and_then(|rest| rest.split("/pdfium-").next())
        .expect("tools/pdfium.sh names its release");
    let text = notices();
    let line = text.lines().find(|line| line.starts_with("pdfium ")).expect("the inventory lists PDFium");
    assert!(line.contains(pin), "PDFium's notices are for another release than {pin}: {line}");
    for part in [
        "--- pdfium.txt ---",
        "--- freetype.txt ---",
        "--- libjpeg_turbo.md ---",
        "--- libopenjpeg.txt ---",
        "--- icu.txt ---",
    ] {
        assert!(text.contains(part), "PDFium's third-party licences lack {part}");
    }
}

/// Failure: a package that needs a copyleft licence such as the GPL gets into the app, which is MIT and ships as one binary.
/// Each AND term of each shipped package's licence must offer a licence an MIT app can ship under.
#[test]
fn every_shipped_licence_is_one_an_mit_app_can_ship() {
    const ALLOWED: &[&str] = &[
        "MIT",
        "Apache-2.0",
        "Apache-2.0 WITH LLVM-exception",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "ISC",
        "Zlib",
        "Unlicense",
        "0BSD",
        "Unicode-3.0",
        "Unicode-DFS-2016",
        "BSL-1.0",
        "MPL-2.0",
        "CC0-1.0",
    ];
    let (_, shipped) = inventory(&notices());
    for (package, licence) in shipped {
        let spelled = licence.replace('/', " OR ").replace(['(', ')'], " ");
        for term in spelled.split(" AND ") {
            let ok = term.split(" OR ").any(|choice| ALLOWED.contains(&choice.trim()));
            assert!(ok, "{package} needs `{term}`, which an MIT app can't ship as it is");
        }
    }
}

/// The built bundle's `Contents`, unless this run skips the app. Required where a run says the app must be there (`--full`).
fn built_bundle() -> Option<PathBuf> {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        return None;
    }
    let bundle = repo().join("app/build.noindex/RectoPDF.app/Contents");
    if !bundle.join("MacOS/RectoPDF").is_file() {
        assert!(std::env::var_os("RECTO_REQUIRE_APP").is_none(), "the app isn't built, and this run needs it");
        eprintln!("skipped: the app isn't built");
        return None;
    }
    Some(bundle)
}

/// Failure: the bundle ships without its notices, or with an old copy of them. Skipped when the app isn't built.
#[test]
fn the_built_app_carries_the_notices() {
    let Some(bundle) = built_bundle() else { return };
    let copy = std::fs::read_to_string(bundle.join("Resources/Licenses.txt")).expect("the app has no Contents/Resources/Licenses.txt");
    assert!(copy == notices(), "the app's Licenses.txt isn't THIRD-PARTY-NOTICES.txt: build the app again");
}

/// Failure: a file in the bundle carries an extended attribute, as one copied from a checkout can (macOS adds `com.apple.provenance`
/// to files some processes write). A zip holds it as a `._` entry, the updater's reader writes that out as a file the signature
/// doesn't cover, and every update is refused as unsigned: the notices' first copy did this to ten update tests.
#[test]
fn nothing_in_the_built_app_has_extended_attributes() {
    let Some(bundle) = built_bundle() else { return };
    let listed = std::process::Command::new("/usr/bin/xattr")
        .arg("-r")
        .arg(&bundle)
        .output()
        .expect("xattr runs");
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.trim().is_empty(), "files in the bundle with extended attributes:\n{text}");
}
