//! Direct backup markers checked against an OS-generated reference, without a mounted-disk scan.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// An OS-created direct marker is the reference; effective exclusion can also come from an ancestor.
const PROBE: &str = r#"
#include <CoreFoundation/CoreFoundation.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/xattr.h>
#include <unistd.h>

#define MARKER "com.apple.metadata:com_apple_backup_excludeItem"
#define MARKER_CAP 4096

static CFURLRef directory_url(const char *path) {
    size_t length = strlen(path);
    if (length > 4096) return NULL;
    return CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)path, length, true);
}

static int excluded(const char *path) {
    CFURLRef url = directory_url(path);
    if (!url) return -1;
    CFTypeRef value = NULL;
    CFErrorRef error = NULL;
    Boolean ok = CFURLCopyResourcePropertyForKey(url, kCFURLIsExcludedFromBackupKey, &value, &error);
    int result = -1;
    if (ok && value && CFGetTypeID(value) == CFBooleanGetTypeID()) result = CFBooleanGetValue(value);
    else if (error) CFShow(error);
    if (value) CFRelease(value);
    if (error) CFRelease(error);
    CFRelease(url);
    return result;
}

static int set_excluded(const char *path) {
    CFURLRef url = directory_url(path);
    if (!url) return 3;
    CFErrorRef error = NULL;
    Boolean ok = CFURLSetResourcePropertyForKey(url, kCFURLIsExcludedFromBackupKey, kCFBooleanTrue, &error);
    if (!ok && error) CFShow(error);
    if (error) CFRelease(error);
    CFRelease(url);
    return ok ? 0 : 4;
}

static CFPropertyListRef direct_marker(const char *path, int *status) {
    UInt8 bytes[MARKER_CAP];
    ssize_t length = getxattr(path, MARKER, bytes, sizeof(bytes), 0, XATTR_NOFOLLOW);
    if (length < 0) {
        *status = errno == ENOATTR ? 6 : 9;
        fprintf(stderr, "direct backup marker read failed: %s (errno=%d)\n", path, errno);
        return NULL;
    }
    CFDataRef data = CFDataCreate(NULL, bytes, length);
    if (!data) { *status = 9; return NULL; }
    CFErrorRef error = NULL;
    CFPropertyListRef marker = CFPropertyListCreateWithData(NULL, data, kCFPropertyListImmutable, NULL, &error);
    if (!marker) {
        *status = 7;
        fprintf(stderr, "direct backup marker is not a property list: %s\n", path);
        if (error) CFShow(error);
    }
    if (error) CFRelease(error);
    CFRelease(data);
    return marker;
}

static int no_direct_marker(const char *path) {
    errno = 0;
    ssize_t length = getxattr(path, MARKER, NULL, 0, 0, XATTR_NOFOLLOW);
    return length < 0 && errno == ENOATTR;
}

static int prepare_reference(const char *parent, const char *reference_path, const char *unmarked) {
    // Only newly created owned fixture paths reach this mode. Never alter an ancestor or machine policy.
    if (!no_direct_marker(reference_path) || !no_direct_marker(unmarked)) return 9;
    int status = set_excluded(parent);
    if (status) return status;
    if (excluded(parent) != 1) return 4;
    CFPropertyListRef parent_marker = direct_marker(parent, &status);
    if (!parent_marker) return status;
    CFRelease(parent_marker);
    status = set_excluded(reference_path);
    if (status) return status;
    // An inherited true must not hide a setter that skipped writing a direct reference marker.
    CFPropertyListRef reference = direct_marker(reference_path, &status);
    if (!reference) return status;
    CFRelease(reference);
    if (!no_direct_marker(unmarked)) return 9;
    int inherited = excluded(unmarked);
    if (inherited < 0) return 4;
    printf("reference=direct unmarked=absent effective=%d\n", inherited);
    return 0;
}

static int wrong_marker(const char *path) {
    CFDataRef data = CFPropertyListCreateData(NULL, CFSTR("not-a-backup-exclusion"), kCFPropertyListBinaryFormat_v1_0, 0, NULL);
    if (!data) return 9;
    int result = setxattr(path, MARKER, CFDataGetBytePtr(data), CFDataGetLength(data), 0, XATTR_NOFOLLOW);
    CFRelease(data);
    return result == 0 ? 0 : 9;
}

