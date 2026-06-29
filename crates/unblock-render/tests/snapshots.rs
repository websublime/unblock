//! insta golden snapshots — one per (format × representative fixture).
//!
//! These pin the exact human/structured output bytes (NFR-14 snapshot-stability gate). The
//! fixtures use fixed timestamps so `fmt_ts` output is deterministic. Snapshots live under
//! `tests/snapshots/`.

use chrono::{TimeZone, Utc};
use serde_json::json;
use unblock_error::{ErrorCode, StructuredError};
use unblock_model::{
    CountBucket, DepTree, DependencyType, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
    GraphEdge, Issue, OutputFormat, Status,
};
use unblock_render::{RenderOptions, renderer_for};

fn issue() -> Issue {
    Issue {
        id: "ub-abc123".to_string(),
        title: "Render the docs".to_string(),
        description: Some("first line\nsecond line".to_string()),
        assignee: Some("alice".to_string()),
        status: Status::InProgress,
        created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 6).unwrap(),
        labels: vec!["docs".to_string()],
        ..Issue::default()
    }
}

fn issues() -> Vec<Issue> {
    let mut second = issue();
    second.id = "ub-def456".to_string();
    second.title = "Pipes | and, commas".to_string();
    second.status = Status::Open;
    vec![issue(), second]
}

fn counts() -> Vec<CountBucket> {
    vec![
        CountBucket {
            key: "open".to_string(),
            count: 5,
        },
        CountBucket {
            key: "in_progress".to_string(),
            count: 2,
        },
    ]
}

fn dep_tree() -> DepTree {
    DepTree {
        root: "ub-root".to_string(),
        edges: vec![
            GraphEdge {
                from: "ub-root".to_string(),
                to: "ub-child".to_string(),
                dep_type: DependencyType::ParentChild,
            },
            GraphEdge {
                from: "ub-child".to_string(),
                to: "ub-blocked".to_string(),
                dep_type: DependencyType::Blocks,
            },
        ],
    }
}

fn cycles() -> Vec<Vec<String>> {
    vec![vec![
        "ub-a".to_string(),
        "ub-b".to_string(),
        "ub-a".to_string(),
    ]]
}

fn error() -> StructuredError {
    StructuredError::from_code(ErrorCode::ValidationFailed, "title too long")
        .with_hint("shorten the title")
        .with_context("provided", json!("danger\x1b[2Jwipe"))
}

fn diagnostics() -> DiagnosticReport {
    DiagnosticReport {
        kind: DiagnosticKind::Stats,
        findings: vec![
            DiagnosticFinding {
                label: "total".to_string(),
                detail: "42".to_string(),
            },
            DiagnosticFinding {
                label: "open".to_string(),
                detail: "7".to_string(),
            },
        ],
    }
}

fn render(fmt: OutputFormat, opts: &RenderOptions, kind: &str) -> String {
    let r = renderer_for(fmt, opts.clone());
    let result = match kind {
        "issue" => r.issue(&issue(), opts),
        "issues" => r.issues(&issues(), opts),
        "counts" => r.counts(&counts(), opts),
        "dep_tree" => r.dep_tree(&dep_tree(), opts),
        "cycles" => r.cycles(&cycles(), opts),
        "error" => r.structured_error(&error(), opts),
        "diagnostics" => r.diagnostics(&diagnostics(), opts),
        other => panic!("unknown kind {other}"),
    };
    match result {
        Ok(out) => out.stdout,
        Err(e) => format!("<ERR: {e}>"),
    }
}

macro_rules! snap {
    ($name:expr, $fmt:expr, $kind:expr) => {{
        let opts = RenderOptions::default();
        insta::assert_snapshot!($name, render($fmt, &opts, $kind));
    }};
    ($name:expr, $fmt:expr, $kind:expr, $opts:expr) => {{
        insta::assert_snapshot!($name, render($fmt, &$opts, $kind));
    }};
}

#[test]
fn json_goldens() {
    let pretty = RenderOptions::default().with_pretty_json(true);
    snap!("json_issue_pretty", OutputFormat::Json, "issue", pretty);
    snap!("json_issues_pretty", OutputFormat::Json, "issues", pretty);
    snap!("json_error_pretty", OutputFormat::Json, "error", pretty);
    snap!("robot_issues_compact", OutputFormat::Robot, "issues");
    snap!("robot_error_compact", OutputFormat::Robot, "error");
}

#[test]
fn plain_goldens() {
    snap!("plain_issue", OutputFormat::Plain, "issue");
    snap!("plain_issues", OutputFormat::Plain, "issues");
    snap!("plain_counts", OutputFormat::Plain, "counts");
    snap!("plain_dep_tree", OutputFormat::Plain, "dep_tree");
    snap!("plain_cycles", OutputFormat::Plain, "cycles");
    snap!("plain_error", OutputFormat::Plain, "error");
    snap!("plain_diagnostics", OutputFormat::Plain, "diagnostics");

    // Empty-list note.
    let opts = RenderOptions::default();
    let r = renderer_for(OutputFormat::Plain, opts.clone());
    insta::assert_snapshot!("plain_issues_empty", r.issues(&[], &opts).unwrap().stdout);
}

#[test]
fn csv_goldens() {
    snap!("csv_issues_default_fields", OutputFormat::Csv, "issues");

    // Empty list = header only.
    let opts = RenderOptions::default();
    let r = renderer_for(OutputFormat::Csv, opts.clone());
    insta::assert_snapshot!("csv_issues_empty", r.issues(&[], &opts).unwrap().stdout);
}

#[test]
fn markdown_goldens() {
    snap!("markdown_issue", OutputFormat::Markdown, "issue");
    snap!("markdown_issues_table", OutputFormat::Markdown, "issues");
    snap!("markdown_counts", OutputFormat::Markdown, "counts");
    snap!("markdown_dep_tree", OutputFormat::Markdown, "dep_tree");
    snap!("markdown_cycles", OutputFormat::Markdown, "cycles");
    snap!("markdown_error", OutputFormat::Markdown, "error");
    snap!(
        "markdown_diagnostics",
        OutputFormat::Markdown,
        "diagnostics"
    );
}
