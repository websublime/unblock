//! Domain error types using snafu.
//!
//! Defines `DomainError` with variants for all domain-level failure modes.
//! Each variant carries the relevant context and maps to an HTTP status code
//! via `DomainError::status_code`.
//!
//! Variants: `IssueNotFound`, `AlreadyClaimed`, `IssueBlocked`, `IssueDeferred`,
//! `IssueClosed`, `IssueNotClosed`, `IssueAlreadyOpen`, `CircularDependency`,
//! `DuplicateDependency`, `FieldNotFound`, `Validation`.

use snafu::prelude::*;

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
    #[snafu(display("Issue #{number} is blocked by: {blockers:?}"))]
    IssueBlocked {
        /// The issue number.
        number: u64,
        /// Issue numbers that block this issue.
        blockers: Vec<u64>,
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
    #[snafu(display("Circular dependency: adding #{source} → #{target} creates cycle"))]
    CircularDependency {
        /// The source issue number of the proposed edge.
        #[snafu(source(false))]
        source: u64,
        /// The target issue number of the proposed edge.
        target: u64,
    },

    /// The blocking relationship already exists.
    #[snafu(display("Blocking relationship already exists: #{source} → #{target}"))]
    DuplicateDependency {
        /// The source issue number.
        #[snafu(source(false))]
        source: u64,
        /// The target issue number.
        target: u64,
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
}

impl DomainError {
    /// Returns the HTTP status code associated with this error variant.
    ///
    /// Used by the MCP error conversion layer to map domain errors to
    /// protocol-level error codes without coupling to the variant list.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            Self::IssueNotFound { .. } | Self::FieldNotFound { .. } => 404,
            Self::Validation { .. } => 400,
            Self::CircularDependency { .. } => 422,
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
        assert!(err.to_string().contains("7"));
        assert_eq!(err.status_code(), 409);
    }

    #[test]
    fn issue_blocked_display_and_status() {
        let err = IssueBlockedSnafu {
            number: 10_u64,
            blockers: vec![1_u64, 2],
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
        assert!(msg.contains("3"));
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
            source: 1_u64,
            target: 2_u64,
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains('1'));
        assert!(msg.contains('2'));
        assert!(msg.contains("cycle"));
        assert_eq!(err.status_code(), 422);
    }

    #[test]
    fn duplicate_dependency_display_and_status() {
        let err = DuplicateDependencySnafu {
            source: 4_u64,
            target: 5_u64,
        }
        .build();
        let msg = err.to_string();
        assert!(msg.contains('4'));
        assert!(msg.contains('5'));
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
                blockers: vec![2_u64],
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
                source: 1_u64,
                target: 2_u64,
            }
            .build(),
            DuplicateDependencySnafu {
                source: 1_u64,
                target: 2_u64,
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
        ];

        for err in &errors {
            // This line would fail to compile if DomainError didn't impl Error
            let _dyn_err: &dyn std::error::Error = err;
            assert!(!err.to_string().is_empty());
        }
    }
}
