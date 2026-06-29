//! Render contract suite — every `OutputFormat` × every renderable kind (the render analogue of
//! the Storage contract suite, NFR-16).
//!
//! For each (format × kind) it asserts: (a) no panic, (b) byte-determinism across two runs, (c)
//! parse-back for the structured formats — json/robot via serde to the equal §1.10 DTO, csv via
//! `csv::Reader` (dev-dep, read side only) for RFC-4180 well-formedness + cell count.
//!
//! Two HARD acceptance criteria are pinned here (they cannot be skipped at Verify):
//! - the **context-value-ESC escape** test (plain/markdown/csv) — an ESC byte in a
//!   `StructuredError.context` value (and in a `Custom` status for csv) is escaped, never raw;
//! - the **CSV `ALL_FIELDS` 15-column** column-pin snapshot.

use chrono::{TimeZone, Utc};
use serde_json::json;
use unblock_error::{ErrorCode, StructuredError};
use unblock_model::{
    CountBucket, DepTree, DependencyType, DiagnosticFinding, DiagnosticKind, DiagnosticReport,
    GraphEdge, Issue, OutputFormat, Status,
};
use unblock_render::{ContentType, RenderOptions, RenderOutput, renderer_for};

fn all_formats() -> Vec<OutputFormat> {
    let formats = vec![
        OutputFormat::Json,
        OutputFormat::Robot,
        OutputFormat::Plain,
        OutputFormat::Csv,
        OutputFormat::Markdown,
    ];
    #[cfg(feature = "toon")]
    let formats = {
        let mut formats = formats;
        formats.push(OutputFormat::Toon);
        formats
    };
    formats
}

fn issue_fixture() -> Issue {
    Issue {
        id: "ub-abc123".to_string(),
        title: "Render, please".to_string(),
        description: Some("multi\nline".to_string()),
        assignee: Some("alice".to_string()),
        created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 6).unwrap(),
        labels: vec!["backend".to_string(), "p0".to_string()],
        // The model serializes `compaction_level: None` as `0` (bd conformance, D12), so a JSONL
        // round-trip turns `None` into `Some(0)`. Pin `Some(0)` here so the render parse-back is
        // byte-for-byte lossless (this asymmetry is the model's, not render's).
        compaction_level: Some(0),
        ..Issue::default()
    }
}

fn issues_fixture() -> Vec<Issue> {
    let mut second = issue_fixture();
    second.id = "ub-def456".to_string();
    second.title = "Second, with, commas".to_string();
    second.status = Status::InProgress;
    vec![issue_fixture(), second]
}

fn counts_fixture() -> Vec<CountBucket> {
    vec![
        CountBucket {
            key: "open".to_string(),
            count: 3,
        },
        CountBucket {
            key: "closed".to_string(),
            count: 1,
        },
    ]
}

fn dep_tree_fixture() -> DepTree {
    DepTree {
        root: "ub-a".to_string(),
        edges: vec![
            GraphEdge {
                from: "ub-a".to_string(),
                to: "ub-b".to_string(),
                dep_type: DependencyType::Blocks,
            },
            GraphEdge {
                from: "ub-b".to_string(),
                to: "ub-c".to_string(),
                dep_type: DependencyType::ParentChild,
            },
        ],
    }
}

fn cycles_fixture() -> Vec<Vec<String>> {
    vec![vec![
        "ub-a".to_string(),
        "ub-b".to_string(),
        "ub-a".to_string(),
    ]]
}

fn error_fixture() -> StructuredError {
    StructuredError::from_code(ErrorCode::ValidationFailed, "invalid input")
        .with_hint("fix the title")
        .with_context("field", json!("title"))
}

fn diagnostics_fixture() -> DiagnosticReport {
    DiagnosticReport {
        kind: DiagnosticKind::Stats,
        findings: vec![DiagnosticFinding {
            label: "issues".to_string(),
            detail: "42".to_string(),
        }],
    }
}

/// Drive a renderer method twice and assert it never panics and is byte-deterministic.
fn assert_deterministic<F>(render: F) -> RenderOutput
where
    F: Fn() -> Result<RenderOutput, unblock_render::RenderError>,
{
    let first = render();
    let second = render();
    match (&first, &second) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.stdout, b.stdout, "render must be byte-deterministic");
            a.clone()
        }
        (Err(_), Err(_)) => {
            // An unsupported kind must err deterministically; return a sentinel.
            first.expect_err("both runs erred");
            RenderOutput::new(String::new(), ContentType::Text)
        }
        _ => panic!("render must be deterministic across runs (one ok, one err)"),
    }
}

#[test]
fn every_format_every_kind_no_panic_and_deterministic() {
    let opts = RenderOptions::default();
    let issue = issue_fixture();
    let issues = issues_fixture();
    let counts = counts_fixture();
    let tree = dep_tree_fixture();
    let cycles = cycles_fixture();
    let error = error_fixture();
    let diagnostics = diagnostics_fixture();

    for fmt in all_formats() {
        let r = renderer_for(fmt, opts.clone());
        assert_eq!(r.format(), fmt, "factory round-trip");

        assert_deterministic(|| r.issue(&issue, &opts));
        assert_deterministic(|| r.issues(&issues, &opts));
        assert_deterministic(|| r.counts(&counts, &opts));
        assert_deterministic(|| r.dep_tree(&tree, &opts));
        assert_deterministic(|| r.cycles(&cycles, &opts));
        assert_deterministic(|| r.structured_error(&error, &opts));
        assert_deterministic(|| r.diagnostics(&diagnostics, &opts));
    }
}

