//! Pure issue validation (spine §1.9; no I/O).
//!
//! [`IssueValidator::validate`] is a **uniform aggregate** (D-E1, spine §2.1): it collects EVERY
//! field-level failure into a `Vec<FieldError>` and returns
//! `Err(ModelError::ValidationFailed { fields })` iff that vec is non-empty. An empty/whitespace
//! title is a `FieldError { field: "title", reason: "cannot be empty" }` row **inside** the
//! aggregate — it is never routed to the scalar [`unblock_error::ModelError::RequiredField`] (that
//! scalar is reserved for the single-field `FromStr` paths). The full rule set is ported verbatim
//! from the original `validation/mod.rs` (L85–256).

use unblock_error::{FieldError, ModelError};

use crate::enums::{IssueType, Priority, Status};
use crate::id::{MAX_ID_LENGTH, is_valid_id_format};
use crate::issue::Issue;

/// Maximum title length, in `char`s (not UTF-8 bytes).
pub const TITLE_MAX_CHARS: usize = 500;
/// Maximum length, in `char`s, of an actor-style field (`assignee`/`owner`/`created_by`/`source_system`).
pub const ACTOR_MAX_CHARS: usize = 200;
/// Maximum length, in `char`s, of a `Custom` enum variant value (Status/IssueType only; Q5).
pub const CUSTOM_VARIANT_MAX_CHARS: usize = 50;
/// Maximum number of labels on an issue.
pub const ISSUE_LABEL_MAX_COUNT: usize = 64;
/// Maximum `estimated_minutes` (~1 year).
pub const ESTIMATED_MINUTES_MAX: i32 = 525_960;

/// Maximum length of an `external_ref`, in `char`s.
///
/// **INTENTIONAL byte→char deviation from the verbatim Go port.** The original measured this bound
/// (and the id-length bound below) in bytes; we measure `char`s — UTF-8-correct and consistent with
/// the other length rules here (`title`, the actor fields, the custom-variant cap). External refs
/// are effectively ASCII so the observable behaviour is identical, and `external_ref` is **not**
/// part of the frozen `content_hash`, so import idempotency (FR-26) is unaffected. This is a
/// deliberate correctness improvement, not a port error.
///
/// `pub` (additive, re-exported from the crate root) so callers that **repair** an issue back to a
/// validatable shape (e.g. the `unblock-fuzz` `normalize_issue`) can clamp to the validator's own
/// bound instead of duplicating the magic number.
pub const EXTERNAL_REF_MAX_CHARS: usize = 200;
/// Maximum length of a single label, in bytes (ASCII-only charset, so bytes == chars).
///
/// `pub` (additive, re-exported from the crate root) for the same reason as
/// [`EXTERNAL_REF_MAX_CHARS`]: a repair pass clamps to the validator's bound, not a copy of it.
pub const LABEL_MAX_LEN: usize = 50;

/// Bound a **resolved** actor string (single-home actor validation — Seam A, spine §1.9).
///
/// The model owns this rule once; `unblock-config` is its v1 caller (CLI/MCP become later callers),
/// so the bound lives in exactly one place. An actor is valid iff it is at most
/// [`ACTOR_MAX_CHARS`] **`char`s** long (counted with [`str::chars`], NOT UTF-8 bytes), contains no
/// NUL, and contains no other control character ([`char::is_control`]).
///
/// The caller is expected to have already treated empty/whitespace as "unset" (the resolved actor
/// reaching here is the non-empty value chosen by the precedence chain); this function does not
/// trim or reject emptiness — it only enforces the upper bound and the control-char rule.
///
/// # Errors
///
/// Returns a [`FieldError`] (`field: "actor"`) when the actor exceeds [`ACTOR_MAX_CHARS`] `char`s,
/// contains a NUL byte, or contains any other control character.
pub fn validate_actor(actor: &str) -> Result<(), FieldError> {
    if actor.chars().count() > ACTOR_MAX_CHARS {
        return Err(FieldError::new(
            "actor",
            format!("exceeds {ACTOR_MAX_CHARS} characters"),
        ));
    }
    if actor.contains('\0') {
        return Err(FieldError::new("actor", "cannot contain NUL bytes"));
    }
    if actor.chars().any(char::is_control) {
        return Err(FieldError::new(
            "actor",
            "cannot contain control characters",
        ));
    }
    Ok(())
}

