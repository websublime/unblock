//! Deterministic minting of [`unblock_model::CacheKey`] for ready/blocked projections, plus a
//! canonical, order-independent fingerprint of a [`unblock_model::ListFilters`] (plan §2
//! `cache_key.rs`; spine §1.9 `CacheKey` / §1.10 `ListFilters`).
//!
//! This module does **not** store anything — the engine/storage own the projection cache; policy
//! only *mints* keys. The `ready:` and `blocked:` prefixes give the two projection namespaces
//! disjoint key spaces (proven non-colliding by the cross-namespace property test).
//!
//! # Fingerprint canonicalization
//!
//! [`filters_fingerprint`] is **order-independent** and **idempotent**: the set-valued fields
//! (`status`, `issue_type`, `labels_all`, `labels_any`) are sorted and de-duplicated before being
//! rendered, so two `ListFilters` that differ only in the order/duplication of those vectors
//! produce the same fingerprint. `labels_all` and `labels_any` are kept in **distinct** sections
//! (an AND-label is not interchangeable with an OR-label), and every scalar field
//! (`assignee`, `priority_min`/`priority_max`, `text_contains`, `include_deferred`/
//! `include_closed`/`include_tombstone`, `limit`/`offset`) is serialized in a fixed, labelled order.
//! Logically-equal filters fingerprint equal; any field difference fingerprints different.

use std::fmt::Write as _;

use unblock_model::{CacheKey, ListFilters};

/// The cache-key prefix for the `ready` projection namespace.
const READY_PREFIX: &str = "ready:";
/// The cache-key prefix for the `blocked` projection namespace.
const BLOCKED_PREFIX: &str = "blocked:";

/// Mint the cache key for a `ready` projection over the given filter fingerprint.
///
/// # Examples
///
/// ```
/// use unblock_policy::cache_key_ready;
/// use unblock_model::CacheKey;
///
/// assert_eq!(cache_key_ready("abc"), CacheKey("ready:abc".to_string()));
/// ```
#[must_use]
pub fn cache_key_ready(filters_fingerprint: &str) -> CacheKey {
    CacheKey(format!("{READY_PREFIX}{filters_fingerprint}"))
}

/// Mint the cache key for a `blocked` projection over the given filter fingerprint.
///
/// The `blocked:` namespace is disjoint from the `ready:` namespace, so a `ready` key and a
/// `blocked` key built from the **same** fingerprint never collide.
///
/// # Examples
///
/// ```
/// use unblock_policy::cache_key_blocked;
/// use unblock_model::CacheKey;
///
/// assert_eq!(cache_key_blocked("abc"), CacheKey("blocked:abc".to_string()));
/// ```
#[must_use]
pub fn cache_key_blocked(filters_fingerprint: &str) -> CacheKey {
    CacheKey(format!("{BLOCKED_PREFIX}{filters_fingerprint}"))
}

/// Append a sorted, de-duplicated, length-prefixed set section to the fingerprint.
///
/// Each value is written as `<len>:<value>;` after sorting + dedup, so the section is independent
/// of the input order/duplication and unambiguous (a `;` inside a value cannot forge a boundary,
/// because the length prefix pins the byte count). The `tag` keeps distinct fields
/// (`labels_all` vs `labels_any`) in disjoint sections.
fn push_set(out: &mut String, tag: &str, values: &[String]) {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    out.push_str(tag);
    out.push('=');
    out.push('[');
    for value in sorted {
        // `write!` to a String is infallible; ignore the (impossible) error without unwrap/expect.
        let _ = write!(out, "{}:{};", value.len(), value);
    }
    out.push(']');
    out.push(';');
}

/// Append a labelled optional scalar section, distinguishing `None` from `Some("")`.
fn push_opt_str(out: &mut String, tag: &str, value: Option<&str>) {
    out.push_str(tag);
    out.push('=');
    match value {
        None => out.push_str("none;"),
        Some(value) => {
            let _ = write!(out, "some({}:{});", value.len(), value);
        }
    }
}

