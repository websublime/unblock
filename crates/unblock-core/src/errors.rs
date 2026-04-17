//! Domain error types using snafu.
//!
//! Defines `DomainError` with variants for all domain-level failure modes.
//! Each variant carries the relevant context and maps to an HTTP status code
//! via `DomainError::status_code`.
//!
//! 13 variants: `IssueNotFound`, `AlreadyClaimed`, `IssueBlocked`, `IssueDeferred`,
//! `IssueClosed`, `IssueNotClosed`, `IssueAlreadyOpen`, `CircularDependency`,
//! `DuplicateDependency`, `FieldNotFound`, `Validation`, `InvalidIssueRef`,
//! `CrossRepoAccessDenied`.

use snafu::prelude::*;

use crate::types::IssueRef;

/// Renders a list of [`IssueRef`] via their `Display` impl for use in
/// `#[snafu(display(...))]` attributes.
///
/// Joins each ref with a comma + space so the output reads like
/// `"#1, #2, acme/widgets#3"` — matching the `Display`-based rendering
/// contract at SPEC §11.1 and avoiding the variant-leaking `Debug`
/// formatter (`[Local(1), Local(2)]`).
fn render_blockers(blockers: &[IssueRef]) -> String {
    blockers
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Domain-level errors for the unblock system.
///
/// Each variant represents a specific business-rule violation or lookup failure.
/// Use the generated snafu context selectors (e.g. [`IssueNotFoundSnafu`]) to
/// construct errors ergonomically.
///
/// The [`status_code`](DomainError::status_code) method maps each variant to
/// the appropriate HTTP status code for MCP error conversion.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum DomainError {
    /// The requested issue does not exist.
    #[snafu(display("Issue not found: #{number}"))]
    IssueNotFound {
        /// The issue number that was not found.
        number: u64,
    },

    /// The issue is already claimed by another agent.
    #[snafu(display("Issue #{number} is already claimed by {agent}"))]
    AlreadyClaimed {
        /// The issue number.
        number: u64,
        /// The agent that currently holds the claim.
        agent: String,
    },

    /// The issue has unresolved blocking dependencies.
    ///
    /// `blockers` carries [`IssueRef`] values so cross-repo blockers
    /// (which are observable via GitHub's native `trackedByIssues`
    /// connection) render with `owner/repo#n` qualification in the
    /// error message, rather than aliasing to a same-numbered local
    /// issue. See SPEC §11.1 Decision 1 (2026-04-17).
    #[snafu(display("Issue #{number} is blocked by: {}", render_blockers(blockers)))]
    IssueBlocked {
        /// The issue number.
        number: u64,
        /// Issue references that block this issue. Each entry is an
        /// [`IssueRef`] so cross-repo blockers are disambiguated from
        /// same-numbered local issues in the rendered message.
        blockers: Vec<IssueRef>,
    },

    /// The issue is deferred until a future date.
    #[snafu(display("Issue #{number} is deferred until {until}"))]
    IssueDeferred {
        /// The issue number.
        number: u64,
        /// Human-readable deferral timestamp or date string.
        until: String,
    },

    /// The issue is already closed.
    #[snafu(display("Issue #{number} is already closed"))]
    IssueClosed {
        /// The issue number.
        number: u64,
    },

    /// The issue is not closed, so it cannot be reopened.
    #[snafu(display("Issue #{number} is not closed — cannot reopen"))]
    IssueNotClosed {
        /// The issue number.
        number: u64,
    },

    /// The issue is already open.
    #[snafu(display("Issue #{number} is already open"))]
    IssueAlreadyOpen {
        /// The issue number.
        number: u64,
    },

    /// Adding the dependency would create a cycle in the graph.
    ///
    /// `source` / `target` carry [`IssueRef`] so cross-repo participants
    /// render with `owner/repo#n` qualification. The literal `#` prefix
    /// is intentionally absent from the format string because
    /// `IssueRef::Local(n)` renders as `#n` via its own `Display` impl —
    /// adding the prefix here would produce `##n` in the output. See
    /// SPEC §11.1 Decision 1 (2026-04-17) and the byte-for-byte
    /// preservation contract for the `Local`-only case.
    #[snafu(display("Circular dependency: adding {source} → {target} creates cycle"))]
    CircularDependency {
        /// The source issue reference of the proposed edge.
        #[snafu(source(false))]
        source: IssueRef,
        /// The target issue reference of the proposed edge.
        target: IssueRef,
    },

    /// The blocking relationship already exists.
    ///
    /// `source` / `target` carry [`IssueRef`] so cross-repo participants
    /// render with `owner/repo#n` qualification. The literal `#` prefix
    /// is intentionally absent from the format string because
    /// `IssueRef::Local(n)` renders as `#n` via its own `Display` impl —
    /// adding the prefix here would produce `##n` in the output. See
    /// SPEC §11.1 Decision 1 (2026-04-17).
    #[snafu(display("Blocking relationship already exists: {source} → {target}"))]
    DuplicateDependency {
        /// The source issue reference.
        #[snafu(source(false))]
        source: IssueRef,
        /// The target issue reference.
        target: IssueRef,
    },

    /// A referenced field does not exist.
    #[snafu(display("Field not found: {name}"))]
    FieldNotFound {
        /// The name of the missing field.
        name: String,
    },

    /// Input validation failed.
    #[snafu(display("Validation: {message}"))]
    Validation {
        /// Description of the validation failure.
        message: String,
    },

    /// An issue reference string failed to parse into an [`IssueRef`].
    ///
    /// Emitted whenever `IssueRef::from_str` fails on tool input at the
    /// MCP tool boundary (`show`, `depends`, `dep_remove`,
    /// `create.blocked_by`). Carries the raw user-provided string so
    /// agents can see exactly what they sent. Per SPEC §11.1 this maps
    /// to HTTP 400.
    #[snafu(display("Invalid issue reference: '{input}'"))]
    InvalidIssueRef {
        /// The raw user input that failed to parse as an [`IssueRef`].
        input: String,
    },

    /// The configured token lacks access to a referenced cross-repo issue.
    ///
    /// Emitted when a GraphQL fetch against a cross-repo node returns
    /// `FORBIDDEN` (or equivalent HTTP 403). Carries the target
    /// `owner/repo` so the agent can surface which cross-repo access
    /// the token is missing. Per SPEC §11.1 this maps to HTTP 403.
    #[snafu(display("Access denied to cross-repo issue {owner}/{repo}"))]
    CrossRepoAccessDenied {
        /// The owner of the cross-repo the token cannot access.
        owner: String,
        /// The repository name of the cross-repo the token cannot access.
        repo: String,
    },
}

