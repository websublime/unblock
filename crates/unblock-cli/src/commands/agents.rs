//! `unblock agents` (FR-14, D27/AF-3, T3.4.3/D33) — inject/maintain a managed `AGENTS.md` block: a
//! FULL capabilities table rendered from the typed `unblock_mcp::agents_digest()` (Option C).
//!
//! `agents` is a pure file op and a separate command, and `init --agents` calls the same write. It
//! opens resolve-only (`open_workspace_with_cli`, no database) to find `workspace_dir`, then merges
//! a managed block delimited by markers, so a re-run updates only the block. It requires an
//! existing workspace (`WorkspaceNotFound` → `NotInitialized`, exit 2), so `AGENTS.md` sits next to
//! the workspace dir. [`write_managed_block`] holds the merge and the terse "wrote X" note on
//! stderr, and it returns the raw I/O error, so each caller attaches its own.
//!
//! [`managed_block`] is a THIN markdown renderer over [`unblock_mcp::agents_digest`] (D33): the
//! schema-shape walk (incl. resolving an arm-root `$ref`, e.g. `issue create` → `title`) lives in
//! `unblock-mcp`, which already owns `capabilities()`/`schema_bundle()` — this crate adds NO
//! `serde_json` production dependency and stays a plain-string renderer.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

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
    write_managed_block(&agents_path(&ctx.workspace_dir))
        .await
        .context(IoSnafu)?;
    Ok(None)
}

/// Returns the `AGENTS.md` path at the workspace root `workspace_dir`.
pub(crate) fn agents_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(AGENTS_FILENAME)
}

/// Merges the managed block into the `AGENTS.md` at `path`, then notes the write on stderr.
///
/// This is the one `AGENTS.md` write. It returns the raw I/O error, so each caller attaches its
/// own `CliError`.
///
/// # Errors
/// Returns the I/O error if reading or writing `path` fails. A missing file is not an error.
pub(crate) async fn write_managed_block(path: &Path) -> std::io::Result<()> {
    let existing = read_existing(path).await?;
    let digest = unblock_mcp::agents_digest();
    let merged = merge_managed_block(existing.as_deref(), &managed_block(&digest));
    tokio::fs::write(path, merged).await?;
    output::diag(&format!("wrote {}", path.display()));
    Ok(())
}

/// Read the existing `AGENTS.md` (if any); a missing file is `None` (not an error).
async fn read_existing(path: &Path) -> std::io::Result<Option<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The managed block content (between the markers, exclusive): a FULL capabilities table rendered
/// from `digest`, the pure typed [`unblock_mcp::agents_digest`] (Option C, D33). Every table and
/// the discriminant intro come from `digest`, so a synthetic digest exercises the same path as the
/// live one; [`write_managed_block`] calls `agents_digest()` once and passes it.
fn managed_block(digest: &unblock_mcp::AgentsDigest) -> String {
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
        "Actions (structural — derived from the `unblock://schema` `oneOf` discriminants; the\n\
         Discriminant cell is the exact `key: value` pair to send, and each row lists the FULL\n\
         parameter surface, required AND optional):\n\n",
    );
    out.push_str(&discriminant_intro(&digest.tools));
    out.push_str("\n\n");
    out.push_str("| Tool | Discriminant | Required params | Optional params |\n");
    out.push_str("|---|---|---|---|\n");
    for tool in &digest.tools {
        for action in &tool.actions {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} |",
                tool.name,
                discriminant_cell(action.discriminant.as_ref()),
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

/// The ONE intro sentence naming which tools use which discriminant key (ub-q2v, spine §5b), derived
/// from the digest, never hard-coded. Shape: ``Discriminant keys: `<key>` — `<tool>`, `<tool>`;
/// none (flat) — `<tool>`.`` Keyed groups follow first appearance in digest tool order, tools keep
/// digest order, every group (the flat one included) is joined by `; `, the `none (flat)` group comes
/// LAST and is omitted when no flat tool exists, and a tool with no actions is skipped.
fn discriminant_intro(tools: &[unblock_mcp::ToolDigest]) -> String {
    let mut keyed: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut flat: Vec<&str> = Vec::new();
    for tool in tools {
        let Some(action) = tool.actions.first() else {
            continue;
        };
        match &action.discriminant {
            Some(discriminant) => {
                let key = discriminant.key.as_str();
                match keyed.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, names)) => names.push(&tool.name),
                    None => keyed.push((key, vec![&tool.name])),
                }
            }
            None => flat.push(&tool.name),
        }
    }

    let mut groups: Vec<String> = keyed
        .iter()
        .map(|(key, names)| format!("`{key}` — {}", backticked(names)))
        .collect();
    if !flat.is_empty() {
        groups.push(format!("none (flat) — {}", backticked(&flat)));
    }
    format!("Discriminant keys: {}.", groups.join("; "))
}

