//! What the indexer holds of the Keychain: nothing.

/// Failure: the indexer carries the code that reaches the Keychain: the Find key's, or Security.framework linked and called by anything (the coordinator's decision, 2026-11-15: only the app's own process does). Looked at as it was just built for this test.
#[test]
fn the_indexer_carries_no_keychain_code() {
    let found = recto_platform::find_key::audit::keychain_code_in(std::path::Path::new(env!("CARGO_BIN_EXE_recto-indexer")));
    assert!(found.is_empty(), "the indexer reaches the Keychain: {found:?}");
}
