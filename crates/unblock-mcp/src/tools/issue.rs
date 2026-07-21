//! Tool **#1 `issue`** — the 7-action issue lifecycle (spine §5.1/§5.2).
//!
//! Actions: `create` / `show` / `update` / `close` / `reopen` / `delete` / `restore`, mapped to
//! `Session::{create_issue, get, update, close_with_suggestions, update, delete, restore}`.
//!
//! - `create` maps [`IssueInput::Create`] → the engine-owned [`unblock_engine::NewIssue`] and calls
//!   the **MINTING** `Session::create_issue` (D21 — the engine mints the id under the write permit).
//!   It is NOT `Session::create(&Issue)`, the id-PRESERVING import path. The wire-only `quick` /
//!   `attribution` are not `NewIssue` fields and are dropped. `quick=true` returns `IssueOutput::Id`.
//! - `close{suggest_next}` returns `IssueOutput::Close` carrying `newly_unblocked` (FR-11).
//! - `reopen` is an `update` patch to a non-terminal status — there is NO `Session::reopen`.
//! - `restore` is SINGLE-ID (scalar, D20) → `Session::restore(id)` (the dedicated un-tombstone path),
//!   NOT `update`.

use chrono::{DateTime, Utc};
// D42 SEAM: this is the CRATE-LOCAL `Parameters` (`crate::tools::args`), NOT rmcp's. It defers
// deserialization so argument errors reach the FR-11 in-band channel instead of an out-of-band
// `-32602`. The NAME IS LOAD-BEARING (rmcp-macros matches the ident `Parameters` to pick the
// published inputSchema) — see `tools/args.rs`. Do NOT "fix" this back to rmcp's wrapper.
use crate::tools::args::{Parameters, parse_args};
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_engine::{DeleteMode, DeletePlan, IssuePatch, NewIssue};
use unblock_model::{IssueType, Priority, Status};

