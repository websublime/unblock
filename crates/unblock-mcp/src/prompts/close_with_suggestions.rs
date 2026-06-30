//! The `close_with_suggestions` prompt — close + surface newly-unblocked (spine §5.5, FR-11).
//!
//! Builds the guided message set that closes an issue and acts on the newly-unblocked set the close
//! returns. Pure (no `Session`).

use rmcp::model::{PromptMessage, PromptMessageRole};

/// Build the `close_with_suggestions` guided-workflow messages.
#[must_use]
pub(crate) fn messages() -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(
        PromptMessageRole::User,
        "Close an issue and act on what it unblocks. Steps:\n\
         1. Call `issue` with `{\"action\":\"close\",\"id\":\"<id>\",\"suggest_next\":true}` — the \
            result's `newly_unblocked` lists the issues this close just made ready.\n\
         2. For each newly-unblocked issue, decide whether to claim it now (via `claim`) or leave it \
            for the next planning pass.\n\
         3. Report what was closed and which issues became ready.",
    )]
}