/// Validates comment fields (FR-6/D37, spine §1.9). Pure; no I/O.
///
/// The model owns the comment rule set ONCE (the §1.9 single-home rule). There are **two** public
/// entry points because the two engine call sites carry different field sets (spine §4.1):
///
/// * `Session::add_comment(issue_id, body)` has `issue_id` + `author` (= the session actor) + `body`
///   → [`CommentValidator::validate_comment`];
/// * `Session::update_comment(comment_id, body)` carries ONLY a comment id + a body — that path has
///   no `issue_id` and there is no `Storage::get_comment(comment_id)` (spine §3.2) to fetch one →
///   [`CommentValidator::validate_body`].
///
/// **Composition (NORMATIVE — `validate_comment` must NOT call `validate_body`):** `validate_body`
/// returns an ALREADY-SEALED `Result`, so the natural `validate_body(body)?` inside
/// `validate_comment` would be fail-fast — the FR-11 wire `context["fields"]` would carry ONE entry
/// where [`IssueValidator::validate`] carries N, silently breaking the D-E1 uniform aggregate
/// carrier (spine §2.1). Instead BOTH public entry points call the private `body_rules`, each
/// sealing its own aggregate: the body rule set stays single-homed AND every `CommentValidator`
/// error is a full multi-fault aggregate.
pub struct CommentValidator;

impl CommentValidator {
    /// THE single home of the body rule set. Pushes into the CALLER's aggregate; never seals.
    ///
    /// The body must be non-empty when trimmed (`field: "content"` — bd wire parity) and NUL-free
    /// (`SQLite` compatibility). It is otherwise UNBOUNDED: the L7 MCP `Quotas.max_string_len`
    /// (64 KiB) is the transport cap.
    fn body_rules(body: &str, fields: &mut Vec<FieldError>) {
        if body.trim().is_empty() {
            fields.push(FieldError::new("content", "cannot be empty"));
        }
        reject_nul("content", body, fields);
    }

    /// The update path (spine §4.1 `update_comment`): validate the body only.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ValidationFailed`] carrying one [`FieldError`] per violated body rule.
    pub fn validate_body(body: &str) -> Result<(), ModelError> {
        let mut fields: Vec<FieldError> = Vec::new();
        Self::body_rules(body, &mut fields);
        seal(fields)
    }

    /// The add path (spine §4.1 `add_comment`): the body rules PLUS `author` and `issue_id`, all
    /// collected into ONE aggregate and sealed ONCE.
    ///
    /// The author rules DELEGATE the bound / NUL / control-char checks to [`validate_actor`] (their
    /// single home) and RELABEL the returned [`FieldError`]'s `field` from `"actor"` to `"author"`,
    /// ADDING the non-empty-when-trimmed rule `validate_actor` deliberately does not enforce (its
    /// contract: the resolved actor is already non-empty via the config precedence chain). The
    /// bound therefore stays [`ACTOR_MAX_CHARS`] **`char`s** — a deliberate adaptation of the Go
    /// original's `len() > 200` bytes-on-untrimmed rule; the original's `id <= 0` rule is DROPPED
    /// (the id is storage-minted here, never caller-supplied).
    ///
    /// The [`FieldError`] names are WIRE CONTRACT (FR-11 `context["fields"][].field`):
    /// body → `"content"`, author → `"author"`, `issue_id` → `"issue_id"`.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ValidationFailed`] carrying one [`FieldError`] per violated rule.
    pub fn validate_comment(issue_id: &str, author: &str, body: &str) -> Result<(), ModelError> {
        let mut fields: Vec<FieldError> = Vec::new();

        if issue_id.trim().is_empty() {
            fields.push(FieldError::new("issue_id", "cannot be empty"));
        }

        if author.trim().is_empty() {
            fields.push(FieldError::new("author", "cannot be empty"));
        }
        if let Err(mut err) = validate_actor(author) {
            err.field = "author".to_string();
            fields.push(err);
        }

        Self::body_rules(body, &mut fields);

        seal(fields)
    }
}

