//! Environment-based agent client detection.
//!
//! [`ClientDetector`](crate::detection::ClientDetector) provides a fallback detection layer when the MCP
//! `clientInfo` field is absent or contains an unrecognised name. It probes
//! well-known environment variables set by hosting clients (Claude Code,
//! GitHub Copilot, Cursor) and resolves the connected
//! [`AgentKind`](crate::client::AgentKind).
//!
//! # Detection order
//!
//! | Priority | Env var | Resolves to |
//! |----------|---------|-------------|
//! | 1 | `CLAUDE_CODE_ENTRYPOINT` | [`AgentKind::ClaudeCode`](crate::client::AgentKind::ClaudeCode) |
//! | 2 | `GITHUB_COPILOT_TOKEN` | [`AgentKind::Copilot`](crate::client::AgentKind::Copilot) |
//! | 3 | `CURSOR_TRACE_ID` | [`AgentKind::Cursor`](crate::client::AgentKind::Cursor) |
//!
//! `VSCODE_PID` is intentionally **excluded** — it is set for any VS Code
//! session, not specifically GitHub Copilot. See design decision D6 in
//! `docs/unblock-epic-agent-client-detection.md`.
//!
//! # Resolution priority (Design Decision D1)
//!
//! MCP `clientInfo` is authoritative. Environment variables are a fallback
//! for clients that do not populate `clientInfo` correctly.
//! [`AgentKind::Unknown`](crate::client::AgentKind::Unknown) is a valid,
//! non-fatal state — the server always starts regardless.
//!
//! # Testability
//!
//! The public API (`from_env`, `resolve`) delegates to `_with` variants that
//! accept an injectable environment reader, following the same pattern as
//! [`crate::config::Config::load_from`]. This allows unit tests to exercise
//! all code paths without mutating process-global environment variables
//! (which is `unsafe` in Rust edition 2024).

use std::env::VarError;

use crate::client::{AgentClient, AgentKind};

/// Environment variable checked for Claude Code presence.
const ENV_CLAUDE_CODE_ENTRYPOINT: &str = "CLAUDE_CODE_ENTRYPOINT";
/// Environment variable checked for GitHub Copilot presence.
const ENV_GITHUB_COPILOT_TOKEN: &str = "GITHUB_COPILOT_TOKEN";
/// Environment variable checked for Cursor IDE presence.
const ENV_CURSOR_TRACE_ID: &str = "CURSOR_TRACE_ID";

/// Detects the connected AI agent from environment signals.
///
/// This is the fallback detection layer when the MCP `clientInfo` field is
/// absent or unrecognised. All methods are pure functions with no async or
/// I/O side effects beyond reading environment variables.
///
/// See the [module-level documentation](self) for detection order and design
/// rationale.
pub struct ClientDetector;

impl ClientDetector {
    /// Detect the agent kind from well-known environment variables.
    ///
    /// Returns the first match in priority order, or `None` if no known
    /// environment variable is set.
    ///
    /// Delegates to [`ClientDetector::from_env_reader`] with [`std::env::var`]
    /// as the reader.
    #[must_use]
    pub fn from_env() -> Option<AgentKind> {
        Self::from_env_reader(|key| std::env::var(key))
    }

    /// Detect the agent kind using a custom environment reader.
    ///
    /// Accepts any function with the signature `Fn(&str) -> Result<String, VarError>`.
    /// This enables testing without mutating process-global environment variables.
    ///
    /// # Detection order
    ///
    /// 1. `CLAUDE_CODE_ENTRYPOINT` present → [`AgentKind::ClaudeCode`]
    /// 2. `GITHUB_COPILOT_TOKEN` present → [`AgentKind::Copilot`]
    /// 3. `CURSOR_TRACE_ID` present → [`AgentKind::Cursor`]
    /// 4. None of the above → `None`
    #[must_use]
    pub fn from_env_reader(env: impl Fn(&str) -> Result<String, VarError>) -> Option<AgentKind> {
        if env(ENV_CLAUDE_CODE_ENTRYPOINT).is_ok() {
            return Some(AgentKind::ClaudeCode);
        }
        if env(ENV_GITHUB_COPILOT_TOKEN).is_ok() {
            return Some(AgentKind::Copilot);
        }
        if env(ENV_CURSOR_TRACE_ID).is_ok() {
            return Some(AgentKind::Cursor);
        }
        None
    }

