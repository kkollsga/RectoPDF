//! The UI thread never calls the Keychain (rule 3; the security review's ask on the Find text's design, 2026-11-15), shown on a real main thread: a test's own runs on a thread the harness made, so the probe is a program of its own.

/// Failure: the indexer takes the Find key on the process's main thread, the app's UI thread, where the Keychain is one call that may block; or it never takes it at all, from any thread. On the main thread it answers no key and doesn't reach the loader; from another thread it reaches it and has the key.
#[test]
fn the_main_thread_never_reaches_the_keys_loader() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_recto-main-thread-probe")).output().expect("the probe runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "main=true main_got=false main_reached=false other_got=true other_reached=true"
    );
}