impl DomainError {
    /// Returns the HTTP status code associated with this error variant.
    ///
    /// Used by the MCP error conversion layer to map domain errors to
    /// protocol-level error codes without coupling to the variant list.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            // 404 Not Found
            Self::IssueNotFound { .. } | Self::FieldNotFound { .. } => 404,
            // 400 Bad Request
            Self::Validation { .. } | Self::InvalidIssueRef { .. } => 400,
            // 403 Forbidden — cross-repo access denial
            Self::CrossRepoAccessDenied { .. } => 403,
            // 422 Unprocessable Entity
            Self::CircularDependency { .. } => 422,
            // 409 Conflict
            Self::AlreadyClaimed { .. }
            | Self::IssueBlocked { .. }
            | Self::IssueDeferred { .. }
            | Self::IssueClosed { .. }
            | Self::IssueNotClosed { .. }
            | Self::IssueAlreadyOpen { .. }
            | Self::DuplicateDependency { .. } => 409,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_not_found_display_and_status() {
        let err = IssueNotFoundSnafu { number: 42_u64 }.build();
        assert!(!err.to_string().is_empty());
        assert!(err.to_string().contains("42"));
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn already_claimed_display_and_status() {
        let err = AlreadyClaimedSnafu {
            number: 7_u64,
            agent: "bot-1".to_owned(),
        }
        .build();
        assert!(err.to_string().contains("bot-1"));
        assert!(err.to_string().contains('7'));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_blocked_display_and_status() {
        let err = IssueBlockedSnafu {
            number: 10_u64,
            blockers: vec![IssueRef::Local(1), IssueRef::Local(2)],
        }
        .build();
        let msg = err.to_string();
        assert!(!msg.is_empty());
        assert!(msg.contains("10"));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_deferred_display_and_status() {
        let err = IssueDeferredSnafu {
            number: 5_u64,
            until: "2026-04-01".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains("2026-04-01"));
        assert!(!msg.is_empty());
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_closed_display_and_status() {
        let err = IssueClosedSnafu { number: 99_u64 }.build();
        assert!(err.to_string().contains("99"));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_not_closed_display_and_status() {
        let err = IssueNotClosedSnafu { number: 3_u64 }.build();
        let msg = err.to_string();
        assert!(msg.contains('3'));
        assert!(msg.contains("not closed"));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_already_open_display_and_status() {
        let err = IssueAlreadyOpenSnafu { number: 15_u64 }.build();
        let msg = err.to_string();
        assert!(msg.contains("15"));
        assert!(msg.contains("already open"));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn circular_dependency_display_and_status() {
        let err = CircularDependencySnafu {
            source: IssueRef::Local(1),
            target: IssueRef::Local(2),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains('1'));
        assert!(msg.contains('2'));
        assert!(msg.contains("cycle"));
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn circular_dependency_display_byte_for_byte_local_only() {
        // SPEC §11.1:1722 locks the Local-only Display form:
        // "Circular dependency: adding #1 → #2 creates cycle".
        // This guards against format-string drift now that `IssueRef`
        // owns the `#` prefix.
        let err = CircularDependencySnafu {
            source: IssueRef::Local(1),
            target: IssueRef::Local(2),
        }
        .build();
        assert_eq!(
            err.to_string(),
            "Circular dependency: adding #1 → #2 creates cycle"
        );
    }

    #[test]
    fn circular_dependency_cross_repo_display() {
        // Cross-repo source; local target. Verifies `owner/repo#n` is
        // surfaced in the error message (plan Task 02.02 contract).
        let err = CircularDependencySnafu {
            source: IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 1,
            },
            target: IssueRef::Local(2),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("acme/widgets#1"),
            "expected qualified source ref in message, got: {msg}"
        );
        assert!(msg.contains("#2"), "expected local target ref, got: {msg}");
        assert!(msg.contains("cycle"));
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn duplicate_dependency_display_and_status() {
        let err = DuplicateDependencySnafu {
            source: IssueRef::Local(4),
            target: IssueRef::Local(5),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains('4'));
        assert!(msg.contains('5'));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn duplicate_dependency_display_byte_for_byte_local_only() {
        // SPEC §11.1:1723 locks the Local-only Display form:
        // "Blocking relationship already exists: #4 → #5".
        let err = DuplicateDependencySnafu {
            source: IssueRef::Local(4),
            target: IssueRef::Local(5),
        }
        .build();
        assert_eq!(
            err.to_string(),
            "Blocking relationship already exists: #4 → #5"
        );
    }

    #[test]
    fn duplicate_dependency_cross_repo_display() {
        // Cross-repo source AND cross-repo target (both in the same
        // foreign repo — the shape produced by
        // `add_blocked_by_refs` cross-repo source arm).
        let err = DuplicateDependencySnafu {
            source: IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 4,
            },
            target: IssueRef::CrossRepo {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
                number: 5,
            },
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("acme/widgets#4"),
            "expected qualified source ref in message, got: {msg}"
        );
        assert!(
            msg.contains("acme/widgets#5"),
            "expected qualified target ref in message, got: {msg}"
        );
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_blocked_cross_repo_display() {
        // Verifies the `render_blockers` Display helper replaces the
        // legacy `{blockers:?}` Debug formatter: variant names must NOT
        // leak, and each cross-repo blocker must carry its
        // `owner/repo#n` qualification.
        let err = IssueBlockedSnafu {
            number: 10_u64,
            blockers: vec![
                IssueRef::CrossRepo {
                    owner: "acme".to_owned(),
                    repo: "widgets".to_owned(),
                    number: 1,
                },
                IssueRef::Local(2),
            ],
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains("10"), "expected issue number, got: {msg}");
        assert!(
            msg.contains("acme/widgets#1"),
            "expected qualified cross-repo blocker, got: {msg}"
        );
        assert!(msg.contains("#2"), "expected local blocker ref, got: {msg}");
        assert!(
            !msg.contains("Local(") && !msg.contains("CrossRepo"),
            "Debug-formatted variant names must not leak into Display, got: {msg}"
        );
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn field_not_found_display_and_status() {
        let err = FieldNotFoundSnafu {
            name: "priority".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains("priority"));
        assert_eq!(err.status_code(), 404);
    }

    #[test]
    fn validation_display_and_status() {
        let err = ValidationSnafu {
            message: "GITHUB_TOKEN is required".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains("GITHUB_TOKEN"));
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn invalid_issue_ref_display_and_status() {
        // SPEC §11.1 — `InvalidIssueRef { input }` surfaces the raw user
        // input in the Display output so agents can see exactly what
        // they sent, and maps to HTTP 400.
        let err = InvalidIssueRefSnafu {
            input: "not-a-ref".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("not-a-ref"),
            "expected raw input in message, got: {msg}"
        );
        assert_eq!(err.status_code(), 400);
    }

    #[test]
    fn cross_repo_access_denied_display_and_status() {
        // SPEC §11.1 — `CrossRepoAccessDenied { owner, repo }` surfaces
        // the target `owner/repo` so agents know which cross-repo their
        // token cannot access. Maps to HTTP 403 (first variant to claim
        // this status code in the domain-error bucket).
        let err = CrossRepoAccessDeniedSnafu {
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
        }
        .build();
        let msg = err.to_string();
        assert!(
            msg.contains("acme"),
            "expected owner in message, got: {msg}"
        );
        assert!(
            msg.contains("widgets"),
            "expected repo in message, got: {msg}"
        );
        assert_eq!(err.status_code(), 403);
    }

    #[test]
    fn all_variants_implement_error_trait() {
        // Verify DomainError implements std::error::Error by using it as &dyn Error
        let errors: Vec<DomainError> = vec![
            IssueNotFoundSnafu { number: 1_u64 }.build(),
            AlreadyClaimedSnafu {
                number: 1_u64,
                agent: "a".to_owned(),
            }
            .build(),
            IssueBlockedSnafu {
                number: 1_u64,
                blockers: vec![IssueRef::Local(2)],
            }
            .build(),
            IssueDeferredSnafu {
                number: 1_u64,
                until: "tomorrow".to_owned(),
            }
            .build(),
            IssueClosedSnafu { number: 1_u64 }.build(),
            IssueNotClosedSnafu { number: 1_u64 }.build(),
            IssueAlreadyOpenSnafu { number: 1_u64 }.build(),
            CircularDependencySnafu {
                source: IssueRef::Local(1),
                target: IssueRef::Local(2),
            }
            .build(),
            DuplicateDependencySnafu {
                source: IssueRef::Local(1),
                target: IssueRef::Local(2),
            }
            .build(),
            FieldNotFoundSnafu {
                name: "x".to_owned(),
            }
            .build(),
            ValidationSnafu {
                message: "bad".to_owned(),
            }
            .build(),
            InvalidIssueRefSnafu {
                input: "bad-ref".to_owned(),
            }
            .build(),
            CrossRepoAccessDeniedSnafu {
                owner: "acme".to_owned(),
                repo: "widgets".to_owned(),
            }
            .build(),
        ];

        for err in &errors {
            // This line would fail to compile if DomainError didn't impl Error
            let dyn_err: &dyn std::error::Error = err;
            _ = dyn_err;
            assert!(!err.to_string().is_empty());
        }
    }
}
