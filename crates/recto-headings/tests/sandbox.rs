//! The real `recto-headings` in its sandboxed helper, `RectoStructure.app`: laid out and signed (ad hoc) by `app/render-helper.sh --layout-structure`, as the app's build will, and checked by it
//! as the release is. Proven from outside, by asking the kernel about the live helper, and by what it answers. The helper's own walls, tried by a stand-in for a compromised one, are
//! `recto-platform`'s `tests/walls.rs`.
#![cfg(target_os = "macos")]

use recto_index::corpus::CorpusKey;
use recto_platform::helper::{self, Helper, Priority};
use recto_platform::wall::{pid_may_read, pid_sandboxed};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const EXE: &str = env!("CARGO_BIN_EXE_recto-headings");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/render-helper.sh");

/// Never with the owner's identity, whatever this shell has set: a test signs ad hoc, and raises no keychain prompt. Says what it printed when it fails.
fn script(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(SCRIPT).args(args).env_remove("RECTO_SIGN_IDENTITY").output().unwrap()
}

fn script_ok(args: &[&std::ffi::OsStr]) {
    let output = script(args);
    assert!(
        output.status.success(),
        "render-helper.sh {args:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// This build's `recto-headings` in a helper of its own, laid out and checked, in a temp folder that goes with it.
struct Bundle {
    scratch: recto_testpdf::TempDir,
}

impl Bundle {
    fn new(name: &str) -> Self {
        let scratch = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), &format!("recto-structure-{name}-"));
        let bundle = Self { scratch };
        script_ok(&["--layout-structure".as_ref(), bundle.contents().as_os_str(), EXE.as_ref()]);
        script_ok(&["--check".as_ref(), bundle.helper().as_os_str()]);
        bundle
    }

    fn contents(&self) -> PathBuf {
        self.scratch.join("Host.app/Contents")
    }

    fn helper(&self) -> PathBuf {
        self.contents().join("Helpers/RectoStructure.app")
    }

    fn exe(&self) -> PathBuf {
        self.helper().join("Contents/MacOS/recto-headings")
    }
}

/// A three-page document.
fn three_pages() -> TempPdf {
    let all: Vec<Page> = (1..=3).map(|number| Page::new(text_lines(&[&format!("page {number}")]))).collect();
    TempPdf::new("three.pdf", &pages(&all))
}

/// A helper started on `exe` with `document` on its descriptor 3, its stdout read by line.
struct Started {
    child: Helper,
    stdin: std::fs::File,
    stdout: BufReader<std::fs::File>,
}

fn start(exe: &Path, document: &Path) -> Started {
    let file = std::fs::File::open(document).unwrap();
    let helper::Spawned { child, stdin, stdout } =
        helper::spawn_with_document(exe, &[("MallocLargeCache", std::ffi::OsStr::new("0"))], Priority::Inherited, &file).unwrap();
    Started {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    }
}

impl Started {
    fn line(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("not a line of JSON ({error}): {line:?}"))
    }

    /// Sends one job and takes everything it says to the end, as a host does; the hello is the first line.
    fn job_to_the_end(mut self, job: &Value) -> (Vec<Value>, Option<ExitStatus>) {
        let _ = writeln!(self.stdin, "{job}");
        // Stdin stays open until the helper has ended: its closing is the helper's sign to go, whatever it's doing.
        let mut said = String::new();
        let _ = self.stdout.read_to_string(&mut said);
        drop(self.stdin);
        let status = self.child.wait().ok();
        (said.lines().map(|line| serde_json::from_str(line).unwrap_or(Value::Null)).collect(), status)
    }
}

/// Failure: the helper in its sandbox reads its document by a name (a path the sandbox would refuse), or isn't sandboxed at all and says so falsely, or answers a different number than the
/// same helper without the walls: the descriptor isn't the document, or the walls changed what it reads. The kernel says the helper is sandboxed (and this process isn't), that it may not
/// read the document by name, and that it may read its own bundle, which shows the first isn't a denial of everything. The count comes out of the descriptor all the same.
#[test]
fn the_walled_helper_reads_its_descriptor_and_nothing_by_name() {
    let bundle = Bundle::new("count");
    let pdf = three_pages();
    let mut walled = start(&bundle.exe(), &pdf.0);
    assert_eq!(walled.line(), json!({ "state": "hello", "walled": true }));
    let pid = walled.child.id();
    assert_eq!(
        (pid_sandboxed(pid), pid_sandboxed(std::process::id())),
        (1, 0),
        "the helper isn't in a sandbox, or this process is"
    );
    assert_eq!(pid_may_read(pid, &pdf.0), 1, "the sandbox lets the helper read the document by its name");
    assert_eq!(
        pid_may_read(pid, &bundle.helper().join("Contents/Info.plist")),
        0,
        "the control: its own bundle is readable, so the denial above is the document's"
    );
    let (said, status) = walled.job_to_the_end(&json!({ "kind": "count" }));
    assert!(status.is_some_and(|status| status.success()) && !said.is_empty(), "{said:?} {status:?}");
    assert_eq!((said[0]["state"].as_str(), said[0]["pages"].as_u64()), (Some("counted"), Some(3)), "{said:?}");
    let (bare_said, _) = start(Path::new(EXE), &pdf.0).job_to_the_end(&json!({ "kind": "count" }));
    assert_eq!(bare_said[1]["pages"], said[0]["pages"], "walled {said:?} against bare {bare_said:?}");
}