static int validate_marker(const char *path, const char *reference_path) {
    int status = 0;
    CFPropertyListRef marker = direct_marker(path, &status);
    if (!marker) return status;
    CFPropertyListRef reference = direct_marker(reference_path, &status);
    if (!reference) { CFRelease(marker); return status; }
    Boolean same = CFEqual(marker, reference);
    CFRelease(marker);
    CFRelease(reference);
    if (!same) { fprintf(stderr, "direct backup marker differs from the OS reference\n"); return 8; }
    if (excluded(path) != 1) return 4;
    printf("direct=1 reference=1 excluded=1\n");
    return 0;
}

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
    if (argc == 4 && strcmp(argv[1], "--validate") == 0) return validate_marker(argv[2], argv[3]);
    if (argc == 9 && strcmp(argv[1], "--check") == 0) {
        int status = prepare_reference(argv[3], argv[4], argv[5]);
        if (status) return status;
        status = validate_marker(argv[2], argv[4]);
        if (status) return status;
        const char bytes[] = "not a property list";
        if (setxattr(argv[6], MARKER, bytes, sizeof(bytes) - 1, 0, XATTR_NOFOLLOW) != 0) return 9;
        status = wrong_marker(argv[7]);
        if (status) return status;
        char oversized[MARKER_CAP + 1];
        memset(oversized, 'x', sizeof(oversized));
        if (setxattr(argv[8], MARKER, oversized, sizeof(oversized), 0, XATTR_NOFOLLOW) != 0) return 9;
        int missing = validate_marker(argv[5], argv[4]);
        int malformed = validate_marker(argv[6], argv[4]);
        int wrong = validate_marker(argv[7], argv[4]);
        int too_large = validate_marker(argv[8], argv[4]);
        printf("controls missing=%d malformed=%d wrong=%d oversized=%d\n", missing, malformed, wrong, too_large);
        return missing == 6 && malformed == 7 && wrong == 8 && too_large == 9 ? 0 : 10;
    }
    return 2;
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

/// How long a probe step (compile, check, the peer's cleanup) may take before it is called hung. The product has no bound here (the check is an in-process xattr read), so this only ends a hang, and a slow busy Mac is not a failure. `forced_hang`'s 20 ms is a different thing: it is the bound that proves cleanup.
const HANG_GUARD: Duration = Duration::from_secs(60);

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
            assert!(began.elapsed() < HANG_GUARD, "native probes did not reach their forced waits within the {HANG_GUARD:?} guard");
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        let stopped = self.finish("forced-hang", leader, Duration::from_millis(20));
        // A broken single-PID signal leaves this member alive. The bound cleans it up, then fails, instead of stranding it.
        let peer_stopped = self.finish("peer", peer, HANG_GUARD);
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

/// The existing session test validates a direct OS marker under an excluded parent, rejects missing/corrupt marks, and proves hung-query cleanup.
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
    let compiled = probe.run("compile", compile, HANG_GUARD);
    assert!(compiled.status.success() && !compiled.timed_out, "{compiled:?}");
    eprintln!("backup exclusion probe compiler: {:?}", compiled.elapsed);

    // The negative control must remain meaningful even when the evidence root is already excluded (as cargo's target is).
    let parent = probe.0.join("excluded-parent");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    let reference = parent.join("os-reference");
    let unmarked = parent.join("unmarked");
    let malformed = parent.join("malformed");
    let wrong = parent.join("wrong-value");
    let oversized = parent.join("oversized");
    for path in [&reference, &unmarked, &malformed, &wrong, &oversized] {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    // One native invocation reuses CoreFoundation for every control instead of paying for another tool process each time.
    let mut check = Command::new(&program);
    check
        .arg("--check")
        .args([folder, &parent, &reference, &unmarked, &malformed, &wrong, &oversized]);
    let checked = probe.run("check", check, HANG_GUARD);
    assert!(checked.status.success() && !checked.timed_out, "native marker validation: {checked:?}");
    assert!(checked.stdout.contains("reference=direct unmarked=absent"), "{checked:?}");
    assert!(checked.stdout.contains("direct=1 reference=1 excluded=1"), "{checked:?}");
    assert!(checked.stdout.contains("controls missing=6 malformed=7 wrong=8 oversized=9"), "{checked:?}");
    eprintln!("backup native controls: {}", checked.stdout.trim());

    probe.forced_hang(&program);
}
