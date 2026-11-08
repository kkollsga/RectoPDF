//! The renderer helper's walls, from inside, against a stand-in for a compromised renderer. `recto-sandbox-probe` is laid out and signed by `app/render-helper.sh` in place of the
//! worker, exactly as the app's helper is (ad hoc), puts up the worker's walls with the worker's own calls, then tries what a compromised renderer would. Every attempt also runs with
//! no sandbox, so a wall can't pass because the attempt was broken. What only a Developer ID build can prove is in `docs/research/2026-11-07-sandbox-spike.md`.
//!
//! Each test's files are in its own temp folder, removed with it, and nothing is made in the home folder. The helper's ad hoc identifier is the development one, so these share the
//! one container every development build uses.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROBE: &str = env!("CARGO_BIN_EXE_recto-sandbox-probe");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const ADHOC_ID: &str = "io.github.kkollsga.RectoPDF.Render.adhoc";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

fn helper_script() -> PathBuf {
    Path::new(REPO).join("app/render-helper.sh")
}

fn pdfium() -> PathBuf {
    let path = Path::new(REPO).join("target/pdfium/libpdfium.dylib");
    assert!(path.is_file(), "no {}: run tools/pdfium.sh", path.display());
    path
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{command:?}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// A scratch folder with a canary in it, and a listener to connect to. Removed on drop, with anything a launch left in the LaunchServices database.
struct Fixture {
    scratch: PathBuf,
    listener: TcpListener,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let scratch = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-walls-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("canary.txt"), "canary").unwrap();
        Self { scratch, listener: TcpListener::bind("127.0.0.1:0").unwrap() }
    }

    fn contents(&self) -> PathBuf {
        self.scratch.join("Host.app/Contents")
    }

    fn helper(&self) -> PathBuf {
        self.contents().join("Helpers/RectoRender.app")
    }

    fn escape_app(&self) -> PathBuf {
        self.helper().join("Contents/Resources/Escape.app")
    }

    fn marker(&self) -> PathBuf {
        self.scratch.join("marker.txt")
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
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.kkollsga.RectoPDF.WallsEscape</string>
<key>CFBundleExecutable</key><string>escape</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSBackgroundOnly</key><true/>
</dict></plist>
"#,
        )
        .unwrap();
        let script = escape.join("MacOS/escape");
        std::fs::write(&script, format!("#!/bin/sh\n{{ echo ran; cat '{}'; }} > '{}' 2>&1\n", self.scratch.join("canary.txt").display(), self.marker().display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        run(Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(self.escape_app()));
        // Signed again, as the app's build signs it, so the seal covers what was added.
        run(Command::new(helper_script()).arg("--sign").arg(self.contents()));
        run(Command::new(helper_script()).arg("--check").arg(self.helper()));
        self.helper().join("Contents/MacOS/recto-render-service")
    }

    /// The attempts that must all fail in a sandboxed renderer.
    fn attempts(&self) -> Vec<String> {
        let port = self.listener.local_addr().unwrap().port();
        let home = std::env::var("HOME").unwrap();
        let mut args = Vec::new();
        for (flag, value) in [
            ("--read", format!("scratch={}", self.scratch.join("canary.txt").display())),
            ("--list", format!("home={home}")),
            ("--write", format!("scratch={}", self.scratch.join("written.txt").display())),
            ("--net", port.to_string()),
            ("--spawn", "/usr/bin/true".to_string()),
            ("--mach", "com.apple.coreservices.launchservicesd,com.apple.windowserver.active,com.apple.pasteboard.1".to_string()),
        ] {
            args.extend([flag.to_string(), value]);
        }
        args.extend(["--fork".to_string(), "--signal-parent".to_string()]);
        args
    }

    /// Waits long enough for a launched app to have written its marker, and says what's in it.
    fn marker_after_launch(&self) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Ok(text) = std::fs::read_to_string(self.marker())
                && text.lines().count() >= 2
            {
                return Some(text);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::read_to_string(self.marker()).ok()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(LSREGISTER).arg("-u").arg(self.escape_app()).output();
        let _ = Command::new(LSREGISTER).arg("-u").arg(self.helper()).output();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// Runs the probe. Returns each attempt's name and whether it got through.
fn probe(executable: &Path, args: &[String]) -> BTreeMap<String, bool> {
    let output = Command::new(executable).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().unwrap();
    assert!(output.status.success(), "the probe itself failed: {:?}, said {}", output.status, String::from_utf8_lossy(&output.stdout));
    String::from_utf8_lossy(&output.stdout)
        .lines()
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
        assert_eq!(results.get(wall), Some(&true), "{wall} failed with no sandbox, so it proves nothing: {results:?}");
    }
}

/// Failure: a compromised renderer, in the helper as the app lays it out and signs it, with the worker's walls up, reads a file it wasn't given, writes outside its container, reaches
/// the network, starts a process, signals the app, asks a system service for anything, or has LaunchServices start an app for it.
#[test]
fn the_helpers_walls_hold() {
    let fixture = Fixture::new("walls");
    let executable = fixture.sandboxed_probe();
    let mut args = with_walls(fixture.attempts());
    args.extend(["--ls-open".to_string(), fixture.escape_app().display().to_string()]);
    let results = probe(&executable, &args);
    assert_eq!(results.get("walls"), Some(&true), "the worker's walls didn't go up in the helper: {results:?}");
    for wall in WALLS {
        assert_eq!(results.get(wall), Some(&false), "{wall} wasn't stopped, or wasn't tried: {results:?}");
    }
    assert_eq!(results.get("launchservices:open"), Some(&false), "LaunchServices took the request: {results:?}");
    assert_eq!(fixture.marker_after_launch(), None, "something was launched outside the sandbox");
}

/// Failure: the App Sandbox stops letting a process launch an app, and nobody notices that the Mach drop no longer carries its own weight. Or this stays true and the drop is taken out
/// as unneeded, when this is the reason for it. The launched app reads a canary the sandbox can't, which is what running outside it means. Launches only the test's own background app.
#[test]
fn the_sandbox_alone_lets_launchservices_start_an_unsandboxed_app() {
    let fixture = Fixture::new("escape");
    let executable = fixture.sandboxed_probe();
    let results = probe(&executable, &["--harden".to_string(), "--ls-open".to_string(), fixture.escape_app().display().to_string()]);
    assert_eq!(results.get("launchservices:open"), Some(&true), "{results:?}");
    let marker = fixture.marker_after_launch().expect("the launched app left no marker");
    assert!(marker.contains("canary"), "the launched app ran inside the sandbox: {marker}");
}

/// Failure: a compromised renderer leaves a file in its container for the next one to read. The container is writable to the sandbox by design, so `RLIMIT_FSIZE` is what holds this;
/// the first half proves the sandbox alone allows it.
#[test]
fn the_container_is_writable_only_without_the_file_size_limit() {
    let fixture = Fixture::new("container");
    let executable = fixture.sandboxed_probe();
    // The container exists once a sandboxed process of this identifier has run: the probe, just now, if no build had before.
    let _ = probe(&executable, &[]);
    let target = PathBuf::from(std::env::var_os("HOME").unwrap()).join(format!("Library/Containers/{ADHOC_ID}/Data/tmp/recto-walls-{}.txt", std::process::id()));
    let args = vec!["--write".to_string(), format!("container={}", target.display())];
    assert_eq!(probe(&executable, &args).get("write:container"), Some(&true), "the sandbox alone stopped it, so the limit isn't what's being tested");
    assert_eq!(probe(&executable, &with_walls(args)).get("write:container"), Some(&false), "a compromised renderer can write to its container");
}

/// Failure: a compromised renderer fetches a right to launchd, or to some new process, from one of the places the kernel hands them out, and with it undoes the Mach drop. One right
/// can't be taken away: `TASK_ACCESS_PORT`, slot 9, which goes to the task access server and refuses to be cleared. It reaches no launchd, which this checks, and any other that appears
/// fails here. Leans on the kernel's description of ports, so it should run on every macOS supported.
#[test]
fn no_right_to_launchd_can_be_fetched_back_after_the_drop() {
    let fixture = Fixture::new("regain");
    let executable = fixture.sandboxed_probe();
    let before = probe(&executable, &["--harden".to_string(), "--regain".to_string()]);
    assert_eq!(before.get("regain_launchd:task_special_4"), Some(&true), "launchd's right isn't where this test looks, so the test proves nothing: {before:?}");
    let after = probe(&executable, &with_walls(vec!["--regain".to_string()]));
    assert_eq!(after.get("walls"), Some(&true), "{after:?}");
    let reaching: Vec<_> = after.iter().filter(|(name, allowed)| name.starts_with("regain_launchd:") && **allowed).collect();
    assert!(reaching.is_empty(), "these reach launchd: {reaching:?}");
    let back: Vec<_> = after.iter().filter(|(name, allowed)| name.starts_with("regain:") && **allowed).map(|(name, _)| name.as_str()).collect();
    assert!(back.is_empty() || back == ["regain:task_special_9"], "a right came back that isn't the known one: {back:?}");
}

/// Failure: a helper signed without the sandbox, the mistake a build script could make, still has its walls reported as up. And the check the release runs doesn't catch it.
#[test]
fn a_helper_signed_without_the_sandbox_puts_no_walls_up_and_fails_the_check() {
    let fixture = Fixture::new("unsigned");
    let executable = fixture.sandboxed_probe();
    run(Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(fixture.helper()));
    let results = probe(&executable, &with_walls(Vec::new()));
    assert_eq!(results.get("walls"), Some(&false), "{results:?}");
    let check = Command::new(helper_script()).arg("--check").arg(fixture.helper()).output().unwrap();
    assert!(!check.status.success(), "the check passed a helper with no sandbox");
    assert!(String::from_utf8_lossy(&check.stderr).contains("entitlements"), "{}", String::from_utf8_lossy(&check.stderr));
}
