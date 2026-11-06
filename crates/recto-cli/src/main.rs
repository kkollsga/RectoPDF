//! `recto`: a thin client of the control API. Until the app's socket exists, it runs a headless session for tests and QA.

use recto_agent::{Command, DEFAULT_SCALE, Level, envelope, exit, parse, run};
use recto_session::Session;
use serde_json::json;
use std::io::BufRead;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
recto: RectoPDF's command line

  recto inspect <file.pdf>                       Facts and contents as JSON
  recto session                                  Headless session: one JSON command per line on stdin
  recto budget tabs --file <file.pdf> --count N  Measure memory and handles with N tabs of a real PDF
  recto render <file.pdf> --page N --out <file.png> [--scale S]
                                                 Draw a page to a PNG, in a worker process. Scale 2 by default
  recto text <file.pdf> --page N                 A page's text, normalised for search, with the hidden-text flag
  recto budget render --file <file.pdf> --count N [--first P] [--scale S] [--fresh] [--tiles] [--samples]
                      [--limit-mib M] [--retire-mib M]
                                                 Measure a worker: idle, N pages, peak, retirements";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["inspect", file] => inspect(Path::new(file)),
        ["session"] => session(),
        ["budget", "tabs", "--file", file, "--count", count] => match count.parse() {
            Ok(count) => budget_tabs(Path::new(file), count),
            Err(_) => usage(),
        },
        ["render", file, rest @ ..] => render(Path::new(file), rest),
        ["text", file, rest @ ..] => text(Path::new(file), rest),
        ["budget", "render", rest @ ..] => budget_render(rest),
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

/// The value after a flag.
fn flag<'a>(args: &[&'a str], name: &str) -> Option<&'a str> {
    args.iter().position(|arg| *arg == name).and_then(|at| args.get(at + 1)).copied()
}

/// Whether a flag is there.
fn switch(args: &[&str], name: &str) -> bool {
    args.contains(&name)
}

/// A flag's parsed value: absent is `Some(None)`, unparsable is `None`.
fn parsed<T: std::str::FromStr>(args: &[&str], name: &str) -> Option<Option<T>> {
    match flag(args, name) {
        Some(value) => value.parse().ok().map(Some),
        None => Some(None),
    }
}

/// Opens one file in a fresh headless session and runs one command on it, through the same path agents use.
fn run_on_file(file: &Path, command: impl FnOnce(String) -> Command) -> i32 {
    let mut session = Session::default();
    let result = session
        .open(file)
        .map_err(recto_agent::AgentError::from)
        .and_then(|id| run(&mut session, Level::Full, &command(format!("#{}", id.0))));
    println!("{}", envelope(&result));
    result.map_or_else(|error| error.exit, |_| exit::OK)
}

fn render(file: &Path, args: &[&str]) -> i32 {
    let (Some(Some(page)), Some(out), Some(scale)) = (parsed::<u32>(args, "--page"), flag(args, "--out"), parsed::<f32>(args, "--scale")) else {
        return usage();
    };
    run_on_file(file, |tab| Command::Render { tab, page, scale: scale.unwrap_or(DEFAULT_SCALE), out: out.into() })
}

fn text(file: &Path, args: &[&str]) -> i32 {
    let Some(Some(page)) = parsed::<u32>(args, "--page") else { return usage() };
    run_on_file(file, |tab| Command::Text { tab, page })
}

fn budget_render(args: &[&str]) -> i32 {
    let (Some(file), Some(Some(pages)), Some(first), Some(scale), Some(limit_mib), Some(retire_mib)) = (
        flag(args, "--file"),
        parsed::<u32>(args, "--count"),
        parsed::<u32>(args, "--first"),
        parsed::<f32>(args, "--scale"),
        parsed::<u32>(args, "--limit-mib"),
        parsed::<u32>(args, "--retire-mib"),
    ) else {
        return usage();
    };
    run_on_file(Path::new(file), |tab| Command::BudgetRender {
        tab,
        pages,
        first,
        scale: scale.unwrap_or(DEFAULT_SCALE),
        fresh: switch(args, "--fresh"),
        tiles: switch(args, "--tiles"),
        samples: switch(args, "--samples"),
        limit_mib,
        retire_mib,
    })
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
