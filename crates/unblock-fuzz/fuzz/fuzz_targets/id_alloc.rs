#![no_main]
//! Fuzz target: `id_alloc` core (the id child-counter high-water mark advances monotonically).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_id_alloc_case(data);
});
