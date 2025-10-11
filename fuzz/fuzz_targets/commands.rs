#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| lob_fuzz::commands(data));