/// Failure: the walled helper, given a job that names a file, opens it by name (the sandbox would refuse, but the refusal is the sandbox's, not the helper's) or reads it as if there were
/// no walls. It answers `unsupported_job` without touching the file.
#[test]
fn a_walled_helper_refuses_a_job_that_names_a_path() {
    let bundle = Bundle::new("names");
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (said, status) = start(&bundle.exe(), &pdf.0).job_to_the_end(&json!({ "file": pdf.0, "key": key.hex() }));
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(said[1], json!({ "state": "failed", "code": "unsupported_job" }));
    assert!(status.is_some_and(|status| status.success()), "{status:?}");
}

/// Failure: the walls change what the helper finds, because something it needs is read by a name (an encoding, a CMap, a font table, a path distillPDF opens again) and the sandbox refuses it,
/// so a document gets other contents, or none, behind the walls than in a test: and the session, which only ever starts the walled one, would never have been tested on what it gets. The
/// headings job is run walled and bare on three documents (plain, in heavy type, and one with a hidden heading), and the blocks job on the blocks document; every line must be the same but for
/// the two numbers that are a run's own.
#[test]
fn the_walled_helper_finds_what_the_bare_one_does() {
    let bundle = Bundle::new("same");
    let hidden = recto_testpdf::HIDDEN_HEADINGS[0];
    let headings = json!({ "kind": "headings", "memory_limit": 512u64 << 20 });
    let blocks = json!({ "kind": "blocks", "memory_limit": 512u64 << 20, "scan": "typography" });
    let documents = [
        (&headings, TempPdf::new("plain.pdf", &recto_testpdf::headings_document())),
        (&headings, TempPdf::new("bold.pdf", &recto_testpdf::bold_headings_document())),
        (
            &headings,
            TempPdf::new("hidden.pdf", &recto_testpdf::headings_document_with(hidden.1, [0.0, 0.0, 612.0, 792.0], "")),
        ),
        (&blocks, TempPdf::new("blocks.pdf", &recto_testpdf::blocks::blocks_document())),
    ];
    for (job, pdf) in &documents {
        let (walled, status) = start(&bundle.exe(), &pdf.0).job_to_the_end(job);
        let (bare, _) = start(Path::new(EXE), &pdf.0).job_to_the_end(job);
        assert!(status.is_some_and(|status| status.success()), "{status:?}");
        assert_eq!((&walled[0]["walled"], &bare[0]["walled"]), (&json!(true), &json!(false)));
        let own = |lines: &[Value]| -> Vec<Value> {
            lines[1..]
                .iter()
                .cloned()
                .map(|mut line| {
                    if let Some(object) = line.as_object_mut() {
                        object.remove("distilled_ms");
                        object.remove("helper_peak_bytes");
                    }
                    line
                })
                .collect()
        };
        assert!(walled.len() > 2, "it found something to compare: {walled:?}");
        assert_eq!(own(&walled), own(&bare), "{:?}", pdf.0);
    }
}

/// Failure: a helper signed without the sandbox (a build script's mistake) reads a hostile document with no walls, and the release's check lets it through. After it's signed again
/// with no entitlements it exits with the code for a missing sandbox (77) before it says anything or reads a byte, and `--check` fails it.
#[test]
fn a_structure_helper_signed_without_the_sandbox_reads_nothing() {
    let bundle = Bundle::new("unsigned");
    let signed = Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(bundle.helper()).output().unwrap();
    assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
    let pdf = three_pages();
    let (said, status) = start(&bundle.exe(), &pdf.0).job_to_the_end(&json!({ "kind": "count" }));
    assert_eq!(
        (said.is_empty(), status.and_then(|status| status.code())),
        (true, Some(77)),
        "{said:?} {status:?}"
    );
    let check = script(&["--check".as_ref(), bundle.helper().as_os_str()]);
    assert!(!check.status.success(), "the check passed a helper with no sandbox");
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("entitlements"),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// Failure: the app's check (the release's, and `build.sh`'s) lets through an app with no structure helper, which then makes generated contents with nothing behind it, or one with a
/// bare `MacOS/recto-headings` beside the app's own programs, which the app could start with no walls. The check passes the whole app, fails it without `RectoStructure.app`, and fails it
/// with the bare program, naming it.
#[test]
fn the_apps_check_requires_the_structure_helper_and_refuses_a_bare_copy() {
    let scratch = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-structure-app-");
    let contents = scratch.join("Host.app/Contents");
    // The renderer helper's library: the real PDFium when it's fetched, else this program again, which is all the check reads of it (that it is signed with the helper).
    let pdfium = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/pdfium/libpdfium.dylib");
    let library = if pdfium.exists() { pdfium } else { scratch.join("libpdfium.dylib") };
    if !library.exists() {
        std::fs::copy(EXE, &library).unwrap();
    }
    std::fs::create_dir_all(&contents).unwrap();
    script_ok(&["--layout".as_ref(), contents.as_os_str(), EXE.as_ref(), library.as_os_str()]);
    script_ok(&["--layout-structure".as_ref(), contents.as_os_str(), EXE.as_ref()]);
    let app = scratch.join("Host.app");
    script_ok(&["--check".as_ref(), app.as_os_str()]);

    let helper = contents.join("Helpers/RectoStructure.app");
    let aside = scratch.join("aside.app");
    std::fs::rename(&helper, &aside).unwrap();
    let without = script(&["--check".as_ref(), app.as_os_str()]);
    assert!(!without.status.success(), "the check passed an app with no structure helper");
    std::fs::rename(&aside, &helper).unwrap();

    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::copy(EXE, contents.join("MacOS/recto-headings")).unwrap();
    let bare = script(&["--check".as_ref(), app.as_os_str()]);
    assert!(!bare.status.success(), "the check passed an app with a bare structure program");
    let said = String::from_utf8_lossy(&bare.stderr);
    assert!(said.contains("MacOS/recto-headings") && said.contains("outside the sandbox"), "{said}");
}
