//! How long reading a page's links takes, the median of each page's, over some pages of a file: `cargo run --release -p recto-core --example links_cost -- FILE.pdf`.

use std::time::Instant;

fn main() {
    let Some(path) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        eprintln!("usage: links_cost FILE.pdf");
        std::process::exit(2);
    };
    let facts = recto_core::inspect(&path).expect("a PDF");
    let pages = facts.page_count;
    let mut times = Vec::new();
    let mut links = 0;
    for page in (0..pages).step_by((pages as usize / 40).max(1)) {
        let started = Instant::now();
        let read = recto_core::nav::page_links(&path, &[page]).expect("readable");
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        links += read[0].1.links.len();
    }
    times.sort_by(f64::total_cmp);
    let at = |share: f64| times[((times.len() - 1) as f64 * share) as usize];
    println!(
        "{pages} pages, {} read, {links} links, ms per page p50 {:.2} p95 {:.2} max {:.2}, label ranges {}",
        times.len(),
        at(0.5),
        at(0.95),
        at(1.0),
        facts.toc.labels.ranges().len()
    );
}
