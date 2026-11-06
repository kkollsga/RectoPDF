//! `recto`: a thin client of the control API. Until the app's socket exists, it runs a headless session for tests and QA.

use recto_agent::{Level, envelope, exit, parse, run};
use recto_session::Session;
use serde_json::json;
use std::io::BufRead;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
recto: RectoPDF's command line

  recto inspect <file.pdf>                       Facts and contents as JSON
  recto session                                  Headless session: one JSON command per line on stdin
  recto budget tabs --file <file.pdf> --count N  Measure memory and handles with N tabs of a real PDF";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["inspect", file] => inspect(Path::new(file)),
        ["session"] => session(),
        ["budget", "tabs", "--file", file, "--count", count] => match count.parse() {
            Ok(count) => budget_tabs(Path::new(file), count),
            Err(_) => usage(),
        },
        _ => usage(),
    };
    std::process::exit(code);
}

fn usage() -> i32 {
    eprintln!("{USAGE}");
    2
}

fn inspect(path: &Path) -> i32 {
    match recto_core::inspect(path) {
        Ok(facts) => {
            let toc: Vec<_> = facts
                .toc
                .entries()
                .iter()
                .map(|e| json!({ "title": facts.toc.title(e), "depth": e.depth, "page": (e.page != recto_core::NONE).then(|| e.page + 1) }))
                .collect();
            let data = json!({
                "pages": facts.page_count,
                "bytes": facts.byte_len,
                "encrypted": facts.encrypted,
                "version": &*facts.version,
                "untrusted": { "title": facts.title.as_deref(), "contents": toc },
            });
            println!("{}", envelope(&Ok(data)));
            exit::OK
        }
        Err(error) => {
            let data = json!({ "v": recto_agent::VERSION, "ok": false, "error": { "code": error.code(), "message": error.message() } });
            println!("{data}");
            exit::FAILED
        }
    }
}

/// Reads commands line by line and answers each one. This is what QA scripts and blind agent tests drive.
fn session() -> i32 {
    let mut session = Session::default();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let result = parse(&line).and_then(|command| run(&mut session, Level::Full, &command));
        println!("{}", envelope(&result));
    }
    exit::OK
}

/// Opens one real PDF as many separate tabs and reports the whole process's memory and handles.
/// Each tab gets its own link in a temporary folder, because opening the same path twice reuses its tab.
fn budget_tabs(file: &Path, count: usize) -> i32 {
    let Ok(file) = file.canonicalize() else {
        eprintln!("That file couldn't be found.");
        return exit::NOT_FOUND;
    };
    let dir = std::env::temp_dir().join(format!("recto-budget-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("Couldn't make a temporary folder.");
        return exit::FAILED;
    }
    let code = measure(&file, &dir, count);
    let _ = std::fs::remove_dir_all(&dir);
    code
}

fn measure(file: &Path, dir: &Path, count: usize) -> i32 {
    let mut session = Session::default();
    let start_footprint = recto_platform::footprint_bytes();
    let start_handles = recto_platform::open_handle_count();
    let started = std::time::Instant::now();
    let mut peak_note = Vec::new();
    for i in 0..count {
        let link: PathBuf = dir.join(format!("tab-{i:05}.pdf"));
        if make_link(file, &link).is_err() {
            eprintln!("Couldn't make a test link.");
            return exit::FAILED;
        }
        if let Err(error) = session.open(&link) {
            eprintln!("{}", error.message());
            return exit::FAILED;
        }
        if i + 1 == 2 || i + 1 == count {
            peak_note.push(json!({ "tabs": i + 1, "footprint_bytes": recto_platform::footprint_bytes() }));
        }
    }
    let per_open_ms = started.elapsed().as_secs_f64() * 1000.0 / count.max(1) as f64;
    let data = json!({
        "file_bytes": std::fs::metadata(file).map(|m| m.len()).ok(),
        "tabs": count,
        "start_footprint_bytes": start_footprint,
        "samples": peak_note,
        "start_handles": start_handles,
        "end_handles": recto_platform::open_handle_count(),
        "ms_per_open": per_open_ms,
    });
    println!("{}", envelope(&Ok(data)));
    exit::OK
}

#[cfg(unix)]
fn make_link(file: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(file, link)
}

#[cfg(not(unix))]
fn make_link(file: &Path, link: &Path) -> std::io::Result<()> {
    std::fs::hard_link(file, link)
}
