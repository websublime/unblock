//! `unblock agents` (FR-14, D27/AF-3, T3.4.3/D33) — inject/maintain a managed `AGENTS.md` block: a
//! FULL capabilities table rendered from the typed `unblock_mcp::agents_digest()` (Option C).
//!
//! A pure file op (SEPARATE from `init`): resolve-only open (`open_workspace_with_cli`, NO DB) to find
//! `workspace_dir`, then an idempotent merge of a MANAGED block delimited by markers (a re-run updates
//! ONLY the block). Requires an existing workspace (`WorkspaceNotFound` → `NotInitialized`, exit 2) so
//! `AGENTS.md` sits next to `.unblock/`. Writes a terse "wrote X" note to stderr.
//!
//! [`managed_block`] is a THIN markdown renderer over [`unblock_mcp::agents_digest`] (D33): the
//! schema-shape walk (incl. resolving an arm-root `$ref`, e.g. `issue create` → `title`) lives in
//! `unblock-mcp`, which already owns `capabilities()`/`schema_bundle()` — this crate adds NO
//! `serde_json` production dependency and stays a plain-string renderer.

use std::fmt::Write as _;
use std::path::Path;

use snafu::ResultExt;
use unblock_config::open_workspace_with_cli;

use crate::cli::{AgentsArgs, GlobalArgs};
use crate::exit::{CliError, IoSnafu};
use crate::output;

/// The managed-block start marker (a re-run replaces everything between the markers, inclusive).
const BEGIN_MARKER: &str = "<!-- BEGIN unblock -->";
/// The managed-block end marker.
const END_MARKER: &str = "<!-- END unblock -->";
/// The `AGENTS.md` filename written at the workspace root.
const AGENTS_FILENAME: &str = "AGENTS.md";

/// Run `unblock agents`.
///
/// # Errors
/// - [`CliError::Config`] if no workspace is found (`WorkspaceNotFound` → `NotInitialized`, exit 2);
/// - [`CliError::Io`] if reading or writing `AGENTS.md` fails.
pub async fn run(_args: &AgentsArgs, global: &GlobalArgs) -> Result<Option<u8>, CliError> {
    // Resolve-only open (NO DB) to learn the workspace root next to `.unblock/`.
    let ctx = open_workspace_with_cli(&global.to_overrides()).await?;
    let path = ctx.workspace_dir.join(AGENTS_FILENAME);

    let existing = read_existing(&path)?;
    let merged = merge_managed_block(existing.as_deref(), &managed_block());
    std::fs::write(&path, merged).context(IoSnafu)?;

    output::diag(&format!("wrote {}", path.display()));
    Ok(None)
}

/// Read the existing `AGENTS.md` (if any); a missing file is `None` (not an error).
fn read_existing(path: &Path) -> Result<Option<String>, CliError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(CliError::Io { source: err }),
    }
}

/// The managed block content (between the markers, exclusive): a FULL capabilities table rendered
/// from the pure typed [`unblock_mcp::agents_digest`] (Option C, D33). Zero-arg — it calls
/// `agents_digest()` internally so `run`/the merge tests below stay untouched.
fn managed_block() -> String {
    let digest = unblock_mcp::agents_digest();
    let mut out = String::new();

    out.push_str("## unblock (MCP)\n\n");
    out.push_str(
        "This workspace is tracked by **unblock**. Issue-data operations are exposed over MCP — the\n\
         `unblock` CLI is lifecycle/ops only.\n\n",
    );
    out.push_str("- Start the server: `unblock mcp` (MCP over stdio).\n");
    let _ = writeln!(out, "- Contract: `{}`.", digest.contract_version);
    out.push_str(
        "- Machine-readable discovery: read `unblock://capabilities` (the source of these tables) and\n\
         `unblock://schema` (the full JsonSchema bundle for every tool I/O).\n\n",
    );

    out.push_str("### Tools\n\n");
    out.push_str("Descriptors (from `unblock://capabilities`):\n\n");
    out.push_str("| Tool | Description |\n|---|---|\n");
    for tool in &digest.tools {
        let _ = writeln!(out, "| `{}` | {} |", tool.name, tool.description);
    }
    out.push('\n');

    out.push_str(
        "Actions (structural — derived from the `unblock://schema` `oneOf` discriminants; each row\n\
         lists an action's FULL parameter surface, required AND optional):\n\n",
    );
    out.push_str("| Tool | Action | Required params | Optional params |\n|---|---|---|---|\n");
    for tool in &digest.tools {
        for action in &tool.actions {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} |",
                tool.name,
                action_cell(action.name.as_deref()),
                params_cell(&action.required),
                params_cell(&action.optional),
            );
        }
    }
    out.push('\n');

    out.push_str("### Resources\n\n");
    out.push_str("| Resource | Description |\n|---|---|\n");
    for resource in &digest.resources {
        let _ = writeln!(out, "| `{}` | {} |", resource.uri, resource.description);
    }
    out.push('\n');

    out.push_str("### Prompts\n\n");
    out.push_str("| Prompt | Description |\n|---|---|\n");
    for prompt in &digest.prompts {
        let _ = writeln!(out, "| `{}` | {} |", prompt.name, prompt.description);
    }
    out.push('\n');

    out.push_str("### Error codes\n\n");
    out.push_str("| Code | Exit | Retryable |\n|---|---|---|\n");
    for error in &digest.error_codes {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            error.code,
            error.exit_code,
            yes_no(error.retryable),
        );
    }

    out
}

