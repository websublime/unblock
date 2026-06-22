#![no_main]
//! Fuzz target: `content_hash` core (proves `compute_content_hash` is total + transport-independent).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_content_hash_case(data);
});