/// Seal a collected aggregate into the uniform D-E1 carrier (spine §2.1).
fn seal(fields: Vec<FieldError>) -> Result<(), ModelError> {
    if fields.is_empty() {
        Ok(())
    } else {
        Err(ModelError::ValidationFailed { fields })
    }
}

/// Validates issue fields and invariants (spine §1.9). Pure; no I/O.
pub struct IssueValidator;

impl IssueValidator {
    /// Validate an issue, collecting **all** field-level failures into a single aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::ValidationFailed`] carrying one [`FieldError`] per violated rule when
    /// any rule fails; otherwise `Ok(())`.
    pub fn validate(issue: &Issue) -> Result<(), ModelError> {
        let mut fields: Vec<FieldError> = Vec::new();

        Self::validate_id(issue, &mut fields);
        Self::validate_text_fields(issue, &mut fields);

        // Priority: 0..=4.
        if issue.priority.0 < Priority::CRITICAL.0 || issue.priority.0 > Priority::BACKLOG.0 {
            fields.push(FieldError::new("priority", "must be 0-4"));
        }

        // Timestamps: created_at <= updated_at.
        if issue.updated_at < issue.created_at {
            fields.push(FieldError::new("updated_at", "cannot be before created_at"));
        }

        // Estimated minutes: non-negative and bounded.
        if let Some(minutes) = issue.estimated_minutes {
            if minutes < 0 {
                fields.push(FieldError::new("estimated_minutes", "cannot be negative"));
            } else if minutes > ESTIMATED_MINUTES_MAX {
                fields.push(FieldError::new(
                    "estimated_minutes",
                    "exceeds maximum (525960 minutes / ~1 year)",
                ));
            }
        }

        // Closed-state coherence.
        if issue.status == Status::Closed && issue.closed_at.is_none() {
            fields.push(FieldError::new(
                "closed_at",
                "closed issues must set closed_at",
            ));
        }
        if !matches!(issue.status, Status::Closed | Status::Tombstone) && issue.closed_at.is_some()
        {
            fields.push(FieldError::new(
                "closed_at",
                "only closed or tombstone issues may set closed_at",
            ));
        }
        if let Some(closed_at) = issue.closed_at
            && closed_at < issue.created_at
        {
            fields.push(FieldError::new("closed_at", "cannot be before created_at"));
        }

        if fields.is_empty() {
            Ok(())
        } else {
            Err(ModelError::ValidationFailed { fields })
        }
    }

    fn validate_id(issue: &Issue, fields: &mut Vec<FieldError>) {
        if issue.id.trim().is_empty() {
            fields.push(FieldError::new("id", "cannot be empty"));
        }
        // Byte length, ported verbatim from the Go original. This is NOT the `char`-based deviation
        // noted on `EXTERNAL_REF_MAX_CHARS`: a syntactically valid id is ASCII-only (see
        // `id::parse_id`'s charset), so `len()` == `chars().count()` for anything that passes the
        // format check — keeping bytes here matches the port without changing observable behaviour.
        if issue.id.len() > MAX_ID_LENGTH {
            fields.push(FieldError::new(
                "id",
                format!("exceeds {MAX_ID_LENGTH} characters"),
            ));
        }
        if !issue.id.is_empty() && !is_valid_id_format(&issue.id) {
            fields.push(FieldError::new(
                "id",
                "invalid format (expected prefix-hash)",
            ));
        }
    }

