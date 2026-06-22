#![no_main]
//! Fuzz target: `sanitize` core (`sanitize_message` total + leaks no control byte + idempotent).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_sanitize_case(data);
});
