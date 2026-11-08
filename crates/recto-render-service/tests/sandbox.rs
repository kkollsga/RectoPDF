//! The real worker in the app's sandboxed helper: laid out and signed (ad hoc) by `app/render-helper.sh`, as `app/build.sh` does, and checked by it as the release is. Proven from
//! outside, by asking the kernel about the live worker, and by what comes back from a file that tries to reach out. A stand-in for a compromised worker, which tries the calls
//! themselves, is `recto-platform`'s `tests/walls.rs`.
#![cfg(target_os = "macos")]
// `sandbox_check`, to ask the kernel about another process. Test code only.
#![allow(unsafe_code)]

mod common;

use common::{kill_process, wait_until};
use recto_platform::isolate::Channel;
use recto_platform::isolate::process_memory;
use recto_platform::surface::PixelSurface;
use recto_proto::CharRecord;
use recto_render_service::transport::send_all;
use recto_render_service::{
    DocRenderer, HeavySlot, KillReason, Policy, RenderFailure, ServiceConfig, Surface, TILE_EDGE, TileKind, TileRect, TileRequest, TileResult, WallsDown,
};
use recto_testpdf::{Object, Page, TempPdf, build, form_bomb, hidden_text_page, pages, text_lines};
use std::ffi::{CString, c_char, c_int};
use std::io::Read as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const WORKER: &str = env!("CARGO_BIN_EXE_recto-render-service");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

unsafe extern "C" {
    /// libsandbox, in libSystem, and in no public header: 0 allowed, 1 denied, -1 couldn't tell. Only these tests use it, on processes they started, with a control each time.
    fn sandbox_check(pid: c_int, operation: *const c_char, filter: c_int, ...) -> c_int;
}
const SANDBOX_FILTER_PATH: c_int = 1;

/// Whether `pid` runs under any sandbox at all.
fn sandboxed(pid: u32) -> c_int {
    // SAFETY: a null operation with no filter asks only whether there's a sandbox, and passes nothing to read.
    unsafe { sandbox_check(pid as c_int, std::ptr::null(), 0) }
}

/// Whether `pid` may read `path` by its path: 0 allowed, 1 denied.
fn may_read(pid: u32, path: &Path) -> c_int {
    let path = CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: both strings are NUL-terminated and outlive the call, and the path filter takes one string.
    unsafe { sandbox_check(pid as c_int, c"file-read-data".as_ptr(), SANDBOX_FILTER_PATH, path.as_ptr()) }
}

fn pdfium() -> PathBuf {
    let path = Path::new(REPO).join("target/pdfium/libpdfium.dylib");
    assert!(path.is_file(), "no {}: run tools/pdfium.sh", path.display());
    path
}

