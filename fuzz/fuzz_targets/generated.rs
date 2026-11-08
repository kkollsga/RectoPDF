#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| recto_fuzz::targets::generated::check(data));
