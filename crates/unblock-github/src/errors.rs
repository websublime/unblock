//! Infrastructure error types for the GitHub API client.
//!
//! Defines `Error` with variants for all infrastructure-level failure modes:
//! network errors, API errors, GraphQL errors, rate limiting, and git remote
//! detection failures.
//!
//! Domain errors from `unblock-core` are wrapped transparently via the
//! `Domain` variant with `#[snafu(context(false))]`.

use snafu::prelude::*;
use unblock_core::errors::DomainError;
use unblock_core::types::QualifiedId;

use chrono::{DateTime, Utc};

/// Infrastructure-level errors for the GitHub API client.
///
/// Each variant represents a specific infrastructure failure mode. Domain errors
/// from `unblock-core` are wrapped transparently via the [`Domain`](Self::Domain)
/// variant.
///
/// Use the generated snafu context selectors (e.g. [`GitHubApiSnafu`]) to
/// construct errors ergonomically.
///
/// # Forward-compat: `#[non_exhaustive]`
///
/// This enum is marked `#[non_exhaustive]` per idiomatic Rust posture for
/// error enums that grow over time (`std::io::ErrorKind`, `reqwest::Error`,
/// snafu-generated enums in mature crates). Without it, every additive
/// variant becomes a MAJOR semver bump because downstream exhaustive
/// `match` blocks break silently. With it, downstream consumers MUST add
/// a `_ =>` wildcard arm, and we retain freedom to add variants in
/// PATCH/MINOR releases. See bead `unblock-29p.70` for the rationale.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[non_exhaustive]
pub enum Error {
    /// Wraps a domain-level error from `unblock-core`.
    #[snafu(display("{source}"))]
    #[snafu(context(false))]
    Domain {
        /// The underlying domain error.
        source: DomainError,
    },

    /// A non-2xx response from the GitHub REST API.
    #[snafu(display("GitHub API error ({status}): {message}"))]
    GitHubApi {
        /// HTTP status code from the API response.
        status: u16,
        /// Human-readable error message from the API response.
        message: String,
    },

    /// One or more errors in a GitHub GraphQL response.
    #[snafu(display("GitHub GraphQL error: {}", errors.join("; ")))]
    GitHubGraphQL {
        /// Individual error messages from the GraphQL response.
        errors: Vec<String>,
    },

    /// A GitHub GraphQL response containing at least one error with
    /// `type == "FORBIDDEN"`.
    ///
    /// The GraphQL spec (and GitHub's implementation) returns HTTP 200
    /// with a typed `errors` array for permission-denied cases — the
    /// `type` field is the wire-safe signal rather than the free-text
    /// `message`. The GraphQL-level reducer in `graphql_with_features`
    /// on [`crate::client::GitHubClient`] inspects `type` BEFORE
    /// reducing to messages (per SPEC §11.1 wiring, user decision
    /// 2026-04-17) and emits this variant when any error carries
    /// `FORBIDDEN`. Callers that know a cross-repo `owner/repo` context
    /// (e.g. `fetch_issue_in_repo`, `resolve_issue_ref` cross-repo arm)
    /// upgrade this variant to [`DomainError::CrossRepoAccessDenied`].
    ///
    /// **Variant presence — not vector contents — is authoritative.**
    /// The `errors` vector MAY be empty when every FORBIDDEN-typed entry
    /// in the response lacked a populated `message` body (the
    /// message-partition loop drops empty messages for hygiene per
    /// unblock-eos.22). The emission of this variant is driven solely
    /// by at least one response error carrying `type == "FORBIDDEN"`;
    /// downstream classifiers (`classify_cross_repo_fetch`) pattern-
    /// match on the variant and therefore handle the empty-vector case
    /// transparently. The `Display` impl renders an explicit `(no
    /// details)` sentinel when `errors.is_empty()` so log output stays
    /// self-describing.
    ///
    /// [`DomainError::CrossRepoAccessDenied`]:
    ///     unblock_core::errors::DomainError::CrossRepoAccessDenied
    #[snafu(display("{}", format_forbidden_display(errors)))]
    GitHubGraphQLForbidden {
        /// Messages from the GraphQL errors whose `type` was
        /// `FORBIDDEN`. Non-FORBIDDEN messages from the same response
        /// are discarded; the classifier treats any FORBIDDEN as
        /// authoritative. This vector MAY be empty — see the variant
        /// docs for the `(no details)` sentinel semantics.
        errors: Vec<String>,
    },