    fn validate_text_fields(issue: &Issue, fields: &mut Vec<FieldError>) {
        // Title: required, <= 500 chars (counted in `char`s), NUL-free.
        if issue.title.trim().is_empty() {
            fields.push(FieldError::new("title", "cannot be empty"));
        }
        if issue.title.chars().count() > TITLE_MAX_CHARS {
            fields.push(FieldError::new("title", "exceeds 500 characters"));
        }
        reject_nul("title", &issue.title, fields);

        // Long-text fields are unbounded by design; only NUL is rejected (SQLite compatibility).
        reject_nul_opt("description", issue.description.as_deref(), fields);
        reject_nul_opt("design", issue.design.as_deref(), fields);
        reject_nul_opt(
            "acceptance_criteria",
            issue.acceptance_criteria.as_deref(),
            fields,
        );
        reject_nul_opt("notes", issue.notes.as_deref(), fields);

        reject_nul("status", issue.status.as_str(), fields);
        validate_custom_status(&issue.status, fields);
        reject_nul("issue_type", issue.issue_type.as_str(), fields);
        validate_custom_issue_type(&issue.issue_type, fields);

        reject_bounded_chars_opt(
            "assignee",
            issue.assignee.as_deref(),
            ACTOR_MAX_CHARS,
            fields,
        );
        reject_bounded_chars_opt("owner", issue.owner.as_deref(), ACTOR_MAX_CHARS, fields);
        reject_bounded_chars_opt(
            "created_by",
            issue.created_by.as_deref(),
            ACTOR_MAX_CHARS,
            fields,
        );
        validate_external_ref(issue.external_ref.as_deref(), fields);
        reject_bounded_chars_opt(
            "source_system",
            issue.source_system.as_deref(),
            ACTOR_MAX_CHARS,
            fields,
        );
        validate_labels(issue, fields);
    }
}

fn validate_external_ref(external_ref: Option<&str>, fields: &mut Vec<FieldError>) {
    if let Some(external_ref) = external_ref {
        reject_nul("external_ref", external_ref, fields);
        if external_ref.chars().count() > EXTERNAL_REF_MAX_CHARS {
            fields.push(FieldError::new("external_ref", "exceeds 200 characters"));
        }
        if external_ref.chars().any(char::is_whitespace) {
            fields.push(FieldError::new("external_ref", "cannot contain whitespace"));
        }
    }
}

fn reject_nul(field: &str, value: &str, fields: &mut Vec<FieldError>) {
    if value.contains('\0') {
        fields.push(FieldError::new(field, "cannot contain NUL bytes"));
    }
}

fn reject_nul_opt(field: &str, value: Option<&str>, fields: &mut Vec<FieldError>) {
    if let Some(value) = value {
        reject_nul(field, value, fields);
    }
}

fn reject_bounded_chars_opt(
    field: &str,
    value: Option<&str>,
    max_chars: usize,
    fields: &mut Vec<FieldError>,
) {
    if let Some(value) = value {
        reject_nul(field, value, fields);
        if value.chars().count() > max_chars {
            fields.push(FieldError::new(
                field,
                format!("exceeds {max_chars} characters"),
            ));
        }
    }
}

fn validate_custom_status(status: &Status, fields: &mut Vec<FieldError>) {
    if let Status::Custom(value) = status
        && value.chars().count() > CUSTOM_VARIANT_MAX_CHARS
    {
        fields.push(FieldError::new(
            "status",
            "custom status exceeds 50 characters",
        ));
    }
}

fn validate_custom_issue_type(issue_type: &IssueType, fields: &mut Vec<FieldError>) {
    if let IssueType::Custom(value) = issue_type
        && value.chars().count() > CUSTOM_VARIANT_MAX_CHARS
    {
        fields.push(FieldError::new(
            "issue_type",
            "custom issue type exceeds 50 characters",
        ));
    }
}

fn validate_labels(issue: &Issue, fields: &mut Vec<FieldError>) {
    if issue.labels.len() > ISSUE_LABEL_MAX_COUNT {
        fields.push(FieldError::new(
            "labels",
            format!("exceeds {ISSUE_LABEL_MAX_COUNT} labels"),
        ));
    }
    for (idx, label) in issue.labels.iter().enumerate() {
        if let Err(err) = LabelValidator::validate(label) {
            fields.push(FieldError::new(
                "labels",
                format!("label at index {idx}: {}", err.reason),
            ));
        }
    }
}

/// Validates a single label value (charset + length).
pub struct LabelValidator;