use crate::server::UnblockServer;
use crate::tools::dto::{Attribution, DepInput};
use crate::tools::output::{DeleteModeOutput, DeletePlanOutput, IdOnly, IssueList, IssueOutput};
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `issue` tool input (spine §5.2 — EXACT shape; 7 actions).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
// §5.2a (CD-1): inject the root `"type": "object"` so the published inputSchema is MCP-conformant. A
// `#[serde(tag)]` tagged enum renders a root `oneOf` with NO root `type`; strict clients reject the
// whole `tools/list`. schemars lowers this to a post-mutator that inserts `type: object` AFTER the
// derived `oneOf` body — the union (and instance validation) is preserved verbatim.
#[schemars(extend("type" = "object"))]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) enum IssueInput {
    /// Create a new issue (interactive MINTING create, D21).
    ///
    /// The payload is boxed (it is the largest variant) — the wire shape is unchanged: a newtype
    /// variant over a `#[serde(flatten)]`-equivalent struct keeps `{action:"create", title, ...}`.
    Create(Box<CreateInput>),
    /// Bulk-create issues from an inline markdown document (D22 — `{action:"create_bulk", markdown}`).
    ///
    /// The `markdown` is INLINE content (NOT a path). The adapter parses it (all-or-nothing
    /// pre-mutation), caps the record count at `Quotas::max_batch`, maps each `ParsedIssue` →
    /// `NewIssue` (carrying the symbolic dep/parent refs verbatim), and calls the ATOMIC
    /// `Session::create_bulk` — NOT a loop over `create_issue`. The engine mints all ids + resolves the
    /// 2-phase intra-file dep/parent IN MEMORY + inserts the whole batch in ONE tx (rollback-on-any-
    /// failure → ZERO writes). Output reuses `IssueOutput::Issues`.
    ///
    /// The document is REJECTED as a whole (`isError:true`, `VALIDATION_FAILED`, zero writes) when
    /// it contains: an unrecognized or empty `### ` section header; a `### ` section before the
    /// first `## `; or an invalid `### Priority` value. Each of these was previously accepted and
    /// its content silently discarded.
    CreateBulk {
        /// The inline bulk-markdown document content.
        ///
        /// Each issue starts with an H2 line `## Issue Title`. Per-issue fields are H3 sections,
        /// case-insensitive, from this CLOSED set: `ID`, `Parent`, `Priority`, `Type`,
        /// `Description`, `Design`, `Acceptance Criteria` (alias `Acceptance`), `Assignee`,
        /// `Labels`, `Dependencies` (alias `Deps`), `Agent Context` (aliases `agent-context`,
        /// `agent_context`). Any other `### ` header rejects the whole document.
        ///
        /// `### ` always starts a NEW section — use `#### ` for a sub-heading inside a section body,
        /// otherwise the enclosing section is terminated and the rest of it is lost.
        markdown: String,
    },
    /// Show a single issue by id.
    Show {
        /// The issue id.
        id: String,
    },
    /// Update one or more issues with a patch.
    Update {
        /// The target issue ids.
        ids: Vec<String>,
        /// The patch (boxed — `PatchInput` is large; the flattened wire shape is unchanged).
        #[serde(flatten)]
        patch: Box<PatchInput>,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Close an issue, optionally surfacing the newly-unblocked set.
    Close {
        /// The issue id.
        id: String,
        #[serde(default)]
        reason: Option<String>,
        /// When `true`, return the `newly_unblocked` set (FR-11).
        #[serde(default)]
        suggest_next: bool,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Reopen an issue (an `update` patch to a non-terminal status).
    Reopen {
        /// The issue id.
        id: String,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Delete one or more issues (tombstone/cascade/hard/dry-run).
    Delete {
        /// The target issue ids.
        ids: Vec<String>,
        #[serde(default)]
        mode: DeleteModeInput,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Restore (un-tombstone) a single soft-deleted issue (D20).
    Restore {
        /// The issue id.
        id: String,
        #[serde(flatten)]
        attribution: Attribution,
    },
}

/// The `create` action payload (boxed in [`IssueInput::Create`]).
///
/// Mirrors the spine §5.2 `Create` fields; mapped to the engine-owned [`NewIssue`] (D21) minus the
/// wire-only `quick`/`attribution` (which are not `NewIssue` fields).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct CreateInput {
    /// The issue title (required).
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub issue_type: Option<IssueType>,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub deps: Vec<DepInput>,
    #[serde(default)]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub defer_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub estimated_minutes: Option<i32>,
    #[serde(default)]
    pub slug: Option<String>,
    /// The `### Design` content (D22 — maps onto `NewIssue::design`).
    #[serde(default)]
    pub design: Option<String>,
    /// The `### Acceptance Criteria` / `### Acceptance` content (D22 — `NewIssue::acceptance_criteria`).
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
    /// The `### Assignee` content (D22 — `NewIssue::assignee`).
    #[serde(default)]
    pub assignee: Option<String>,
    /// The `### Agent Context` content (D22 — `NewIssue::agent_context`).
    #[serde(default)]
    pub agent_context: Option<String>,
    #[serde(default)]
    pub ephemeral: bool,
    /// Quick-create: the output is the minted id only.
    #[serde(default)]
    pub quick: bool,
    /// Capture-only attribution (never enforced).
    #[serde(flatten)]
    pub attribution: Attribution,
}

/// A partial-update patch on the wire (mirrors the engine [`IssuePatch`] updatable columns).
///
/// Nullable text columns use `Option<Option<String>>` (`None` leave / `Some(None)` clear /
/// `Some(Some)` set), mirroring the storage patch semantics (spine §3.1).
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[allow(clippy::option_option)]
// outer=present, inner=clear-vs-set — mirrors IssuePatch.
// D42: `#[serde(deny_unknown_fields)]` — an unknown/misspelled argument is REJECTED in-band
// instead of being silently dropped. NOT recursive and inert on a flatten TARGET: every nested
// container needs its OWN attribute (see `tools/args.rs` + the CHECK-3 container guard).
#[serde(deny_unknown_fields)]
pub(crate) struct PatchInput {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<Option<String>>,
    #[serde(default)]
    design: Option<Option<String>>,
    #[serde(default)]
    acceptance_criteria: Option<Option<String>>,
    #[serde(default)]
    notes: Option<Option<String>>,
    #[serde(default)]
    owner: Option<Option<String>>,
    #[serde(default)]
    external_ref: Option<Option<String>>,
    #[serde(default)]
    assignee: Option<Option<String>>,
    #[serde(default)]
    close_reason: Option<Option<String>>,
    #[serde(default)]
    status: Option<Status>,
    #[serde(default)]
    priority: Option<Priority>,
    #[serde(default)]
    issue_type: Option<IssueType>,
    #[serde(default)]
    estimated_minutes: Option<i32>,
    #[serde(default)]
    due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    labels_add: Vec<String>,
    #[serde(default)]
    labels_remove: Vec<String>,
    #[serde(default)]
    labels_set: Option<Vec<String>>,
    #[serde(default)]
    parent: Option<Option<String>>,
}

impl PatchInput {
    /// Build the engine [`IssuePatch`] from this wire patch.
    fn into_issue_patch(self) -> IssuePatch {
        IssuePatch {
            title: self.title,
            description: self.description,
            design: self.design,
            acceptance_criteria: self.acceptance_criteria,
            notes: self.notes,
            owner: self.owner,
            external_ref: self.external_ref,
            assignee: self.assignee,
            close_reason: self.close_reason,
            status: self.status,
            priority: self.priority,
            issue_type: self.issue_type,
            estimated_minutes: self.estimated_minutes,
            due_at: self.due_at,
            labels_add: self.labels_add,
            labels_remove: self.labels_remove,
            labels_set: self.labels_set,
            parent: self.parent,
        }
    }
}

/// The delete mode on the wire (spine §5.2 `DeleteModeInput` → [`DeleteMode`]).
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeleteModeInput {
    /// Soft-delete (default): tombstone the targets.
    #[default]
    Tombstone,
    /// Tombstone the targets and their children.
    Cascade,
    /// Permanently delete the rows.
    Hard,
    /// Compute the plan only; mutate nothing.
    DryRun,
}

impl DeleteModeInput {
    /// Map to the storage [`DeleteMode`].
    fn into_delete_mode(self) -> DeleteMode {
        match self {
            Self::Tombstone => DeleteMode::Tombstone,
            Self::Cascade => DeleteMode::Cascade,
            Self::Hard => DeleteMode::Hard,
            Self::DryRun => DeleteMode::DryRun,
        }
    }
}

#[rmcp::tool_router(router = issue_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Create, inspect, or mutate issues (the 7-action issue lifecycle, FR-1a/1b/1c).
    #[tool(
        name = "issue",
        description = "Create, show, update, close, reopen, delete, or restore issues."
    )]
    pub(crate) async fn issue(&self, Parameters(raw, _): Parameters<IssueInput>) -> CallToolResult {
        // D42 PROLOGUE: the ONLY deserialization of tool arguments. The NFR-18 quota already
        // ran once in `call_tool` over the whole `params`. `IssueInput` carries
        // `#[serde(deny_unknown_fields)]`, so an unknown/misspelled argument is REJECTED here,
        // in-band, instead of being silently discarded.
        let input: IssueInput = match parse_args(raw) {
            Ok(input) => input,
            Err(structured) => return err_json(&structured),
        };
        match input {
            IssueInput::Create(create) => {
                let CreateInput {
                    title,
                    description,
                    issue_type,
                    priority,
                    labels,
                    parent,
                    deps,
                    due_at,
                    defer_until,
                    estimated_minutes,
                    slug,
                    design,
                    acceptance_criteria,
                    assignee,
                    agent_context,
                    ephemeral,
                    quick,
                    attribution: _,
                } = *create;
                let now = Utc::now();
                let actor = self.session.actor().to_string();
                let new = NewIssue {
                    title,
                    description,
                    issue_type,
                    priority,
                    labels,
                    parent,
                    deps: deps
                        .into_iter()
                        .map(|d| d.into_dependency(&actor, now))
                        .collect(),
                    due_at,
                    defer_until,
                    estimated_minutes,
                    slug,
                    ephemeral,
                    design,
                    acceptance_criteria,
                    assignee,
                    agent_context,
                    ..NewIssue::default()
                };
                match self.session.create_issue(new).await {
                    Ok(issue) if quick => ok_json(&IssueOutput::Id(IdOnly { id: issue.id })),
                    Ok(issue) => ok_json(&IssueOutput::Issue(Box::new(issue))),
                    Err(err) => engine_err_json(&err),
                }
            }
            IssueInput::CreateBulk { markdown } => self.create_bulk_action(&markdown).await,
            IssueInput::Show { id } => match self.session.get(&id).await {
                Ok(Some(issue)) => ok_json(&IssueOutput::Issue(Box::new(issue))),
                Ok(None) => err_json(&issue_not_found(&id)),
                Err(err) => engine_err_json(&err),
            },
            IssueInput::Update {
                ids,
                patch,
                attribution: _,
            } => {
                let engine_patch = (*patch).into_issue_patch();
                let mut updated = Vec::with_capacity(ids.len());
                for id in &ids {
                    match self.session.update(id, &engine_patch).await {
                        Ok(issue) => updated.push(issue),
                        Err(err) => return engine_err_json(&err),
                    }
                }
                ok_json(&IssueOutput::Issues(IssueList { issues: updated }))
            }
            IssueInput::Close {
                id,
                reason,
                suggest_next,
                attribution: _,
            } => match self.session.close_with_suggestions(&id, reason).await {
                Ok(mut outcome) => {
                    if !suggest_next {
                        // Without suggest_next the caller wants only the closed issue surfaced.
                        outcome.newly_unblocked.clear();
                    }
                    ok_json(&IssueOutput::Close(Box::new(outcome)))
                }
                Err(err) => engine_err_json(&err),
            },
            IssueInput::Reopen { id, attribution: _ } => {
                // Reopen = an update patch to a non-terminal status (no Session::reopen, spine §5.2).
                let patch = IssuePatch {
                    status: Some(Status::Open),
                    ..IssuePatch::default()
                };
                match self.session.update(&id, &patch).await {
                    Ok(issue) => ok_json(&IssueOutput::Issue(Box::new(issue))),
                    Err(err) => engine_err_json(&err),
                }
            }
            IssueInput::Delete {
                ids,
                mode,
                attribution: _,
            } => {
                let plan = DeletePlan {
                    mode: mode.into_delete_mode(),
                    targets: ids,
                    cascade_children: Vec::new(),
                };
                match self.session.delete(&plan).await {
                    Ok(resolved) => ok_json(&IssueOutput::Delete(DeletePlanOutput {
                        mode: DeleteModeOutput::from(resolved.mode),
                        targets: resolved.targets,
                        cascade_children: resolved.cascade_children,
                    })),
                    Err(err) => engine_err_json(&err),
                }
            }
            IssueInput::Restore { id, attribution: _ } => match self.session.restore(&id).await {
                Ok(issue) => ok_json(&IssueOutput::Issue(Box::new(issue))),
                Err(err) => engine_err_json(&err),
            },
        }
    }
}

