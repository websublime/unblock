//! Tool **#8 `comment`** — the DEDICATED comment tool (spine §5.1/§5.2, FR-6/D37/D-B).
//!
//! The deliberate §6.6 exception to "extend an existing tool by discriminator before adding one":
//! commenting is a distinct domain verb, not an `issue` arm. It brings the RK-3 tool budget to
//! **8 ≤ 8** — the budget is now FULL, so any further domain surface must extend an existing tool.
//!
//! Maps `CommentToolInput{action}` to `Session::{add_comment, list_comments, update_comment,
//! delete_comment}`:
//! - `add` → `Session::add_comment(issue_id, body)`; the author is `self.session.actor()` — there
//!   is no per-comment author on the wire (FORK-M1b).
//! - `list` → `Session::list_comments(issue_id)`, CD-2 object-wrapped as `{"comments":[…]}`.
//! - `update` → `Session::update_comment(comment_id, body)`; provenance-preserving (D-D).
//! - `delete` → `Session::delete_comment(comment_id)`; a SOFT-REDACT (D-E), never a hard delete:
//!   the returned `Comment` carries `redacted_at` + `"text": ""`.
//!
//! Body validation (non-empty when trimmed / NUL-rejected) runs in the ENGINE before the mutation
//! (→ `ValidationFailed`), so it stays single-homed in the model (spine §1.9).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};

use crate::server::UnblockServer;
use crate::tools::dto::Attribution;
use crate::tools::output::{CommentList, CommentOutput};
use crate::tools::{engine_err_json, err_json, ok_json};

/// The one-line `comment` tool description.
///
/// **CONTRACT BYTES, declared ONCE.** This string ships in TWO places — the `#[tool(description)]`
/// attribute below (the live `tools/list` wire) and the `capabilities()` tool descriptor
/// (`resources/capabilities.rs`) — and no test cross-checks the two. Both sites must carry these
/// exact bytes.
pub(crate) const COMMENT_TOOL_DESCRIPTION: &str =
    "Comment on issues: add, list, update, or delete (soft-redact).";

/// The `comment` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
// §5.2a (CD-1): inject the root `"type": "object"` (the tagged-enum `oneOf` root omits it, which
// strict MCP clients reject — that would take the WHOLE tools/list dark) — the union is preserved
// verbatim.
#[schemars(extend("type" = "object"))]
pub(crate) enum CommentToolInput {
    /// Add a comment to an issue.
    Add {
        /// The issue id to comment on.
        issue_id: String,
        /// The comment body.
        body: String,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// List the comments on an issue.
    List {
        /// The issue id.
        issue_id: String,
    },
    /// Update a comment's body (provenance-preserving: sets `updated_at`, audits the edit).
    Update {
        /// The comment id.
        comment_id: i64,
        /// The new comment body.
        body: String,
        #[serde(flatten)]
        attribution: Attribution,
    },
    /// Delete a comment — a SOFT-REDACT: the row is kept, the body masked, `redacted_at` set.
    Delete {
        /// The comment id.
        comment_id: i64,
        #[serde(flatten)]
        attribution: Attribution,
    },
}

#[rmcp::tool_router(router = comment_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Comment on issues (FR-6/D37).
    #[tool(
        name = "comment",
        description = "Comment on issues: add, list, update, or delete (soft-redact)."
    )]
    pub(crate) async fn comment(
        &self,
        Parameters(input): Parameters<CommentToolInput>,
    ) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        match input {
            CommentToolInput::Add {
                issue_id,
                body,
                attribution: _,
            } => match self.session.add_comment(&issue_id, &body).await {
                Ok(comment) => ok_json(&CommentOutput::Comment(comment)),
                Err(err) => engine_err_json(&err),
            },
            CommentToolInput::List { issue_id } => {
                match self.session.list_comments(&issue_id).await {
                    Ok(comments) => ok_json(&CommentOutput::Comments(CommentList { comments })),
                    Err(err) => engine_err_json(&err),
                }
            }
            CommentToolInput::Update {
                comment_id,
                body,
                attribution: _,
            } => match self.session.update_comment(comment_id, &body).await {
                Ok(comment) => ok_json(&CommentOutput::Comment(comment)),
                Err(err) => engine_err_json(&err),
            },
            CommentToolInput::Delete {
                comment_id,
                attribution: _,
            } => match self.session.delete_comment(comment_id).await {
                Ok(comment) => ok_json(&CommentOutput::Comment(comment)),
                Err(err) => engine_err_json(&err),
            },
        }
    }
}
