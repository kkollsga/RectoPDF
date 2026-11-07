//! Writes a target's starting points to a folder, for libFuzzer. `tools/fuzz.sh` runs this: `fuzz-seeds <target> <folder>`.

use recto_fuzz::seeds;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [target, folder] = args.as_slice() else {
        eprintln!("usage: fuzz-seeds <target> <folder>, where target is one of {}", seeds::TARGET_NAMES.join(", "));
        std::process::exit(2);
    };
    if seeds::check_for(target).is_none() {
        eprintln!("no target called {target}");
        std::process::exit(2);
    }
    let folder = Path::new(folder);
    if let Err(error) = std::fs::create_dir_all(folder) {
        eprintln!("can't make {}: {error}", folder.display());
        std::process::exit(1);
    }
    let found = seeds::for_target(target);
    for (at, seed) in found.iter().enumerate() {
        if let Err(error) = std::fs::write(folder.join(format!("seed-{at:05}")), seed) {
            eprintln!("can't write a seed: {error}");
            std::process::exit(1);
        }
    }
    println!("{target}: {} seeds", found.len());
}