    /// Network or connection failure when reaching GitHub.
    #[snafu(display("Cannot connect to GitHub: {source}"))]
    GitHubUnavailable {
        /// The underlying reqwest error.
        source: reqwest::Error,
    },

    /// GitHub returned HTTP 429 — rate limit exceeded.
    #[snafu(display("GitHub rate limit exceeded — resets at {reset_at}"))]
    RateLimited {
        /// When the rate limit resets (from the `X-RateLimit-Reset` header).
        reset_at: DateTime<Utc>,
    },

    /// Circuit breaker is open due to repeated GitHub failures (Phase 2 stub).
    #[snafu(display("Circuit breaker open — GitHub consistently failing (tripped at {since:?})"))]
    CircuitBreakerOpen {
        /// The instant the circuit breaker was tripped.
        since: std::time::Instant,
    },

    /// A GitHub mutation succeeded but the subsequent cache rebuild failed,
    /// leaving the tool unable to compute post-mutation fields locally.
    ///
    /// Emitted by write-tool handlers (`close`, `dep_remove`, `reopen`) when
    /// the mutation is durable on GitHub but a transient 503, rate-limit,
    /// or network error during the `execute_write_tool` rebuild leaves the
    /// cache empty (or — for `reopen` — present but missing the mutated
    /// issue due to a concurrent re-close race). Preserves spec §8.5 /
    /// §8.7 R3: the handler MUST NOT fabricate `has_open_blockers` /
    /// `blocked` / Status claims against a missing graph; instead it
    /// surfaces this variant and instructs the caller to re-run `show` to
    /// observe the final post-mutation state.
    ///
    /// Maps to HTTP 503 via [`Error::status_code`]; `github_error_to_mcp`
    /// in `unblock-mcp` then maps 503 → `ErrorCode::INTERNAL_ERROR`. The
    /// `mutation` field names the preceding mutation (`"reopen"`,
    /// `"remove_blocked_by"`, `"close_cascade"`, …) for log/trace
    /// disambiguation; `qid` carries the [`QualifiedId`] of the mutated
    /// issue (or the source endpoint for `dep_remove`) so same-numbered
    /// local vs. cross-repo issues render unambiguously in the rendered
    /// message.
    ///
    /// This is a transient wiring-class failure — NOT a domain-meaningful
    /// outcome — which is why it lives on the infrastructure `Error` enum
    /// rather than on [`DomainError`]. Matches the `infrastructure`-typed
    /// error-contract rows at spec §8.5 / §8.7 R3.
    ///
    /// [`DomainError`]: unblock_core::errors::DomainError
    #[snafu(display(
        "{mutation} mutation on {qid} succeeded, but the post-mutation cache rebuild failed — please re-run `show` to observe the final state"
    ))]
    PostMutationRebuildFailed {
        /// Identifier of the mutation that preceded the rebuild failure
        /// (e.g. `"reopen"`, `"remove_blocked_by"`, `"close_cascade"`).
        /// Free-form for log/trace diagnostics; no downstream code
        /// branches on this value.
        mutation: String,
        /// The [`QualifiedId`] of the mutated issue (or of the source
        /// endpoint, for `dep_remove`). Rendered via `Display`
        /// (`owner/repo#n` form) so cross-repo vs. local same-number
        /// collisions surface unambiguously in logs and in the message
        /// forwarded to the MCP caller.
        qid: QualifiedId,
    },

    /// A pre-mutation cache prime failed, so the tool cannot capture the
    /// pre-mutation graph state required to proceed safely.
    ///
    /// Symmetric pre-mutation counterpart to
    /// [`PostMutationRebuildFailed`](Self::PostMutationRebuildFailed).
    /// Emitted by write-tool handlers that depend on a primed cache to
    /// freeze a pre-mutation snapshot — currently only the `close` handler
    /// (Phase 0 PRE-close cascade capture, SPEC §8.2 step 2 / §3.4
    /// Critical) — when the cache is cold at entry and the cold-cache
    /// prime via [`crate::client::GitHubClient::fetch_graph_data`] fails
    /// (transient 503, rate-limit, or network error).
    ///
    /// **Distinct semantic from `PostMutationRebuildFailed`.** This
    /// variant fires BEFORE any mutation lands on GitHub: there is no
    /// mutation to reconcile. Using `PostMutationRebuildFailed` here
    /// would misrepresent the contract — its `Display` instructs the
    /// caller to re-run `show` to "observe the final state", but no
    /// mutation has occurred. The `Display` for this variant instead
    /// guides the caller to retry or run the `prime` tool first, which
    /// is the correct recovery for a pre-mutation prime failure.
    ///
    /// `qid` carries the [`QualifiedId`] of the issue whose mutation was
    /// gated behind the prime so same-numbered local vs. cross-repo
    /// issues render unambiguously in logs and in the message forwarded
    /// to the MCP caller. There is no `mutation` field — by construction,
    /// no mutation was attempted.
    ///
    /// Maps to HTTP 503 via [`Error::status_code`]; `github_error_to_mcp`
    /// in `unblock-mcp` then maps 503 → `ErrorCode::INTERNAL_ERROR`,
    /// matching the existing wiring contract for transient
    /// infrastructure failures.
    ///
    /// See bead `unblock-29p.69` for the full taxonomy rationale and
    /// the `unblock-29p.35` DEVIATION it closes.
    #[snafu(display(
        "Cannot proceed with mutation on {qid}: failed to prime the dependency graph before cascade capture — please retry or run `prime` first"
    ))]
    PreMutationPrimeFailed {
        /// The [`QualifiedId`] of the issue whose mutation was gated
        /// behind the prime. Rendered via `Display` (`owner/repo#n`
        /// form) so same-numbered local vs. cross-repo issues surface
        /// unambiguously in logs and in the message forwarded to the
        /// MCP caller.
        qid: QualifiedId,
    },

    /// No Projects V2 project is configured or discoverable.
    #[snafu(display("Projects V2 not configured — run `setup` first"))]
    ProjectNotConfigured,

    /// Failed to read or parse the git remote URL.
    #[snafu(display("Failed to detect git remote: {message}"))]
    GitRemote {
        /// Description of the git remote detection failure.
        message: String,
    },

    /// GitHub returned an account type that is not recognized as either
    /// `Organization` or `User`.
    ///
    /// This guards against silent misrouting of REST calls to `/users/{owner}`
    /// when GitHub introduces new account types (e.g., `Bot`, `Enterprise`).
    #[snafu(display(
        "Unknown GitHub account type '{account_type}' for owner '{owner}' — expected 'Organization' or 'User'"
    ))]
    UnknownOwnerType {
        /// The GitHub login of the owner being queried.
        owner: String,
        /// The `type` field returned by the GitHub REST API.
        account_type: String,
    },

    /// A pre-existing single-select Projects V2 field could not be healed
    /// to match the canonical option set defined in the spec.
    ///
    /// Emitted by
    /// [`GitHubClient::setup_fields`](crate::client::GitHubClient::setup_fields)
    /// when an existing single-select required field (e.g. the GitHub-default
    /// built-in `Status` field with options `[Todo, In Progress, Done]`) is
    /// detected with options that diverge from the spec
    /// (`[ready, in_progress, blocked, deferred, closed]` for Status), and
    /// the auto-heal `updateProjectV2Field` mutation either:
    ///
    /// - returned a malformed response (missing `id` or `options`); or
    /// - completed successfully but the post-mutation option set still
    ///   does not match the spec (set inequality, ignoring order).
    ///
    /// Transport-level GraphQL errors during the heal mutation surface as
    /// the more general [`GitHubGraphQL`](Self::GitHubGraphQL) variant
    /// instead — this variant is reserved for heal-specific contract
    /// violations where GitHub responded but the response is wrong.
    ///
    /// **Diagnostic value.** The pre-existing silent-pass behaviour
    /// returned a `FieldMeta` with the wrong options, surfacing as a
    /// downstream test assertion failure (`Status should have 5 options,
    /// got 3`) far from the root cause. This variant fires the alarm at
    /// the heal call site, with the field name and the specific contract
    /// breach in the `reason` field — agent-actionable.
    ///
    /// **Remediation hint.** The Display impl appends a pointer to
    /// `scripts/setup-test-project.sh <owner> <project-number>` (the
    /// cleanup script that wipes the 6 unblock-managed custom fields)
    /// and a fallback to manual edits via the project's web UI. This
    /// mirrors the `rerun X` nudge precedent on
    /// [`PreMutationPrimeFailed`](Self::PreMutationPrimeFailed) and was
    /// added per rework finding S3 in bead `unblock-aa2`.
    ///
    /// Maps to HTTP 422 via [`Error::status_code`] (Unprocessable Entity)
    /// — the request was well-formed but the upstream state cannot be
    /// reconciled to the requested shape.
    ///
    /// See bead `unblock-aa2` for the full investigation and the
    /// empirical-verification trail.
    #[snafu(display(
        "Failed to heal Projects V2 field '{field}' to canonical option set: {reason}. \
         Run `scripts/setup-test-project.sh <owner> <project-number>` to wipe stale \
         custom fields, or open the project in a browser and edit the field's options \
         manually to match the spec."
    ))]
    FieldOptionHealFailed {
        /// The field name (e.g. `"Status"`, `"Priority"`,
        /// `"PipelineStage"`) whose option-set heal failed.
        field: String,
        /// Human-readable description of the contract breach (e.g. missing
        /// id in response, post-heal option set mismatch).
        reason: String,
    },

    /// The configured GitHub token lacks the `admin:org` scope required
    /// to create org-level issue types via the REST endpoint
    /// `POST /orgs/{org}/issue-types`.
    ///
    /// Emitted by
    /// [`GitHubClient::ensure_issue_types`](crate::client::GitHubClient::ensure_issue_types)
    /// (introduced by `unblock-wgj`) when GitHub returns HTTP 403 from
    /// the issue-type-creation endpoint. The `setup_fields` `IssueType`
    /// ensure-and-heal step (spec §5.7 step 3) needs `admin:org` to
    /// allocate any of the eight canonical `IssueType` variants
    /// (`Task`, `Bug`, `Feature`, `Spike`, `Epic`, `Chore`, `Refactor`,
    /// `Docs`) that are missing on the org. Subsequent `setup` runs
    /// only need `read:org` once all eight types already exist.
    ///
    /// **Remediation hint.** The Display impl follows the
    /// [`FieldOptionHealFailed`](Self::FieldOptionHealFailed) pattern
    /// (per `unblock-aa2` finding S3) — it points operators at the
    /// fix: upgrade the configured `GITHUB_TOKEN` to a PAT carrying the
    /// `admin:org` scope (or have an org admin pre-create the missing
    /// types in the org settings UI). Agent-actionable: the message
    /// names the affected `org` and the missing capability.
    ///
    /// Maps to HTTP 403 via [`Error::status_code`]; `github_error_to_mcp`
    /// in `unblock-mcp` then maps 403 → `ErrorCode::INVALID_PARAMS`.
    ///
    /// See spec §12 / §13.3 / Appendix B and bead `unblock-wgj.21` for
    /// the full taxonomy rationale.
    #[snafu(display(
        "Cannot manage org-level GitHub issue types for '{org}': the configured token lacks the `admin:org` scope. \
         Upgrade the GITHUB_TOKEN to a PAT with `admin:org`, or have an org admin pre-create the eight canonical issue \
         types (Task, Bug, Feature, Spike, Epic, Chore, Refactor, Docs) in the org settings UI. Once they exist, \
         subsequent `setup` runs only need `read:org`."
    ))]
    IssueTypeManagementForbidden {
        /// The GitHub organisation login on which the issue-type
        /// management call was attempted (e.g. `"websublime"`).
        org: String,
    },

    /// A `MockGitHubClient` method was called without a queued stub response.
    ///
    /// Only constructible when the `test-hooks` feature is enabled. Tests
    /// should pre-load the relevant stub queue (e.g. via
    /// `MockGitHubClient::push_resolve_project_info`) before exercising the
    /// code under test, otherwise the mock returns this variant to surface
    /// the missing setup loudly instead of silently producing a default.
    #[cfg(feature = "test-hooks")]
    #[snafu(display("MockGitHubClient: `{method}` was called without a queued stub response"))]
    MockNotStubbed {
        /// The trait method that was invoked without a stub.
        method: &'static str,
    },
}