/// Never with the owner's identity, whatever this shell has set: a test signs ad hoc, and raises no keychain prompt.
fn helper(args: &[&std::ffi::OsStr]) {
    let output = Command::new(Path::new(REPO).join("app/render-helper.sh"))
        .args(args)
        .env_remove("RECTO_SIGN_IDENTITY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "render-helper.sh {args:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// This build's worker in a helper of its own, in a temp folder that goes with it.
struct Bundle {
    scratch: PathBuf,
}

impl Bundle {
    fn new(name: &str) -> Self {
        let scratch = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-sandbox-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let bundle = Self { scratch };
        let contents = bundle.contents();
        helper(&["--layout".as_ref(), contents.as_os_str(), WORKER.as_ref(), pdfium().as_os_str()]);
        helper(&["--check".as_ref(), bundle.helper().as_os_str()]);
        bundle
    }

    fn contents(&self) -> PathBuf {
        self.scratch.join("Host.app/Contents")
    }

    fn helper(&self) -> PathBuf {
        self.contents().join("Helpers/RectoRender.app")
    }

    fn worker(&self) -> PathBuf {
        self.helper().join("Contents/MacOS/recto-render-service")
    }

    fn config(&self) -> ServiceConfig {
        let mut config = ServiceConfig::for_worker(self.worker()).unwrap();
        assert_eq!(
            config.pdfium_lib,
            self.helper().join("Contents/Frameworks/libpdfium.dylib"),
            "PDFium must come from the helper, which is all the sandbox lets it read"
        );
        config.heavy_slot = HeavySlot::new();
        config
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

fn plain_config() -> ServiceConfig {
    let mut config = ServiceConfig::for_worker(WORKER.into()).unwrap();
    config.heavy_slot = HeavySlot::new();
    config
}

const EDGE: u32 = 512;

fn request(page: u32) -> TileRequest {
    TileRequest {
        page,
        rect: TileRect {
            x: 0,
            y: 0,
            width: EDGE,
            height: EDGE,
        },
        scale: 1.0,
        stride: EDGE * 4,
        generation: 0,
    }
}

/// Draws a tile of a page and returns its pixels.
fn draw(doc: &mut DocRenderer, page: u32) -> (TileKind, Vec<u8>) {
    let surface = Surface::create((EDGE * EDGE * 4) as usize).unwrap();
    let kind = match doc.render_tile(&request(page), &surface).unwrap() {
        TileResult::Done(tile) => tile.kind,
        TileResult::Cancelled { .. } => panic!("nothing cancelled this"),
    };
    let mut pixels = vec![0u8; (EDGE * EDGE * 4) as usize];
    assert!(surface.copy_out(0, &mut pixels));
    (kind, pixels)
}

fn text_of(doc: &mut DocRenderer, page: u32) -> (String, Vec<bool>, bool) {
    let characters = doc.page_characters(page, true, true).unwrap();
    let text = characters.chars.iter().map(|c| c.ch()).collect();
    let flags = characters.chars.iter().map(|c| c.has(CharRecord::NO_INK)).collect();
    (text, flags, characters.hidden_checked)
}

/// Failure: the worker in the shipped layout runs outside the sandbox, or can read a file by its path, and only its own word says otherwise. The kernel is asked about the live process,
/// with this test process as the control, and the helper's own library as a path the sandbox must allow, so a check that always says "denied" can't pass.
#[test]
fn the_bundled_worker_runs_sandboxed_and_cant_read_by_path() {
    let bundle = Bundle::new("proof");
    let file = TempPdf::new("proof.pdf", &pages(&[Page::new(text_lines(&["Drawn in the sandbox"]))]));
    let canary = bundle.scratch.join("canary.txt");
    std::fs::write(&canary, "canary").unwrap();
    let mut doc = DocRenderer::new(&file.0, None, bundle.config(), Policy::default()).unwrap();
    draw(&mut doc, 0);
    let pid = doc.worker_pid().expect("a live worker");
    assert_eq!(doc.worker_walled(), Some(true), "its hello didn't say its walls were up");
    assert_eq!(doc.stats().walled_spawns, doc.stats().spawns);
    let me = std::process::id();
    assert_eq!(sandboxed(me), 0, "this test process isn't sandboxed, so the question is a real one");
    assert_eq!(sandboxed(pid), 1, "the bundled worker isn't sandboxed");
    assert_eq!(may_read(me, &canary), 0);
    assert_eq!(may_read(pid, &canary), 1, "the worker may read a file it wasn't given");
    assert_eq!(
        may_read(pid, &file.0),
        1,
        "the worker may read the document by its path, when it only ever needs the descriptor it was given"
    );
    assert_eq!(
        may_read(pid, &bundle.helper().join("Contents/Frameworks/libpdfium.dylib")),
        0,
        "the check can't tell allowed from denied"
    );
    let plain = DocRenderer::new(&file.0, None, plain_config(), Policy::default());
    let mut plain = plain.unwrap();
    draw(&mut plain, 0);
    assert_eq!(
        sandboxed(plain.worker_pid().unwrap()),
        0,
        "a bare development worker isn't sandboxed, so the sandbox comes from the helper's signing"
    );
    assert_eq!(plain.worker_walled(), Some(false), "a bare worker said it was walled");
}

/// Failure: a helper signed without the sandbox, the mistake a build could make, draws a hostile file anyway and nobody can tell. The worker tests its wall before any document
/// reaches it and refuses, and the host says plainly that the app is signed wrong.
#[test]
fn a_bundled_worker_with_no_sandbox_refuses_to_read_anything() {
    let bundle = Bundle::new("unsigned");
    let output = Command::new("/usr/bin/codesign").args(["-f", "-s", "-"]).arg(bundle.helper()).output().unwrap();
    assert!(output.status.success());
    let file = TempPdf::new("refused.pdf", &pages(&[Page::new(text_lines(&["Never drawn"]))]));
    let mut doc = DocRenderer::new(&file.0, None, bundle.config(), Policy::default()).unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::WallsDown(WallsDown::NotSandboxed));
    assert!(RenderFailure::WallsDown(WallsDown::NotSandboxed).message().contains("signed wrong"));
}

/// Failure: the sandbox changes what's drawn or read: a font it can't reach is silently substituted, which the spike's first profile did, or the hidden-text check comes out
/// differently. The page's only font is Helvetica, not embedded, so it comes from the system. Compared with a bare worker on this machine, so no pixel is pinned.
#[test]
fn drawing_text_and_the_hidden_check_are_the_same_in_the_sandbox() {
    let bundle = Bundle::new("same");
    let file = TempPdf::new(
        "same.pdf",
        &pages(&[
            Page::new(text_lines(&["Words in a font this file doesn't carry", "AVA Wax fi fl"])),
            Page::new(hidden_text_page()),
        ]),
    );
    let mut ours = DocRenderer::new(&file.0, None, bundle.config(), Policy::default()).unwrap();
    let mut theirs = DocRenderer::new(&file.0, None, plain_config(), Policy::default()).unwrap();
    let (kind, pixels) = draw(&mut ours, 0);
    assert_ne!(kind, TileKind::Blank { rgb: 0xFF_FF_FF }, "the page came out blank");
    assert!(pixels.iter().any(|byte| *byte != 255), "the page came out blank");
    assert!(pixels == draw(&mut theirs, 0).1, "the sandbox changed the picture");
    let (text, flags, checked) = text_of(&mut ours, 1);
    assert!(checked, "the hidden-text check didn't run in the sandbox");
    assert!(text.contains("VISIBLE-SENTINEL") && text.contains("HIDDEN-WHITE"), "{text:?}");
    assert!(flags.iter().any(|flag| *flag), "nothing was flagged as hidden");
    assert_eq!(
        (text, flags, checked),
        text_of(&mut theirs, 1),
        "the sandbox changed the text or the hidden-text check"
    );
}

/// Failure: a sandboxed worker that crashes or is killed, mid-page or between pages, leaves its document broken, or its replacement starts outside the sandbox.
#[test]
fn a_crash_or_kill_in_the_sandbox_is_recovered() {
    let bundle = Bundle::new("kill");
    let file = TempPdf::new("kill.pdf", &form_bomb());
    let policy = Policy {
        memory_limit_bytes: 2 << 30,
        page_deadline: Duration::from_secs(60),
        ..Policy::default()
    };
    let mut doc = DocRenderer::new(&file.0, None, bundle.config(), policy).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    let first = doc.worker_pid().unwrap();
    // The bomb runs until something stops it, so this kill lands inside the page whenever it arrives.
    let killer = std::thread::spawn(move || {
        wait_until("the bomb to start growing", || {
            process_memory(first).is_some_and(|memory| memory.footprint_bytes > 32 * 1024 * 1024)
        });
        kill_process(first);
    });
    let surface = Surface::create((EDGE * EDGE * 4) as usize).unwrap();
    let failure = doc.render_tile(&request(0), &surface).unwrap_err();
    killer.join().unwrap();
    assert_eq!(
        failure,
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Crashed
        }
    );
    assert!(
        matches!(draw(&mut doc, 1).0, TileKind::Grey | TileKind::Colour),
        "the next page draws in a new worker"
    );
    let second = doc.worker_pid().unwrap();
    assert_ne!(second, first);
    assert_eq!(sandboxed(second), 1, "the replacement isn't sandboxed");
    kill_process(second);
    wait_until("the killed worker to be gone", || process_memory(second).is_none());
    assert!(
        matches!(draw(&mut doc, 1).0, TileKind::Grey | TileKind::Colour),
        "a page draws after a kill between pages"
    );
    assert_eq!(sandboxed(doc.worker_pid().unwrap()), 1);
}

/// A file that asks, every way PDF offers, for a file by its path and for the network: external streams for a page's contents, an image and a font, all naming a canary file that is
/// itself a page of text, a launch, an import and a script on opening, links, a remote go-to and a form that submits, all to the canary or to a listener of the test's.
fn reaching_pdf(canary: &Path, port: u16) -> Vec<u8> {
    let canary = canary.display();
    let url = format!("http://127.0.0.1:{port}/leak");
    let stream = |dict: String, data: &[u8]| Object::Stream { dict, data: data.to_vec() };
    build(&[
        // 1: opening the file launches the canary, imports it, runs a script that fetches and reads, and opens the link.
        Object::Dict(format!(
            "<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /Launch /F ({canary}) /Next [<< /S /ImportData /F ({canary}) >> << /S /JavaScript /JS (app.launchURL\\('{url}'\\); util.readFileIntoStream\\('{canary}'\\);) >> << /S /URI /URI ({url}) >>] >> >>"
        )),
        Object::Dict("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".into()),
        Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
        // 4: a page drawing an external image and text in a font from an external file, with a link, a remote go-to, a submitting form and a launch on opening the page.
        Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 3 0 R /F2 8 0 R >> /XObject << /Leak 7 0 R >> >> \
             /Annots [<< /Type /Annot /Subtype /Link /Rect [0 0 612 792] /A << /S /URI /URI ({url}) >> >> << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /GoToR /F ({canary}) /D [0 /Fit] >> >> \
             << /Type /Annot /Subtype /Widget /FT /Btn /T (go) /Rect [20 20 40 40] /A << /S /SubmitForm /F << /FS /URL /F ({url}) >> >> >>] /AA << /O << /S /Launch /F ({canary}) >> >> >>"
        )),
        stream(String::new(), b"q 100 0 0 100 72 400 cm /Leak Do Q BT /F2 12 Tf 72 300 Td (Typed in a font from a file) Tj ET BT /F1 12 Tf 72 700 Td (The page that reaches out) Tj ET"),
        // 6: a page whose only contents are the canary file.
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 10 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into()),
        stream(format!("/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceGray /F ({canary})"), b""),
        Object::Dict("<< /Type /Font /Subtype /TrueType /BaseFont /Leak /FirstChar 32 /LastChar 126 /FontDescriptor 9 0 R >>".into()),
        Object::Dict("<< /Type /FontDescriptor /FontName /Leak /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 11 0 R >>".into()),
        stream(format!("/F ({canary})"), b""),
        stream(format!("/F ({canary})"), b""),
    ])
}

