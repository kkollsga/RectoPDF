//! Makes the spike's worker variants from the real worker's source, so they can't drift from it. Small edits, each asserted to apply: the file's inner doc comments go (not allowed
//! in an `include!`), `main` becomes `worker_main`, and a hook runs straight after PDFium has loaded and before any hostile byte arrives. The arguments and the channel also come
//! from functions, so the XPC variant can get them from a message.
use std::path::Path;

fn main() {
    let source_path = "../../crates/recto-render-service/src/bin/worker.rs";
    println!("cargo:rerun-if-changed={source_path}");
    println!("cargo:rerun-if-changed=build.rs");
    let source = std::fs::read_to_string(source_path).unwrap();
    let body: String = source.lines().filter(|line| !line.starts_with("//!")).map(|line| format!("{line}\n")).collect();
    let edit = |text: String, from: &str, to: &str| {
        assert!(text.contains(from), "the worker changed: {from}");
        text.replacen(from, to, 1)
    };
    let body = edit(body, "fn main() {", "fn worker_main() {");
    let body = edit(body, "std::env::args_os().skip(1)", "spike_args().into_iter()");
    let body = edit(body, "Channel::inherited()", "spike_channel()");
    let load = "let Ok(pdfium) = Pdfium::load(&args.pdfium) else { std::process::exit(exit_code::NO_LIBRARY) };";
    let body = edit(body, load, &format!("{load}\n    spike_after_load();"));
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out).join("worker_body.rs"), body).unwrap();
}