/// Formats the `errors` vector of a
/// [`Error::GitHubGraphQLForbidden`] variant for `Display`.
///
/// Renders a `(no details)` sentinel when the vector is empty (which
/// happens when every FORBIDDEN-typed response entry lacked a populated
/// `message` body; see the variant docs). Otherwise joins the messages
/// with `"; "` to match the historical `GitHubGraphQL` format.
fn format_forbidden_display(errors: &[String]) -> String {
    if errors.is_empty() {
        "GitHub GraphQL FORBIDDEN: (no details)".to_owned()
    } else {
        format!("GitHub GraphQL FORBIDDEN: {}", errors.join("; "))
    }
}

impl Error {
    /// Returns the HTTP status code associated with this error variant.
    ///
    /// Used by the MCP error conversion layer to map infrastructure errors to
    /// protocol-level error codes without coupling to the variant list.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            Self::Domain { source } => source.status_code(),
            Self::GitHubApi { status, .. } => *status,
            Self::GitHubGraphQL { .. } | Self::FieldOptionHealFailed { .. } => 422,
            Self::IssueTypeManagementForbidden { .. } => 403,
            // 403 Forbidden — GraphQL FORBIDDEN-typed error before
            // cross-repo classification upgrades it to DomainError
            // CrossRepoAccessDenied. An un-upgraded variant still
            // reaches the MCP layer with the right status-code bucket
            // so `github_error_to_mcp` maps it to INVALID_PARAMS.
            Self::GitHubGraphQLForbidden { .. } => 403,
            Self::GitHubUnavailable { .. }
            | Self::CircuitBreakerOpen { .. }
            | Self::PostMutationRebuildFailed { .. }
            | Self::PreMutationPrimeFailed { .. } => 503,
            Self::RateLimited { .. } => 429,
            Self::ProjectNotConfigured => 412,
            Self::GitRemote { .. } => 500,
            // 502 Bad Gateway: the upstream (GitHub) returned an unexpected account
            // type, so our server cannot produce a valid response.  This distinguishes
            // "GitHub gave us something we don't understand" (502) from "our own logic
            // broke" (500).  The MCP layer maps all github errors to INTERNAL_ERROR
            // regardless, so the distinction is informational for logs/diagnostics only.
            Self::UnknownOwnerType { .. } => 502,
            #[cfg(feature = "test-hooks")]
            Self::MockNotStubbed { .. } => 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::IntoError;
    use unblock_core::errors::IssueNotFoundSnafu;