/// Failure: a hostile file gets the sandboxed worker to read a file by its path, or to reach the network: what it fetched shows up in the page's text, or the listener sees a
/// connection. PDFium without V8 or XFA isn't meant to follow any of these, and a bare worker given this file follows none of them either (tried 2026-11-08), so this is
/// that nothing changed in the shipped arrangement, with the canary and the listener where a leak would show. What proves the wall is the kernel's answer about the live worker, above. The calls themselves, from a compromised worker, are `recto-platform`'s `tests/walls.rs`.
#[test]
fn a_hostile_pdf_cant_reach_a_file_or_the_network_from_the_sandbox() {
    let bundle = Bundle::new("hostile");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let canary = bundle.scratch.join("canary.pdfpart");
    std::fs::write(&canary, "BT /F1 12 Tf 72 600 Td (CANARY-LEAKED) Tj ET").unwrap();
    let file = TempPdf::new("reaching.pdf", &reaching_pdf(&canary, listener.local_addr().unwrap().port()));
    let mut doc = DocRenderer::new(&file.0, None, bundle.config(), Policy::default()).unwrap();
    let count = doc.page_count().unwrap();
    assert_eq!(count, 2);
    let (first_page, _, _) = text_of(&mut doc, 0);
    assert!(
        first_page.contains("The page that reaches out"),
        "the hostile page wasn't read at all, so this proves nothing: {first_page:?}"
    );
    for page in 0..count {
        let _ = doc.render_tile(&request(page), &Surface::create((EDGE * EDGE * 4) as usize).unwrap());
        if let Ok(characters) = doc.page_characters(page, true, true) {
            let text: String = characters.chars.iter().map(|c| c.ch()).collect();
            assert!(!text.contains("CANARY"), "page {} read the canary file: {text:?}", page + 1);
        }
        let _ = doc.image_boxes(page, 0);
    }
    assert!(
        doc.worker_pid().is_some_and(|pid| sandboxed(pid) == 1),
        "the worker that read it isn't sandboxed"
    );
    match listener.accept() {
        Ok((mut stream, _)) => {
            let mut seen = Vec::new();
            let _ = stream.read_to_end(&mut seen);
            panic!("the file reached the network: {:?}", String::from_utf8_lossy(&seen));
        }
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}"),
    }
}