/// Compute the canonical, order-independent fingerprint of a [`ListFilters`] (spine §1.10).
///
/// Two filters that are logically equal (differing only in the order/duplication of their set
/// fields) produce the **same** string; any difference in any field produces a different string.
/// The result is stable across runs (no hashing, no map iteration order) and idempotent. It is the
/// fingerprint fed to [`cache_key_ready`] / [`cache_key_blocked`].
///
/// # Examples
///
/// ```
/// use unblock_policy::filters_fingerprint;
/// use unblock_model::{ListFilters, Status};
///
/// let a = ListFilters { status: vec![Status::Open, Status::Blocked], ..ListFilters::default() };
/// let b = ListFilters { status: vec![Status::Blocked, Status::Open, Status::Open],
///     ..ListFilters::default() };
/// // Order + duplication of the status set is irrelevant.
/// assert_eq!(filters_fingerprint(&a), filters_fingerprint(&b));
/// ```
#[must_use]
pub fn filters_fingerprint(filters: &ListFilters) -> String {
    let mut out = String::new();

    // Set fields: sorted + deduped, each in its own tagged section (kept distinct).
    let statuses: Vec<String> = filters
        .status
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    push_set(&mut out, "status", &statuses);

    let types: Vec<String> = filters
        .issue_type
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    push_set(&mut out, "type", &types);

    push_set(&mut out, "labels_all", &filters.labels_all);
    push_set(&mut out, "labels_any", &filters.labels_any);

    // Scalar fields, fixed order, fully labelled.
    push_opt_str(&mut out, "assignee", filters.assignee.as_deref());
    push_opt_str(&mut out, "text_contains", filters.text_contains.as_deref());

    // The numeric/bool scalars below are NOT length-prefixed (unlike the set sections and the
    // string scalars in `push_opt_str`): each renders over a closed `[0-9-]` / `true`/`false`
    // charset that cannot contain the `;`/`=`/`(`/`)` section delimiters, and each carries a unique
    // tag — so no cross-section boundary can be forged. A future *string*-typed scalar MUST instead
    // use the length-prefixed `push_opt_str` form.
    //
    // Priority bounds (serialize the inner i32 explicitly; `None` distinct from any value).
    match filters.priority_min {
        None => out.push_str("priority_min=none;"),
        Some(p) => {
            let _ = write!(out, "priority_min=some({});", p.0);
        }
    }
    match filters.priority_max {
        None => out.push_str("priority_max=none;"),
        Some(p) => {
            let _ = write!(out, "priority_max=some({});", p.0);
        }
    }

    let _ = write!(out, "include_deferred={};", filters.include_deferred);
    let _ = write!(out, "include_closed={};", filters.include_closed);
    // `include_tombstone` (FORK-1/D23) folded in so the fingerprint stays INJECTIVE — two filter
    // sets differing only in this field must fingerprint differently (else a ready/blocked
    // projection cache key collides). Bool renders over the `true`/`false` charset, delimiter-safe.
    let _ = write!(out, "include_tombstone={};", filters.include_tombstone);

    match filters.limit {
        None => out.push_str("limit=none;"),
        Some(n) => {
            let _ = write!(out, "limit=some({n});");
        }
    }
    match filters.offset {
        None => out.push_str("offset=none;"),
        Some(n) => {
            let _ = write!(out, "offset=some({n});");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{cache_key_blocked, cache_key_ready, filters_fingerprint};
    use unblock_model::{IssueType, ListFilters, Priority, Status};

    #[test]
    fn ready_and_blocked_prefixes() {
        assert_eq!(cache_key_ready("fp").0, "ready:fp");
        assert_eq!(cache_key_blocked("fp").0, "blocked:fp");
    }

    #[test]
    fn ready_and_blocked_never_collide_for_same_fingerprint() {
        let fp = filters_fingerprint(&ListFilters::default());
        assert_ne!(cache_key_ready(&fp), cache_key_blocked(&fp));
    }

    #[test]
    fn set_field_order_is_irrelevant() {
        let a = ListFilters {
            status: vec![Status::Open, Status::Blocked],
            labels_all: vec!["b".into(), "a".into()],
            ..ListFilters::default()
        };
        let b = ListFilters {
            status: vec![Status::Blocked, Status::Open],
            labels_all: vec!["a".into(), "b".into()],
            ..ListFilters::default()
        };
        assert_eq!(filters_fingerprint(&a), filters_fingerprint(&b));
    }

    #[test]
    fn duplicate_set_entries_are_deduped() {
        let a = ListFilters {
            status: vec![Status::Open],
            ..ListFilters::default()
        };
        let b = ListFilters {
            status: vec![Status::Open, Status::Open, Status::Open],
            ..ListFilters::default()
        };
        assert_eq!(filters_fingerprint(&a), filters_fingerprint(&b));
    }

    #[test]
    fn labels_all_and_labels_any_are_distinct_namespaces() {
        let all = ListFilters {
            labels_all: vec!["x".into()],
            ..ListFilters::default()
        };
        let any = ListFilters {
            labels_any: vec!["x".into()],
            ..ListFilters::default()
        };
        assert_ne!(filters_fingerprint(&all), filters_fingerprint(&any));
    }

    #[test]
    fn different_scalar_fields_differ() {
        let base = ListFilters::default();
        let with_min = ListFilters {
            priority_min: Some(Priority::HIGH),
            ..ListFilters::default()
        };
        let with_limit = ListFilters {
            limit: Some(10),
            ..ListFilters::default()
        };
        let with_closed = ListFilters {
            include_closed: true,
            ..ListFilters::default()
        };
        let with_tombstone = ListFilters {
            include_tombstone: true,
            ..ListFilters::default()
        };
        let with_assignee = ListFilters {
            assignee: Some("alice".into()),
            ..ListFilters::default()
        };
        let with_type = ListFilters {
            issue_type: vec![IssueType::Bug],
            ..ListFilters::default()
        };
        let base_fp = filters_fingerprint(&base);
        assert_ne!(base_fp, filters_fingerprint(&with_min));
        assert_ne!(base_fp, filters_fingerprint(&with_limit));
        assert_ne!(base_fp, filters_fingerprint(&with_closed));
        // FORK-1/D23 injectivity: a filter differing ONLY in `include_tombstone` fingerprints
        // distinctly (else a ready/blocked projection cache key would collide).
        assert_ne!(base_fp, filters_fingerprint(&with_tombstone));
        assert_ne!(base_fp, filters_fingerprint(&with_assignee));
        assert_ne!(base_fp, filters_fingerprint(&with_type));
    }

    #[test]
    fn none_distinct_from_some_empty_string() {
        let none = ListFilters {
            assignee: None,
            ..ListFilters::default()
        };
        let some_empty = ListFilters {
            assignee: Some(String::new()),
            ..ListFilters::default()
        };
        assert_ne!(filters_fingerprint(&none), filters_fingerprint(&some_empty));
    }

    #[test]
    fn priority_min_distinct_from_priority_max() {
        let min = ListFilters {
            priority_min: Some(Priority::HIGH),
            ..ListFilters::default()
        };
        let max = ListFilters {
            priority_max: Some(Priority::HIGH),
            ..ListFilters::default()
        };
        assert_ne!(filters_fingerprint(&min), filters_fingerprint(&max));
    }

    #[test]
    fn fingerprint_is_idempotent() {
        let f = ListFilters {
            status: vec![Status::Open],
            labels_any: vec!["z".into(), "a".into()],
            limit: Some(5),
            ..ListFilters::default()
        };
        assert_eq!(filters_fingerprint(&f), filters_fingerprint(&f));
    }

    #[test]
    fn label_value_with_separator_char_cannot_forge_boundary() {
        // A label containing the `;` separator must not collide with two separate labels, because
        // the length prefix pins each value's byte count.
        let one = ListFilters {
            labels_all: vec!["a;b".into()],
            ..ListFilters::default()
        };
        let two = ListFilters {
            labels_all: vec!["a".into(), "b".into()],
            ..ListFilters::default()
        };
        assert_ne!(filters_fingerprint(&one), filters_fingerprint(&two));
    }

    #[test]
    fn fingerprint_wire_format_is_stable() {
        // Pin the EXACT canonical encoding as a contract: a refactor of `push_set`/`push_opt_str`
        // that stayed internally consistent could silently re-mint every cache key with no
        // relational test failing — this golden catches it. All 13 `ListFilters` fields are set
        // explicitly so adding a field forces a deliberate snapshot update.
        let filters = ListFilters {
            status: vec![Status::Open, Status::Blocked],
            issue_type: vec![IssueType::Bug],
            assignee: Some("alice".into()),
            labels_all: vec!["b".into(), "a".into()],
            labels_any: vec!["x".into()],
            priority_min: Some(Priority::HIGH),
            priority_max: Some(Priority::BACKLOG),
            text_contains: Some("foo".into()),
            include_deferred: true,
            include_closed: false,
            include_tombstone: false,
            limit: Some(50),
            offset: Some(10),
        };
        insta::assert_snapshot!(filters_fingerprint(&filters));
    }
}
