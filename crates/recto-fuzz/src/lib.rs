//! Fuzzing what handles untrusted input. Each check in [`targets`] is a plain function on bytes, run three ways: by the seeded smoke run in this crate's tests, which is short and on stable, so it's part of every `tools/check.sh`; by libFuzzer through `fuzz/`, which needs nightly and stays out of the workspace; and by a regression test, on the input that once broke it.
//!
//! `docs/engineering/crates/recto-fuzz.md` says how to run a campaign. PDFium isn't a target: it runs in a worker that can be killed, and what comes back from it is what's checked here.

pub mod guard;
pub mod input;
pub mod mutate;
pub mod seeds;
pub mod targets;

use input::Rng;
use std::sync::Mutex;

/// One check at a time, since memory limits read one count for the whole process.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Runs a target over its seeds and then mutations of them, with no coverage feedback: a fixed seed gives the same inputs on any machine, so a failure here is a failure anywhere. A failing input is written to the temp folder, and named in the panic.
pub fn smoke(name: &str, seed: u64, rounds: usize, max_len: usize) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let check = seeds::check_for(name).unwrap_or_else(|| panic!("no target called {name}"));
    let mut starts = seeds::for_target(name);
    starts.retain(|start| start.len() <= max_len);
    if starts.is_empty() {
        starts.push(Vec::new());
    }
    let is_pdf = name == "pdf_bytes";
    let run = |input: &[u8], what: &str| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(input)));
        if let Err(cause) = result {
            let path = std::env::temp_dir().join(format!("recto-fuzz-{name}-{seed}-{what}.bin"));
            let _ = std::fs::write(&path, input);
            std::panic::resume_unwind(cause);
        }
    };
    for (at, start) in starts.iter().enumerate() {
        run(start, &format!("seed{at}"));
    }
    let mut rng = Rng::new(seed);
    for round in 0..rounds {
        let mut input = starts[rng.below(starts.len())].clone();
        if is_pdf {
            mutate::pdf(&mut input, &mut rng, max_len);
        } else {
            mutate::bytes(&mut input, &mut rng, max_len);
        }
        run(&input, &format!("round{round}"));
    }
}