#[test]
fn json_and_robot_parse_back_to_equal_dto() {
    let opts = RenderOptions::default();
    for fmt in [OutputFormat::Json, OutputFormat::Robot] {
        let r = renderer_for(fmt, opts.clone());

        // issues round-trip.
        let issues = issues_fixture();
        let out = r.issues(&issues, &opts).unwrap();
        let back: Vec<Issue> = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back, issues);

        // counts round-trip.
        let counts = counts_fixture();
        let out = r.counts(&counts, &opts).unwrap();
        let back: Vec<CountBucket> = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back, counts);

        // dep_tree round-trip.
        let tree = dep_tree_fixture();
        let out = r.dep_tree(&tree, &opts).unwrap();
        let back: DepTree = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back, tree);

        // structured_error round-trip + always-valid JSON (FR-11).
        let error = error_fixture();
        let out = r.structured_error(&error, &opts).unwrap();
        let back: StructuredError = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back, error);

        // diagnostics round-trip.
        let diag = diagnostics_fixture();
        let out = r.diagnostics(&diag, &opts).unwrap();
        let back: DiagnosticReport = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(back, diag);
    }
}

#[test]
fn csv_reparses_to_same_cell_count() {
    let opts = RenderOptions::default();
    let r = renderer_for(OutputFormat::Csv, opts.clone());
    let issues = issues_fixture();
    let out = r.issues(&issues, &opts).unwrap();

    // Read side only: re-parse the emitted rows with the `csv` dev-dep.
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(out.stdout.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let mut data_rows = 0usize;
    for record in reader.records() {
        let record = record.expect("emitted CSV must be RFC-4180 well-formed");
        assert_eq!(
            record.len(),
            headers.len(),
            "every row has the header's cell count"
        );
        data_rows += 1;
    }
    assert_eq!(data_rows, issues.len());
}

#[test]
fn csv_rejects_non_issue_kinds() {
    let opts = RenderOptions::default();
    let r = renderer_for(OutputFormat::Csv, opts.clone());
    assert!(r.counts(&counts_fixture(), &opts).is_err());
    assert!(r.dep_tree(&dep_tree_fixture(), &opts).is_err());
    assert!(r.cycles(&cycles_fixture(), &opts).is_err());
    assert!(r.structured_error(&error_fixture(), &opts).is_err());
    assert!(r.diagnostics(&diagnostics_fixture(), &opts).is_err());
}

// ----- HARD AC 1: context-value ESC escape across plain/markdown (csv: Custom status) -----

#[test]
fn context_value_esc_escaped_in_plain_and_markdown() {
    let opts = RenderOptions::default();
    let err = StructuredError::from_code(ErrorCode::ValidationFailed, "bad")
        .with_context("provided", json!("danger\x1b[2Jwipe\x07"));

    for fmt in [OutputFormat::Plain, OutputFormat::Markdown] {
        let r = renderer_for(fmt, opts.clone());
        let out = r.structured_error(&err, &opts).unwrap();
        assert!(
            !out.stdout.contains('\x1b'),
            "{fmt:?}: raw ESC must never reach output"
        );
        assert!(
            !out.stdout.contains('\x07'),
            "{fmt:?}: raw BEL must never reach output"
        );
        // The escaped byte sequence is visible (`1b`/`7` tokens survive escaping/markdown).
        assert!(
            out.stdout.contains("1b"),
            "{fmt:?}: escaped ESC token present: {}",
            out.stdout
        );
    }
}

#[test]
fn custom_status_esc_escaped_in_csv() {
    let opts = RenderOptions::default();
    let mut issue = issue_fixture();
    issue.status = Status::Custom("state\x1b[2J\x07".to_string());
    let r = renderer_for(OutputFormat::Csv, opts.clone());
    let out = r.issues(std::slice::from_ref(&issue), &opts).unwrap();
    assert!(!out.stdout.contains('\x1b'), "raw ESC must never reach CSV");
    assert!(!out.stdout.contains('\x07'), "raw BEL must never reach CSV");
    assert!(out.stdout.contains("\\u{1b}"));
}

// ----- HARD AC 2: CSV ALL_FIELDS 15-column column-pin snapshot -----

#[test]
fn csv_all_fields_15_column_pin() {
    let all = [
        "id",
        "title",
        "description",
        "status",
        "priority",
        "issue_type",
        "assignee",
        "owner",
        "created_at",
        "updated_at",
        "closed_at",
        "due_at",
        "defer_until",
        "notes",
        "external_ref",
    ];
    assert_eq!(all.len(), 15);
    let opts = RenderOptions::default()
        .with_csv_fields(Some(all.iter().map(|s| (*s).to_string()).collect()));
    let r = renderer_for(OutputFormat::Csv, opts.clone());
    let issue = issue_fixture();
    let out = r.issues(std::slice::from_ref(&issue), &opts).unwrap();

    // Pin the exact header + the single data row (column order + bare-int priority + fmt_ts).
    insta::assert_snapshot!("csv_all_fields_15_column_pin", out.stdout);

    // Independently assert the 15-column header.
    let header = out.stdout.lines().next().unwrap();
    assert_eq!(header, all.join(","));
}
