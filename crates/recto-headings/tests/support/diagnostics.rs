//! Bounded pre-kill observations of the exact helper owned by this test.
use recto_platform::test_command;
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const CAP: usize = 64 * 1024;

pub fn capture(pid: u32, elapsed: Duration, job: &serde_json::Value) -> String {
    match collect(pid, elapsed, job) {
        Ok(path) => format!(
            "evidence {}; rerun: HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo test --locked --manifest-path crates/recto-headings/Cargo.toml --test helper it_goes_at_once_when_its_stdin_closes_whatever_it_is_doing -- --exact --nocapture",
            path.display()
        ),
        Err(error) => format!("pre-kill evidence failed: {error}"),
    }
}

fn collect(pid: u32, elapsed: Duration, job: &serde_json::Value) -> std::io::Result<std::path::PathBuf> {
    let path = loop {
        let path = std::env::temp_dir().join(format!(
            "recto-headings-failure-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    std::fs::write(
        path.join("context.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"pid":pid,"parent":std::process::id(),"at":format!("{:?}",SystemTime::now()),"elapsed_ms":elapsed.as_millis(),"executable":env!("CARGO_BIN_EXE_recto-headings"),"environment":{"MallocLargeCache":"0"},"priority":"Inherited","job":job}),
        )?,
    )?;
    save(&path, "sample", test_command::sample_process(pid, 1, Duration::from_secs(2), CAP))?;
    let mut lsof = Command::new("/usr/sbin/lsof");
    lsof.args(["-nP", "-p", &format!("{pid},{}", std::process::id())]);
    save(&path, "lsof", test_command::output_with_cap(&mut lsof, Duration::from_secs(2), CAP))?;
    let mut hash = Command::new("/usr/bin/shasum");
    hash.args(["-a", "256", env!("CARGO_BIN_EXE_recto-headings")]);
    save(
        &path,
        "executable-sha256",
        test_command::output_with_cap(&mut hash, Duration::from_secs(2), CAP),
    )?;
    Ok(path)
}

fn save(root: &Path, name: &str, result: std::io::Result<(bool, std::process::Output)>) -> std::io::Result<()> {
    match result {
        Ok((timed_out, output)) => {
            std::fs::write(root.join(format!("{name}.stdout")), output.stdout)?;
            std::fs::write(root.join(format!("{name}.stderr")), output.stderr)?;
            std::fs::write(root.join(format!("{name}.status")), format!("timed_out={timed_out} status={:?}", output.status))
        }
        Err(error) => std::fs::write(root.join(format!("{name}.error")), error.to_string()),
    }
}