    #[test]
    fn domain_display_delegates_to_source() {
        let domain_err = IssueNotFoundSnafu { number: 42_u64 }.build();
        let expected = domain_err.to_string();
        let err = Error::from(domain_err);
        assert_eq!(err.to_string(), expected);
    }

    #[test]
    fn github_api_display() {
        let err = GitHubApiSnafu {
            status: 404_u16,
            message: "Not Found".to_owned(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub API error (404): Not Found");
    }

    #[test]
    fn github_graphql_display() {
        let err = GitHubGraphQLSnafu {
            errors: vec!["Field 'x' not found".to_owned()],
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL error: Field 'x' not found");
    }

    #[test]
    fn github_graphql_display_multi_error() {
        let err = GitHubGraphQLSnafu {
            errors: vec![
                "Field 'x' not found".to_owned(),
                "Variable '$id' is not defined".to_owned(),
            ],
        }
        .build();
        assert_eq!(
            err.to_string(),
            "GitHub GraphQL error: Field 'x' not found; Variable '$id' is not defined"
        );
    }

    #[test]
    fn github_graphql_display_empty_vec() {
        let err = GitHubGraphQLSnafu {
            errors: Vec::<String>::new(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL error: ");
    }

    #[test]
    fn github_graphql_forbidden_display_and_status() {
        // The `GitHubGraphQLForbidden` variant is emitted by the
        // GraphQL reducer when any errors[i].type == "FORBIDDEN" (per
        // SPEC §11.1 wiring, user decision 2026-04-17). Display embeds
        // the FORBIDDEN messages; status_code is 403 so the un-upgraded
        // variant still reaches `github_error_to_mcp` with the right
        // bucket (INVALID_PARAMS).
        //
        // Uses an exact-string `assert_eq!` so the Display contract is
        // symmetric with the empty-vector companion test below
        // (`github_graphql_forbidden_display_empty_vec_renders_no_details_sentinel`):
        // both cases pin the full rendered string, making the contract
        // explicit and harder to accidentally weaken (bead
        // `unblock-eos.36`).
        let err = GitHubGraphQLForbiddenSnafu {
            errors: vec!["Resource not accessible by integration".to_owned()],
        }
        .build();
        assert_eq!(
            err.to_string(),
            "GitHub GraphQL FORBIDDEN: Resource not accessible by integration"
        );
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn github_graphql_forbidden_display_empty_vec_renders_no_details_sentinel() {
        // Per unblock-eos.24: the reducer emits
        // `GitHubGraphQLForbidden { errors: vec![] }` when every
        // FORBIDDEN-typed response entry lacked a populated `message`
        // body (the message-partition loop drops empty messages for
        // hygiene per unblock-eos.22, but the variant is driven by
        // `type` presence, not message presence). When the vector is
        // empty, `Display` MUST render an explicit `(no details)`
        // sentinel — not `"GitHub GraphQL FORBIDDEN: "` with a bare
        // trailing space — so log output stays self-describing. This
        // test pins the exact sentinel string.
        let err = GitHubGraphQLForbiddenSnafu {
            errors: Vec::<String>::new(),
        }
        .build();
        assert_eq!(err.to_string(), "GitHub GraphQL FORBIDDEN: (no details)");
        assert_eq!(err.status_code(), 403);
    }

    #[tokio::test]
    async fn github_unavailable_display() {
        // Connect to IPv4 loopback port 1 — port 1 is reserved/privileged and
        // reliably refused on all platforms. Uses 127.0.0.1 instead of [::1] to
        // avoid failures on CI environments where IPv6 is disabled.
        let reqwest_err = reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .expect_err("connection to port 1 should be refused");
        let err = GitHubUnavailableSnafu.into_error(reqwest_err);
        let msg = err.to_string();
        assert!(
            msg.starts_with("Cannot connect to GitHub: "),
            "unexpected display: {msg}"
        );
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn rate_limited_display() {
        let reset_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let err = RateLimitedSnafu { reset_at }.build();
        let msg = err.to_string();
        assert!(
            msg.contains("rate limit exceeded"),
            "unexpected display: {msg}"
        );
        assert!(
            msg.contains("2026-01-01"),
            "display must include reset_at: {msg}"
        );
    }

    #[test]
    fn circuit_breaker_open_display() {
        let since = std::time::Instant::now();
        let err = CircuitBreakerOpenSnafu { since }.build();
        let msg = err.to_string();
        assert!(
            msg.starts_with("Circuit breaker open"),
            "unexpected display: {msg}"
        );
    }

    #[test]
    fn project_not_configured_display() {
        let err = ProjectNotConfiguredSnafu.build();
        assert_eq!(
            err.to_string(),
            "Projects V2 not configured \u{2014} run `setup` first"
        );
    }

    #[test]
    fn git_remote_display() {
        let err = GitRemoteSnafu {
            message: "no origin remote".to_owned(),
        }
        .build();
        assert_eq!(
            err.to_string(),
            "Failed to detect git remote: no origin remote"
        );
    }

    #[test]
    fn all_non_network_variants_implement_error_trait() {
        let errors: Vec<Error> = vec![
            Error::from(IssueNotFoundSnafu { number: 1_u64 }.build()),
            GitHubApiSnafu {
                status: 500_u16,
                message: "err".to_owned(),
            }
            .build(),
            GitHubGraphQLSnafu {
                errors: vec!["err".to_owned()],
            }
            .build(),
            GitHubGraphQLForbiddenSnafu {
                errors: vec!["Resource not accessible by integration".to_owned()],
            }
            .build(),
            RateLimitedSnafu {
                reset_at: Utc::now(),
            }
            .build(),
            CircuitBreakerOpenSnafu {
                since: std::time::Instant::now(),
            }
            .build(),
            ProjectNotConfiguredSnafu.build(),
            GitRemoteSnafu {
                message: "err".to_owned(),
            }
            .build(),
        ];

        for err in &errors {
            let dyn_err: &dyn std::error::Error = err;
            _ = dyn_err;
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn status_code_domain_delegates() {
        let err = Error::from(IssueNotFoundSnafu { number: 7_u64 }.build());
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn status_code_github_api() {
        let err = GitHubApiSnafu {
            status: 403_u16,
            message: "Forbidden".to_owned(),
        }
        .build();
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn status_code_github_graphql() {
        let err = GitHubGraphQLSnafu {
            errors: vec!["bad field".to_owned()],
        }
        .build();
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn status_code_rate_limited() {
        let err = RateLimitedSnafu {
            reset_at: Utc::now(),
        }
        .build();
        assert_eq!(err.status_code(), 429);
    }

    #[test]
    fn status_code_circuit_breaker_open() {
        let err = CircuitBreakerOpenSnafu {
            since: std::time::Instant::now(),
        }
        .build();
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn status_code_project_not_configured() {
        let err = ProjectNotConfiguredSnafu.build();
        assert_eq!(err.status_code(), 412);
    }

    #[test]
    fn status_code_git_remote() {
        let err = GitRemoteSnafu {
            message: "no origin".to_owned(),
        }
        .build();
        assert_eq!(err.status_code(), 500);
    }

    #[test]
    fn status_code_post_mutation_rebuild_failed() {
        // SPEC §8.5 / §8.7 R3: post-mutation-rebuild failure is a
        // 503-class infrastructure error (transient wiring-class
        // failure). The MCP layer's `github_error_to_mcp` maps 503 →
        // `ErrorCode::INTERNAL_ERROR`; pinning the status_code here
        // guards the spec's error-contract rows against drift.
        let err = PostMutationRebuildFailedSnafu {
            mutation: "reopen".to_owned(),
            qid: unblock_core::types::QualifiedId::new("acme", "widgets", 42),
        }
        .build();
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn post_mutation_rebuild_failed_display_is_agent_actionable() {
        // Display output MUST name the preceding mutation, render the
        // QualifiedId unambiguously (`owner/repo#n` form), and instruct
        // the caller to re-run `show`. This is the agent-actionable
        // contract the `github_error_to_mcp` layer forwards to MCP
        // clients — weakening it would regress spec §8.5 / §8.7 R3.
        let err = PostMutationRebuildFailedSnafu {
            mutation: "remove_blocked_by".to_owned(),
            qid: unblock_core::types::QualifiedId::new("acme", "widgets", 42),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("remove_blocked_by"),
            "display must name the mutation: {msg}"
        );
        assert!(
            msg.contains("acme/widgets#42"),
            "display must render the QualifiedId in `owner/repo#n` form: {msg}"
        );
        assert!(
            msg.contains("re-run `show`"),
            "display must guide the caller to re-run `show`: {msg}"
        );
    }

    #[test]
    fn status_code_pre_mutation_prime_failed() {
        // Bead unblock-29p.69: pre-mutation prime failure is a 503-class
        // infrastructure error, mirroring the post-mutation rebuild
        // failure posture (PostMutationRebuildFailed). The MCP layer's
        // `github_error_to_mcp` maps 503 → `ErrorCode::INTERNAL_ERROR`;
        // pinning the status_code here guards against drift.
        let err = PreMutationPrimeFailedSnafu {
            qid: unblock_core::types::QualifiedId::new("acme", "widgets", 42),
        }
        .build();
        assert_eq!(err.status_code(), 503);
    }

    #[test]
    fn field_option_heal_failed_display_and_status() {
        // Bead unblock-aa2: emitted by setup_fields when the
        // updateProjectV2Field auto-heal cannot reconcile a pre-existing
        // single-select required field to the spec's canonical option set.
        // Display MUST name the field (so the operator knows which one),
        // surface the contract breach in `reason` so it is agent-actionable,
        // AND point operators at the cleanup script (rework finding S3).
        let err = FieldOptionHealFailedSnafu {
            field: "Status".to_owned(),
            reason: "post-heal options {\"Todo\"} do not match spec {\"ready\", \"in_progress\", \"blocked\", \"deferred\", \"closed\"}".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains("Status"), "display must name the field: {msg}");
        assert!(
            msg.contains("do not match spec"),
            "display must surface the contract breach: {msg}"
        );
        // Bead unblock-aa2 finding S3 — the message must point operators
        // at the cleanup script so the recovery path is obvious from the
        // error alone (precedent: PreMutationPrimeFailed includes a
        // 'rerun X' nudge).
        assert!(
            msg.contains("scripts/setup-test-project.sh"),
            "display must point operators at the cleanup script: {msg}"
        );
        // Status code is 422 — Unprocessable Entity — the request was
        // well-formed but the upstream state cannot be reconciled.
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn issue_type_management_forbidden_display_and_status() {
        // unblock-wgj.21: emitted by ensure_issue_types when GitHub
        // returns HTTP 403 from POST /orgs/{org}/issue-types because
        // the configured token lacks `admin:org`. Display MUST name the
        // org, surface the missing scope, and point operators at the
        // fix — same pattern as FieldOptionHealFailed (unblock-aa2 S3).
        let err = IssueTypeManagementForbiddenSnafu {
            org: "websublime".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("websublime"),
            "display must name the org: {msg}"
        );
        assert!(
            msg.contains("admin:org"),
            "display must surface the required scope: {msg}"
        );
        assert!(
            msg.contains("GITHUB_TOKEN"),
            "display must point operators at the token they need to upgrade: {msg}"
        );
        // Spec §12: maps to HTTP 403 — `github_error_to_mcp` then maps
        // 403 → INVALID_PARAMS.
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn pre_mutation_prime_failed_display_is_agent_actionable() {
        // Bead unblock-29p.69: Display output MUST render the
        // QualifiedId unambiguously (`owner/repo#n` form), make the
        // pre-mutation phase clear (`failed to prime the dependency
        // graph before cascade capture`), and guide the caller toward
        // the correct recovery (`retry or run `prime` first`) — NOT
        // toward the post-mutation `re-run `show`` recovery.
        let err = PreMutationPrimeFailedSnafu {
            qid: unblock_core::types::QualifiedId::new("acme", "widgets", 42),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("acme/widgets#42"),
            "display must render the QualifiedId in `owner/repo#n` form: {msg}"
        );
        assert!(
            msg.contains("prime the dependency graph"),
            "display must reference `prime the dependency graph` (pre-mutation path): {msg}"
        );
        assert!(
            msg.contains("before cascade capture"),
            "display must reference `before cascade capture` to identify the PRE-close ordering: {msg}"
        );
        assert!(
            msg.contains("retry") || msg.contains("`prime`"),
            "display must advise retry or running `prime` first (pre-mutation recovery hint, distinct from the post-mutation `re-run `show``): {msg}"
        );
        // Strict negative: the post-mutation R3 recovery hint MUST NOT
        // appear here — collapsing the two surfaces into the same
        // wording would regress the SPEC §8.2 pre-vs-post-mutation
        // contract that this variant exists to encode.
        assert!(
            !msg.contains("re-run `show`"),
            "Pre-mutation Display must NOT use the post-mutation `re-run `show`` wording — collapsing the two 503 surfaces breaks the SPEC §8.2 pre-vs-post-mutation contract: {msg}"
        );
    }
}