impl LabelValidator {
    /// Validate a label for length and allowed characters.
    ///
    /// # Errors
    ///
    /// Returns a [`FieldError`] (`field: "label"`) if the label is empty, longer than 50
    /// characters, or contains a character outside `[A-Za-z0-9_:-]`.
    pub fn validate(label: &str) -> Result<(), FieldError> {
        if label.is_empty() {
            return Err(FieldError::new("label", "cannot be empty"));
        }
        if label.len() > LABEL_MAX_LEN {
            return Err(FieldError::new("label", "exceeds 50 characters"));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
        {
            return Err(FieldError::new(
                "label",
                "invalid characters (only alphanumeric, hyphen, underscore, colon allowed)",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ACTOR_MAX_CHARS, CommentValidator, IssueValidator, LabelValidator, validate_actor,
    };
    use crate::enums::{IssueType, Priority, Status};
    use crate::issue::Issue;
    use chrono::{TimeZone, Utc};
    use unblock_error::ModelError;

    fn base() -> Issue {
        Issue {
            id: "ub-abc123".to_string(),
            title: "Test issue".to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ..Issue::default()
        }
    }

    fn failing_fields(issue: &Issue) -> Vec<String> {
        match IssueValidator::validate(issue) {
            Err(ModelError::ValidationFailed { fields }) => {
                fields.into_iter().map(|f| f.field).collect()
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn valid_issue_passes() {
        assert!(IssueValidator::validate(&base()).is_ok());
    }

    #[test]
    fn empty_title_is_field_error_not_required_field() {
        let mut issue = base();
        issue.title = "   ".to_string();
        // It MUST be a ValidationFailed aggregate with a "title" row, NOT scalar RequiredField.
        match IssueValidator::validate(&issue) {
            Err(ModelError::ValidationFailed { fields }) => {
                assert!(
                    fields
                        .iter()
                        .any(|f| f.field == "title" && f.reason == "cannot be empty")
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn title_length_counts_chars_not_bytes() {
        let mut issue = base();
        issue.title = "\u{1f980}".repeat(500);
        assert!(IssueValidator::validate(&issue).is_ok());
        issue.title = "\u{1f980}".repeat(501);
        assert!(failing_fields(&issue).contains(&"title".to_string()));
    }

    #[test]
    fn priority_out_of_range_fails() {
        let mut issue = base();
        issue.priority = Priority(9);
        assert!(failing_fields(&issue).contains(&"priority".to_string()));
    }

    #[test]
    fn updated_before_created_fails() {
        let mut issue = base();
        issue.updated_at = Utc.with_ymd_and_hms(2025, 12, 31, 0, 0, 0).unwrap();
        assert!(failing_fields(&issue).contains(&"updated_at".to_string()));
    }

    #[test]
    fn estimated_minutes_bounds() {
        let mut neg = base();
        neg.estimated_minutes = Some(-1);
        assert!(failing_fields(&neg).contains(&"estimated_minutes".to_string()));

        let mut huge = base();
        huge.estimated_minutes = Some(525_961);
        assert!(failing_fields(&huge).contains(&"estimated_minutes".to_string()));
    }

    #[test]
    fn nul_rejected_in_all_twelve_fields() {
        let mut issue = base();
        issue.title = "nul\0title".to_string();
        issue.description = Some("nul\0d".to_string());
        issue.design = Some("nul\0d".to_string());
        issue.acceptance_criteria = Some("nul\0a".to_string());
        issue.notes = Some("nul\0n".to_string());
        issue.status = Status::Custom("nul\0s".to_string());
        issue.issue_type = IssueType::Custom("nul\0t".to_string());
        issue.assignee = Some("nul\0a".to_string());
        issue.owner = Some("nul\0o".to_string());
        issue.created_by = Some("nul\0c".to_string());
        issue.external_ref = Some("nul\0e".to_string());
        issue.source_system = Some("nul\0s".to_string());

        let got = failing_fields(&issue);
        for field in [
            "title",
            "description",
            "design",
            "acceptance_criteria",
            "notes",
            "status",
            "issue_type",
            "assignee",
            "owner",
            "created_by",
            "external_ref",
            "source_system",
        ] {
            assert!(
                got.contains(&field.to_string()),
                "missing NUL rejection for {field}"
            );
        }
    }

    #[test]
    fn closed_requires_closed_at() {
        let mut issue = base();
        issue.status = Status::Closed;
        assert!(failing_fields(&issue).contains(&"closed_at".to_string()));
    }

    #[test]
    fn non_terminal_with_closed_at_fails() {
        let mut issue = base();
        issue.closed_at = Some(issue.updated_at);
        assert!(failing_fields(&issue).contains(&"closed_at".to_string()));
    }

    #[test]
    fn tombstone_without_closed_at_ok() {
        let mut issue = base();
        issue.status = Status::Tombstone;
        assert!(IssueValidator::validate(&issue).is_ok());
    }

    #[test]
    fn too_many_labels_fails() {
        let mut issue = base();
        issue.labels = (0..65).map(|i| format!("l{i}")).collect();
        assert!(failing_fields(&issue).contains(&"labels".to_string()));

        let mut ok = base();
        ok.labels = (0..64).map(|i| format!("l{i}")).collect();
        assert!(IssueValidator::validate(&ok).is_ok());
    }

    #[test]
    fn custom_variant_cap_status_and_issue_type_only() {
        // Status::Custom over 50 chars fails.
        let mut s = base();
        s.status = Status::Custom("x".repeat(51));
        assert!(failing_fields(&s).contains(&"status".to_string()));

        // IssueType::Custom over 50 chars fails.
        let mut t = base();
        t.issue_type = IssueType::Custom("x".repeat(51));
        assert!(failing_fields(&t).contains(&"issue_type".to_string()));

        // Exactly 50 (multibyte) passes for both.
        let mut ok = base();
        ok.status = Status::Custom("\u{1f980}".repeat(50));
        ok.issue_type = IssueType::Custom("\u{1f980}".repeat(50));
        assert!(IssueValidator::validate(&ok).is_ok());
    }

    #[test]
    fn external_ref_whitespace_fails() {
        let mut issue = base();
        issue.external_ref = Some("gh 12".to_string());
        assert!(failing_fields(&issue).contains(&"external_ref".to_string()));
    }

    #[test]
    fn collects_multiple_errors() {
        let mut issue = base();
        issue.id = String::new();
        issue.title = String::new();
        issue.priority = Priority(9);
        issue.updated_at = Utc.with_ymd_and_hms(2025, 12, 31, 0, 0, 0).unwrap();

        let got = failing_fields(&issue);
        for field in ["id", "title", "priority", "updated_at"] {
            assert!(got.contains(&field.to_string()), "missing {field}");
        }
    }

    #[test]
    fn label_validator_charset() {
        assert!(LabelValidator::validate("team:backend").is_ok());
        assert!(LabelValidator::validate("bad label").is_err());
        assert!(LabelValidator::validate("has/slash").is_err());
        assert!(LabelValidator::validate("").is_err());
    }

    #[test]
    fn validate_actor_accepts_ascii_and_boundary_length() {
        assert!(validate_actor("alice").is_ok());
        // Exactly ACTOR_MAX_CHARS (200) chars passes (char-counted boundary).
        assert!(validate_actor(&"a".repeat(ACTOR_MAX_CHARS)).is_ok());
        // A 200-char multibyte actor passes (counted in chars, not bytes).
        assert!(validate_actor(&"\u{1f980}".repeat(ACTOR_MAX_CHARS)).is_ok());
    }

    #[test]
    fn validate_actor_rejects_over_length_char_counted() {
        // 201 ASCII chars fails.
        let err = validate_actor(&"a".repeat(ACTOR_MAX_CHARS + 1)).expect_err("over length");
        assert_eq!(err.field, "actor");
        // 201 multibyte chars (well over 200 BYTES) fails — proving the bound is char-counted, not
        // byte-counted (200 multibyte chars passed above).
        let err =
            validate_actor(&"\u{1f980}".repeat(ACTOR_MAX_CHARS + 1)).expect_err("over length mb");
        assert_eq!(err.field, "actor");
    }

    #[test]
    fn validate_actor_rejects_nul() {
        let err = validate_actor("ali\0ce").expect_err("nul rejected");
        assert_eq!(err.field, "actor");
        assert_eq!(err.reason, "cannot contain NUL bytes");
    }

    #[test]
    fn validate_actor_rejects_other_control_chars() {
        for ctrl in ["a\tb", "a\nb", "a\rb"] {
            let err = validate_actor(ctrl).expect_err("control char rejected");
            assert_eq!(err.field, "actor");
            assert_eq!(err.reason, "cannot contain control characters");
        }
    }

    // --- CommentValidator (FR-6/D37, spine §1.9) -----------------------------------------------

    fn comment_fields(result: Result<(), ModelError>) -> Vec<super::FieldError> {
        match result {
            Err(ModelError::ValidationFailed { fields }) => fields,
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn validate_body_accepts_a_normal_body() {
        assert!(CommentValidator::validate_body("looks good to me").is_ok());
    }

    #[test]
    fn validate_body_rejects_empty_trimmed_under_the_content_field() {
        let fields = comment_fields(CommentValidator::validate_body("   \t "));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "content");
        assert_eq!(fields[0].reason, "cannot be empty");
    }

    #[test]
    fn validate_body_rejects_nul() {
        let fields = comment_fields(CommentValidator::validate_body("a\0b"));
        assert!(
            fields
                .iter()
                .any(|f| f.field == "content" && f.reason == "cannot contain NUL bytes")
        );
    }

    #[test]
    fn validate_body_is_otherwise_unbounded() {
        // The 64 KiB transport cap lives at the L7 MCP quota, not here.
        assert!(CommentValidator::validate_body(&"x".repeat(100_000)).is_ok());
    }

    #[test]
    fn validate_comment_accepts_a_well_formed_comment() {
        assert!(CommentValidator::validate_comment("ub-abc123", "alice", "hi").is_ok());
    }

    #[test]
    fn validate_comment_rejects_empty_issue_id() {
        let fields = comment_fields(CommentValidator::validate_comment("  ", "alice", "hi"));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "issue_id");
    }

    #[test]
    fn validate_comment_relabels_the_actor_rule_as_author() {
        // Delegated to validate_actor, whose FieldError says "actor" — it MUST be relabelled.
        let fields = comment_fields(CommentValidator::validate_comment(
            "ub-abc123",
            &"a".repeat(ACTOR_MAX_CHARS + 1),
            "hi",
        ));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "author");
        assert_eq!(
            fields[0].reason,
            format!("exceeds {ACTOR_MAX_CHARS} characters")
        );
        assert!(
            !fields.iter().any(|f| f.field == "actor"),
            "the delegated FieldError must be relabelled author, never leak as actor"
        );
    }

    #[test]
    fn validate_comment_author_bound_is_char_counted_not_bytes() {
        // 200 multibyte chars (well over 200 BYTES) passes — the deliberate adaptation of the Go
        // original's `len() > 200` bytes rule.
        assert!(
            CommentValidator::validate_comment(
                "ub-abc123",
                &"\u{1f980}".repeat(ACTOR_MAX_CHARS),
                "hi"
            )
            .is_ok()
        );
        assert!(
            CommentValidator::validate_comment(
                "ub-abc123",
                &"\u{1f980}".repeat(ACTOR_MAX_CHARS + 1),
                "hi"
            )
            .is_err()
        );
    }

    #[test]
    fn validate_comment_adds_the_author_non_empty_rule_validate_actor_omits() {
        // validate_actor itself accepts "" (its contract: the resolved actor is already non-empty).
        assert!(validate_actor("  ").is_ok());
        let fields = comment_fields(CommentValidator::validate_comment("ub-abc123", "  ", "hi"));
        assert!(
            fields
                .iter()
                .any(|f| f.field == "author" && f.reason == "cannot be empty")
        );
    }

    #[test]
    fn validate_comment_is_a_multi_fault_aggregate_not_fail_fast() {
        // D-E1 uniform aggregate carrier (spine §1.9 COMPOSITION): an empty body AND an
        // over-length author must BOTH surface — 2 entries, not 1. This is exactly what
        // `validate_comment(body)?`-style fail-fast composition would break.
        let fields = comment_fields(CommentValidator::validate_comment(
            "ub-abc123",
            &"a".repeat(ACTOR_MAX_CHARS + 1),
            "   ",
        ));
        assert_eq!(
            fields.len(),
            2,
            "expected a 2-fault aggregate, got {fields:?}"
        );
        assert!(fields.iter().any(|f| f.field == "author"));
        assert!(fields.iter().any(|f| f.field == "content"));
    }

    #[test]
    fn validate_comment_collects_all_three_field_faults() {
        let fields = comment_fields(CommentValidator::validate_comment("", "", ""));
        for field in ["issue_id", "author", "content"] {
            assert!(fields.iter().any(|f| f.field == field), "missing {field}");
        }
    }
}
