//! The renderer's commands end to end, through the real `recto` binary and a real worker: what an agent sees.

mod common;

use common::State;
use recto_agent::Level;
use recto_testpdf::{Page, TempPdf, hidden_text_page, pages, text_lines};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;

/// One command on one file, in the in-process route, at the top level. Levels have their own tests.
fn recto(args: &[&str]) -> (Value, i32) {
    let state = State::at(Level::Full);
    state.json(args)
}

fn doc() -> TempPdf {
    TempPdf::new(
        "doc.pdf",
        &pages(&[
            Page::new(text_lines(&["First page words", "more words"])),
            Page::new(text_lines(&["Second page"])),
        ]),
    )
}

fn path_of(file: &TempPdf) -> &str {
    file.0.to_str().unwrap()
}

/// Failure: `render` reports success but the PNG isn't a picture of the size it says, or it's left half-written.
#[test]
fn render_writes_a_png_of_the_size_it_reports() {
    let file = doc();
    let out = file.0.with_file_name("page.png");
    let (reply, code) = recto(&["local", path_of(&file), "render", "--page", "1", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["ok"], true);
    assert_eq!((&reply["data"]["width"], &reply["data"]["height"]), (&Value::from(1224), &Value::from(1584)));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(bytes[..4], [0x89, b'P', b'N', b'G']);
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1224);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 1584);
    assert!(!out.with_extension("png.partial").exists(), "the partial file was left behind");
    std::fs::remove_file(&out).unwrap();
}