impl UnblockServer {
    /// The `create_bulk` action (D22): parse the inline markdown (all-or-nothing), cap the record
    /// count at `Quotas::max_batch` (before any mint), map each `ParsedIssue` → `NewIssue` (carrying
    /// the symbolic dep/parent refs verbatim), and call the ATOMIC `Session::create_bulk`. Output
    /// reuses `IssueOutput::Issues` (the Vec of created issues).
    async fn create_bulk_action(&self, markdown: &str) -> CallToolResult {
        // (1) Parse the whole document (all-or-nothing pre-mutation parse).
        let parsed = match crate::tools::bulk_markdown::parse_bulk_markdown(markdown) {
            Ok(parsed) => parsed,
            Err(structured) => return err_json(&structured),
        };

        // (2) Cap the parsed record count at max_batch BEFORE any mint (the spy Session sees zero calls).
        if let Err(structured) = crate::tools::enforce_batch_quota(parsed.len(), &self.quotas) {
            return err_json(&structured);
        }

        // (3) Map each ParsedIssue → NewIssue (carrying the symbolic refs verbatim for the engine).
        // D42: the map is FALLIBLE — a present-but-invalid `### Priority` / `### Type` rejects the
        // whole document here, still strictly BEFORE `Session::create_bulk`, so zero writes. The
        // step order (parse -> batch quota -> map -> create_bulk) is PRESERVED deliberately: hoisting
        // this above the quota would make an over-cap document with an invalid priority report
        // `kind:"section_value"` instead of `kind:"batch"`.
        let records: Vec<NewIssue> = match parsed
            .into_iter()
            .enumerate()
            .map(|(index, record)| parsed_to_new_issue(record, index))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(records) => records,
            Err(structured) => return err_json(&structured),
        };

