//! What the snapshot tool's copy of a box costs, the whole process tree: `cargo run --release -p recto-agent --example clip_memory -- FILE PAGE X,Y,W,H [svg]`, the page from 1. Prints the time, this process's peak footprint, the largest worker's, and the tree's peak (this process and every worker at once, sampled every 5 ms). With `svg`, a paste of the SVG afterwards, as its own figures.

use recto_agent::extract::{self, ClipOutcome, ClipStop};
use recto_platform::isolate::process_memory;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MIB: f64 = 1024.0 * 1024.0;

/// Samples the tree until told to stop: (tree peak, largest worker peak).
fn sampler(worker: PathBuf, stop: Arc<AtomicBool>, tree: Arc<AtomicU64>, largest: Arc<AtomicU64>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let me = std::process::id();
        while !stop.load(Ordering::SeqCst) {
            let mine = process_memory(me).map_or(0, |memory| memory.footprint_bytes);
            let mut sum = mine;
            for pid in recto_platform::local::processes_running(&worker) {
                if let Some(memory) = process_memory(pid) {
                    sum += memory.footprint_bytes;
                    largest.fetch_max(memory.peak_footprint_bytes, Ordering::SeqCst);
                }
            }
            tree.fetch_max(sum, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
        }
    })
}

fn measure(label: &str, worker: &Path, step: impl FnOnce() -> String) {
    let (stop, tree, largest) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let handle = sampler(worker.to_path_buf(), Arc::clone(&stop), Arc::clone(&tree), Arc::clone(&largest));
    let started = Instant::now();
    let said = step();
    let took = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    let host_peak = process_memory(std::process::id()).map_or(0, |memory| memory.peak_footprint_bytes);
    println!(
        "{label}: {:.2} s, host peak {:.1} MiB, largest worker {:.0} MiB, tree peak {:.0} MiB; {said}",
        took.as_secs_f64(),
        host_peak as f64 / MIB,
        largest.load(Ordering::SeqCst) as f64 / MIB,
        tree.load(Ordering::SeqCst) as f64 / MIB
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = PathBuf::from(args.next().expect("a PDF"));
    let page: u32 = args.next().and_then(|word| word.parse().ok()).expect("a page, from 1");
    let rect: Vec<f32> = args.next().expect("x,y,w,h").split(',').map(|part| part.parse().unwrap()).collect();
    let rect = [rect[0], rect[1], rect[2], rect[3]];
    let paste = args.next().as_deref() == Some("svg");
    let worker = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("recto-render-service");
    measure("copy", &worker, || {
        match extract::snapshot_clip(&file, page - 1, rect, (0, None), false, &ClipStop::default()) {
            ClipOutcome::Ready(clip) => format!(
                "{:?}, {} bytes made",
                clip.types(),
                clip.items.iter().filter_map(|item| item.bytes()).map(<[u8]>::len).sum::<usize>()
            ),
            other => format!("{other:?}"),
        }
    });
    if paste {
        measure("paste svg", &worker, || {
            extract::snapshot_svg(&file, page - 1, rect, 0, None, extract::MAX_CLIP_ITEM)
                .map_or_else(|error| error.message, |svg| format!("{} bytes", svg.len()))
        });
    }
}
