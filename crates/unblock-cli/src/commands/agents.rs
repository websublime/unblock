//! `unblock agents` (FR-14, D27/AF-3) — inject/maintain a managed `AGENTS.md` block describing the
//! MCP wiring (how an agent connects to `unblock serve`).
//!
//! A pure file op (SEPARATE from `init`): resolve-only open (`open_workspace_with_cli`, NO DB) to find
//! `workspace_dir`, then an idempotent merge of a MANAGED block delimited by markers (a re-run updates
//! ONLY the block). Requires an existing workspace (`WorkspaceNotFound` → `NotInitialized`, exit 2) so
//! `AGENTS.md` sits next to `.unblock/`. Writes a terse "wrote X" note to stderr.

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

/// The managed block content (between the markers, exclusive) describing the MCP wiring.
fn managed_block() -> String {
    format!(
        "## unblock (MCP)\n\
         \n\
         This workspace is tracked by **unblock**. Issue-data operations are exposed over MCP — the\n\
         `unblock` CLI is lifecycle/ops only.\n\
         \n\
         - Start the server: `unblock serve` (MCP over stdio).\n\
         - Contract: `{contract}`.\n\
         - Tools: use the MCP tool set (create/list/close/dep/…) — do NOT shell out to the CLI for\n\
         issue data.\n",
        contract = unblock_mcp::CONTRACT_VERSION,
    )
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
    fn managed_block_mentions_serve_and_contract() {
        let block = managed_block();
        assert!(block.contains("unblock serve"));
        assert!(block.contains(unblock_mcp::CONTRACT_VERSION));
    }

    #[test]
    fn fresh_file_gets_a_wrapped_block() {
        let out = merge_managed_block(None, &managed_block());
        assert!(out.starts_with(BEGIN_MARKER));
        assert!(out.contains(END_MARKER));
        assert!(out.contains("unblock serve"));
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
