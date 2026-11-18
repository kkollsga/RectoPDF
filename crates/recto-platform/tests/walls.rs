//! The renderer helper's walls, from inside, against a stand-in for a compromised renderer. `recto-sandbox-probe` is laid out and signed by `app/render-helper.sh` in place of the
//! worker, exactly as the app's helper is (ad hoc), puts up the worker's walls with the worker's own calls, then tries what a compromised renderer would. Every attempt also runs with
//! no sandbox, so a wall can't pass because the attempt was broken. What only a Developer ID build can prove is in `docs/research/2026-11-07-sandbox-spike.md`.
//!
//! Each test's files are in its own temp folder, removed on success; a failure retains bounded receipts and a rerun line. The first sandboxed run of the helper's ad hoc identifier makes its container,
//! `~/Library/Containers/io.github.kkollsga.RectoPDF.Render.adhoc`, the one every development build uses; these tests share it and leave nothing else in the home folder.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// This standalone test uses the same collector source as feature-gated test clients.
#[allow(dead_code)]
#[path = "../src/test_command.rs"]
mod command;

const PROBE: &str = env!("CARGO_BIN_EXE_recto-sandbox-probe");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const ADHOC_ID: &str = "io.github.kkollsga.RectoPDF.Render.adhoc";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
const RECEIPT_CAP: u64 = 4096;

fn helper_script() -> PathBuf {
    Path::new(REPO).join("app/render-helper.sh")
}

fn pdfium() -> PathBuf {
    let path = Path::new(REPO).join("target/pdfium/libpdfium.dylib");
    assert!(path.is_file(), "no {}: run tools/pdfium.sh", path.display());
    path
}

