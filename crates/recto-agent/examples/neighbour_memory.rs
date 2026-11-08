//! What reading a page's neighbours for their edge lines costs the worker, as a copy and the app's Services do: their text with no drawing. `cargo run --release -p recto-agent --example neighbour_memory -- <target-dir> FILE PAGE`, pages from 1. Prints the worker's footprint after the page's own read and after each neighbour's, and the time each took.

use recto_render_service::{DocRenderer, Policy, ServiceConfig};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(args.next().expect("the folder the worker's built in"));
    let file = PathBuf::from(args.next().expect("a PDF"));
    let page: u32 = args.next().and_then(|word| word.parse().ok()).expect("a page, from 1");
    let config = ServiceConfig::for_worker(target.join("recto-render-service")).expect("the worker");
    let mut doc = DocRenderer::new(&file, None, config, Policy::default()).expect("the file");
    let count = doc.page_count().expect("its pages");
    let mib = |doc: &mut DocRenderer| doc.worker_footprint().map_or(f64::NAN, |bytes| bytes as f64 / 1024.0 / 1024.0);
    let started = Instant::now();
    doc.page_characters(page - 1, true, false).expect("the page");
    println!("page {page}: {:.1} ms, worker {:.1} MiB", started.elapsed().as_secs_f64() * 1000.0, mib(&mut doc));
    for other in recto_index::select::neighbour_pages(page - 1, count) {
        let started = Instant::now();
        let read = doc.page_characters(other, true, false).map(|read| read.chars.len());
        println!("neighbour {}: {:?} characters, {:.1} ms, worker {:.1} MiB", other + 1, read.ok(), started.elapsed().as_secs_f64() * 1000.0, mib(&mut doc));
    }
}