/// Failure: a page wider and taller than one strip comes out with rows missing or out of step, or a whole-page draw uses a surface the size of the page. Drawn in strips, a
/// large page is the size it says, and no surface is larger than a strip.
#[test]
fn a_large_page_is_drawn_in_strips() {
    let big = Page {
        content: text_lines(&["Large page"]),
        page_extra: String::new(),
        media: (3000.0, 3000.0),
    };
    let file = TempPdf::new("big.pdf", &pages(&[big]));
    let out = file.0.with_file_name("big.png");
    let (reply, code) = recto(&["local", path_of(&file), "render", "--page", "1", "--scale", "2", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!((&reply["data"]["width"], &reply["data"]["height"]), (&Value::from(6000), &Value::from(6000)));
    std::fs::remove_file(&out).unwrap();
    let (budget, _) = recto(&["local", path_of(&file), "budget", "render", "--count", "1", "--scale", "2"]);
    assert!(
        budget["data"]["surface_bytes"].as_u64().unwrap() <= recto_render_service::STRIP_MAX_BYTES,
        "{budget}"
    );
    assert!(
        budget["data"]["peak_footprint_bytes"].as_u64().unwrap() < 144 * 1024 * 1024,
        "the page's 144 MiB of pixels shouldn't all be in memory at once: {budget}"
    );
}

/// Failure: a bad page or output path writes something anyway, or overwrites the PDF itself through `--out`, breaking the one-writer rule.
#[test]
fn render_refuses_what_it_shouldnt_write() {
    let file = doc();
    let before = std::fs::read(&file.0).unwrap();
    let (reply, code) = recto(&["local", path_of(&file), "render", "--page", "1", "--out", path_of(&file)]);
    assert_eq!(reply["error"]["code"], "bad_output");
    assert_ne!(code, 0);
    assert_eq!(std::fs::read(&file.0).unwrap(), before, "the PDF was touched");
    let out = file.0.with_file_name("no.png");
    let (reply, code) = recto(&["local", path_of(&file), "render", "--page", "99", "--out", out.to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "no_such_page");
    assert_eq!(code, recto_agent::exit::NOT_FOUND);
    assert!(!out.exists(), "a failed render left a file");
    let (reply, code) = recto(&["local", path_of(&file), "render", "--page", "0", "--out", out.to_str().unwrap()]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("usage"), recto_agent::exit::USAGE),
        "pages count from 1"
    );
}

/// Failure: a word broken across a line can't be found in `text`, because the agent gets PDFium's raw marker rather than the normalised form.
#[test]
fn text_is_normalised_for_search() {
    let file = TempPdf::new(
        "hyphen.pdf",
        &pages(&[Page::new(text_lines(&[
            "this is very valu-",
            "able to know and it's",
            "simulation-",
            "based too",
        ]))]),
    );
    let (reply, code) = recto(&["local", path_of(&file), "text", "--page", "1"]);
    assert_eq!(code, 0, "{reply}");
    let text = reply["data"]["untrusted"]["text"].as_str().unwrap();
    assert!(text.contains("valuable"), "{text:?}");
    assert!(text.contains("simulation-based"), "{text:?}");
    assert!(!text.contains('\u{FFFE}') && !text.contains('\r'), "{text:?}");
}

/// Failure: hidden text reaches the agent unflagged, or is silently dropped. The flag must be up and the text still there, labelled untrusted.
#[test]
fn text_flags_hidden_text_without_dropping_it() {
    let file = TempPdf::new("hidden.pdf", &pages(&[Page::new(hidden_text_page())]));
    let (reply, code) = recto(&["local", path_of(&file), "text", "--page", "1"]);
    assert_eq!(code, 0, "{reply}");
    let data = &reply["data"];
    assert_eq!(data["hidden_text"]["checked"], true);
    assert_eq!(data["hidden_text"]["flagged"], true);
    assert!(data["hidden_text"]["chars"].as_u64().unwrap() > 50);
    let text = data["untrusted"]["text"].as_str().unwrap();
    assert!(
        text.contains("HIDDEN-WHITE") && text.contains("VISIBLE-SENTINEL"),
        "nothing may be dropped: {text:?}"
    );
    assert!(!data["untrusted"]["hidden_runs"].as_array().unwrap().is_empty());
}

/// Failure: an ordinary page reports hidden text, so the flag stops meaning anything to an agent.
#[test]
fn an_ordinary_page_isnt_flagged() {
    let file = doc();
    let (reply, _) = recto(&["local", path_of(&file), "text", "--page", "1"]);
    assert_eq!(reply["data"]["hidden_text"]["checked"], true);
    assert_eq!(reply["data"]["hidden_text"]["flagged"], false);
}

/// Failure: `budget render` misses a number the budget depends on, or reports nothing about an idle worker.
#[test]
fn budget_render_reports_idle_peak_and_retirement() {
    let file = doc();
    let (reply, code) = recto(&["local", path_of(&file), "budget", "render", "--count", "2", "--samples"]);
    assert_eq!(code, 0, "{reply}");
    let data = &reply["data"];
    assert!(data["idle_footprint_bytes"].as_u64().unwrap() > 0);
    assert!(data["peak_footprint_bytes"].as_u64().unwrap() >= data["idle_footprint_bytes"].as_u64().unwrap());
    assert_eq!(data["pages_drawn"], 2);
    assert_eq!(data["pages_failed"], 0);
    assert_eq!(data["retirements"], 0);
    assert_eq!(data["spawns"], 1);
    assert_eq!(data["samples"].as_array().unwrap().len(), 2);
    let (retired, _) = recto(&["local", path_of(&file), "budget", "render", "--count", "2", "--retire-mib", "1"]);
    assert!(retired["data"]["retirements"].as_u64().unwrap() >= 1, "{retired}");
    let (tiled, _) = recto(&["local", path_of(&file), "budget", "render", "--count", "1", "--tiles", "--fresh"]);
    assert!(tiled["data"]["tiles"]["blank"].as_u64().unwrap() > 0, "most of this page is empty: {tiled}");
}

/// Failure: the agent's session can't reach the new commands, so only the one-shot CLI can use them; or a bad selector crashes.
#[test]
fn the_session_runs_the_same_commands() {
    use std::io::Write;
    let file = doc();
    let out = file.0.with_file_name("session.png");
    let state = State::at(Level::Full);
    let mut child = state.command().arg("session").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let path = serde_json::to_string(path_of(&file)).unwrap();
    let out_json = serde_json::to_string(out.to_str().unwrap()).unwrap();
    writeln!(input, r#"{{"cmd":"open","path":{path}}}"#).unwrap();
    writeln!(input, r##"{{"cmd":"text","tab":"#1","page":2}}"##).unwrap();
    writeln!(input, r##"{{"cmd":"render","tab":"#1","page":1,"out":{out_json}}}"##).unwrap();
    writeln!(input, r##"{{"cmd":"text","tab":"#9","page":1}}"##).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[1]["data"]["untrusted"]["text"].as_str().unwrap().contains("Second page"));
    assert_eq!(lines[2]["ok"], true, "{}", lines[2]);
    assert!(Path::new(&out).exists());
    assert_eq!(lines[3]["error"]["code"], "no_such_tab");
    std::fs::remove_file(&out).unwrap();
}

/// Failure: `budget render --viewport` misses a step, or claims work a scroll didn't need: a scroll draws only what it brings into view, and zoom draws all of it again.
#[test]
fn the_viewport_budget_reports_first_view_scroll_and_zoom() {
    let file = TempPdf::new(
        "view.pdf",
        &pages(&[
            Page::new(recto_testpdf::busy_page()),
            Page::new(recto_testpdf::busy_page()),
            Page::new(recto_testpdf::busy_page()),
        ]),
    );
    for strategy in [&["--strategy", "per_tile"][..], &["--strategy", "region"][..], &[][..]] {
        let mut args = vec!["local", path_of(&file), "budget", "viewport", "--viewport", "1600x1000", "--scale", "2"];
        args.extend_from_slice(strategy);
        let (reply, code) = recto(&args);
        assert_eq!(code, 0, "{reply}");
        let steps = reply["data"]["steps"].as_array().unwrap();
        let names: Vec<&str> = steps.iter().map(|step| step["step"].as_str().unwrap()).collect();
        assert_eq!(names, ["first_view", "scroll", "zoom"]);
        let tiles = |step: usize| steps[step]["tiles"].as_u64().unwrap();
        assert!(tiles(0) > 0);
        assert!(tiles(1) < tiles(0), "a scroll redrew what it already had: {reply}");
        assert!(tiles(2) >= tiles(0), "zooming needs everything again: {reply}");
        assert!(steps.iter().all(|step| step["failed"] == 0 && step["low_res"] == 0), "{reply}");
        assert!(steps[2]["peak_footprint_bytes"].as_u64().unwrap() > 0);
    }
    let (bad, code) = recto(&["local", path_of(&file), "budget", "viewport", "--viewport", "wide"]);
    assert_eq!((bad["error"]["code"].as_str(), code), (Some("usage"), recto_agent::exit::USAGE));
}

/// Failure: a missing file through `text` reads as an internal error, rather than saying the file isn't there.
#[test]
fn a_missing_file_says_so() {
    let (reply, code) = recto(&["local", "/no/such/file.pdf", "text", "--page", "1"]);
    assert_eq!(reply["ok"], false);
    assert_eq!(code, recto_agent::exit::NOT_FOUND);
}
