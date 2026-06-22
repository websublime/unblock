#![no_main]
//! Fuzz target: `cycle_detect` core (the cycle detector always terminates; planted cycles found).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_cycle_detect_case(data);
});
