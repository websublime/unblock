//! Prompts (spine §5.5) — the 3 guided workflows surfaced on [`crate::server::UnblockServer`].
//!
//! The prompt functions live here under one `#[prompt_router]` (composed into the server's
//! `#[prompt_handler]`). Each delegates its message construction to a per-prompt module (pure
//! builders) so the routing stays thin.

pub(crate) mod close_with_suggestions;
pub(crate) mod plan_next_work;
pub(crate) mod triage;

use rmcp::model::PromptMessage;
use rmcp::prompt;

use crate::server::UnblockServer;

#[rmcp::prompt_router(router = "prompt_router", vis = "pub(crate)")]
impl UnblockServer {
    /// A guided triage workflow over blocked/unassigned/deferred work.
    #[prompt(
        name = "triage",
        description = "Guided triage of blocked, unassigned, and deferred work."
    )]
    pub(crate) async fn triage(&self) -> Vec<PromptMessage> {
        triage::messages()
    }

    /// Drive the ready → claim selection (FR-20).
    #[prompt(
        name = "plan_next_work",
        description = "Plan the next unit of work: pick a ready issue and claim it."
    )]
    pub(crate) async fn plan_next_work(&self) -> Vec<PromptMessage> {
        plan_next_work::messages()
    }

    /// Close an issue and surface the newly-unblocked set (FR-11).
    #[prompt(
        name = "close_with_suggestions",
        description = "Close an issue and act on the issues it newly unblocks."
    )]
    pub(crate) async fn close_with_suggestions(&self) -> Vec<PromptMessage> {
        close_with_suggestions::messages()
    }
}