/// Failure: the tile path the app's pages take, an IOSurface sent over the Mach link, breaks once the worker has dropped its Mach rights, so pages draw in tests and stay blank on
/// screen. One tile into a `PixelSurface` through the helper's worker, which has dropped them before its hello.
#[test]
fn a_surface_tile_arrives_from_the_sandboxed_worker_after_the_mach_drop() {
    let bundle = Bundle::new("surface");
    let file = TempPdf::new(
        "surface.pdf",
        &pages(&[Page::new(text_lines(&["Words drawn into a surface from inside the sandbox"]))]),
    );
    let mut doc = DocRenderer::new(&file.0, None, bundle.config(), Policy::default()).unwrap();
    let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let request = TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: TILE_EDGE,
            height: TILE_EDGE,
        },
        scale: 2.0,
        stride: surface.stride(),
        generation: 1,
    };
    assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    assert_eq!(doc.worker_walled(), Some(true), "the worker that drew it hadn't put its walls up");
    let drawn: Vec<u8> = surface
        .with_pixels(|pixels| pixels[..(surface.stride() * TILE_EDGE) as usize].to_vec())
        .unwrap();
    assert!(drawn.chunks(4).any(|pixel| pixel[0] < 200), "nothing arrived in the surface");
}

const STAND_IN: &str = "RECTO_SANDBOX_STAND_IN";

