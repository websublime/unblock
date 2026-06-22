#![no_main]
//! Fuzz target: `issue_ingest` core (arbitrary bytes through `from_slice::<Issue>` never panic).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_issue_ingest_case(data);
});
