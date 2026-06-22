#![no_main]
//! Fuzz target: `parse_id` core (`parse_id`/`is_valid_id_format` never panic + agree).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_parse_id_case(data);
});