/// Never with the owner's identity, whatever this shell has set: a test signs ad hoc, and raises no keychain prompt.
fn run(command: &mut Command) {
    let (exceeded, output) = command::output(command.env_remove("RECTO_SIGN_IDENTITY"), Duration::from_secs(10)).unwrap();
    assert!(
        !exceeded && output.status.success(),
        "{command:?}, bound_exceeded {exceeded}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn capped_text(path: &Path) -> std::io::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?.take(RECEIPT_CAP + 1).read_to_string(&mut text)?;
    if text.len() as u64 > RECEIPT_CAP {
        return Err(std::io::Error::other("receipt exceeds its cap"));
    }
    Ok(text)
}

/// A scratch folder with a canary in it, and a listener to connect to. Removed on drop, with anything a launch left in the LaunchServices database.
struct Fixture {
    scratch: Option<recto_testpdf::TempDir>,
    listener: TcpListener,
    nonce: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let scratch = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), &format!("recto-walls-{name}-"));
        std::fs::create_dir_all(&scratch).unwrap();
        let nonce = format!("{:016x}", recto_platform::local::random_u64());
        std::fs::write(scratch.join("canary.txt"), format!("canary:{nonce}\n")).unwrap();
        Self {
            scratch: Some(scratch),
            listener: TcpListener::bind("127.0.0.1:0").unwrap(),
            nonce,
        }
    }

    fn scratch(&self) -> &Path {
        self.scratch.as_ref().expect("fixture still owned").path()
    }

    fn contents(&self) -> PathBuf {
        self.scratch().join("Host.app/Contents")
    }

    fn helper(&self) -> PathBuf {
        self.contents().join("Helpers/RectoRender.app")
    }

    fn escape_app(&self) -> PathBuf {
        self.helper().join("Contents/Resources/Escape.app")
    }

    fn marker(&self) -> PathBuf {
        self.scratch().join("marker.txt")
    }

    fn escape_executable(&self) -> PathBuf {
        self.escape_app().join("Contents/MacOS/escape")
    }

    fn own_processes(&self) -> Vec<u32> {
        recto_platform::local::processes_running(&self.escape_executable())
    }

    /// The probe as the helper's worker, laid out and signed by the app's own script, with the sandbox. Inside it, a small background app that writes a marker when launched,
    /// holding the canary only if what got launched is outside the sandbox. The sandbox can read it, as it's in the helper's own bundle, and LaunchServices only launches what the
    /// caller can read.
    fn sandboxed_probe(&self) -> PathBuf {
        run(Command::new(helper_script()).arg("--layout").arg(self.contents()).arg(PROBE).arg(pdfium()));
        let escape = self.escape_app().join("Contents");
        std::fs::create_dir_all(escape.join("MacOS")).unwrap();
        std::fs::write(
            escape.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.kkollsga.RectoPDF.WallsEscape.{}</string>
<key>CFBundleExecutable</key><string>escape</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSBackgroundOnly</key><true/>
</dict></plist>
"#,
                self.nonce
            ),
        )
        .unwrap();
        let script = escape.join("MacOS/escape");
        std::fs::copy(PROBE, &script).unwrap();
        std::fs::create_dir_all(escape.join("Resources")).unwrap();
        std::fs::write(escape.join("Resources/escape.txt"), format!("{}\n{}\n", self.scratch().display(), self.nonce)).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        run(Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(self.escape_app()));
        // Signed again, as the app's build signs it, so the seal covers what was added.
        run(Command::new(helper_script()).arg("--sign").arg(self.contents()));
        run(Command::new(helper_script()).arg("--check").arg(self.helper()));
        self.helper().join("Contents/MacOS/recto-render-service")
    }

    /// The attempts that must all fail in a sandboxed renderer, each with the names it reports.
    fn attempt_list(&self) -> Vec<(Vec<String>, Vec<&'static str>)> {
        let port = self.listener.local_addr().unwrap().port();
        let home = std::env::var("HOME").unwrap();
        let one = |flag: &str, value: String| vec![flag.to_string(), value];
        vec![
            (
                one("--read", format!("scratch={}", self.scratch().join("canary.txt").display())),
                vec!["read:scratch"],
            ),
            (one("--list", format!("home={home}")), vec!["list:home"]),
            (
                one("--write", format!("scratch={}", self.scratch().join("written.txt").display())),
                vec!["write:scratch"],
            ),
            (
                one("--net", port.to_string()),
                vec!["net:connect_loopback", "net:bind_loopback", "net:connect_dns_socket"],
            ),
            (one("--spawn", "/usr/bin/true".to_string()), vec!["spawn"]),
            (
                one(
                    "--mach",
                    "com.apple.coreservices.launchservicesd,com.apple.windowserver.active,com.apple.pasteboard.1".to_string(),
                ),
                vec![
                    "mach:com.apple.coreservices.launchservicesd",
                    "mach:com.apple.windowserver.active",
                    "mach:com.apple.pasteboard.1",
                ],
            ),
            (vec!["--fork".to_string()], vec!["fork"]),
            (vec!["--signal-parent".to_string()], vec!["signal:parent"]),
        ]
    }

    /// All of them as one probe's arguments.
    fn attempts(&self) -> Vec<String> {
        self.attempt_list().into_iter().flat_map(|(args, _)| args).collect()
    }

    /// The same five-second limit covers both the complete atomic receipt and the exact private executable exiting.
    fn marker_after_launch(&self) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Ok(text) = capped_text(&self.marker())
                && self.own_processes().is_empty()
            {
                return Some(text);
            }
            std::thread::sleep(Duration::from_millis(50)); // ratchet: poll
        }
        assert!(self.own_processes().is_empty(), "the owned app didn't exit: {:?}", self.own_processes());
        capped_text(&self.marker()).ok()
    }

    fn assert_receipt(&self, marker: &str) {
        let fields: BTreeMap<_, _> = marker.lines().filter_map(|line| line.split_once('\t')).collect();
        assert_eq!(
            fields.keys().copied().collect::<Vec<_>>(),
            ["bytes", "exe", "fsize", "nonce", "nproc", "pid", "read"],
            "invalid receipt fields: {marker:?}"
        );
        assert_eq!(marker.lines().count(), 7, "extra receipt fields: {marker:?}");
        assert_eq!(fields.get("nonce"), Some(&self.nonce.as_str()), "wrong launch receipt: {marker:?}");
        let executable = self.escape_executable().canonicalize().unwrap();
        assert_eq!(fields.get("exe").copied(), executable.to_str(), "another executable answered: {marker:?}");
        let pid = fields
            .get("pid")
            .and_then(|pid| pid.parse::<u32>().ok())
            .filter(|pid| *pid > 1 && *pid <= i32::MAX as u32)
            .expect("receipt PID");
        assert_ne!(
            recto_platform::local::process_path(pid),
            Some(executable),
            "the receipt's app still runs: {marker:?}"
        );
        assert_eq!(fields.get("read"), Some(&"ok"), "the app didn't read its canary: {marker:?}");
        let expected = format!("canary:{}\n", self.nonce).bytes().map(|byte| format!("{byte:02x}")).collect::<String>();
        assert_eq!(fields.get("bytes"), Some(&expected.as_str()), "the exact canary wasn't read: {marker:?}");
        println!("owned native launch receipt:\n{marker}");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let executable = self.escape_executable();
        let mut cleanup = format!("exact executable: {}\nowned processes: {:?}\n", executable.display(), self.own_processes());
        // A path lookup cannot keep a numeric PID from being recycled. The Escape process owns its exit timer; cleanup only waits or retains its bundle.
        let began = Instant::now();
        while !self.own_processes().is_empty() && began.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(10)); // ratchet: poll
        }
        let remaining = self.own_processes();
        let processes_gone = remaining.is_empty();
        let mut clean = processes_gone;
        cleanup.push_str(&format!("remaining processes: {remaining:?}\n"));
        for (app, identifier) in [
            (self.escape_app(), format!("io.github.kkollsga.RectoPDF.WallsEscape.{}", self.nonce)),
            (self.helper(), ADHOC_ID.into()),
        ] {
            if app.exists() {
                match command::output(Command::new(LSREGISTER).arg("-u").arg(&app), Duration::from_secs(5)) {
                    Ok((exceeded, output)) => {
                        let mut absent = !exceeded && output.status.success();
                        cleanup.push_str(&format!(
                            "unregister {}: {:?}, bound_exceeded {exceeded}\n{}\n",
                            app.display(),
                            output.status,
                            String::from_utf8_lossy(&output.stderr)
                        ));
                        if !absent {
                            match command::output(
                                Command::new(PROBE).args(["--registered-app", &format!("{identifier}|{}", app.display())]),
                                Duration::from_secs(5),
                            ) {
                                Ok((exceeded, query)) => {
                                    let said = String::from_utf8_lossy(&query.stdout);
                                    let fields = parse(&said);
                                    absent = !exceeded
                                        && query.status.success()
                                        && fields.get("registration:lookup") == Some(&true)
                                        && fields.get("registration:present") == Some(&false);
                                    cleanup.push_str(&format!("exact registration query: {:?}, bound_exceeded {exceeded}\n{said}\n", query.status));
                                }
                                Err(error) => cleanup.push_str(&format!("exact registration query failed: {error}\n")),
                            }
                        }
                        clean &= absent;
                    }
                    Err(error) => {
                        clean = false;
                        cleanup.push_str(&format!("unregister {}: {error}\n", app.display()));
                    }
                }
            }
        }
        if std::thread::panicking() || !clean {
            let scratch = self.scratch.take().expect("fixture still owned");
            let thread = std::thread::current();
            let test = thread.name().unwrap_or("the_sandbox_alone_lets_launchservices_start_an_unsandboxed_app");
            let profile = if std::env::var("NEXTEST_PROFILE").as_deref() == Ok("ci") {
                "ci"
            } else {
                "default"
            };
            let rerun = format!(
                "cd '{}' && tools/check.sh --like-ci 'binary_id(=recto-platform::walls) & test(={test})' --profile {profile}\n",
                REPO.replace('\'', "'\\''")
            );
            let _ = std::fs::write(scratch.join("cleanup.txt"), &cleanup);
            let _ = std::fs::write(scratch.join("rerun.txt"), &rerun);
            // Keep only bounded diagnostics, not copied executables or PDFium. A rerun rebuilds its own fixture.
            for (from, name) in [
                (
                    scratch.join("Host.app/Contents/Helpers/RectoRender.app/Contents/Resources/Escape.app/Contents/Info.plist"),
                    "escape-Info.plist",
                ),
                (
                    scratch.join("Host.app/Contents/Helpers/RectoRender.app/Contents/Resources/Escape.app/Contents/Resources/escape.txt"),
                    "escape-config.txt",
                ),
            ] {
                if let Ok(text) = capped_text(&from) {
                    let _ = std::fs::write(scratch.join(name), text);
                }
            }
            if processes_gone {
                let _ = std::fs::remove_dir_all(scratch.join("Host.app"));
            }
            eprintln!("walls evidence retained at {}\n{cleanup}rerun: {rerun}", scratch.display());
            std::mem::forget(scratch);
        }
        if !std::thread::panicking() {
            assert!(clean, "owned fixture cleanup failed: {cleanup}");
        }
    }
}

