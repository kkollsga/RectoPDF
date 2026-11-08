//! The helper's own lockfile against the main workspace's.

/// The `source` line of the one `lopdf` package a lockfile has.
fn lopdf_source(lock: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut in_lopdf = false;
    for line in lock.lines() {
        if line == "[[package]]" {
            in_lopdf = false;
        } else if line == "name = \"lopdf\"" {
            in_lopdf = true;
        } else if in_lopdf && line.starts_with("source = ") {
            sources.push(line.to_string());
        }
    }
    sources
}

/// Failure: the helper's lockfile, kept by hand beside the main one, drifts to another lopdf, so distillPDF reads files with a parser the app and the session don't use, and nothing says so. Both must pin the same single commit.
#[test]
fn the_helper_and_the_app_pin_the_same_lopdf() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let helper = std::fs::read_to_string(here.join("Cargo.lock")).unwrap();
    let main = std::fs::read_to_string(here.join("../../Cargo.lock")).unwrap();
    let (helper, main) = (lopdf_source(&helper), lopdf_source(&main));
    assert_eq!(helper.len(), 1, "one lopdf in the helper: {helper:?}");
    assert_eq!(helper, main, "the helper's lopdf isn't the app's");
}
