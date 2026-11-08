#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| recto_fuzz::targets::pdf::check_save(data));
