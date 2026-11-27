#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| recto_fuzz::targets::removal_caches::check(data));
