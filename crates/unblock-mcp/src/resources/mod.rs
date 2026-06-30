//! Resources (spine §5.4) — read-only `unblock://...` URIs, never acquiring the write permit (FR-10).
//!
//! rmcp has NO resource macro / URI-template matcher (its `resource.rs` is empty), so the server
//! hand-writes `get_info`/`list_resource_templates`/`read_resource` and hand-parses the URI. This
//! module owns:
//!
//! - [`ResourceUri`] — the parsed URI (the 5 resources + an unknown fallback) and [`parse_uri`].
//! - the read helpers ([`issues`]) and the pure capability/schema builders ([`capabilities`],
//!   [`schema`]) surfaced as the crate's public discovery API.

pub(crate) mod capabilities;
pub(crate) mod issues;
pub(crate) mod schema;

pub use capabilities::{
    Capabilities, ErrorCodeDescriptor, PromptDescriptor, ResourceDescriptor, ToolDescriptor,
    capabilities,
};
pub use schema::{SchemaBundle, schema_bundle};

/// The `unblock://issues/` prefix; the `{id}`/`ready`/`blocked` tail is parsed off it.
const ISSUES_PREFIX: &str = "unblock://issues/";

/// A parsed resource URI (the 5 resources + an unknown fallback for a -32002 not-found).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceUri {
    /// `unblock://issues/{id}` — a single issue.
    IssueById(String),
    /// `unblock://issues/ready` — the default-complete ready set.
    Ready,
    /// `unblock://issues/blocked` — the blocked set.
    Blocked,
    /// `unblock://capabilities` — the discovery document.
    Capabilities,
    /// `unblock://schema` — the schema bundle.
    Schema,
    /// An unrecognised URI (→ `ErrorData::resource_not_found`).
    Unknown,
}

/// Parse a raw resource URI (spine §5.4). The `{id}` tail is hand-extracted off the `issues/` prefix.
///
/// `unblock://issues/ready` and `unblock://issues/blocked` are the two reserved collection tails;
/// any other non-empty `issues/` tail is an `{id}`. Anything else → [`ResourceUri::Unknown`].
pub(crate) fn parse_uri(uri: &str) -> ResourceUri {
    match uri {
        "unblock://capabilities" => ResourceUri::Capabilities,
        "unblock://schema" => ResourceUri::Schema,
        "unblock://issues/ready" => ResourceUri::Ready,
        "unblock://issues/blocked" => ResourceUri::Blocked,
        _ => {
            if let Some(id) = uri.strip_prefix(ISSUES_PREFIX) {
                if id.is_empty() {
                    ResourceUri::Unknown
                } else {
                    ResourceUri::IssueById(id.to_string())
                }
            } else {
                ResourceUri::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResourceUri, parse_uri};

    #[test]
    fn parses_the_five_known_uris() {
        assert_eq!(
            parse_uri("unblock://capabilities"),
            ResourceUri::Capabilities
        );
        assert_eq!(parse_uri("unblock://schema"), ResourceUri::Schema);
        assert_eq!(parse_uri("unblock://issues/ready"), ResourceUri::Ready);
        assert_eq!(parse_uri("unblock://issues/blocked"), ResourceUri::Blocked);
        assert_eq!(
            parse_uri("unblock://issues/ub-abc123"),
            ResourceUri::IssueById("ub-abc123".to_string())
        );
    }

    #[test]
    fn unknown_uris_fall_back() {
        assert_eq!(parse_uri("unblock://issues/"), ResourceUri::Unknown);
        assert_eq!(parse_uri("unblock://nope"), ResourceUri::Unknown);
        assert_eq!(parse_uri("file:///etc/passwd"), ResourceUri::Unknown);
    }
}
