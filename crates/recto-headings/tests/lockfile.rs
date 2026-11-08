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

/// Failure: the helper's lockfile, kept by hand beside the main one, drifts to another lopdf, so distillPDF reads files with a parser the app and the session don't use, and nothing says so. Both must pin the same single commit, whatever spelling of the fork's address reaches it.
#[test]
fn the_helper_and_the_app_pin_the_same_lopdf() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let helper = std::fs::read_to_string(here.join("Cargo.lock")).unwrap();
    let main = std::fs::read_to_string(here.join("../../Cargo.lock")).unwrap();
    let (helper, main) = (lopdf_source(&helper), lopdf_source(&main));
    assert_eq!(helper.len(), 1, "one lopdf in the helper: {helper:?}");
    // The commit, after `#`: the helper reaches it by another spelling of the same repository, since Cargo won't patch a source to itself.
    let commit = |sources: &[String]| {
        sources
            .iter()
            .map(|source| source.rsplit('#').next().unwrap_or_default().trim_end_matches('"').to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(commit(&helper), commit(&main), "the helper's lopdf isn't the app's commit");
    assert!(helper[0].contains("github.com/kkollsga/lopdf.git"), "and it's the owner's fork: {helper:?}");
}