/// Not a test: a stand-in worker that says hello with its walls down. Does nothing unless `RECTO_SANDBOX_STAND_IN` says this process was started as one.
#[test]
fn stand_in_worker() {
    if std::env::var_os(STAND_IN).is_none() {
        return;
    }
    let channel = Channel::inherited().unwrap();
    send_all(
        &channel,
        &recto_proto::encode_hello(recto_proto::Hello {
            pid: std::process::id(),
            walled: false,
        }),
        &[],
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

/// Failure: a host takes a worker from an app's helper that says its walls aren't up, as a release would take anything, and hands it a document. This stand-in sits where a helper's
/// worker does and says hello with the bit clear: it's killed, and the host says the app is signed wrong.
#[test]
fn a_worker_from_a_helper_that_says_it_isnt_walled_is_refused() {
    let bundle = Bundle::new("stand-in");
    let fake = bundle.scratch.join("Fake.app/Contents/MacOS/recto-render-service");
    std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &fake).unwrap();
    std::fs::copy(pdfium(), fake.with_file_name("libpdfium.dylib")).unwrap();
    let mut config = ServiceConfig::for_worker(fake).unwrap();
    config.heavy_slot = HeavySlot::new();
    config.extra_env = vec![(STAND_IN.into(), "1".into())];
    config.replace_args = Some(
        ["--exact", "stand_in_worker", "--nocapture", "--test-threads=1"]
            .into_iter()
            .map(Into::into)
            .collect(),
    );
    let file = TempPdf::new("refused.pdf", &pages(&[Page::new(text_lines(&["Never sent"]))]));
    let mut doc = DocRenderer::new(&file.0, None, config, Policy::default()).unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::WallsDown(WallsDown::NotSandboxed));
    assert!(!doc.has_worker(), "the refused worker was kept");
}

const INSIDE: &str = "RECTO_SANDBOX_INSIDE_ANOTHER";

/// Failure: `recto` run inside an agent's sandbox can't start the renderer, and says the app is signed wrong, sending the user to reinstall for nothing. This test runs itself again
/// under `sandbox-exec` with a profile that allows everything, which is sandboxed all the same, and there the helper's worker can't start: the system ends it before its first line.
/// The positive check can't be fooled the other way either: a bare process inside that seatbelt is sandboxed and has no container, so it doesn't read as walled.
#[test]
fn inside_another_sandbox_the_renderer_says_so() {
    if let Some(worker) = std::env::var_os(INSIDE) {
        assert!(recto_platform::wall::in_a_sandbox(), "sandbox-exec didn't sandbox this run");
        assert!(!recto_platform::wall::sandbox_in_force(), "an inherited seatbelt read as the App Sandbox");
        let mut config = ServiceConfig::for_worker(worker.into()).unwrap();
        config.heavy_slot = HeavySlot::new();
        let file = TempPdf::new("inside.pdf", &pages(&[Page::new(text_lines(&["Never drawn"]))]));
        let mut doc = DocRenderer::new(&file.0, None, config, Policy::default()).unwrap();
        let failure = doc.page_count().unwrap_err();
        assert_eq!(failure, RenderFailure::WallsDown(WallsDown::InsideAnotherSandbox));
        assert!(
            failure.message().contains("from inside another sandbox") && !failure.message().contains("signed wrong"),
            "{}",
            failure.message()
        );
        return;
    }
    let bundle = Bundle::new("inside");
    let output = Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "inside_another_sandbox_the_renderer_says_so", "--nocapture", "--test-threads=1"])
        .env(INSIDE, bundle.worker())
        .output()
        .unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success() && said.contains("1 passed"), "inside the seatbelt: {said}");
}
