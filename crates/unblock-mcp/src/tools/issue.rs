//! Tool **#1 `issue`** — the 7-action issue lifecycle (spine §5.1/§5.2).
//!
//! Actions: `create` / `show` / `update` / `close` / `reopen` / `delete` / `restore`, mapped to
//! `Session::{create_issue, get, update, close_with_suggestions, update, delete, restore}`.
//!
//! - `create` maps [`IssueInput::Create`] → the engine-owned [`unblock_engine::NewIssue`] and calls
//!   the **MINTING** `Session::create_issue` (D21 — the engine mints the id under the write permit).
//!   It is NOT `Session::create(&Issue)`, the id-PRESERVING import path. The wire-only `quick` /
//!   `attribution` are not `NewIssue` fields and are dropped. `quick=true` returns `ToolOutput::Id`.
//! - `close{suggest_next}` returns `ToolOutput::Close` carrying `newly_unblocked` (FR-11).
//! - `reopen` is an `update` patch to a non-terminal status — there is NO `Session::reopen`.
//! - `restore` is SINGLE-ID (scalar, D20) → `Session::restore(id)` (the dedicated un-tombstone path),
//!   NOT `update`.

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_engine::{DeleteMode, DeletePlan, IssuePatch, NewIssue};
use unblock_model::{IssueType, Priority, Status};

use crate::server::UnblockServer;
use crate::tools::dto::{Attribution, DepInput};
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `issue` tool input (spine §5.2 — EXACT shape; 7 actions).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum IssueInput {
    /// Create a new issue (interactive MINTING create, D21).
    ///
    /// The payload is boxed (it is the largest variant) — the wire shape is unchanged: a newtype
    /// variant over a `#[serde(flatten)]`-equivalent struct keeps `{action:"create", title, ...}`.
    Create(Box<CreateInput>),
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
#[allow(clippy::option_option)] // outer=present, inner=clear-vs-set — mirrors IssuePatch.
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
    pub(crate) async fn issue(&self, Parameters(input): Parameters<IssueInput>) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
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
                };
                match self.session.create_issue(new).await {
                    Ok(issue) if quick => ok_json(&crate::tools::output::IdOnly { id: issue.id }),
                    Ok(issue) => ok_json(&issue),
                    Err(err) => engine_err_json(&err),
                }
            }
            IssueInput::Show { id } => match self.session.get(&id).await {
                Ok(Some(issue)) => ok_json(&issue),
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
                ok_json(&updated)
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
                    ok_json(&outcome)
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
                    Ok(issue) => ok_json(&issue),
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
                    Ok(resolved) => ok_json(&delete_plan_json(&resolved)),
                    Err(err) => engine_err_json(&err),
                }
            }
            IssueInput::Restore { id, attribution: _ } => match self.session.restore(&id).await {
                Ok(issue) => ok_json(&issue),
                Err(err) => engine_err_json(&err),
            },
        }
    }
}

/// Build the not-found structured error for a missing `show` target.
fn issue_not_found(id: &str) -> unblock_error::StructuredError {
    unblock_error::StructuredError::from_code(
        unblock_error::ErrorCode::IssueNotFound,
        format!("issue not found: {id}"),
    )
    .with_context("id", serde_json::json!(id))
}

/// Serialize a resolved [`DeletePlan`] (which has no `Serialize` derive) to a stable JSON shape.
fn delete_plan_json(plan: &DeletePlan) -> serde_json::Value {
    let mode = match plan.mode {
        DeleteMode::Tombstone => "tombstone",
        DeleteMode::Cascade => "cascade",
        DeleteMode::Hard => "hard",
        DeleteMode::DryRun => "dry_run",
    };
    serde_json::json!({
        "mode": mode,
        "targets": plan.targets,
        "cascade_children": plan.cascade_children,
    })
}
