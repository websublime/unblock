//! The `plan_next_work` prompt — drive the ready → claim selection (spine §5.5, FR-20).
//!
//! Builds the guided message set that walks an agent from the default-complete `ready` set to a
//! `claim`. Pure (no `Session`).

use rmcp::model::{PromptMessage, PromptMessageRole};

/// Build the `plan_next_work` guided-workflow messages.
#[must_use]
pub(crate) fn messages() -> Vec<PromptMessage> {
    vec![PromptMessage::new_text(
        PromptMessageRole::User,
        "Plan the next unit of work. Steps:\n\
         1. Read the `unblock://issues/ready` resource (or call `query` with `{\"kind\":\"ready\"}`) \
            — it is default-complete and already hybrid-ranked.\n\
         2. Pick the highest-ranked ready issue that fits the current session's scope.\n\
         3. Call `claim` with `{\"id\":\"<chosen-id>\",\"assignee\":\"<you>\"}` to take it \
            atomically (if the claim loses a race, pick the next ready issue and retry).\n\
         4. Report the claimed issue and a one-line plan to complete it.",
    )]
}