/// Backtick each name and join with `, `.
fn backticked(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render an action's discriminant cell: the exact wire pair `` `key: value` ``, or `—` for a flat
/// tool's implicit action.
fn discriminant_cell(discriminant: Option<&unblock_mcp::Discriminant>) -> String {
    discriminant.map_or_else(|| "—".to_string(), |d| format!("`{}: {}`", d.key, d.value))
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
    use unblock_mcp::{Discriminant, ToolAction, ToolDigest};

    use super::{BEGIN_MARKER, END_MARKER, discriminant_intro, managed_block, merge_managed_block};

    #[test]
    fn managed_block_mentions_mcp_and_contract() {
        let block = managed_block(&unblock_mcp::agents_digest());
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
        let block = managed_block(&unblock_mcp::agents_digest());
        assert!(
            block.contains("| `issue` | `action: create_bulk` |"),
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
                block.contains(&format!("| `issue` | `action: {action}` |")),
                "missing issue action {action}"
            );
        }
    }

    /// ub-q2v: every row names the WIRE discriminant key. `query` and `diagnostics` are keyed `kind`
    /// (their schemas reject `action`), so a row rendered as `action: …` would mislead agents into a
    /// rejected call; flat `claim` renders `—`.
    #[test]
    fn managed_block_names_query_discriminant_kind() {
        let block = managed_block(&unblock_mcp::agents_digest());
        for kind in ["list", "ready", "blocked", "search", "count", "stale"] {
            assert!(
                block.contains(&format!("| `query` | `kind: {kind}` |")),
                "query {kind} must render keyed `kind`"
            );
        }
        assert!(block.contains("| `diagnostics` | `kind: stats` |"));
        // Every row of the Actions table (header through the next blank line).
        let rows: Vec<&str> = block
            .lines()
            .skip_while(|line| !line.starts_with("| Tool | Discriminant |"))
            .skip(2)
            .take_while(|line| !line.is_empty())
            .collect();
        let mut kind_rows = 0;
        let mut claim_rows = 0;
        for row in &rows {
            if row.starts_with("| `query` |") || row.starts_with("| `diagnostics` |") {
                kind_rows += 1;
                assert!(
                    row.contains(" | `kind: "),
                    "row must be keyed `kind`: {row}"
                );
                assert!(
                    !row.contains("`action:"),
                    "row must not be keyed `action`: {row}"
                );
            }
            if row.starts_with("| `claim` |") {
                claim_rows += 1;
                assert!(row.starts_with("| `claim` | — |"), "claim is flat: {row}");
            }
        }
        assert!(kind_rows > 0, "no query/diagnostics action rows rendered");
        assert_eq!(claim_rows, 1, "claim renders exactly one flat row");
    }

    /// A synthetic tool whose every action carries `key` (or none for a flat tool), `actions` long.
    fn tool(name: &str, key: Option<&str>, actions: usize) -> ToolDigest {
        ToolDigest {
            name: name.to_string(),
            description: String::new(),
            actions: (0..actions)
                .map(|i| ToolAction {
                    discriminant: key.map(|key| Discriminant {
                        key: key.to_string(),
                        value: format!("v{i}"),
                    }),
                    required: Vec::new(),
                    optional: Vec::new(),
                })
                .collect(),
        }
    }

    /// ub-q2v: the intro sentence is DERIVED from the digest, so a hard-coded sentence fails here.
    /// A SYNTHETIC digest moves `query` to `action`, adds a third key (`verb`), puts the first `kind`
    /// tool and the flat tool ahead of the `action` tools, and adds a tool with no actions: keyed
    /// groups follow first appearance, tools keep digest order, `none (flat)` comes last, and the
    /// actionless tool is skipped. A digest with no keyed group, and one with no flat tool, keep the
    /// same separators.
    #[test]
    fn managed_block_intro_derives_discriminant_keys() {
        let tools = [
            tool("diagnostics", Some("kind"), 2),
            tool("claim", None, 1),
            tool("issue", Some("action"), 3),
            tool("empty", Some("kind"), 0),
            tool("query", Some("action"), 2),
            tool("plan", Some("verb"), 1),
            tool("lock", None, 1),
        ];
        assert_eq!(
            discriminant_intro(&tools),
            "Discriminant keys: `kind` — `diagnostics`; `action` — `issue`, `query`; \
             `verb` — `plan`; none (flat) — `claim`, `lock`."
        );

        let flat_only = [tool("claim", None, 1), tool("empty", None, 0)];
        assert_eq!(
            discriminant_intro(&flat_only),
            "Discriminant keys: none (flat) — `claim`."
        );

        let keyed_only = [
            tool("issue", Some("action"), 1),
            tool("dep", Some("action"), 1),
        ];
        assert_eq!(
            discriminant_intro(&keyed_only),
            "Discriminant keys: `action` — `issue`, `dep`."
        );

        // The live block carries the derived sentence directly after the lead-in, which names no
        // tool or key.
        let block = managed_block(&unblock_mcp::agents_digest());
        let intro = discriminant_intro(&unblock_mcp::agents_digest().tools);
        assert_eq!(
            intro,
            "Discriminant keys: `action` — `issue`, `defer`, `dep`, `sync`, `comment`; \
             `kind` — `query`, `diagnostics`; none (flat) — `claim`."
        );
        assert!(
            block.contains(&format!(
                "required AND optional):\n\n{intro}\n\n| Tool | Discriminant |"
            )),
            "the intro must sit directly after the Actions lead-in"
        );

        // The rendered block is driven by its digest argument, not by the live digest: a synthetic
        // digest (`query` moved to `action`, a third key `verb`) renders its own derived sentence.
        let synthetic = unblock_mcp::AgentsDigest {
            contract_version: "test".to_string(),
            tools: vec![
                tool("issue", Some("action"), 1),
                tool("query", Some("action"), 1),
                tool("diagnostics", Some("kind"), 1),
                tool("plan", Some("verb"), 1),
                tool("claim", None, 1),
            ],
            resources: Vec::new(),
            prompts: Vec::new(),
            error_codes: Vec::new(),
        };
        let synthetic_block = managed_block(&synthetic);
        let synthetic_intro = synthetic_block
            .lines()
            .find(|line| line.starts_with("Discriminant keys:"))
            .expect("rendered block carries an intro line");
        assert_eq!(
            synthetic_intro,
            "Discriminant keys: `action` — `issue`, `query`; `kind` — `diagnostics`; \
             `verb` — `plan`; none (flat) — `claim`."
        );
        assert_eq!(synthetic_intro, discriminant_intro(&synthetic.tools));
        assert_ne!(
            synthetic_intro, intro,
            "synthetic intro must differ from the live one"
        );
    }

    /// Drift-guard (D33, Miguel's ruling): the `issue`/`create` row lists `title` as a REQUIRED param —
    /// proving the arm-root `$ref` (`issue.create` → `#/$defs/CreateInput`) is resolved one level. A
    /// walk that ignored the arm-root `$ref` would show `—` in the Required-params column instead.
    #[test]
    fn managed_block_shows_create_title_param() {
        let block = managed_block(&unblock_mcp::agents_digest());
        assert!(
            block.contains("| `issue` | `action: create` | `title` |"),
            "issue create must list `title` as its required param (arm-root $ref resolved)"
        );
    }

    #[test]
    fn fresh_file_gets_a_wrapped_block() {
        let out = merge_managed_block(None, &managed_block(&unblock_mcp::agents_digest()));
        assert!(out.starts_with(BEGIN_MARKER));
        assert!(out.contains(END_MARKER));
        assert!(out.contains("unblock mcp"));
    }

    #[test]
    fn append_preserves_existing_content() {
        let existing = "# My project\n\nSome notes.\n";
        let out = merge_managed_block(
            Some(existing),
            &managed_block(&unblock_mcp::agents_digest()),
        );
        assert!(out.starts_with("# My project"));
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains("Some notes."));
    }

    #[test]
    fn rerun_replaces_only_the_block_idempotently() {
        let first = merge_managed_block(
            Some("# Header\n"),
            &managed_block(&unblock_mcp::agents_digest()),
        );
        let second =
            merge_managed_block(Some(&first), &managed_block(&unblock_mcp::agents_digest()));
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

    #[tokio::test]
    async fn write_managed_block_creates_then_rewrites_identically() {
        use super::{agents_path, write_managed_block};

        let root = tempfile::tempdir().expect("tempdir");
        let path = agents_path(root.path());
        assert_eq!(path, root.path().join("AGENTS.md"));

        write_managed_block(&path).await.expect("first write");
        let first = std::fs::read_to_string(&path).expect("read AGENTS.md");
        assert_eq!(first.matches(BEGIN_MARKER).count(), 1, "one block: {first}");
        assert_eq!(first.matches(END_MARKER).count(), 1, "one block: {first}");

        write_managed_block(&path).await.expect("second write");
        let second = std::fs::read_to_string(&path).expect("re-read AGENTS.md");
        assert_eq!(first, second, "a second write leaves the bytes identical");
    }

    /// A read failure other than a missing file stops the write, so hand-written bytes that are not
    /// UTF-8 survive untouched.
    #[tokio::test]
    async fn write_managed_block_propagates_a_read_failure() {
        use super::write_managed_block;

        let dir_root = tempfile::tempdir().expect("tempdir");
        let dir_path = dir_root.path().join("AGENTS.md");
        std::fs::create_dir(&dir_path).expect("mkdir AGENTS.md");
        let err = write_managed_block(&dir_path)
            .await
            .expect_err("a directory is no text file");
        assert_ne!(err.kind(), std::io::ErrorKind::NotFound, "{err}");

        let bytes_root = tempfile::tempdir().expect("tempdir");
        let bytes_path = bytes_root.path().join("AGENTS.md");
        let hand_written = b"# Mine\n\xff\xfe notes\n";
        std::fs::write(&bytes_path, hand_written).expect("seed AGENTS.md");
        write_managed_block(&bytes_path)
            .await
            .expect_err("invalid UTF-8 is a read failure");
        assert_eq!(
            std::fs::read(&bytes_path).expect("re-read AGENTS.md"),
            hand_written,
            "the hand-written bytes are untouched"
        );
    }
}
