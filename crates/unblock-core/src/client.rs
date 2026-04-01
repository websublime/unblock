//! Agent client domain types.
//!
//! Identifies which AI agent is connected to the MCP server. The primary type
//! is `AgentKind`, an enum of known agent clients, with an `Unknown(String)`
//! variant for unrecognised clients.
//!
//! `AgentClient` pairs a raw client name and version (as received from the
//! MCP `initialize` handshake) and can derive its `AgentKind` via the `kind()`
//! method.
//!
//! # Placement rationale
//!
//! These types live in `unblock-core` (not `unblock-mcp`) because agent kind is
//! a domain concept — reusable by the desktop app, future CLI tools, and test
//! harnesses without pulling in MCP dependencies. See design decision D3 in
//! `docs/unblock-epic-agent-client-detection.md`.

use std::fmt;

/// The kind of AI agent connected to the MCP server.
///
/// Detected from the MCP `initialize` handshake (`clientInfo.name`) or,
/// as a fallback, from environment variables set by the hosting client.
///
/// `AgentKind` is **informational only** — it never changes tool behaviour.
/// See design decision D2 in `docs/unblock-epic-agent-client-detection.md`.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentKind {
    /// Anthropic Claude Code.
    ClaudeCode,
    /// GitHub Copilot.
    Copilot,
    /// Cursor IDE.
    Cursor,
    /// Cline (formerly Continue).
    Cline,
    /// Aider.
    Aider,
    /// Any client whose name was not recognised.
    Unknown(String),
}

impl AgentKind {
    /// Derive the kind from a raw client name string.
    ///
    /// Matching is **case-insensitive** and uses **substring containment**
    /// (`contains`), so `"Claude Code v1.2"` matches [`AgentKind::ClaudeCode`]
    /// and `"GitHub Copilot Chat"` matches [`AgentKind::Copilot`].
    ///
    /// Unrecognised names produce [`AgentKind::Unknown`] with the original
    /// string preserved verbatim.
    #[must_use]
    pub fn from_client_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("claude") {
            Self::ClaudeCode
        } else if lower.contains("copilot") {
            Self::Copilot
        } else if lower.contains("cursor") {
            Self::Cursor
        } else if lower.contains("cline") {
            Self::Cline
        } else if lower.contains("aider") {
            Self::Aider
        } else {
            Self::Unknown(name.to_owned())
        }
    }

    /// A stable, lowercase string identifier suitable for log fields and metrics.
    ///
    /// Known variants return a fixed `&'static str`. The [`Unknown`](AgentKind::Unknown)
    /// variant returns the inner string as-is.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Aider => "aider",
            Self::Unknown(name) => name.as_str(),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Metadata about the MCP client connected to this server session.
///
/// Constructed from the `clientInfo` field of the MCP `initialize` request.
/// Use [`AgentClient::kind`] to derive the normalised [`AgentKind`].
#[derive(Debug, Clone, PartialEq)]
pub struct AgentClient {
    /// Raw `clientInfo.name` from the MCP `initialize` request.
    pub name: String,
    /// Raw `clientInfo.version` from the MCP `initialize` request.
    pub version: String,
}

impl AgentClient {
    /// Derive the [`AgentKind`] from this client's name.
    #[must_use]
    pub fn kind(&self) -> AgentKind {
        AgentKind::from_client_name(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven test: known client names produce the correct `AgentKind` variant.
    #[test]
    fn from_client_name_known_names() {
        let cases: &[(&str, AgentKind)] = &[
            // Exact lowercase
            ("claude", AgentKind::ClaudeCode),
            ("copilot", AgentKind::Copilot),
            ("cursor", AgentKind::Cursor),
            ("cline", AgentKind::Cline),
            ("aider", AgentKind::Aider),
            // Mixed case
            ("Claude Code", AgentKind::ClaudeCode),
            ("CLAUDE", AgentKind::ClaudeCode),
            ("GitHub Copilot Chat", AgentKind::Copilot),
            ("COPILOT", AgentKind::Copilot),
            ("Cursor IDE", AgentKind::Cursor),
            ("CURSOR", AgentKind::Cursor),
            ("Cline v2.0", AgentKind::Cline),
            ("CLINE", AgentKind::Cline),
            ("Aider 0.50", AgentKind::Aider),
            ("AIDER", AgentKind::Aider),
            // Substring containment
            ("my-claude-wrapper", AgentKind::ClaudeCode),
            ("vscode-copilot-extension", AgentKind::Copilot),
            ("cursor-nightly", AgentKind::Cursor),
            ("cline-fork", AgentKind::Cline),
            ("aider-chat", AgentKind::Aider),
        ];

        for (input, expected) in cases {
            assert_eq!(
                AgentKind::from_client_name(input),
                *expected,
                "from_client_name({input:?}) should be {expected:?}"
            );
        }
    }

    /// Unrecognised client names produce `Unknown` with the original string preserved.
    #[test]
    fn from_client_name_unknown_passthrough() {
        let names = ["some-random-client", "MyCustomAgent", "", "   "];
        for name in names {
            let kind = AgentKind::from_client_name(name);
            assert_eq!(
                kind,
                AgentKind::Unknown(name.to_owned()),
                "from_client_name({name:?}) should be Unknown"
            );
        }
    }

    /// `Display` delegates to `as_str()` — the formatted output matches exactly.
    #[test]
    fn display_matches_as_str() {
        let variants = [
            AgentKind::ClaudeCode,
            AgentKind::Copilot,
            AgentKind::Cursor,
            AgentKind::Cline,
            AgentKind::Aider,
            AgentKind::Unknown("custom-agent".into()),
        ];

        for kind in &variants {
            assert_eq!(
                kind.to_string(),
                kind.as_str(),
                "Display and as_str() must agree for {kind:?}"
            );
        }
    }

    /// Roundtrip: for known variants, `from_client_name(kind.as_str())` recovers
    /// the same variant (since each `as_str()` output contains the detection substring).
    #[test]
    fn display_roundtrip_known_variants() {
        let known = [
            AgentKind::ClaudeCode,
            AgentKind::Copilot,
            AgentKind::Cursor,
            AgentKind::Cline,
            AgentKind::Aider,
        ];

        for kind in &known {
            let recovered = AgentKind::from_client_name(kind.as_str());
            assert_eq!(
                &recovered, kind,
                "from_client_name(as_str()) roundtrip failed for {kind:?}"
            );
        }
    }

    /// `AgentClient::kind()` delegates to `AgentKind::from_client_name`.
    #[test]
    fn agent_client_kind() {
        let client = AgentClient {
            name: "Claude Code".into(),
            version: "1.0.0".into(),
        };
        assert_eq!(client.kind(), AgentKind::ClaudeCode);

        let unknown_client = AgentClient {
            name: "SomeOtherTool".into(),
            version: "0.1.0".into(),
        };
        assert_eq!(
            unknown_client.kind(),
            AgentKind::Unknown("SomeOtherTool".into())
        );
    }

    /// `as_str()` returns stable identifiers for all known variants.
    #[test]
    fn as_str_stable_identifiers() {
        assert_eq!(AgentKind::ClaudeCode.as_str(), "claude-code");
        assert_eq!(AgentKind::Copilot.as_str(), "copilot");
        assert_eq!(AgentKind::Cursor.as_str(), "cursor");
        assert_eq!(AgentKind::Cline.as_str(), "cline");
        assert_eq!(AgentKind::Aider.as_str(), "aider");
        assert_eq!(AgentKind::Unknown("my-tool".into()).as_str(), "my-tool");
    }
}
