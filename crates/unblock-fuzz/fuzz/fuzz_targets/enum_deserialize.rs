#![no_main]
//! Fuzz target: `enum_deserialize` core (open-enum `Deserialize` never panics + round-trips).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_enum_deserialize_case(data);
});
