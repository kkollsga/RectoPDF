//! The OS's direct cache-exclusion property, without tmutil's scan of every mounted disk.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The documented CFBoolean for caches, rather than a second interpretation of our xattr bytes.
const PROBE: &str = r#"
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "--hang") == 0) {
        fprintf(stderr, "backup exclusion probe entered forced wait\n");
        fflush(stderr);
        FILE *ready = fopen(argv[2], "w");
        if (!ready) return 5;
        fprintf(ready, "%d\n", getpid());
        fclose(ready);
        for (;;) pause();
    }
    if (argc != 2) return 2;
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)argv[1], strlen(argv[1]), true);
    if (!url) return 3;
    CFTypeRef value = NULL;
    CFErrorRef error = NULL;
    Boolean ok = CFURLCopyResourcePropertyForKey(url, kCFURLIsExcludedFromBackupKey, &value, &error);
    if (!ok || !value || CFGetTypeID(value) != CFBooleanGetTypeID()) {
        if (error) CFShow(error);
        if (value) CFRelease(value);
        if (error) CFRelease(error);
        CFRelease(url);
        return 4;
    }
    printf("excluded=%d\n", CFBooleanGetValue(value));
    CFRelease(value);
    CFRelease(url);
    return 0;
}
"#;

struct Probe(PathBuf);

impl Drop for Probe {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("backup exclusion evidence: {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

/// A compiler/query child belongs to its own group, so a timed-out compiler cannot leave its linker running.
struct Owned {
    child: Child,
    reaped: bool,
}

impl Owned {
    fn stop(&mut self) {
        let _ = recto_platform::local::signal_process_group(self.child.id(), true);
        let _ = self.child.kill();
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.reaped {
            self.stop();
            let _ = self.child.wait();
        }
    }
}

#[derive(Debug)]
struct Ran {
    status: ExitStatus,
    timed_out: bool,
    elapsed: Duration,
    stdout: String,
    stderr: String,
}

impl Probe {
    fn new() -> Self {
        let root = std::env::var_os(super::common::evidence::ROOT_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-evidence/adhoc"));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let probe = (0..32)
            .find_map(|next| {
                let path = root.join(format!("index--backup-check-{}-{next}", std::process::id()));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => Some(Self(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("making {}: {error}", path.display()),
                }
            })
            .expect("32 backup-query evidence names are already occupied");
        fs::write(
            probe.0.join("rerun.txt"),
            "HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest run --locked -p recto-cli --test index -E 'test(=a_session_marks_its_corpus_folder)'\n",
        )
        .unwrap();
        probe
    }

    // Output files avoid waiting for EOF from a compiler descendant; every child has a known owned group.
    fn start(&self, label: &str, mut command: Command, group: Option<u32>) -> Owned {
        let out = self.0.join(format!("{label}.stdout"));
        let err = self.0.join(format!("{label}.stderr"));
        fs::write(self.0.join(format!("{label}.command")), format!("{command:?}\nprocess group: {group:?}\n")).unwrap();
        Owned {
            child: command
                .process_group(group.map(|id| i32::try_from(id).unwrap()).unwrap_or(0))
                .stdin(Stdio::null())
                .stdout(File::create(&out).unwrap())
                .stderr(File::create(&err).unwrap())
                .spawn()
                .unwrap(),
            reaped: false,
        }
    }

    fn run(&self, label: &str, command: Command, limit: Duration) -> Ran {
        self.finish(label, self.start(label, command, None), limit)
    }

    fn finish(&self, label: &str, mut child: Owned, limit: Duration) -> Ran {
        let began = Instant::now();
        let (status, timed_out) = loop {
            if let Some(status) = child.child.try_wait().unwrap() {
                child.reaped = true;
                break (status, false);
            }
            if began.elapsed() >= limit {
                child.stop();
                let status = child.child.wait().unwrap();
                child.reaped = true;
                break (status, true);
            }
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        };
        let read = |path: &Path| {
            let mut bytes = Vec::new();
            File::open(path).unwrap().take(8192).read_to_end(&mut bytes).unwrap();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let ran = Ran {
            status,
            timed_out,
            elapsed: began.elapsed(),
            stdout: read(&self.0.join(format!("{label}.stdout"))),
            stderr: read(&self.0.join(format!("{label}.stderr"))),
        };
        fs::write(self.0.join(format!("{label}.result")), format!("{ran:?}\n")).unwrap();
        ran
    }

    fn forced_hang(&self, program: &Path) {
        let ready = [self.0.join("hang.ready"), self.0.join("peer.ready")];
        let mut hang = Command::new(program);
        hang.arg("--hang").arg(&ready[0]);
        let leader = self.start("forced-hang", hang, None);
        // Group membership determines group signal delivery; both members are ours to reap, without an orphan grandchild.
        let mut other = Command::new(program);
        other.arg("--hang").arg(&ready[1]);
        let peer = self.start("peer", other, Some(leader.child.id()));
        let began = Instant::now();
        while !ready.iter().all(|path| path.metadata().is_ok_and(|meta| meta.len() > 0)) {
            assert!(began.elapsed() < Duration::from_secs(10), "native probes did not reach their forced waits");
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        let stopped = self.finish("forced-hang", leader, Duration::from_millis(20));
        // A broken single-PID signal leaves this member alive. The bound cleans it up, then fails, instead of stranding it.
        let peer_stopped = self.finish("peer", peer, Duration::from_secs(5));
        assert!(stopped.timed_out && stopped.status.signal() == Some(9), "{stopped:?}");
        assert!(
            !peer_stopped.timed_out && peer_stopped.status.signal() == Some(9),
            "group member survived: {peer_stopped:?}"
        );
        assert!(
            stopped.stderr.contains("entered forced wait"),
            "the child reached its injected hang: {stopped:?}"
        );
    }
}

/// The existing session test also checks an unmarked control and forces a hung native query to prove bounded cleanup.
pub fn assert_excluded(folder: &Path) {
    let older = Probe::new();
    fs::write(older.0.join("earlier-evidence"), "keep this earlier result").unwrap();
    let probe = Probe::new();
    assert_ne!(probe.0, older.0, "PID reuse must not reuse an earlier evidence leaf");
    assert_eq!(fs::read_to_string(older.0.join("earlier-evidence")).unwrap(), "keep this earlier result");
    let source = probe.0.join("backup.c");
    let program = probe.0.join("backup");
    fs::write(&source, PROBE).unwrap();
    let mut compile = Command::new("/usr/bin/clang");
    compile.args(["-O2", "-framework", "CoreFoundation"]).arg(&source).arg("-o").arg(&program);
    let compiled = probe.run("compile", compile, Duration::from_secs(20));
    assert!(compiled.status.success() && !compiled.timed_out, "{compiled:?}");
    eprintln!("backup exclusion probe compiler: {:?}", compiled.elapsed);

    let unmarked = probe.0.join("unmarked");
    fs::create_dir(&unmarked).unwrap();
    for (label, path, expected) in [("unmarked", unmarked.as_path(), "excluded=0\n"), ("marked", folder, "excluded=1\n")] {
        let mut query = Command::new(&program);
        query.arg(path);
        let said = probe.run(label, query, Duration::from_secs(5));
        assert!(said.status.success() && !said.timed_out && said.stdout == expected, "{label}: {said:?}");
    }

    probe.forced_hang(&program);
}
