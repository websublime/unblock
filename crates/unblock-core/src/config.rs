//! Environment-based configuration.
//!
//! Reads configuration from environment variables:
//! - `GITHUB_TOKEN` (required)
//! - `GITHUB_API_URL` (optional, default: `https://api.github.com`)
//! - `UNBLOCK_REPO` (optional, auto-detect from git remote)
//! - `UNBLOCK_PROJECT` (optional, auto-detect from linked Projects V2)
//! - `UNBLOCK_AGENT` (optional, default: `agent`)
//! - `UNBLOCK_CACHE_TTL` (optional, default: `30` seconds)
//! - `UNBLOCK_LOG_LEVEL` (optional, default: `info`)
//! - `UNBLOCK_OTEL_ENDPOINT` (optional)
