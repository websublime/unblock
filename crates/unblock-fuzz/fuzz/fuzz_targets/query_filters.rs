#![no_main]
//! Fuzz target: `query_filters` core (list/ready/blocked/search/count/stale never panic; consistent).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_query_filters_case(data);
});
