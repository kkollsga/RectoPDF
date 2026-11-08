//! Runs the built helper on PDFs the way the session does, and prints each file's own contents beside the generated ones, with the time and the helper's peak footprint from outside.
//!
//! ```text
//! cargo build --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target
//! cargo run --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target --example sample -- [--scan typography] [--brief] [--background] FILE...
//! ```
//!
//! The helper is `recto-headings` beside this example's folder, and the render worker beside that, as in a build.

use recto_index::corpus::CorpusKey;
use recto_index::generated::Generated;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn helper() -> PathBuf {
    let this = std::env::current_exe().unwrap();
    this.parent().unwrap().parent().unwrap().join("recto-headings")
}

fn main() {
    let mut scan = "full".to_string();
    let mut brief = false;
    let mut background = false;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scan" => scan = args.next().unwrap(),
            "--brief" => brief = true,
            "--background" => background = true,
            _ => files.push(PathBuf::from(arg)),
        }
    }
    for file in files {
        run(&file, &scan, brief, background);
    }
}

fn run(file: &Path, scan: &str, brief: bool, background: bool) {
    let key = CorpusKey::of_file(file).unwrap();
    let started = Instant::now();
    let mut command = Command::new(helper());
    command.env_clear();
    // `SAMPLE_LARGE_CACHE=1` leaves it out, to see what it's worth.
    if std::env::var_os("SAMPLE_LARGE_CACHE").is_none() {
        command.env("MallocLargeCache", "0");
    }
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "file": file, "key": key.hex(), "background": background, "scan": scan })).unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut said = String::new();
        let _ = stdout.read_to_string(&mut said);
        said
    });
    let mut peak = 0u64;
    while child.try_wait().unwrap().is_none() {
        if let Some(memory) = recto_platform::isolate::process_memory(pid) {
            peak = peak.max(memory.peak_footprint_bytes);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stdin);
    let said = reader.join().unwrap();
    let elapsed = started.elapsed();
    let mut lines = said.lines();
    let report: Value = lines.next().and_then(|line| serde_json::from_str(line).ok()).unwrap_or(Value::Null);
    let generated = lines.next().and_then(|line| Generated::decode(line.as_bytes(), key).ok());
    let own = recto_core::inspect(file).ok();
    let name = file.file_name().unwrap().to_string_lossy();
    let mib = |bytes: u64| bytes as f64 / 1048576.0;
    println!(
        "== {name}  pages {}  own {}  generated {}  {:.2} s  helper peak {:.1} MiB (watched {:.1})  worker peak {:.1} MiB  state {} {}  hidden dropped {}  not checked {}  failed {}",
        own.as_ref().map_or(0, |facts| facts.page_count),
        own.as_ref().map_or(0, |facts| facts.toc.entries().len()),
        generated.as_ref().map_or(0, |generated| generated.entries.len()),
        elapsed.as_secs_f64(),
        mib(report["helper_peak_bytes"].as_u64().unwrap_or(0)),
        mib(peak),
        mib(report["worker_peak_bytes"].as_u64().unwrap_or(0)),
        report["state"],
        report["code"],
        generated.as_ref().map_or(0, |generated| generated.hidden_dropped),
        generated.as_ref().map_or(0, |generated| generated.pages_not_checked),
        generated.as_ref().map_or(0, |generated| generated.pages_failed),
    );
    if brief {
        return;
    }
    if let Some(facts) = &own {
        println!("-- own");
        for entry in facts.toc.entries().iter().take(40) {
            let page = if entry.page == recto_core::NONE {
                "-".to_string()
            } else {
                (entry.page + 1).to_string()
            };
            println!("{}{} .. {}", "  ".repeat(usize::from(entry.depth)), facts.toc.title(entry), page);
        }
    }
    if let Some(generated) = &generated {
        println!("-- generated");
        let toc = generated.toc();
        for entry in toc.entries().iter().take(40) {
            println!("{}{} .. {}", "  ".repeat(usize::from(entry.depth)), toc.title(entry), entry.page + 1);
        }
    }
}
