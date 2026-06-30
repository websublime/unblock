//! The `triage` prompt — a guided triage workflow (spine §5.5).
//!
//! Builds the guided message set that walks an agent through triaging blocked / unassigned /
//! deferred work via the read tools/resources. Pure (no `Session`) — the agent drives the actual
//! reads via the `query`/`dep` tools and the `unblock://issues/...` resources.

use rmcp::model::{PromptMessage, PromptMessageRole};

/// Build the `triage` guided-workflow messages.
#[must_use]
pub(crate) fn messages() -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(
        PromptMessageRole::User,
        "Triage the workspace. Steps:\n\
         1. Call `query` with `{\"kind\":\"blocked\"}` to find blocked issues, and `dep` with \
            `{\"action\":\"cycles\"}` to surface any dependency cycles.\n\
         2. Call `query` with `{\"kind\":\"list\",\"assignee\":null}` to find unassigned work, and \
            `{\"kind\":\"stale\"}` for stale issues.\n\
         3. For each blocked issue, inspect its `dep` `tree` and propose the next unblocking action.\n\
         4. Summarise: what is blocked, what is unassigned, and the highest-leverage next step.",
    )]
}
