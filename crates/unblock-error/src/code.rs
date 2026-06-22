//! The stable [`ErrorCode`] vocabulary and its `const fn` views.
//!
//! `ErrorCode` is the `SCREAMING_SNAKE_CASE` boundary vocabulary (spine §2.2) that every
//! per-crate error enum maps into via [`crate::CodedError`]. The three `const fn` views —
//! [`ErrorCode::as_str`], [`ErrorCode::exit_code`], and [`ErrorCode::is_retryable`] — are the
//! normative 0–8 exit-code table (spine §2.3) and the retryability contract (FR-11).

use serde::{Deserialize, Serialize};

/// Machine-readable, stable error codes (spine §2.2).
///
/// These codes are stable across versions and serialize as `SCREAMING_SNAKE_CASE` JSON
/// strings (e.g. `ErrorCode::IssueNotFound` ↔ `"ISSUE_NOT_FOUND"`). Each maps to exactly one
/// 0–8 exit code via [`ErrorCode::exit_code`] (spine §2.3, golden-snapshot pinned, FR-11).
///
/// New variants are added **additively** in later versions (never renumbered) so the golden
/// exit-code snapshot only ever grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    // === exit 2 — Database ===
    /// Database file not found.
    DatabaseNotFound,
    /// Database is locked by another process (retryable).
    DatabaseLocked,
    /// Database schema version mismatch.
    SchemaMismatch,
    /// Database operation failed.
    DatabaseError,
    /// Workspace not initialized.
    NotInitialized,
    /// Workspace already initialized.
    AlreadyInitialized,

    // === exit 3 — Issue / operational ===
    /// Issue with the specified id not found.
    IssueNotFound,
    /// A partial id matches multiple issues (retryable: disambiguate and retry).
    AmbiguousId,
    /// Issue id collision on create.
    IdCollision,
    /// Invalid issue id format.
    InvalidId,
    /// Nothing to do — all requested items were skipped.
    NothingToDo,
    /// Atomic claim lost: another actor already claimed the issue (retryable; FR-2).
    AlreadyClaimed,

    // === exit 4 — Validation / policy ===
    /// One or more field validations failed (retryable; carries per-field detail).
    ValidationFailed,
    /// Invalid status value (retryable).
    InvalidStatus,
    /// Invalid issue-type value (retryable).
    InvalidType,
    /// Priority out of range 0..=4 (retryable).
    InvalidPriority,
    /// A required field is missing (retryable).
    RequiredField,
    /// A closure-time policy gate fired (exit 4, non-retryable).
    PolicyViolation,

    // === exit 5 — Dependency ===
    /// A dependency cycle was detected.
    CycleDetected,
    /// The dependency target was not found.
    DependencyNotFound,
    /// Cannot proceed: the issue has dependents.
    HasDependents,
    /// An issue cannot depend on itself.
    SelfDependency,
    /// Duplicate dependency edge.
    DuplicateDependency,

    // === exit 6 — Sync / JSONL ===
    /// JSONL parse error during import.
    JsonlParseError,
    /// Prefix mismatch during import.
    PrefixMismatch,
    /// Import collision detected.
    ImportCollision,
    /// Conflict between local database changes and newer JSONL.
    SyncConflict,
    /// Conflict markers present in JSONL.
    ConflictMarkers,
    /// A path-traversal attempt was blocked.
    PathTraversal,

    // === exit 7 — Config ===
    /// Configuration error.
    ConfigError,
    /// Config file not found.
    ConfigNotFound,
    /// Config parse error.
    ConfigParseError,

    // === exit 8 — I/O ===
    /// File I/O error.
    IoError,
    /// JSON serialization error.
    JsonError,

    // === exit 1 — Internal ===
    /// Unexpected internal error.
    InternalError,
}

impl ErrorCode {
    /// Every `ErrorCode` variant, in declaration order.
    ///
    /// This exhaustive array drives the golden contract suite (spine §2.3): a new variant must
    /// be added here, which forces the exit-code table snapshot to be re-blessed deliberately.
    /// The compiler enforces the count via the type — adding a variant without extending this
    /// array does not break compilation, so the `as_str` ↔ serde cross-assertion and the
    /// `tests/exit_code_table.rs` golden are the guards that catch an omission.
    pub const ALL: [Self; 35] = [
        Self::DatabaseNotFound,
        Self::DatabaseLocked,
        Self::SchemaMismatch,
        Self::DatabaseError,
        Self::NotInitialized,
        Self::AlreadyInitialized,
        Self::IssueNotFound,
        Self::AmbiguousId,
        Self::IdCollision,
        Self::InvalidId,
        Self::NothingToDo,
        Self::AlreadyClaimed,
        Self::ValidationFailed,
        Self::InvalidStatus,
        Self::InvalidType,
        Self::InvalidPriority,
        Self::RequiredField,
        Self::PolicyViolation,
        Self::CycleDetected,
        Self::DependencyNotFound,
        Self::HasDependents,
        Self::SelfDependency,
        Self::DuplicateDependency,
        Self::JsonlParseError,
        Self::PrefixMismatch,
        Self::ImportCollision,
        Self::SyncConflict,
        Self::ConflictMarkers,
        Self::PathTraversal,
        Self::ConfigError,
        Self::ConfigNotFound,
        Self::ConfigParseError,
        Self::IoError,
        Self::JsonError,
        Self::InternalError,
    ];

