//! Runs the built helper on PDFs the way the session does, with the document on descriptor 3 and the `headings` job on stdin, and prints what it said: the page lines, the found count, the
//! wall time, the helper's peak footprint watched from outside, its own peak and whether it was walled. The check for hidden text is the session's, so it isn't here.
//!
//! ```text
//! cargo build --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target
//! cargo run --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target --example sample -- [--helper EXE] [--scan typography] [--brief] [--utility] FILE...
//! ```
//!
//! `--helper` is the program to run: a walled copy laid out by `app/render-helper.sh --layout-structure`, or a bare one. Without it, `recto-headings` beside this example's folder, which is bare.

use recto_platform::helper::{self, Priority, Spawned};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn default_helper() -> PathBuf {
    let this = std::env::current_exe().unwrap();
    this.parent().unwrap().parent().unwrap().join("recto-headings")
}

fn main() {
    let mut scan = "full".to_string();
    let mut brief = false;
    let mut priority = Priority::Inherited;
    let mut exe = default_helper();
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scan" => scan = args.next().unwrap(),
            "--brief" => brief = true,
            "--utility" => priority = Priority::Utility,
            "--helper" => exe = PathBuf::from(args.next().unwrap()),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    for file in files {
        run(&exe, &file, &scan, brief, priority);
    }
}

fn run(exe: &Path, file: &Path, scan: &str, brief: bool, priority: Priority) {
    let document = std::fs::File::open(file).unwrap();
    let started = Instant::now();
    // `SAMPLE_LARGE_CACHE=1` leaves it out, to see what it's worth.
    let env: &[(&str, &OsStr)] = if std::env::var_os("SAMPLE_LARGE_CACHE").is_none() {
        &[("MallocLargeCache", OsStr::new("0"))]
    } else {
        &[]
    };
    let Spawned {
        mut child,
        mut stdin,
        mut stdout,
    } = helper::spawn_with_document(exe, env, priority, &document).unwrap();
    let pid = child.id();
    writeln!(stdin, "{}", json!({ "kind": "headings", "memory_limit": 512u64 << 20, "scan": scan })).unwrap();
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
    let lines: Vec<Value> = said.lines().map(|line| serde_json::from_str(line).unwrap_or(Value::Null)).collect();
    let walled = lines.first().map(|hello| hello["walled"].clone()).unwrap_or(Value::Null);
    let end = lines.last().cloned().unwrap_or(Value::Null);
    let pages: Vec<&Value> = lines.iter().filter(|line| line.get("page").is_some()).collect();
    let own = recto_core::inspect(file).ok();
    let name = file.file_name().unwrap().to_string_lossy();
    let mib = |bytes: u64| bytes as f64 / 1048576.0;
    println!(
        "== {name}  pages {}  own {}  page lines {}  found {}  {:.2} s  helper peak {:.1} MiB (watched {:.1})  state {} {}  walled {walled}",
        own.as_ref().map_or(0, |facts| facts.page_count),
        own.as_ref().map_or(0, |facts| facts.toc.entries().len()),
        pages.len(),
        end["found"],
        elapsed.as_secs_f64(),
        mib(end["helper_peak_bytes"].as_u64().unwrap_or(0)),
        mib(peak),
        end["state"],
        end["code"],
    );
    if brief {
        return;
    }
    println!("-- found");
    for line in pages.iter().take(40) {
        for entry in line["found"].as_array().into_iter().flatten().take(3) {
            println!(
                "{}{} .. {}",
                "  ".repeat(entry["level"].as_u64().unwrap_or(1) as usize - 1),
                entry["title"].as_str().unwrap_or(""),
                line["page"].as_u64().unwrap_or(0) + 1
            );
        }
    }
}
