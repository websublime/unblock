//! Tool **#4 `query`** — read queries over the issue store (spine §5.1/§5.2, FR-4).
//!
//! Maps `QueryInput{kind}` + `FilterInput`→`ListFilters` to `Session::{list, ready, blocked, search,
//! count, stale}`. `ready` is default-complete (no limit unless set); `search` applies the engine's
//! default cap of 50 when no limit is set (the engine fills `search_cap`). Reads never acquire the
//! write permit (FR-10).

use chrono::{DateTime, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::tool;
use serde::{Deserialize, Serialize};
use unblock_model::{CountGroupBy, ListFilters};

use crate::server::UnblockServer;
use crate::tools::dto::FilterInput;
use crate::tools::output::QueryOutput;
use crate::tools::{engine_err_json, err_json, ok_json};

/// The `query` tool input (spine §5.2 — EXACT shape).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum QueryInput {
    /// List issues matching the filters.
    List {
        #[serde(flatten)]
        filters: FilterInput,
    },
    /// The ready set (default-complete unless a limit is set).
    Ready {
        #[serde(flatten)]
        filters: FilterInput,
    },
    /// The blocked set.
    Blocked {
        #[serde(flatten)]
        filters: FilterInput,
    },
    /// Full-text search (default cap 50, applied by the engine).
    Search {
        /// The search query.
        query: String,
        /// Optional result cap override.
        #[serde(default)]
        limit: Option<usize>,
        #[serde(flatten)]
        filters: FilterInput,
    },
    /// Count issues, optionally grouped.
    Count {
        /// Optional group-by dimension.
        #[serde(default)]
        group_by: Option<CountGroupBy>,
        #[serde(flatten)]
        filters: FilterInput,
    },
    /// Stale issues (not updated since `older_than`).
    Stale {
        /// The staleness threshold.
        older_than: DateTime<Utc>,
        #[serde(flatten)]
        filters: FilterInput,
    },
}

#[rmcp::tool_router(router = query_router, vis = "pub(crate)")]
impl UnblockServer {
    /// Read queries over the issue store (FR-4).
    #[tool(
        name = "query",
        description = "Query issues: list, ready, blocked, search, count, or stale."
    )]
    pub(crate) async fn query(&self, Parameters(input): Parameters<QueryInput>) -> CallToolResult {
        if let Err(structured) = self.preflight(&input) {
            return err_json(&structured);
        }
        match input {
            QueryInput::List { filters } => {
                let filters: ListFilters = filters.into_list_filters();
                match self.session.list(&filters).await {
                    Ok(issues) => ok_json(&QueryOutput::Issues(issues)),
                    Err(err) => engine_err_json(&err),
                }
            }
            QueryInput::Ready { filters } => {
                let filters: ListFilters = filters.into_list_filters();
                match self.session.ready(&filters).await {
                    Ok(issues) => ok_json(&QueryOutput::Issues(issues)),
                    Err(err) => engine_err_json(&err),
                }
            }
            QueryInput::Blocked { filters } => {
                let filters: ListFilters = filters.into_list_filters();
                match self.session.blocked(&filters).await {
                    Ok(issues) => ok_json(&QueryOutput::Issues(issues)),
                    Err(err) => engine_err_json(&err),
                }
            }
            QueryInput::Search {
                query,
                limit,
                filters,
            } => {
                let mut filters: ListFilters = filters.into_list_filters();
                // An explicit per-query limit overrides the filter limit; otherwise the engine fills
                // the default cap of 50 (FR-4).
                if let Some(limit) = limit {
                    filters.limit = Some(limit);
                }
                match self.session.search(&query, &filters).await {
                    Ok(issues) => ok_json(&QueryOutput::Issues(issues)),
                    Err(err) => engine_err_json(&err),
                }
            }
            QueryInput::Count { group_by, filters } => {
                let filters: ListFilters = filters.into_list_filters();
                match self.session.count(&filters, group_by).await {
                    Ok(buckets) => ok_json(&QueryOutput::Counts(buckets)),
                    Err(err) => engine_err_json(&err),
                }
            }
            QueryInput::Stale {
                older_than,
                filters,
            } => {
                let filters: ListFilters = filters.into_list_filters();
                match self.session.stale(older_than, &filters).await {
                    Ok(issues) => ok_json(&QueryOutput::Issues(issues)),
                    Err(err) => engine_err_json(&err),
                }
            }
        }
    }
}