/// Runs the probe. Returns each attempt's name and whether it got through.
fn probe(executable: &Path, args: &[String]) -> BTreeMap<String, bool> {
    parse(&probe_text(executable, args))
}

/// Runs the probe and returns what it said, line for line, for a test that has to show all of it when it fails. A probe that hangs, waiting on a lookup nobody answers, is killed
/// at 30 s and fails with what it had said by then.
fn probe_text(executable: &Path, args: &[String]) -> String {
    let (status, said) = probe_run(executable, args);
    assert!(
        status.is_some_and(|status| status.success()),
        "the probe itself failed or hung ({status:?}), having said:\n{said}"
    );
    said
}

/// Runs the probe and returns how it ended, `None` if it hung and was killed at 30 s, and what it had said by then.
fn probe_run(executable: &Path, args: &[String]) -> (Option<std::process::ExitStatus>, String) {
    let (exceeded, output) = command::output(Command::new(executable).args(args), Duration::from_secs(30)).unwrap();
    if !output.stderr.is_empty() {
        eprintln!("probe stderr:\n{}", String::from_utf8_lossy(&output.stderr));
    }
    ((!exceeded).then_some(output.status), String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One attempt in a probe of its own, with the walls up first, so a process the system kills for trying one thing doesn't hide the rest. Killed by a signal after its walls went up
/// and while trying this attempt, as its last `trying` line shows, means the attempt was stopped: each of its names counts as denied, and the note says so. Killed anywhere else,
/// such as during the walls, fails with the whole output.
fn attempt_alone(executable: &Path, args: &[String], names: &[&str], notes: &mut Vec<String>) -> BTreeMap<String, bool> {
    let mut full = vec!["--walls".to_string()];
    full.extend(args.iter().cloned());
    let (status, said) = probe_run(executable, &full);
    let mut results = parse(&said);
    let attempt = args[0].trim_start_matches('-');
    if status.is_some_and(|status| status.success()) {
        return results;
    }
    let last_tried = said.lines().rev().find_map(|line| line.strip_prefix("trying\t"));
    let signal = status.and_then(|status| std::os::unix::process::ExitStatusExt::signal(&status));
    assert!(
        signal.is_some() && results.get("walls") == Some(&true) && last_tried == Some(attempt),
        "the probe ended ({status:?}) outside the attempt `{attempt}`, last trying {last_tried:?}, having said:\n{said}"
    );
    notes.push(format!(
        "`{attempt}` was killed by signal {} while being tried, after the walls were up: counted as stopped. It said:\n{said}",
        signal.unwrap_or_default()
    ));
    for name in names {
        results.entry((*name).to_string()).or_insert(false);
    }
    results
}

fn parse(said: &str) -> BTreeMap<String, bool> {
    said.lines()
        .filter(|line| !line.starts_with("trying\t"))
        .filter_map(|line| {
            let mut parts = line.split('\t');
            Some((parts.next()?.to_string(), parts.next()? == "allowed"))
        })
        .collect()
}

fn with_walls(rest: Vec<String>) -> Vec<String> {
    let mut args = vec!["--walls".to_string()];
    args.extend(rest);
    args
}

/// Each is a way to read what wasn't given, reach the network, start something, or ask a system service for anything.
const WALLS: [&str; 12] = [
    "read:scratch",
    "list:home",
    "write:scratch",
    "net:connect_loopback",
    "net:bind_loopback",
    "net:connect_dns_socket",
    "spawn",
    "fork",
    "signal:parent",
    "mach:com.apple.coreservices.launchservicesd",
    "mach:com.apple.windowserver.active",
    "mach:com.apple.pasteboard.1",
];

/// Failure: an attempt is broken in a way that makes it fail anywhere, so the tests below pass whatever the sandbox does.
#[test]
fn with_no_sandbox_every_attempt_gets_through() {
    let fixture = Fixture::new("control");
    let results = probe(Path::new(PROBE), &fixture.attempts());
    for wall in WALLS {
        assert_eq!(
            results.get(wall),
            Some(&true),
            "{wall} failed with no sandbox, so it proves nothing: {results:?}"
        );
    }
}

/// Failure: a compromised renderer, in the helper as the app lays it out and signs it, with the worker's walls up, reads a file it wasn't given, writes outside its container, reaches
/// the network, starts a process, signals the app, asks a system service for anything, or has LaunchServices start an app for it. Each attempt runs in a probe of its own: on a CI
/// runner with macOS 26 the system once killed the probe partway through its attempts, which hid the rest. A kill during an attempt counts as that attempt stopped, and is printed.
#[test]
fn the_helpers_walls_hold() {
    let fixture = Fixture::new("walls");
    let executable = fixture.sandboxed_probe();
    let mut results = BTreeMap::new();
    let mut notes = Vec::new();
    for (args, names) in fixture.attempt_list() {
        let one = attempt_alone(&executable, &args, &names, &mut notes);
        assert_eq!(
            one.get("walls"),
            Some(&true),
            "the worker's walls didn't go up in the helper, before `{}`: {one:?}",
            args[0]
        );
        results.extend(one.into_iter().filter(|(name, _)| name != "walls"));
    }
    let launch = attempt_alone(
        &executable,
        &["--ls-open".to_string(), fixture.escape_app().display().to_string()],
        &["launchservices:open"],
        &mut notes,
    );
    results.extend(launch.into_iter().filter(|(name, _)| name != "walls"));
    for note in &notes {
        println!("{note}");
    }
    for wall in WALLS {
        assert_eq!(
            results.get(wall),
            Some(&false),
            "{wall} wasn't stopped, or wasn't tried: {results:?}\n{}",
            notes.join("\n")
        );
    }
    assert_eq!(results.get("launchservices:open"), Some(&false), "LaunchServices took the request: {results:?}");
    assert_eq!(fixture.marker_after_launch(), None, "something was launched outside the sandbox");
}

/// Failure: the App Sandbox stops letting a process launch an app, and nobody notices that the Mach drop no longer carries its own weight. Or this stays true and the drop is taken out
/// as unneeded, when this is the reason for it. The launched app reads a canary the sandbox can't, which is what running outside it means. An error naming `canary.txt` must never count as a successful read. Launches only the test's own background app.
#[test]
fn the_sandbox_alone_lets_launchservices_start_an_unsandboxed_app() {
    let fixture = Fixture::new("escape");
    let executable = fixture.sandboxed_probe();
    let said = probe_text(
        &executable,
        &["--harden".to_string(), "--ls-open".to_string(), fixture.escape_app().display().to_string()],
    );
    std::fs::write(fixture.scratch().join("probe.txt"), &said).unwrap();
    let results = parse(&said);
    assert_eq!(results.get("launchservices:open"), Some(&true), "{results:?}");
    let marker = fixture.marker_after_launch().expect("the launched app left no marker");
    fixture.assert_receipt(&marker);
}

/// Failure: a compromised renderer leaves a file in its container for the next one to read. The container is writable to the sandbox by design, so `RLIMIT_FSIZE` is what holds this;
/// the first half proves the sandbox alone allows it.
#[test]
fn the_container_is_writable_only_without_the_file_size_limit() {
    let fixture = Fixture::new("container");
    let executable = fixture.sandboxed_probe();
    // The container exists once a sandboxed process of this identifier has run: the probe, just now, if no build had before.
    let _ = probe(&executable, &[]);
    let target =
        PathBuf::from(std::env::var_os("HOME").unwrap()).join(format!("Library/Containers/{ADHOC_ID}/Data/tmp/recto-walls-{}.txt", std::process::id()));
    let args = vec!["--write".to_string(), format!("container={}", target.display())];
    assert_eq!(
        probe(&executable, &args).get("write:container"),
        Some(&true),
        "the sandbox alone stopped it, so the limit isn't what's being tested"
    );
    assert_eq!(
        probe(&executable, &with_walls(args)).get("write:container"),
        Some(&false),
        "a compromised renderer can write to its container"
    );
}

/// Failure: a compromised renderer fetches a right to launchd, or to some new process, from one of the places the kernel hands them out, and with it undoes the Mach drop. The property
/// is checked by doing it: a real bootstrap lookup through every special and registered right that isn't positively a kernel object, so it doesn't rest on how the kernel describes a
/// port, which differs between versions (on macOS 15 CI, launchd's right didn't read as "unknown" to `mach_port_kernel_object`, so the first version of this never looked through it).
/// Before the walls, launchd's own slot should answer, which shows the lookup works here; if it doesn't, there's nothing to regain on this Mac, and the test says so and still checks
/// the walls. Every message carries the probe's whole output, rights and descriptions included, as CI's macOS 15 is the only one there is.
#[test]
fn no_right_to_launchd_can_be_fetched_back_after_the_drop() {
    let fixture = Fixture::new("regain");
    let executable = fixture.sandboxed_probe();
    let before_text = probe_text(&executable, &["--harden".to_string(), "--rights".to_string(), "--regain".to_string()]);
    let before = parse(&before_text);
    if before.get("regain_launchd:task_special_4") == Some(&true) {
        println!("Before the walls, launchd answers through its own slot, so the lookup below is a real test.");
    } else {
        println!(
            "Before the walls, launchd doesn't answer through its own slot on this Mac, so there's nothing to regain; the walls are still checked.\n{before_text}"
        );
    }
    let after_text = probe_text(&executable, &with_walls(vec!["--rights".to_string(), "--regain".to_string()]));
    let after = parse(&after_text);
    assert_eq!(
        after.get("walls"),
        Some(&true),
        "the walls didn't go up.\nAfter:\n{after_text}\nBefore:\n{before_text}"
    );
    let reaching: Vec<_> = after
        .iter()
        .filter(|(name, allowed)| name.starts_with("regain_launchd:") && **allowed)
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        reaching.is_empty(),
        "these reach launchd after the walls: {reaching:?}.\nAfter:\n{after_text}\nBefore:\n{before_text}"
    );
    let back: Vec<_> = after
        .iter()
        .filter(|(name, allowed)| name.starts_with("regain:") && **allowed)
        .map(|(name, _)| name.as_str())
        .collect();
    println!(
        "After the walls, nothing reaches launchd. Rights that came back and aren't positively kernel objects, none of which answered a lookup: {back:?}\n{after_text}"
    );
}

/// Failure: a helper signed without the sandbox, the mistake a build script could make, still has its walls reported as up. And the check the release runs doesn't catch it.
#[test]
fn a_helper_signed_without_the_sandbox_puts_no_walls_up_and_fails_the_check() {
    let fixture = Fixture::new("unsigned");
    let executable = fixture.sandboxed_probe();
    run(Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(fixture.helper()));
    let results = probe(&executable, &with_walls(Vec::new()));
    assert_eq!(results.get("walls"), Some(&false), "{results:?}");
    let (exceeded, check) = command::output(Command::new(helper_script()).arg("--check").arg(fixture.helper()), Duration::from_secs(10)).unwrap();
    assert!(!exceeded, "the helper check exceeded its bound");
    assert!(!check.status.success(), "the check passed a helper with no sandbox");
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("entitlements"),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// What `render-helper.sh --key` says, with a stand-in `security` on `PATH` that lists `valid` for `find-identity -v` and `all` without it.
fn key_with_keychain(name: &str, valid: &str, all: &str) -> (bool, String) {
    let fixture = Fixture::new(&format!("key-{}", name.len()));
    let bin = fixture.scratch().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(fixture.scratch().join("valid.txt"), valid).unwrap();
    std::fs::write(fixture.scratch().join("all.txt"), all).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$2\" = -v ]; then cat '{0}/valid.txt'; else cat '{0}/all.txt'; fi\n",
        fixture.scratch().display()
    );
    std::fs::write(bin.join("security"), script).unwrap();
    std::fs::set_permissions(bin.join("security"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let (exceeded, output) = command::output(
        Command::new(helper_script())
            .args(["--key", name])
            .env("PATH", path)
            .env_remove("RECTO_SIGN_IDENTITY"),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(!exceeded, "the signing identity lookup exceeded its bound");
    (
        output.status.success(),
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
    )
}

/// Failure: two valid certificates with one name, an old and a renewed one, and signing takes whichever the keychain lists first. Or an expired certificate reads as missing, so
/// nobody looks at its date. Or a name with a letter outside ASCII isn't matched. The hash goes to codesign, which can't find such a name itself.
#[test]
fn the_signing_identity_is_one_valid_certificate_matched_by_its_exact_name() {
    let name = "Developer ID Application: Kristian de Figueiredo Kollsgård (6GND8Z2C7Z)";
    let (a, b) = ("1111111111111111111111111111111111111111", "2222222222222222222222222222222222222222");
    let one = format!("  1) {a} \"{name}\"\n     1 valid identities found\n");
    assert_eq!(key_with_keychain(name, &one, &one), (true, format!("{a}\n")));
    let (ok, said) = key_with_keychain(name, &format!("  1) {a} \"{name}\"\n  2) {b} \"{name}\"\n"), "");
    assert!(!ok && said.contains("2 valid certificates") && said.contains(a) && said.contains(b), "{said}");
    let (ok, said) = key_with_keychain(
        name,
        "     0 valid identities found\n",
        &format!("  1) {a} \"{name}\" (CSSMERR_TP_CERT_EXPIRED)\n"),
    );
    assert!(!ok && said.contains("has expired"), "{said}");
    let (ok, said) = key_with_keychain(name, "     0 valid identities found\n", "");
    assert!(!ok && said.contains("isn't in the keychain"), "{said}");
    let (ok, _) = key_with_keychain("Developer ID Application: Kristian", &one, &one);
    assert!(!ok, "part of a name matched");
}

/// Failure: the Mach drop deletes the names of the rights it lets go of, so a library that held one, `libxpc` say, later releases a name that's gone, and the kernel raises a
/// Mach port guard exception (`kGUARD_EXC_INVALID_NAME`). Logged as non-fatal on this Mac, about three a worker before this was fixed and one after on a blank page, and fatal where
/// a system treats those guards so. Every right dropped must leave a dead name in its place, which reaches nothing and can still be let go of.
#[test]
fn every_dropped_right_leaves_a_dead_name_behind() {
    let fixture = Fixture::new("dead");
    let executable = fixture.sandboxed_probe();
    let said = probe_text(&executable, &with_walls(Vec::new()));
    let line = said
        .lines()
        .find(|line| line.starts_with("walls\tallowed\t"))
        .unwrap_or_else(|| panic!("the walls didn't go up:\n{said}"));
    let numbers: Vec<u32> = line.split(|c: char| !c.is_ascii_digit()).filter_map(|word| word.parse().ok()).collect();
    let (dropped, dead) = (numbers[0], numbers[1]);
    assert!(dropped > 0, "{line}");
    assert_eq!(dead, dropped, "a dropped right left no dead name behind: {line}");
}
