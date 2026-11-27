#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| recto_fuzz::targets::distill_line::check(data));
