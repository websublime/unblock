#![no_main]
//! Fuzz target: `dup_scan` core (D43 — the duplicate-JSON-key scanner is total, never
//! under-rejects a real duplicate, and never over-rejects bytes rmcp itself would parse).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_dup_scan_case(data);
});