        // (4) The ATOMIC bulk create (engine mints + resolves + one storage tx). Output = the CD-2
        // object-wrapped Vec (`{"issues":[…]}`).
        match self.session.create_bulk(records).await {
            Ok(issues) => ok_json(&IssueOutput::Issues(IssueList { issues })),
            Err(err) => engine_err_json(&err),
        }
    }
}

/// Parse a PRESENT `### Section` value, or reject it — `None` stays `None`.
///
/// `.transpose()` is what keeps an **absent** section legal while a **present-but-invalid** value
/// errors. Both `priority` and `issue_type` go through this ONE helper so the two spellings cannot
/// drift: today `IssueType::from_str` is infallible by construction (an unknown type is preserved as
/// `IssueType::Custom`), so its `Err` arm is statically unreachable — which is exactly the point. If
/// it ever becomes fallible, the value is REJECTED rather than silently defaulted, with no code
/// change and no compile error to notice.
///
/// # Errors
///
/// `ValidationFailed` with `kind = "section_value"` plus the record `index` + `title` — a 50-record
/// document whose error says only "invalid priority" is unactionable, which is the same usability
/// failure as the silent drop it replaces.
fn parse_section_value<T: std::str::FromStr>(
    raw: Option<&str>,
    index: usize,
    title: &str,
    section: &str,
    hint: &str,
) -> Result<Option<T>, unblock_error::StructuredError> {
    raw.map(str::parse::<T>).transpose().map_err(|_| {
        let value = raw.unwrap_or_default();
        unblock_error::StructuredError::from_code(
            unblock_error::ErrorCode::ValidationFailed,
            format!("record {index} ({title}): `### {section}` value {value:?} rejected"),
        )
        .with_hint(hint)
        .with_context("field", serde_json::json!("markdown"))
        .with_context("kind", serde_json::json!("section_value"))
        .with_context("index", serde_json::json!(index))
        .with_context("title", serde_json::json!(title))
        .with_context("section", serde_json::json!(section))
        .with_context("value", serde_json::json!(value))
    })
}