/// Render an action's discriminant cell: `` `name` `` or `—` for a flat tool's implicit action.
fn action_cell(name: Option<&str>) -> String {
    name.map_or_else(|| "—".to_string(), |n| format!("`{n}`"))
}

/// Render a sorted param list as backtick-joined, comma-separated names, or `—` when empty.
fn params_cell(params: &[String]) -> String {
    if params.is_empty() {
        "—".to_string()
    } else {
        params
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `yes`/`no` for a retryability cell.
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Idempotently merge the managed `block` into `existing` (or a fresh file). If the delimited block is
/// present it is REPLACED in place; otherwise it is appended (with markers). Content outside the block
/// is preserved verbatim.
fn merge_managed_block(existing: Option<&str>, block: &str) -> String {
    let wrapped = format!("{BEGIN_MARKER}\n{block}{END_MARKER}\n");
    match existing {
        None => wrapped,
        Some(text) => {
            if let (Some(start), Some(end)) = (text.find(BEGIN_MARKER), text.find(END_MARKER)) {
                // Replace the existing block (markers inclusive) in place.
                let end_of_block = end + END_MARKER.len();
                // Consume a single trailing newline after the end marker to avoid growing blank lines.
                let tail_start = text[end_of_block..]
                    .strip_prefix('\n')
                    .map_or(end_of_block, |_| end_of_block + 1);
                let mut merged = String::with_capacity(text.len() + wrapped.len());
                merged.push_str(&text[..start]);
                merged.push_str(&wrapped);
                merged.push_str(&text[tail_start..]);
                merged
            } else {
                // No managed block yet — append after the existing content (separated by a blank line).
                let mut merged = String::with_capacity(text.len() + wrapped.len() + 1);
                merged.push_str(text);
                if !text.is_empty() && !text.ends_with('\n') {
                    merged.push('\n');
                }
                if !text.is_empty() {
                    merged.push('\n');
                }
                merged.push_str(&wrapped);
                merged
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BEGIN_MARKER, END_MARKER, managed_block, merge_managed_block};

    #[test]
    fn managed_block_mentions_mcp_and_contract() {
        let block = managed_block();
        assert!(block.contains("unblock mcp"));
        assert!(block.contains(unblock_mcp::CONTRACT_VERSION));
        // The four capabilities-table sections (D33).
        for heading in [
            "### Tools",
            "### Resources",
            "### Prompts",
            "### Error codes",
        ] {
            assert!(block.contains(heading), "missing section {heading}");
        }
        // The two machine-readable discovery pointers.
        assert!(block.contains("unblock://capabilities"));
        assert!(block.contains("unblock://schema"));
    }

    /// Drift-guard (D33): `create_bulk` exists ONLY as a `oneOf` arm of the `issue` tool's input — the
    /// tool's one-line description never names it. Its presence in the rendered table proves the
    /// structural `oneOf` walk drives the render, not a scrape of the description prose.
    #[test]
    fn managed_block_lists_every_tool_action() {
        let block = managed_block();
        assert!(
            block.contains("| `issue` | `create_bulk` |"),
            "create_bulk action must render under `issue` (structural oneOf walk)"
        );
        for action in [
            "create",
            "create_bulk",
            "show",
            "update",
            "close",
            "reopen",
            "delete",
            "restore",
        ] {
            assert!(
                block.contains(&format!("| `issue` | `{action}` |")),
                "missing issue action {action}"
            );
        }
    }

    /// Drift-guard (D33, Miguel's ruling): the `issue`/`create` row lists `title` as a REQUIRED param —
    /// proving the arm-root `$ref` (`issue.create` → `#/$defs/CreateInput`) is resolved one level. A
    /// walk that ignored the arm-root `$ref` would show `—` in the Required-params column instead.
    #[test]
    fn managed_block_shows_create_title_param() {
        let block = managed_block();
        assert!(
            block.contains("| `issue` | `create` | `title` |"),
            "issue create must list `title` as its required param (arm-root $ref resolved)"
        );
    }

    #[test]
    fn fresh_file_gets_a_wrapped_block() {
        let out = merge_managed_block(None, &managed_block());
        assert!(out.starts_with(BEGIN_MARKER));
        assert!(out.contains(END_MARKER));
        assert!(out.contains("unblock mcp"));
    }

    #[test]
    fn append_preserves_existing_content() {
        let existing = "# My project\n\nSome notes.\n";
        let out = merge_managed_block(Some(existing), &managed_block());
        assert!(out.starts_with("# My project"));
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains("Some notes."));
    }

    #[test]
    fn rerun_replaces_only_the_block_idempotently() {
        let first = merge_managed_block(Some("# Header\n"), &managed_block());
        let second = merge_managed_block(Some(&first), &managed_block());
        // Idempotent: a second merge with the same block yields identical bytes.
        assert_eq!(first, second);
        // Exactly one managed block (markers appear once).
        assert_eq!(second.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(second.matches(END_MARKER).count(), 1);
        // Content outside the block is preserved.
        assert!(second.starts_with("# Header"));
    }

    #[test]
    fn changed_block_is_swapped_but_surrounding_text_kept() {
        let existing = merge_managed_block(Some("intro\n"), "OLD CONTENT\n");
        let updated = merge_managed_block(Some(&existing), "NEW CONTENT\n");
        assert!(updated.contains("NEW CONTENT"));
        assert!(!updated.contains("OLD CONTENT"));
        assert!(updated.starts_with("intro"));
        assert_eq!(updated.matches(BEGIN_MARKER).count(), 1);
    }
}
