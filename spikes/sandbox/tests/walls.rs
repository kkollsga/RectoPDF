//! From inside the sandbox, a stand-in for a compromised renderer tries what a compromised renderer would try. Every attempt also runs with no sandbox, so a wall can't pass because
//! the attempt was broken. Ad-hoc signed: what only a Developer ID build can prove is listed in `docs/research/2026-11-07-sandbox-spike.md`.
//!
//! The canary in the home folder is this test's own, in a folder it makes and removes. Reuses the bundle ID `no.rectospike.render`, so it makes no new container.

use recto_platform::isolate::{Channel, ChildExit, ChildProcess};
use recto_render_service::{DocRenderer, Policy, ServiceConfig, Surface, TileRect, TileRequest, TileResult};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ID: &str = "no.rectospike.render";
const ENTITLEMENTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/sandbox.entitlements");
const SBPL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/profile.sb");
const PDFIUM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/pdfium/libpdfium.dylib");

fn plist(id: &str, executable: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleExecutable</key><string>{executable}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSBackgroundOnly</key><true/>
</dict></plist>
"#
    )
}

fn sign(path: &Path, entitlements: Option<&str>) {
    let mut command = Command::new("/usr/bin/codesign");
    command.args(["-f", "-s", "-"]);
    if let Some(entitlements) = entitlements {
        command.args(["--entitlements", entitlements]);
    }
    let output = command.arg(path).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// Everything one test needs: a scratch folder, a canary in it and one in the home folder, a listener to connect to. All removed on drop.
struct Fixture {
    scratch: PathBuf,
    home_dir: PathBuf,
    listener: TcpListener,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let tag = format!("{}-{name}", std::process::id());
        let scratch = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-walls-{tag}"));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("canary.txt"), "canary").unwrap();
        let home_dir = PathBuf::from(std::env::var_os("HOME").unwrap()).join(format!(".recto-sandbox-spike-{tag}"));
        std::fs::create_dir_all(&home_dir).unwrap();
        std::fs::write(home_dir.join("canary.txt"), "canary").unwrap();
        Self { scratch, home_dir, listener: TcpListener::bind("127.0.0.1:0").unwrap() }
    }

    /// A bundle holding `binary` as its executable, sandboxed if `entitled`, and with a small background app inside that writes a marker when launched. The marker holds the home
    /// canary only if whatever got launched is outside the sandbox.
    fn bundle(&self, binary: &str, entitled: bool, extra: &[(&str, &str)]) -> PathBuf {
        let app = self.scratch.join("Probe.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        let executable = Path::new(binary).file_name().unwrap().to_str().unwrap();
        std::fs::copy(binary, contents.join("MacOS").join(executable)).unwrap();
        for (from, to) in extra {
            std::fs::copy(from, contents.join("MacOS").join(to)).unwrap();
        }
        std::fs::write(contents.join("Info.plist"), plist(ID, executable)).unwrap();
        let escape = contents.join("Resources/Escape.app/Contents");
        std::fs::create_dir_all(escape.join("MacOS")).unwrap();
        std::fs::write(escape.join("Info.plist"), plist("no.rectospike.escape", "escape")).unwrap();
        let script = format!("#!/bin/sh\n{{ echo ran; cat '{}'; }} > '{}' 2>&1\n", self.home_dir.join("canary.txt").display(), self.marker().display());
        let script_path = escape.join("MacOS/escape");
        std::fs::write(&script_path, script).unwrap();
        std::fs::set_permissions(&script_path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        sign(&contents.join("Resources/Escape.app"), None);
        for (_, to) in extra {
            sign(&contents.join("MacOS").join(to), None);
        }
        sign(&app, entitled.then_some(ENTITLEMENTS));
        contents.join("MacOS").join(executable)
    }

    fn marker(&self) -> PathBuf {
        self.scratch.join("marker.txt")
    }

    fn escape_app(&self) -> PathBuf {
        self.scratch.join("Probe.app/Contents/Resources/Escape.app")
    }

    /// The attempts that must all fail in a sandboxed renderer.
    fn attempts(&self) -> Vec<String> {
        let port = self.listener.local_addr().unwrap().port();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        [
            format!("--read=scratch={}", self.scratch.join("canary.txt").display()),
            format!("--read=home={}", self.home_dir.join("canary.txt").display()),
            format!("--list=home={}", home.display()),
            format!("--write=scratch={}", self.scratch.join("written.txt").display()),
            format!("--write=home={}", self.home_dir.join("written.txt").display()),
            format!("--net={port}"),
            "--spawn=/usr/bin/true".to_string(),
            "--fork".to_string(),
            "--mach=com.apple.coreservices.launchservicesd,com.apple.windowserver.active,com.apple.pasteboard.1".to_string(),
        ]
        .into_iter()
        .flat_map(|attempt| match attempt.split_once('=') {
            Some((flag, value)) => vec![flag.to_string(), value.to_string()],
            None => vec![attempt],
        })
        .collect()
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

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

impl Drop for Fixture {
    fn drop(&mut self) {
        // A launched app leaves a line in the LaunchServices database. This takes back only this test's own.
        let _ = Command::new(LSREGISTER).arg("-u").arg(self.escape_app()).output();
        let _ = Command::new(LSREGISTER).arg("-u").arg(self.scratch.join("Probe.app")).output();
        let _ = std::fs::remove_dir_all(&self.scratch);
        let _ = std::fs::remove_dir_all(&self.home_dir);
    }
}

/// Runs the probe with these arguments. Returns each attempt's name and whether it got through.
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

fn probe_binary() -> &'static str {
    env!("CARGO_BIN_EXE_sandbox-probe")
}

fn hardened(rest: Vec<String>) -> Vec<String> {
    let mut args = vec!["--harden".to_string(), "--drop-mach".to_string()];
    args.extend(rest);
    args
}

/// The names a sandbox must stop. Each is a way to read what wasn't given, reach the network, or start something.
const WALLS: [&str; 13] = [
    "read:scratch",
    "read:home",
    "list:home",
    "write:scratch",
    "write:home",
    "net:connect_loopback",
    "net:bind_loopback",
    "net:connect_dns_socket",
    "spawn",
    "fork",
    "mach:com.apple.coreservices.launchservicesd",
    "mach:com.apple.windowserver.active",
    "mach:com.apple.pasteboard.1",
];

fn assert_all_denied(results: &BTreeMap<String, bool>) {
    for wall in WALLS {
        assert_eq!(results.get(wall), Some(&false), "{wall} wasn't stopped, or wasn't tried: {results:?}");
    }
}

/// Failure: an attempt is broken in a way that makes it fail anywhere, so the sandbox tests below pass whatever the sandbox does.
#[test]
fn with_no_sandbox_every_attempt_gets_through() {
    let fixture = Fixture::new("control");
    let results = probe(Path::new(probe_binary()), &fixture.attempts());
    for wall in WALLS {
        assert_eq!(results.get(wall), Some(&true), "{wall} failed with no sandbox, so it proves nothing: {results:?}");
    }
}

/// Failure: a compromised worker, sandboxed by entitlement with the limits and the Mach drop on, reads the user's files, writes outside its container, reaches the network, starts a
/// process or asks a system service for anything. The whole of the recommended arrangement.
#[test]
fn the_recommended_walls_hold() {
    let fixture = Fixture::new("walls");
    let executable = fixture.bundle(probe_binary(), true, &[]);
    let mut args = hardened(fixture.attempts());
    args.extend(["--ls-open".to_string(), fixture.escape_app().display().to_string()]);
    let results = probe(&executable, &args);
    assert_eq!(results.get("drop_mach"), Some(&true), "no rights were dropped, so the Mach wall isn't there: {results:?}");
    assert_all_denied(&results);
    assert_eq!(results.get("launchservices:open"), Some(&false), "LaunchServices took the request: {results:?}");
    assert_eq!(fixture.marker_after_launch(), None, "something was launched outside the sandbox");
}

/// Failure: writing into its own container isn't stopped, so a compromised worker can leave a file for the next one to read. `RLIMIT_FSIZE` is what holds this, as the container is
/// writable to the sandbox by design. The control proves the sandbox alone allows it.
#[test]
fn the_container_is_writable_only_without_the_file_size_limit() {
    let fixture = Fixture::new("container");
    let executable = fixture.bundle(probe_binary(), true, &[]);
    let target = PathBuf::from(std::env::var_os("HOME").unwrap()).join(format!("Library/Containers/{ID}/Data/tmp/recto-walls-{}.txt", std::process::id()));
    let args = vec!["--write".to_string(), format!("container={}", target.display())];
    assert_eq!(probe(&executable, &args).get("write:container"), Some(&true), "the sandbox alone stopped it, so the limit isn't what's being tested");
    let hardened_args = hardened(args);
    assert_eq!(probe(&executable, &hardened_args).get("write:container"), Some(&false), "a compromised worker can write to its container");
}

/// Failure: the App Sandbox stops letting a process launch an app, and nobody notices that the Mach drop no longer carries its own weight. Or, the other way, this stays true and the Mach
/// drop is taken out as unneeded, when this is the reason for it. The launched app reads a canary the sandbox can't, which is what running outside it means.
#[test]
fn the_sandbox_alone_lets_launchservices_start_an_unsandboxed_app() {
    let fixture = Fixture::new("escape");
    let executable = fixture.bundle(probe_binary(), true, &[]);
    let results = probe(&executable, &["--harden".to_string(), "--ls-open".to_string(), fixture.escape_app().display().to_string()]);
    assert_eq!(results.get("launchservices:open"), Some(&true), "{results:?}");
    let marker = fixture.marker_after_launch().expect("the launched app left no marker");
    assert!(marker.contains("canary"), "the launched app ran inside the sandbox: {marker}");
}

/// Failure: a compromised worker fetches a right to launchd, or to some new process, from one of the places the kernel hands them out, and with it undoes the Mach drop. One right can't be
/// taken away: `TASK_ACCESS_PORT`, slot 9, which goes to the task access server and refuses to be cleared. It reaches no launchd, which this checks, and any other that appears fails here.
#[test]
fn no_right_to_launchd_can_be_fetched_back_after_the_drop() {
    let fixture = Fixture::new("regain");
    let executable = fixture.bundle(probe_binary(), true, &[]);
    let before = probe(&executable, &["--harden".to_string(), "--regain".to_string()]);
    assert_eq!(before.get("regain_launchd:task_special_4"), Some(&true), "launchd's right isn't where this test looks, so the test proves nothing: {before:?}");
    let after = probe(&executable, &hardened(vec!["--regain".to_string()]));
    let reaching: Vec<_> = after.iter().filter(|(name, allowed)| name.starts_with("regain_launchd:") && **allowed).collect();
    assert!(reaching.is_empty(), "these reach launchd: {reaching:?}");
    let back: Vec<_> = after.iter().filter(|(name, allowed)| name.starts_with("regain:") && **allowed).map(|(name, _)| name.as_str()).collect();
    assert_eq!(back, ["regain:task_special_9"], "a right came back that isn't the known one");
}

/// Failure: the self-restricting alternative, kept as the fallback if the entitlement route ever has to go, stops holding its walls.
#[test]
fn the_sandbox_init_alternative_holds_the_same_walls() {
    let fixture = Fixture::new("sbpl");
    let mut args = vec!["--self".to_string(), format!("sbpl:{SBPL}")];
    args.extend(fixture.attempts());
    let results = probe(Path::new(probe_binary()), &args);
    assert_eq!(results.get("sandbox_init"), Some(&true), "{results:?}");
    assert_all_denied(&results);
}

fn draw_page(worker: &Path, file: &TempPdf) -> Vec<u8> {
    let config = ServiceConfig { worker_exe: worker.to_path_buf(), pdfium_lib: worker.with_file_name("libpdfium.dylib"), ..ServiceConfig::for_worker(worker.to_path_buf()).unwrap() };
    let mut doc = DocRenderer::new(&file.0, None, config, Policy::default()).unwrap();
    let surface = Surface::create(1024 * 1024 * 4).unwrap();
    let request = TileRequest { page: 0, rect: TileRect { x: 0, y: 0, width: 1024, height: 1024 }, scale: 2.0, stride: 4096, generation: 1 };
    let result = doc.render_tile(&request, &surface).unwrap();
    assert!(matches!(result, TileResult::Done(_)), "{result:?}");
    let mut pixels = vec![0u8; 1024 * 1024 * 4];
    assert!(surface.copy_out(0, &mut pixels));
    pixels
}

/// Failure: inside the sandbox PDFium can't reach a system font, so it silently draws a substitute and still says it's done. The first profile tried here did exactly that. The page's
/// only font is Helvetica, not embedded. Compares two runs on this machine, so no pixel is pinned.
#[test]
fn the_sandboxed_worker_draws_system_fonts_the_same_as_a_plain_one() {
    let fixture = Fixture::new("fonts");
    let file = TempPdf::new("fonts.pdf", &pages(&[Page::new(text_lines(&["Words in a font this file doesn't carry", "AVA Wax fi fl"]))]));
    let sandboxed = fixture.bundle(env!("CARGO_BIN_EXE_worker-dropmach"), true, &[(PDFIUM, "libpdfium.dylib")]);
    let plain_dir = fixture.scratch.join("plain");
    std::fs::create_dir_all(&plain_dir).unwrap();
    let plain = plain_dir.join("recto-render-service");
    std::fs::copy(env!("CARGO_BIN_EXE_plain-worker"), &plain).unwrap();
    std::fs::copy(PDFIUM, plain_dir.join("libpdfium.dylib")).unwrap();
    let (ours, theirs) = (draw_page(&sandboxed, &file), draw_page(&plain, &file));
    assert!(ours.iter().any(|byte| *byte != 255), "the page came out blank");
    assert!(ours == theirs, "the sandbox changed the picture");
}

/// Failure: a build signed wrong, with no sandbox in force, draws a hostile file anyway and nobody can tell. The worker tests the wall itself before any hostile byte arrives.
#[test]
fn a_worker_with_no_sandbox_refuses_to_start() {
    let (_ours, theirs) = Channel::pair().unwrap();
    let args = ["--pdfium", PDFIUM, "--idle-exit-ms", "1000", "--memory-limit", "536870912", "--deadline-ms", "1000"];
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let child = ChildProcess::spawn(Path::new(env!("CARGO_BIN_EXE_worker-dropmach")), &args, &[], theirs, None).unwrap();
    assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Code(78)), "it ran with no sandbox");
}