    /// Resolve the agent kind with MCP-first, env-fallback, unknown-last priority.
    ///
    /// 1. If `mcp_client` is `Some`, derives the kind from its name via
    ///    [`AgentClient::kind`].
    /// 2. Otherwise, probes environment variables via [`ClientDetector::from_env`].
    /// 3. If neither source yields a known kind, returns
    ///    `AgentKind::Unknown("unknown")`.
    ///
    /// Delegates to [`ClientDetector::resolve_with`] using [`std::env::var`]
    /// as the reader.
    #[must_use]
    pub fn resolve(mcp_client: Option<&AgentClient>) -> AgentKind {
        Self::resolve_with(mcp_client, |key| std::env::var(key))
    }

    /// Resolve the agent kind using a custom environment reader.
    ///
    /// Same resolution logic as [`ClientDetector::resolve`] but accepts an
    /// injectable environment reader for testability.
    #[must_use]
    pub fn resolve_with(
        mcp_client: Option<&AgentClient>,
        env: impl Fn(&str) -> Result<String, VarError>,
    ) -> AgentKind {
        mcp_client
            .filter(|c| !c.name.trim().is_empty())
            .map(AgentClient::kind)
            .or_else(|| Self::from_env_reader(env))
            .unwrap_or_else(|| AgentKind::Unknown("unknown".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env::VarError;

    use super::*;

    /// Creates an env reader backed by a `HashMap`.
    fn make_env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, VarError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned().ok_or(VarError::NotPresent)
    }

    // ── from_env_reader: each env var path ──────────────────────────────

    /// `CLAUDE_CODE_ENTRYPOINT` set → `ClaudeCode`.
    #[test]
    fn from_env_reader_claude() {
        let env = make_env(&[("CLAUDE_CODE_ENTRYPOINT", "1")]);
        assert_eq!(
            ClientDetector::from_env_reader(env),
            Some(AgentKind::ClaudeCode),
        );
    }

    /// `GITHUB_COPILOT_TOKEN` set → `Copilot`.
    #[test]
    fn from_env_reader_copilot_token() {
        let env = make_env(&[("GITHUB_COPILOT_TOKEN", "ghu_xxxxxxxxxxxxx")]);
        assert_eq!(
            ClientDetector::from_env_reader(env),
            Some(AgentKind::Copilot),
        );
    }

    /// `CURSOR_TRACE_ID` set → `Cursor`.
    #[test]
    fn from_env_reader_cursor() {
        let env = make_env(&[("CURSOR_TRACE_ID", "abc-123")]);
        assert_eq!(
            ClientDetector::from_env_reader(env),
            Some(AgentKind::Cursor),
        );
    }

    /// `VSCODE_PID` alone (without `GITHUB_COPILOT_TOKEN`) → `None`.
    ///
    /// Design decision D6: `VSCODE_PID` is intentionally excluded because it
    /// is set for any VS Code session, not specifically GitHub Copilot.
    #[test]
    fn from_env_reader_vscode_pid_ignored() {
        let env = make_env(&[("VSCODE_PID", "12345")]);
        assert_eq!(ClientDetector::from_env_reader(env), None);
    }

    /// No known env vars set → `None`.
    #[test]
    fn from_env_reader_none() {
        let env = make_env(&[]);
        assert_eq!(ClientDetector::from_env_reader(env), None);
    }

    /// Priority order: `CLAUDE_CODE_ENTRYPOINT` wins even when all env vars are set.
    #[test]
    fn from_env_reader_priority_claude_first() {
        let env = make_env(&[
            ("CLAUDE_CODE_ENTRYPOINT", "1"),
            ("GITHUB_COPILOT_TOKEN", "ghu_xxx"),
            ("CURSOR_TRACE_ID", "abc"),
        ]);
        assert_eq!(
            ClientDetector::from_env_reader(env),
            Some(AgentKind::ClaudeCode),
        );
    }

    /// Priority order: `GITHUB_COPILOT_TOKEN` wins over `CURSOR_TRACE_ID`
    /// when Claude is absent.
    #[test]
    fn from_env_reader_priority_copilot_before_cursor() {
        let env = make_env(&[
            ("GITHUB_COPILOT_TOKEN", "ghu_xxx"),
            ("CURSOR_TRACE_ID", "abc"),
        ]);
        assert_eq!(
            ClientDetector::from_env_reader(env),
            Some(AgentKind::Copilot),
        );
    }

    // ── resolve_with: MCP overrides env ─────────────────────────────────

    /// MCP client present → MCP wins, env ignored.
    #[test]
    fn resolve_with_mcp_overrides_env() {
        let client = AgentClient {
            name: "Cursor IDE".into(),
            version: "1.0.0".into(),
        };
        // Env says Copilot, but MCP says Cursor — MCP wins.
        let env = make_env(&[("GITHUB_COPILOT_TOKEN", "ghu_xxx")]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::Cursor,
        );
    }

    /// No MCP client + env set → env fallback used.
    #[test]
    fn resolve_with_env_fallback() {
        let env = make_env(&[("CURSOR_TRACE_ID", "abc")]);
        assert_eq!(ClientDetector::resolve_with(None, env), AgentKind::Cursor,);
    }

    /// No MCP client + no env vars → `Unknown("unknown")`.
    #[test]
    fn resolve_with_unknown_fallback() {
        let env = make_env(&[]);
        assert_eq!(
            ClientDetector::resolve_with(None, env),
            AgentKind::Unknown("unknown".into()),
        );
    }

    /// MCP client with unknown name + no env → `Unknown` with raw name.
    #[test]
    fn resolve_with_mcp_unknown_name() {
        let client = AgentClient {
            name: "SomeCustomTool".into(),
            version: "0.1.0".into(),
        };
        let env = make_env(&[]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::Unknown("SomeCustomTool".into()),
        );
    }

    /// MCP client with unknown name + env set → MCP still wins (D1: MCP is authoritative).
    #[test]
    fn resolve_with_mcp_unknown_overrides_env() {
        let client = AgentClient {
            name: "SomeCustomTool".into(),
            version: "0.1.0".into(),
        };
        let env = make_env(&[("CLAUDE_CODE_ENTRYPOINT", "1")]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::Unknown("SomeCustomTool".into()),
        );
    }

    /// MCP client with empty name + env set → env fallback wins.
    ///
    /// When `clientInfo` is present but the name is empty, the MCP hint is
    /// meaningless and should not block the env-based fallback path.
    #[test]
    fn resolve_with_empty_name_falls_through_to_env() {
        let client = AgentClient {
            name: String::new(),
            version: "1.0.0".into(),
        };
        let env = make_env(&[("CLAUDE_CODE_ENTRYPOINT", "1")]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::ClaudeCode,
        );
    }

    /// MCP client with empty name + no env → `Unknown("unknown")` sentinel.
    ///
    /// When both MCP name and environment are absent/empty, the standard
    /// unknown sentinel is returned (not `Unknown("")`).
    #[test]
    fn resolve_with_empty_name_no_env_returns_unknown_sentinel() {
        let client = AgentClient {
            name: String::new(),
            version: String::new(),
        };
        let env = make_env(&[]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::Unknown("unknown".into()),
        );
    }

    /// MCP client with whitespace-only name + env set → env fallback wins.
    ///
    /// A name composed entirely of whitespace (spaces, tabs) is semantically
    /// empty and should not be treated as a valid MCP client identity.
    #[test]
    fn resolve_with_whitespace_name_falls_through_to_env() {
        let client = AgentClient {
            name: "   ".into(),
            version: "1.0.0".into(),
        };
        let env = make_env(&[("CLAUDE_CODE_ENTRYPOINT", "1")]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::ClaudeCode,
        );
    }

    /// MCP client with whitespace-only name + no env → `Unknown("unknown")` sentinel.
    ///
    /// When the MCP name is only whitespace and no environment variable is
    /// set, the standard unknown sentinel is returned (not `Unknown("   ")`).
    #[test]
    fn resolve_with_whitespace_name_no_env_returns_unknown_sentinel() {
        let client = AgentClient {
            name: " \t ".into(),
            version: String::new(),
        };
        let env = make_env(&[]);
        assert_eq!(
            ClientDetector::resolve_with(Some(&client), env),
            AgentKind::Unknown("unknown".into()),
        );
    }
}
