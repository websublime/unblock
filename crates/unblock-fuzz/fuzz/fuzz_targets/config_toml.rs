#![no_main]
//! Fuzz target: `config_toml` core (both TOML layers through `unblock-config`, D57).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = unblock_fuzz::run_config_toml_case(data);
});