    /// The stable `SCREAMING_SNAKE_CASE` string for this code (matches the serde representation).
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_error::ErrorCode;
    /// assert_eq!(ErrorCode::IssueNotFound.as_str(), "ISSUE_NOT_FOUND");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            // Database
            Self::DatabaseNotFound => "DATABASE_NOT_FOUND",
            Self::DatabaseLocked => "DATABASE_LOCKED",
            Self::SchemaMismatch => "SCHEMA_MISMATCH",
            Self::DatabaseError => "DATABASE_ERROR",
            Self::NotInitialized => "NOT_INITIALIZED",
            Self::AlreadyInitialized => "ALREADY_INITIALIZED",
            // Issue / operational
            Self::IssueNotFound => "ISSUE_NOT_FOUND",
            Self::AmbiguousId => "AMBIGUOUS_ID",
            Self::IdCollision => "ID_COLLISION",
            Self::InvalidId => "INVALID_ID",
            Self::NothingToDo => "NOTHING_TO_DO",
            Self::AlreadyClaimed => "ALREADY_CLAIMED",
            // Validation / policy
            Self::ValidationFailed => "VALIDATION_FAILED",
            Self::InvalidStatus => "INVALID_STATUS",
            Self::InvalidType => "INVALID_TYPE",
            Self::InvalidPriority => "INVALID_PRIORITY",
            Self::RequiredField => "REQUIRED_FIELD",
            Self::PolicyViolation => "POLICY_VIOLATION",
            // Dependency
            Self::CycleDetected => "CYCLE_DETECTED",
            Self::DependencyNotFound => "DEPENDENCY_NOT_FOUND",
            Self::HasDependents => "HAS_DEPENDENTS",
            Self::SelfDependency => "SELF_DEPENDENCY",
            Self::DuplicateDependency => "DUPLICATE_DEPENDENCY",
            // Sync / JSONL
            Self::JsonlParseError => "JSONL_PARSE_ERROR",
            Self::PrefixMismatch => "PREFIX_MISMATCH",
            Self::ImportCollision => "IMPORT_COLLISION",
            Self::SyncConflict => "SYNC_CONFLICT",
            Self::ConflictMarkers => "CONFLICT_MARKERS",
            Self::PathTraversal => "PATH_TRAVERSAL",
            // Config
            Self::ConfigError => "CONFIG_ERROR",
            Self::ConfigNotFound => "CONFIG_NOT_FOUND",
            Self::ConfigParseError => "CONFIG_PARSE_ERROR",
            // I/O
            Self::IoError => "IO_ERROR",
            Self::JsonError => "JSON_ERROR",
            // Internal
            Self::InternalError => "INTERNAL_ERROR",
        }
    }

    /// The 0–8 exit code for this category (spine §2.3, golden-snapshot pinned).
    ///
    /// Exit code `0` is reserved for success and is emitted by no `ErrorCode`.
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_error::ErrorCode;
    /// assert_eq!(ErrorCode::IssueNotFound.exit_code(), 3);
    /// assert_eq!(ErrorCode::InternalError.exit_code(), 1);
    /// ```
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            // Database (2)
            Self::DatabaseNotFound
            | Self::DatabaseLocked
            | Self::SchemaMismatch
            | Self::DatabaseError
            | Self::NotInitialized
            | Self::AlreadyInitialized => 2,
            // Issue / operational (3)
            Self::IssueNotFound
            | Self::AmbiguousId
            | Self::IdCollision
            | Self::InvalidId
            | Self::NothingToDo
            | Self::AlreadyClaimed => 3,
            // Validation / policy (4)
            Self::ValidationFailed
            | Self::InvalidStatus
            | Self::InvalidType
            | Self::InvalidPriority
            | Self::RequiredField
            | Self::PolicyViolation => 4,
            // Dependency (5)
            Self::CycleDetected
            | Self::DependencyNotFound
            | Self::HasDependents
            | Self::SelfDependency
            | Self::DuplicateDependency => 5,
            // Sync / JSONL (6)
            Self::JsonlParseError
            | Self::PrefixMismatch
            | Self::ImportCollision
            | Self::SyncConflict
            | Self::ConflictMarkers
            | Self::PathTraversal => 6,
            // Config (7)
            Self::ConfigError | Self::ConfigNotFound | Self::ConfigParseError => 7,
            // I/O (8)
            Self::IoError | Self::JsonError => 8,
            // Internal (1)
            Self::InternalError => 1,
        }
    }

    /// Whether an operation that produced this code is potentially retryable.
    ///
    /// Retryable means the agent might succeed if it waits and retries (e.g. a transient lock or
    /// a lost claim) or fixes the input and retries (e.g. a validation error). The exact set is
    /// pinned by the spine §2.2 (no glob): `DatabaseLocked`, `AlreadyClaimed`, `ValidationFailed`,
    /// `InvalidStatus`, `InvalidType`, `InvalidPriority`, `RequiredField`, `AmbiguousId`.
    /// `PolicyViolation` is exit-4 but **not** retryable.
    ///
    /// # Examples
    ///
    /// ```
    /// use unblock_error::ErrorCode;
    /// assert!(ErrorCode::AlreadyClaimed.is_retryable());
    /// assert!(!ErrorCode::PolicyViolation.is_retryable());
    /// ```
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::DatabaseLocked
                | Self::AlreadyClaimed
                | Self::ValidationFailed
                | Self::InvalidStatus
                | Self::InvalidType
                | Self::InvalidPriority
                | Self::RequiredField
                | Self::AmbiguousId
        )
    }
}
