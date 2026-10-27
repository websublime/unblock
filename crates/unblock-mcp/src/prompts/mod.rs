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

#[cfg(test)]
mod tests {
    //! Every tool call a prompt suggests must pass the LIVE argument boundary (`parse_args`, the
    //! D42 prologue every tool runs). A suggested call missing a required field, or carrying an
    //! unknown one, fails here instead of failing for the agent that follows the prompt (ub-kwh).

    use rmcp::model::{JsonObject, PromptMessage, PromptMessageContent, PromptMessageRole};
    use serde_json::Value;
    use unblock_error::StructuredError;

    use crate::tools::args::parse_args;
    use crate::tools::claim::ClaimInput;
    use crate::tools::comment::CommentToolInput;
    use crate::tools::defer::DeferInput;
    use crate::tools::dep::DepToolInput;
    use crate::tools::diagnostics::DiagnosticsInput;
    use crate::tools::issue::IssueInput;
    use crate::tools::query::QueryInput;
    use crate::tools::sync::SyncInput;

    /// Stand-in for a `<...>` placeholder: a timestamp for the timestamp-typed fields, an id otherwise.
    fn placeholder_value(key: &str) -> Value {
        match key {
            "older_than" | "until" | "due_at" | "defer_until" | "since" => {
                Value::String("2026-01-01T00:00:00Z".to_owned())
            }
            _ => Value::String("ub-x".to_owned()),
        }
    }

    /// Why a suggested call fails: it names no real tool, or its tool's argument boundary rejects it.
    enum Rejection {
        UnknownTool(String),
        Args(StructuredError),
    }

    fn validate(tool: &str, mut args: JsonObject) -> Result<(), Rejection> {
        for (key, value) in &mut args {
            if value.as_str().is_some_and(|s| s.starts_with('<')) {
                *value = placeholder_value(key);
            }
        }
        let outcome = match tool {
            "issue" => parse_args::<IssueInput>(args).map(drop),
            "claim" => parse_args::<ClaimInput>(args).map(drop),
            "defer" => parse_args::<DeferInput>(args).map(drop),
            "query" => parse_args::<QueryInput>(args).map(drop),
            "dep" => parse_args::<DepToolInput>(args).map(drop),
            "sync" => parse_args::<SyncInput>(args).map(drop),
            "diagnostics" => parse_args::<DiagnosticsInput>(args).map(drop),
            "comment" => parse_args::<CommentToolInput>(args).map(drop),
            other => return Err(Rejection::UnknownTool(other.to_owned())),
        };
        outcome.map_err(Rejection::Args)
    }

    /// Walk the backticked spans. A span followed by ` with` names the tool (any name, so a typo
    /// reaches `validate` and is rejected); a `{...}` span is a call to the most recently named
    /// tool. Returns the number of calls checked, or the first rejected call.
    fn check_prompt(messages: &[PromptMessage]) -> Result<usize, String> {
        let mut checked = 0;
        for message in messages {
            let PromptMessageContent::Text { text } = &message.content else {
                continue;
            };
            let parts: Vec<&str> = text.split('`').collect();
            if parts.len().is_multiple_of(2) {
                return Err("unbalanced backticks".to_owned());
            }
            let mut tool: Option<&str> = None;
            for (i, span) in parts.iter().enumerate().skip(1).step_by(2) {
                if span.starts_with('{') {
                    let args: JsonObject = serde_json::from_str(span)
                        .map_err(|e| format!("`{span}` is not a JSON object: {e}"))?;
                    let tool = tool.ok_or_else(|| format!("`{span}` names no tool"))?;
                    validate(tool, args).map_err(|err| match err {
                        Rejection::UnknownTool(name) => format!("no tool named `{name}`"),
                        Rejection::Args(err) => {
                            format!("`{tool}` call `{span}` is rejected: {err:?}")
                        }
                    })?;
                    checked += 1;
                } else if parts
                    .get(i + 1)
                    .is_some_and(|prose| prose.starts_with(" with "))
                {
                    tool = Some(span);
                }
            }
        }
        Ok(checked)
    }

    fn text_message(text: &str) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(PromptMessageRole::User, text)]
    }

    #[test]
    fn every_suggested_call_passes_the_argument_boundary() {
        assert_eq!(check_prompt(&super::triage::messages()), Ok(6));
        assert_eq!(check_prompt(&super::plan_next_work::messages()), Ok(2));
        assert_eq!(
            check_prompt(&super::close_with_suggestions::messages()),
            Ok(1)
        );
    }

    #[test]
    fn the_pre_fix_stale_call_is_rejected_for_its_missing_older_than() {
        // The exact call the triage prompt suggested before ub-kwh.
        let args: JsonObject = serde_json::from_str(r#"{"kind":"stale"}"#).expect("object");
        let Err(Rejection::Args(err)) = validate("query", args) else {
            panic!("`{{\"kind\":\"stale\"}}` must be rejected by the query boundary");
        };
        assert_eq!(err.context["kind"], "missing_field");
        assert_eq!(err.context["field"], "older_than");
        assert!(check_prompt(&text_message("Call `query` with `{\"kind\":\"stale\"}`.")).is_err());
    }

    #[test]
    fn a_misspelled_tool_name_is_rejected() {
        let messages = text_message(
            "Call `query` with `{\"kind\":\"ready\"}`, then `querry` with `{\"kind\":\"list\"}`.",
        );
        let err = check_prompt(&messages).expect_err("`querry` is not a tool");
        assert!(err.contains("no tool named `querry`"), "{err}");
    }
}