/// Map a parsed bulk-markdown record to the engine-owned [`NewIssue`] (D22). The dependency / parent
/// references are carried VERBATIM (as `dep_refs` / symbolic `parent` / `stand_in_id`) — the ENGINE
/// resolves them at `create_bulk`.
///
/// **D42:** the `priority` / `issue_type` strings are NO LONGER parsed leniently. The pre-D42
/// `.and_then(|p| p.parse().ok())` collapsed an invalid `### Priority` to `None`, which
/// `write.rs`'s `unwrap_or_default()` then turned into `MEDIUM` (P2) — the user asked for a
/// priority, got P2, and got no error.
///
/// # Errors
///
/// `ValidationFailed` when a PRESENT `### Priority` / `### Type` value does not parse. Runs strictly
/// before `Session::create_bulk`, so zero writes.
fn parsed_to_new_issue(
    parsed: crate::tools::bulk_markdown::ParsedIssue,
    index: usize,
) -> Result<NewIssue, unblock_error::StructuredError> {
    // Bind the title BEFORE the struct literal: the error payload borrows it and it is moved in.
    let title = parsed.title;
    let issue_type = parse_section_value::<IssueType>(
        parsed.issue_type.as_deref(),
        index,
        &title,
        "Type",
        "expected a known or custom issue type",
    )?;
    let priority = parse_section_value::<Priority>(
        parsed.priority.as_deref(),
        index,
        &title,
        "Priority",
        "expected P0..P4 or 0..4",
    )?;
    Ok(NewIssue {
        title,
        description: parsed.description,
        issue_type,
        priority,
        labels: parsed.labels,
        parent: parsed.parent,
        slug: None,
        design: parsed.design,
        acceptance_criteria: parsed.acceptance_criteria,
        assignee: parsed.assignee,
        agent_context: parsed.agent_context,
        stand_in_id: parsed.stand_in_id,
        dep_refs: parsed.dependencies,
        ..NewIssue::default()
    })
}

/// Build the not-found structured error for a missing `show` target.
fn issue_not_found(id: &str) -> unblock_error::StructuredError {
    unblock_error::StructuredError::from_code(
        unblock_error::ErrorCode::IssueNotFound,
        format!("issue not found: {id}"),
    )
    .with_context("id", serde_json::json!(id))
}
