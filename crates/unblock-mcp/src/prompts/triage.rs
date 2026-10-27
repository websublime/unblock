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
         2. Call `query` with `{\"kind\":\"count\",\"group_by\":\"assignee\"}` to size unassigned \
            work (issues with no assignee or an empty one count under the key `\"\"`; a \
            spaces-only assignee gets its own key), then `{\"kind\":\"list\"}` and keep the issues \
            whose `assignee` is absent or blank (empty or spaces only). No filter \
            selects every unassigned issue: `assignee` matches one exact value, and omitting it (or \
            passing null) matches all issues.\n\
         3. Call `query` with `{\"kind\":\"stale\",\"older_than\":\"<RFC 3339 timestamp 30 days \
            before now>\"}` for issues not updated in the last 30 days.\n\
         4. Find deferred work. It takes two forms. The `defer` tool sets a `defer_until` \
            timestamp and leaves the status unchanged (usually `open`): in the step-2 list, keep \
            the issues whose `defer_until` is in the future. An issue whose status was set to \
            `deferred` is hidden from `list`, `count`, `stale` and `search` by default (`blocked` \
            shows it only when it is blocked): call `query` with \
            `{\"kind\":\"list\",\"status\":[\"deferred\"],\"include_deferred\":true}` (without \
            `include_deferred` this returns nothing).\n\
         5. For each blocked issue, inspect its `dep` `tree` and propose the next unblocking action.\n\
         6. Summarise: what is blocked, unassigned, stale and deferred, and the highest-leverage \
            next step.",
    )]
}
