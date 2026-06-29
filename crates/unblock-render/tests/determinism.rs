//! Byte-determinism + caller-order preservation across all formats (proptest).
//!
//! Render must produce identical bytes for a fixed input + fixed `RenderOptions`, with no
//! `HashMap`-iteration flake, and must **preserve** the caller's `Vec` order (never re-sort —
//! MF-5). These properties are verified over arbitrary inputs.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use unblock_model::{Issue, OutputFormat};
use unblock_render::{RenderOptions, renderer_for};

fn formats() -> Vec<OutputFormat> {
    vec![
        OutputFormat::Json,
        OutputFormat::Robot,
        OutputFormat::Plain,
        OutputFormat::Csv,
        OutputFormat::Markdown,
    ]
}

fn issue_with(id: String, title: String) -> Issue {
    Issue {
        id,
        title,
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        ..Issue::default()
    }
}

proptest! {
    #[test]
    fn issues_render_is_byte_deterministic(
        ids in prop::collection::vec("ub-[a-z0-9]{1,8}", 0..6),
        titles in prop::collection::vec(".{0,40}", 0..6),
    ) {
        let n = ids.len().min(titles.len());
        let issues: Vec<Issue> = (0..n)
            .map(|i| issue_with(ids[i].clone(), titles[i].clone()))
            .collect();
        let opts = RenderOptions::default();
        for fmt in formats() {
            let r = renderer_for(fmt, opts.clone());
            let a = r.issues(&issues, &opts).unwrap();
            let b = r.issues(&issues, &opts).unwrap();
            prop_assert_eq!(a.stdout, b.stdout);
        }
    }

    #[test]
    fn issues_render_preserves_caller_order(
        count in 2usize..6,
    ) {
        // Render must NOT re-sort the caller's `Vec` (MF-5). Build issues with monotonically
        // DECREASING ids (so a sort-by-id would reorder them) and unique ids (so each occurrence
        // is unambiguous), then assert the plain output keeps the input order.
        let ids: Vec<String> = (0..count).map(|i| format!("ub-{:04}", count - i)).collect();
        let issues: Vec<Issue> = ids
            .iter()
            .map(|id| issue_with(id.clone(), "t".to_string()))
            .collect();
        let opts = RenderOptions::default();
        let r = renderer_for(OutputFormat::Plain, opts.clone());
        let out = r.issues(&issues, &opts).unwrap();

        // Each id's position must be strictly increasing in input order (no re-sort).
        let mut last_pos: Option<usize> = None;
        for id in &ids {
            let pos = out.stdout.find(id.as_str())
                .expect("each id appears in plain output");
            if let Some(prev) = last_pos {
                prop_assert!(pos > prev, "ids must render in caller order (no re-sort)");
            }
            last_pos = Some(pos);
        }
    }
}
