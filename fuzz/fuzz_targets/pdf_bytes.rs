#![no_main]
use libfuzzer_sys::{fuzz_mutator, fuzz_target, fuzzer_mutate};
use recto_fuzz::input::Rng;
use recto_fuzz::{mutate, targets};

fuzz_target!(|data: &[u8]| targets::pdf::check_bytes(data));

// Half of the time libFuzzer's own mutations, which follow coverage, and half this crate's, which know what a PDF looks like.
fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    if seed % 2 == 0 {
        return fuzzer_mutate(data, size, max_size);
    }
    let mut bytes = data[..size].to_vec();
    let mut rng = Rng::new(u64::from(seed));
    mutate::pdf(&mut bytes, &mut rng, max_size);
    let len = bytes.len().min(max_size);
    data[..len].copy_from_slice(&bytes[..len]);
    len
});
