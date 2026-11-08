//! Writes [`recto_testpdf::linked_document`] to a file, for screenshots of links and labels: `cargo run -p recto-testpdf --example write_linked -- OUT.pdf`.

fn main() {
    let Some(out) = std::env::args_os().nth(1) else {
        eprintln!("usage: write_linked OUT.pdf");
        std::process::exit(2);
    };
    if let Err(error) = std::fs::write(&out, recto_testpdf::linked_document()) {
        eprintln!("couldn't write it: {error}");
        std::process::exit(1);
    }
}
