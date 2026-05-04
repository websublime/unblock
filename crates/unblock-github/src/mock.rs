//! Test-only mock implementation of [`crate::GitHubApi`].
//!
//! `MockGitHubClient` is a hand-written mock that satisfies the entire
//! [`GitHubApi`] trait surface so it can be stored in `Arc<dyn GitHubApi>`
//! and substituted for a real [`crate::client::GitHubClient`] in unit and
//! integration tests across the workspace.
//!
//! # Design
//!
//! - **Gating** — the entire module is compiled only when the `test-hooks`
//!   Cargo feature is enabled on `unblock-github`. Production builds never
//!   see this code. Downstream test crates depend on
//!   `unblock-github = { features = ["test-hooks"] }` in their
//!   `[dev-dependencies]` table.
//! - **Call counters** — each trait method has a dedicated, named
//!   [`std::sync::atomic::AtomicUsize`] field on [`crate::mock::CallCounts`] (no
//!   string-keyed `HashMap`).
//!   This makes counter access type-safe and compile-checked: a typo in a
//!   getter name is a build error, not a silent zero.
//! - **Stub storage** — each fallible async trait method has a dedicated
//!   `std::sync::Mutex<VecDeque<Result<T, Error>>>` queue on [`crate::mock::Stubs`]. Tests
//!   pre-load expected responses with `push_*` helpers; each call pops one
//!   from the front. When the queue is empty the method falls back to a
//!   deterministic default — see the per-method documentation below for
//!   exactly which default. The defaults are chosen so that:
//!     1. methods exercised by `unblock-hd9` (the immediate downstream
//!        consumer) return [`crate::errors::Error::MockNotStubbed`] to force tests to be
//!        explicit about expected return values, and
//!     2. methods that are unlikely to be exercised return
//!        [`crate::errors::Error::MockNotStubbed`] as well, so a forgotten stub fails the
//!        test loudly instead of silently producing a misleading default.
//! - **Sync accessors** — backed by plain owned fields set at construction
//!   time, no atomics required (these are read in tests, not mutated under
//!   contention).
//! - **Trait drift** — adding a new method to [`GitHubApi`] will fail to
//!   compile here until the mock is updated. This is intentional: it
//!   guarantees the mock surface stays in lockstep with the production
//!   trait.
//!
//! # Usage
//!
//! ```ignore
//! use std::sync::Arc;
//! use unblock_github::{GitHubApi, MockGitHubClient};
//! use unblock_github::projects::{ProjectFieldIds, /* ... */};
//!
//! let mock = Arc::new(MockGitHubClient::new("acme", "widgets", Some(1)));
//! mock.push_field_ids(None);
//! let api: Arc<dyn GitHubApi> = mock.clone();
//!
//! // ... exercise code under test that uses `api` ...
//!
//! assert_eq!(mock.calls().field_ids(), 1);
//! ```

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use unblock_core::types::{BlockingEdge, Issue, IssueRef, IssueSummary};

use crate::api::GitHubApi;
use crate::errors::Error;
use crate::mutations::{CreateIssueParams, Milestone};
use crate::projects::{
    CreateViewParams, CreatedProject, FieldValue, OwnerProject, OwnerType, ProjectFieldIds,
    ProjectInfo, ProjectView, RestField, SetupReport, SetupStatus,
};

/// Per-method invocation counters for [`MockGitHubClient`].
///
/// One [`AtomicUsize`] per trait method. Read counts via the named getters.
#[derive(Debug, Default)]
pub struct CallCounts {
    field_ids: AtomicUsize,
    set_field_ids: AtomicUsize,
    resolve_project_info: AtomicUsize,
    setup_fields: AtomicUsize,
    query_setup_status: AtomicUsize,
    query_issue_types_status: AtomicUsize,
    ensure_issue_types: AtomicUsize,
    update_field: AtomicUsize,
    detect_owner_type: AtomicUsize,
    list_rest_fields: AtomicUsize,
    create_view: AtomicUsize,
    list_views: AtomicUsize,
    resolve_owner_node_id: AtomicUsize,
    list_owner_projects: AtomicUsize,
    create_project: AtomicUsize,
    fetch_issue: AtomicUsize,
    fetch_issue_ref: AtomicUsize,
    fetch_graph_data: AtomicUsize,
    create_issue: AtomicUsize,
    close_issue: AtomicUsize,
    reopen_issue: AtomicUsize,
    search_issues: AtomicUsize,
    add_comment: AtomicUsize,
    add_comment_in_repo: AtomicUsize,
    add_comment_ref: AtomicUsize,
    update_issue_body: AtomicUsize,
    update_issue_type: AtomicUsize,
    add_labels_to_issue: AtomicUsize,
    remove_label_from_issue: AtomicUsize,
    add_assignees_to_issue: AtomicUsize,
    remove_assignees_from_issue: AtomicUsize,
    list_milestones: AtomicUsize,
    update_issue_milestone: AtomicUsize,
    add_blocked_by: AtomicUsize,
    remove_blocked_by: AtomicUsize,
    add_sub_issue: AtomicUsize,
    resolve_issue_ref: AtomicUsize,
    get_project_item_id: AtomicUsize,
    ensure_labels: AtomicUsize,
    add_blocked_by_ref: AtomicUsize,
    add_blocked_by_refs: AtomicUsize,
    remove_blocked_by_ref: AtomicUsize,
    remove_blocked_by_refs: AtomicUsize,
}

macro_rules! count_getters {
    ($($name:ident),* $(,)?) => {
        $(
            #[doc = concat!("Returns the number of times `", stringify!($name), "` was invoked on the mock.")]
            #[must_use]
            pub fn $name(&self) -> usize {
                self.$name.load(Ordering::SeqCst)
            }
        )*
    };
}

impl CallCounts {
    count_getters!(
        field_ids,
        set_field_ids,
        resolve_project_info,
        setup_fields,
        query_setup_status,
        query_issue_types_status,
        ensure_issue_types,
        update_field,
        detect_owner_type,
        list_rest_fields,
        create_view,
        list_views,
        resolve_owner_node_id,
        list_owner_projects,
        create_project,
        fetch_issue,
        fetch_issue_ref,
        fetch_graph_data,
        create_issue,
        close_issue,
        reopen_issue,
        search_issues,
        add_comment,
        add_comment_in_repo,
        add_comment_ref,
        update_issue_body,
        update_issue_type,
        add_labels_to_issue,
        remove_label_from_issue,
        add_assignees_to_issue,
        remove_assignees_from_issue,
        list_milestones,
        update_issue_milestone,
        add_blocked_by,
        remove_blocked_by,
        add_sub_issue,
        resolve_issue_ref,
        get_project_item_id,
        ensure_labels,
        add_blocked_by_ref,
        add_blocked_by_refs,
        remove_blocked_by_ref,
        remove_blocked_by_refs,
    );

    /// Resets every counter to zero. Useful between phases of a single test.
    pub fn reset(&self) {
        let zero = || 0_usize;
        macro_rules! reset {
            ($($f:ident),* $(,)?) => {{
                $( self.$f.store(zero(), Ordering::SeqCst); )*
            }};
        }
        reset!(
            field_ids,
            set_field_ids,
            resolve_project_info,
            setup_fields,
            query_setup_status,
            query_issue_types_status,
            ensure_issue_types,
            update_field,
            detect_owner_type,
            list_rest_fields,
            create_view,
            list_views,
            resolve_owner_node_id,
            list_owner_projects,
            create_project,
            fetch_issue,
            fetch_issue_ref,
            fetch_graph_data,
            create_issue,
            close_issue,
            reopen_issue,
            search_issues,
            add_comment,
            add_comment_in_repo,
            add_comment_ref,
            update_issue_body,
            update_issue_type,
            add_labels_to_issue,
            remove_label_from_issue,
            add_assignees_to_issue,
            remove_assignees_from_issue,
            list_milestones,
            update_issue_milestone,
            add_blocked_by,
            remove_blocked_by,
            add_sub_issue,
            resolve_issue_ref,
            get_project_item_id,
            ensure_labels,
            add_blocked_by_ref,
            add_blocked_by_refs,
            remove_blocked_by_ref,
            remove_blocked_by_refs,
        );
    }
}

/// Type alias for the `fetch_graph_data` stub return shape.
pub type GraphDataResult = Result<(Vec<Issue>, Vec<BlockingEdge>), Error>;

/// Per-method response stub queues for [`MockGitHubClient`].
///
/// Each fallible async trait method has a dedicated [`Mutex`] wrapping a
/// [`VecDeque`] of pre-loaded `Result` values. Tests `push_*` responses; the
/// mock pops one per call. When a queue is empty the corresponding trait
/// method falls back to [`crate::errors::Error::MockNotStubbed`].
///
/// `field_ids` is special: it returns `Option<ProjectFieldIds>` (not a
/// `Result`), so its queue stores `Option<ProjectFieldIds>` and the empty-
/// fallback is `None`.
#[derive(Debug, Default)]
pub struct Stubs {
    field_ids: Mutex<VecDeque<Option<ProjectFieldIds>>>,
    resolve_project_info: Mutex<VecDeque<Result<ProjectInfo, Error>>>,
    setup_fields: Mutex<VecDeque<Result<SetupReport, Error>>>,
    query_setup_status: Mutex<VecDeque<Result<SetupStatus, Error>>>,
    query_issue_types_status: Mutex<VecDeque<Result<Vec<String>, Error>>>,
    ensure_issue_types: Mutex<VecDeque<Result<Vec<String>, Error>>>,
    update_field: Mutex<VecDeque<Result<(), Error>>>,
    detect_owner_type: Mutex<VecDeque<Result<OwnerType, Error>>>,
    list_rest_fields: Mutex<VecDeque<Result<Vec<RestField>, Error>>>,
    create_view: Mutex<VecDeque<Result<ProjectView, Error>>>,
    list_views: Mutex<VecDeque<Result<Vec<ProjectView>, Error>>>,
    resolve_owner_node_id: Mutex<VecDeque<Result<String, Error>>>,
    list_owner_projects: Mutex<VecDeque<Result<Vec<OwnerProject>, Error>>>,
    create_project: Mutex<VecDeque<Result<CreatedProject, Error>>>,
    fetch_issue: Mutex<VecDeque<Result<Issue, Error>>>,
    fetch_issue_ref: Mutex<VecDeque<Result<Issue, Error>>>,
    fetch_graph_data: Mutex<VecDeque<GraphDataResult>>,
    create_issue: Mutex<VecDeque<Result<Issue, Error>>>,
    close_issue: Mutex<VecDeque<Result<(), Error>>>,
    reopen_issue: Mutex<VecDeque<Result<(), Error>>>,
    search_issues: Mutex<VecDeque<Result<Vec<IssueSummary>, Error>>>,
    add_comment: Mutex<VecDeque<Result<String, Error>>>,
    add_comment_in_repo: Mutex<VecDeque<Result<String, Error>>>,
    add_comment_ref: Mutex<VecDeque<Result<String, Error>>>,
    /// Argument-aware log for `add_comment_ref` invocations — stores every
    /// [`IssueRef`] the mock saw, in call order. Tests use this to assert
    /// that the close cascade dispatched against the qualified refs,
    /// covering SPEC §8.2 step 6 / §11.4 row 4.
    add_comment_ref_calls: Mutex<Vec<IssueRef>>,
    update_issue_body: Mutex<VecDeque<Result<(), Error>>>,
    update_issue_type: Mutex<VecDeque<Result<(), Error>>>,
    add_labels_to_issue: Mutex<VecDeque<Result<(), Error>>>,
    remove_label_from_issue: Mutex<VecDeque<Result<(), Error>>>,
    add_assignees_to_issue: Mutex<VecDeque<Result<(), Error>>>,
    remove_assignees_from_issue: Mutex<VecDeque<Result<(), Error>>>,
    list_milestones: Mutex<VecDeque<Result<Vec<Milestone>, Error>>>,
    update_issue_milestone: Mutex<VecDeque<Result<(), Error>>>,
    add_blocked_by: Mutex<VecDeque<Result<(), Error>>>,
    remove_blocked_by: Mutex<VecDeque<Result<(), Error>>>,
    add_sub_issue: Mutex<VecDeque<Result<(), Error>>>,
    resolve_issue_ref: Mutex<VecDeque<Result<String, Error>>>,
    get_project_item_id: Mutex<VecDeque<Result<String, Error>>>,
    ensure_labels: Mutex<VecDeque<Result<(), Error>>>,
    add_blocked_by_ref: Mutex<VecDeque<Result<(), Error>>>,
    add_blocked_by_refs: Mutex<VecDeque<Result<(), Error>>>,
    remove_blocked_by_ref: Mutex<VecDeque<Result<(), Error>>>,
    remove_blocked_by_refs: Mutex<VecDeque<Result<(), Error>>>,
}

/// Hand-written mock for [`GitHubApi`].
///
/// See module-level docs for design rationale and usage examples.
#[derive(Debug)]
pub struct MockGitHubClient {
    owner: String,
    repo: String,
    project_number: Option<u64>,
    api_base_url: String,
    calls: CallCounts,
    stubs: Stubs,
}

impl MockGitHubClient {
    /// Constructs a new mock with the supplied repo coordinates.
    ///
    /// The API base URL defaults to `https://api.github.com`. Use
    /// [`MockGitHubClient::with_api_base_url`] to override for GHE-style
    /// tests.
    #[must_use]
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        project_number: Option<u64>,
    ) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            project_number,
            api_base_url: "https://api.github.com".to_owned(),
            calls: CallCounts::default(),
            stubs: Stubs::default(),
        }
    }

    /// Replaces the API base URL backing field. Builder-style.
    #[must_use]
    pub fn with_api_base_url(mut self, api_base_url: impl Into<String>) -> Self {
        self.api_base_url = api_base_url.into();
        self
    }

    /// Returns a reference to the per-method [`crate::mock::CallCounts`].
    #[must_use]
    pub fn calls(&self) -> &CallCounts {
        &self.calls
    }

    /// Returns a snapshot of every [`IssueRef`] passed to
    /// [`add_comment_ref`](GitHubApi::add_comment_ref), in call order.
    ///
    /// Unlike the bare counter on [`CallCounts`], this lets tests assert
    /// that the close-cascade Phase-3 loop (spec §8.2 step 6) dispatched
    /// the comment against the correct qualified ref — required to cover
    /// the SPEC §11.4 row 4 contract.
    ///
    /// # Panics
    ///
    /// Panics only if the internal log `Mutex` is poisoned, which can
    /// only happen if a previous test thread panicked while holding the
    /// lock — never in normal use.
    #[must_use]
    pub fn add_comment_ref_calls(&self) -> Vec<IssueRef> {
        self.stubs
            .add_comment_ref_calls
            .lock()
            .expect("mock call-log mutex poisoned")
            .clone()
    }
}

/// Generates a `push_<method>` helper for a stub queue.
///
/// Two arms are supported:
/// - `Option<$ok>` for queues that store option values directly (empty
///   queue falls back to `None`).
/// - `$ok` for `Result<$ok, Error>` queues (the common fallible case).
///
/// The `Option` arm must be listed first so that the generic arm does not
/// shadow it by matching `Option<T>` as a single type.
macro_rules! push_result {
    ($name:ident, $push:ident, Option<$ok:ty>) => {
        impl MockGitHubClient {
            #[doc = concat!("Queues an `Option<", stringify!($ok), ">` to be returned by the next `", stringify!($name), "` call.")]
            ///
            /// This queue stores the option value directly; the empty-queue
            /// fallback is `None`.
            ///
            /// # Panics
            ///
            /// Panics only if the internal stub `Mutex` is poisoned, which
            /// can only happen if a previous test thread panicked while
            /// holding the lock — never in normal use.
            pub fn $push(&self, response: Option<$ok>) {
                self.stubs
                    .$name
                    .lock()
                    .expect("mock stub mutex poisoned")
                    .push_back(response);
            }
        }
    };
    ($name:ident, $push:ident, $ok:ty) => {
        impl MockGitHubClient {
            #[doc = concat!("Queues a `Result<", stringify!($ok), ", Error>` to be returned by the next `", stringify!($name), "` call.")]
            ///
            /// This queue stores a `Result` value; the empty-queue fallback
            /// is a `MethodNotMocked` error.
            ///
            /// # Panics
            ///
            /// Panics only if the internal stub `Mutex` is poisoned, which
            /// can only happen if a previous test thread panicked while
            /// holding the lock — never in normal use.
            pub fn $push(&self, response: Result<$ok, Error>) {
                self.stubs
                    .$name
                    .lock()
                    .expect("mock stub mutex poisoned")
                    .push_back(response);
            }
        }
    };
}

push_result!(resolve_project_info, push_resolve_project_info, ProjectInfo);
push_result!(setup_fields, push_setup_fields, SetupReport);
push_result!(query_setup_status, push_query_setup_status, SetupStatus);
push_result!(
    query_issue_types_status,
    push_query_issue_types_status,
    Vec<String>
);
push_result!(ensure_issue_types, push_ensure_issue_types, Vec<String>);
push_result!(update_field, push_update_field, ());
push_result!(detect_owner_type, push_detect_owner_type, OwnerType);
push_result!(list_rest_fields, push_list_rest_fields, Vec<RestField>);
push_result!(create_view, push_create_view, ProjectView);
push_result!(list_views, push_list_views, Vec<ProjectView>);
push_result!(resolve_owner_node_id, push_resolve_owner_node_id, String);
push_result!(
    list_owner_projects,
    push_list_owner_projects,
    Vec<OwnerProject>
);
push_result!(create_project, push_create_project, CreatedProject);
push_result!(fetch_issue, push_fetch_issue, Issue);
push_result!(fetch_issue_ref, push_fetch_issue_ref, Issue);
push_result!(
    fetch_graph_data,
    push_fetch_graph_data,
    (Vec<Issue>, Vec<BlockingEdge>)
);
push_result!(create_issue, push_create_issue, Issue);
push_result!(close_issue, push_close_issue, ());
push_result!(reopen_issue, push_reopen_issue, ());
push_result!(search_issues, push_search_issues, Vec<IssueSummary>);
push_result!(add_comment, push_add_comment, String);
push_result!(add_comment_in_repo, push_add_comment_in_repo, String);
push_result!(add_comment_ref, push_add_comment_ref, String);
push_result!(update_issue_body, push_update_issue_body, ());
push_result!(update_issue_type, push_update_issue_type, ());
push_result!(add_labels_to_issue, push_add_labels_to_issue, ());
push_result!(remove_label_from_issue, push_remove_label_from_issue, ());
push_result!(add_assignees_to_issue, push_add_assignees_to_issue, ());
push_result!(
    remove_assignees_from_issue,
    push_remove_assignees_from_issue,
    ()
);
push_result!(list_milestones, push_list_milestones, Vec<Milestone>);
push_result!(update_issue_milestone, push_update_issue_milestone, ());
push_result!(add_blocked_by, push_add_blocked_by, ());
push_result!(remove_blocked_by, push_remove_blocked_by, ());
push_result!(add_sub_issue, push_add_sub_issue, ());
push_result!(resolve_issue_ref, push_resolve_issue_ref, String);
push_result!(get_project_item_id, push_get_project_item_id, String);
push_result!(ensure_labels, push_ensure_labels, ());
push_result!(add_blocked_by_ref, push_add_blocked_by_ref, ());
push_result!(add_blocked_by_refs, push_add_blocked_by_refs, ());
push_result!(remove_blocked_by_ref, push_remove_blocked_by_ref, ());
push_result!(remove_blocked_by_refs, push_remove_blocked_by_refs, ());
push_result!(field_ids, push_field_ids, Option<ProjectFieldIds>);

/// Pops the next stub off a queue, or returns `Error::MockNotStubbed` with
/// the supplied static method name.
fn pop_or_unstubbed<T>(
    queue: &Mutex<VecDeque<Result<T, Error>>>,
    method: &'static str,
) -> Result<T, Error> {
    queue
        .lock()
        .expect("mock stub mutex poisoned")
        .pop_front()
        .unwrap_or(Err(Error::MockNotStubbed { method }))
}

#[async_trait]
impl GitHubApi for MockGitHubClient {
    fn owner(&self) -> &str {
        &self.owner
    }

    fn repo(&self) -> &str {
        &self.repo
    }

    fn project_number(&self) -> Option<u64> {
        self.project_number
    }

    fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    fn rest_url(&self, path: &str) -> String {
        format!(
            "{}/repos/{}/{}{}",
            self.api_base_url, self.owner, self.repo, path
        )
    }

    fn graphql_url(&self) -> String {
        format!("{}/graphql", self.api_base_url)
    }

    async fn field_ids(&self) -> Option<ProjectFieldIds> {
        self.calls.field_ids.fetch_add(1, Ordering::SeqCst);
        self.stubs
            .field_ids
            .lock()
            .expect("mock stub mutex poisoned")
            .pop_front()
            .unwrap_or(None)
    }

    async fn set_field_ids(&self, _ids: ProjectFieldIds) {
        self.calls.set_field_ids.fetch_add(1, Ordering::SeqCst);
    }

    async fn resolve_project_info(&self) -> Result<ProjectInfo, Error> {
        self.calls
            .resolve_project_info
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.resolve_project_info, "resolve_project_info")
    }

    async fn setup_fields(&self, _project_id: &str) -> Result<SetupReport, Error> {
        self.calls.setup_fields.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.setup_fields, "setup_fields")
    }

    async fn query_setup_status(&self, _project_id: &str) -> Result<SetupStatus, Error> {
        self.calls.query_setup_status.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.query_setup_status, "query_setup_status")
    }

    async fn query_issue_types_status(&self, _org: &str) -> Result<Vec<String>, Error> {
        self.calls
            .query_issue_types_status
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(
            &self.stubs.query_issue_types_status,
            "query_issue_types_status",
        )
    }

    async fn ensure_issue_types(&self, _org: &str) -> Result<Vec<String>, Error> {
        self.calls.ensure_issue_types.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.ensure_issue_types, "ensure_issue_types")
    }

    async fn update_field(
        &self,
        _project_id: &str,
        _item_id: &str,
        _field_id: &str,
        _value: &FieldValue,
    ) -> Result<(), Error> {
        self.calls.update_field.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.update_field, "update_field")
    }

    async fn detect_owner_type(&self) -> Result<OwnerType, Error> {
        self.calls.detect_owner_type.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.detect_owner_type, "detect_owner_type")
    }

    async fn list_rest_fields(&self, _owner_type: OwnerType) -> Result<Vec<RestField>, Error> {
        self.calls.list_rest_fields.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.list_rest_fields, "list_rest_fields")
    }

    async fn create_view(
        &self,
        _owner_type: OwnerType,
        _params: &CreateViewParams,
    ) -> Result<ProjectView, Error> {
        self.calls.create_view.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.create_view, "create_view")
    }

    async fn list_views(&self, _owner_type: OwnerType) -> Result<Vec<ProjectView>, Error> {
        self.calls.list_views.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.list_views, "list_views")
    }

    async fn resolve_owner_node_id(&self, _owner_type: OwnerType) -> Result<String, Error> {
        self.calls
            .resolve_owner_node_id
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.resolve_owner_node_id, "resolve_owner_node_id")
    }

    async fn list_owner_projects(
        &self,
        _owner_type: OwnerType,
    ) -> Result<Vec<OwnerProject>, Error> {
        self.calls
            .list_owner_projects
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.list_owner_projects, "list_owner_projects")
    }

    async fn create_project(
        &self,
        _owner_node_id: &str,
        _title: &str,
    ) -> Result<CreatedProject, Error> {
        self.calls.create_project.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.create_project, "create_project")
    }

    async fn fetch_issue(&self, _number: u64) -> Result<Issue, Error> {
        self.calls.fetch_issue.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.fetch_issue, "fetch_issue")
    }

    async fn fetch_issue_ref(&self, _issue_ref: &IssueRef) -> Result<Issue, Error> {
        self.calls.fetch_issue_ref.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.fetch_issue_ref, "fetch_issue_ref")
    }

    async fn fetch_graph_data(&self) -> Result<(Vec<Issue>, Vec<BlockingEdge>), Error> {
        self.calls.fetch_graph_data.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.fetch_graph_data, "fetch_graph_data")
    }

    async fn create_issue(&self, _params: CreateIssueParams) -> Result<Issue, Error> {
        self.calls.create_issue.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.create_issue, "create_issue")
    }

    async fn close_issue(&self, _number: u64, _reason: Option<String>) -> Result<(), Error> {
        self.calls.close_issue.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.close_issue, "close_issue")
    }

    async fn reopen_issue(&self, _number: u64) -> Result<(), Error> {
        self.calls.reopen_issue.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.reopen_issue, "reopen_issue")
    }

    async fn search_issues(
        &self,
        _query: &str,
        _limit: Option<u32>,
    ) -> Result<Vec<IssueSummary>, Error> {
        self.calls.search_issues.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.search_issues, "search_issues")
    }

    async fn add_comment(&self, _number: u64, _body: String) -> Result<String, Error> {
        self.calls.add_comment.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_comment, "add_comment")
    }

    async fn add_comment_in_repo(
        &self,
        _owner: &str,
        _repo: &str,
        _number: u64,
        _body: String,
    ) -> Result<String, Error> {
        self.calls
            .add_comment_in_repo
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_comment_in_repo, "add_comment_in_repo")
    }

    async fn add_comment_ref(&self, issue_ref: &IssueRef, _body: String) -> Result<String, Error> {
        self.calls.add_comment_ref.fetch_add(1, Ordering::SeqCst);
        // Record the qualified ref for argument-aware assertions (see
        // `MockGitHubClient::add_comment_ref_calls`). The clone keeps the
        // log independent of the caller's borrow scope.
        self.stubs
            .add_comment_ref_calls
            .lock()
            .expect("mock call-log mutex poisoned")
            .push(issue_ref.clone());
        pop_or_unstubbed(&self.stubs.add_comment_ref, "add_comment_ref")
    }

    async fn update_issue_body(&self, _number: u64, _body: String) -> Result<(), Error> {
        self.calls.update_issue_body.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.update_issue_body, "update_issue_body")
    }

    async fn update_issue_type(
        &self,
        _number: u64,
        _issue_type: unblock_core::types::IssueType,
    ) -> Result<(), Error> {
        self.calls.update_issue_type.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.update_issue_type, "update_issue_type")
    }

    async fn add_labels_to_issue(&self, _number: u64, _labels: Vec<String>) -> Result<(), Error> {
        self.calls
            .add_labels_to_issue
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_labels_to_issue, "add_labels_to_issue")
    }

    async fn remove_label_from_issue(&self, _number: u64, _label: &str) -> Result<(), Error> {
        self.calls
            .remove_label_from_issue
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(
            &self.stubs.remove_label_from_issue,
            "remove_label_from_issue",
        )
    }

    async fn add_assignees_to_issue(
        &self,
        _number: u64,
        _assignees: Vec<String>,
    ) -> Result<(), Error> {
        self.calls
            .add_assignees_to_issue
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_assignees_to_issue, "add_assignees_to_issue")
    }

    async fn remove_assignees_from_issue(
        &self,
        _number: u64,
        _assignees: Vec<String>,
    ) -> Result<(), Error> {
        self.calls
            .remove_assignees_from_issue
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(
            &self.stubs.remove_assignees_from_issue,
            "remove_assignees_from_issue",
        )
    }

    async fn list_milestones(&self) -> Result<Vec<Milestone>, Error> {
        self.calls.list_milestones.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.list_milestones, "list_milestones")
    }

    async fn update_issue_milestone(
        &self,
        _number: u64,
        _milestone_number: Option<u64>,
    ) -> Result<(), Error> {
        self.calls
            .update_issue_milestone
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.update_issue_milestone, "update_issue_milestone")
    }

    async fn add_blocked_by(
        &self,
        _issue_number: u64,
        _blocked_by_number: u64,
    ) -> Result<(), Error> {
        self.calls.add_blocked_by.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_blocked_by, "add_blocked_by")
    }

    async fn remove_blocked_by(
        &self,
        _issue_number: u64,
        _blocked_by_number: u64,
    ) -> Result<(), Error> {
        self.calls.remove_blocked_by.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.remove_blocked_by, "remove_blocked_by")
    }

    async fn add_sub_issue(&self, _parent_number: u64, _child_number: u64) -> Result<(), Error> {
        self.calls.add_sub_issue.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_sub_issue, "add_sub_issue")
    }

    async fn resolve_issue_ref(&self, _issue_ref: &IssueRef) -> Result<String, Error> {
        self.calls.resolve_issue_ref.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.resolve_issue_ref, "resolve_issue_ref")
    }

    async fn get_project_item_id(
        &self,
        _issue_node_id: &str,
        _project_id: &str,
    ) -> Result<String, Error> {
        self.calls
            .get_project_item_id
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.get_project_item_id, "get_project_item_id")
    }

    async fn ensure_labels(&self, _labels: &[String]) -> Result<(), Error> {
        self.calls.ensure_labels.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.ensure_labels, "ensure_labels")
    }

    async fn add_blocked_by_ref(
        &self,
        _issue_number: u64,
        _blocker: &IssueRef,
    ) -> Result<(), Error> {
        self.calls.add_blocked_by_ref.fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_blocked_by_ref, "add_blocked_by_ref")
    }

    async fn add_blocked_by_refs(
        &self,
        _source: &IssueRef,
        _blocker: &IssueRef,
    ) -> Result<(), Error> {
        self.calls
            .add_blocked_by_refs
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.add_blocked_by_refs, "add_blocked_by_refs")
    }

    async fn remove_blocked_by_ref(
        &self,
        _issue_number: u64,
        _blocker: &IssueRef,
    ) -> Result<(), Error> {
        self.calls
            .remove_blocked_by_ref
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.remove_blocked_by_ref, "remove_blocked_by_ref")
    }

    async fn remove_blocked_by_refs(
        &self,
        _source: &IssueRef,
        _blocker: &IssueRef,
    ) -> Result<(), Error> {
        self.calls
            .remove_blocked_by_refs
            .fetch_add(1, Ordering::SeqCst);
        pop_or_unstubbed(&self.stubs.remove_blocked_by_refs, "remove_blocked_by_refs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::GitHubApiSnafu;

    fn mock() -> MockGitHubClient {
        MockGitHubClient::new("acme", "widgets", Some(7))
    }

    #[test]
    fn sync_accessors_return_constructor_values() {
        let m = mock();
        assert_eq!(m.owner(), "acme");
        assert_eq!(m.repo(), "widgets");
        assert_eq!(m.project_number(), Some(7));
        assert_eq!(m.api_base_url(), "https://api.github.com");
        assert_eq!(
            m.rest_url("/issues"),
            "https://api.github.com/repos/acme/widgets/issues"
        );
        assert_eq!(m.graphql_url(), "https://api.github.com/graphql");
    }

    #[test]
    fn with_api_base_url_overrides_default() {
        let m = mock().with_api_base_url("https://ghe.example.com/api/v3");
        assert_eq!(m.api_base_url(), "https://ghe.example.com/api/v3");
    }

    #[tokio::test]
    async fn field_ids_counter_increments_and_queue_pops() {
        let m = mock();
        assert_eq!(m.calls().field_ids(), 0);

        // Empty queue → None fallback.
        assert!(m.field_ids().await.is_none());
        assert_eq!(m.calls().field_ids(), 1);

        // Queue a Some(...) and verify it pops.
        m.push_field_ids(None);
        assert!(m.field_ids().await.is_none());
        assert_eq!(m.calls().field_ids(), 2);
    }

    #[tokio::test]
    async fn resolve_project_info_returns_queued_err_then_falls_back_to_not_stubbed() {
        let m = mock();

        let err = GitHubApiSnafu {
            status: 500_u16,
            message: "boom".to_owned(),
        }
        .build();
        m.push_resolve_project_info(Err(err));

        let first = m.resolve_project_info().await;
        assert!(matches!(first, Err(Error::GitHubApi { status: 500, .. })));
        assert_eq!(m.calls().resolve_project_info(), 1);

        let second = m.resolve_project_info().await;
        assert!(matches!(
            second,
            Err(Error::MockNotStubbed {
                method: "resolve_project_info"
            })
        ));
        assert_eq!(m.calls().resolve_project_info(), 2);
    }

    #[tokio::test]
    async fn resolve_project_info_returns_queued_ok() {
        let m = mock();
        m.push_resolve_project_info(Ok(ProjectInfo {
            id: "PVT_1".to_owned(),
            number: 7,
        }));
        let info = m.resolve_project_info().await.expect("queued ok");
        assert_eq!(info.id, "PVT_1");
        assert_eq!(info.number, 7);
    }

    #[tokio::test]
    async fn unstubbed_methods_return_mock_not_stubbed() {
        let m = mock();
        let res = m.detect_owner_type().await;
        assert!(matches!(
            res,
            Err(Error::MockNotStubbed {
                method: "detect_owner_type"
            })
        ));
        assert_eq!(m.calls().detect_owner_type(), 1);
    }

    #[tokio::test]
    async fn set_field_ids_increments_counter_only() {
        let m = mock();
        // Construct a minimal ProjectFieldIds via test helper — easier to
        // build via the FieldMeta literals than mocking GraphQL.
        let ids = ProjectFieldIds {
            status: crate::projects::FieldMeta {
                field_id: "f1".to_owned(),
                options: std::collections::HashMap::new(),
                option_colors: std::collections::HashMap::new(),
            },
            priority: crate::projects::FieldMeta {
                field_id: "f2".to_owned(),
                options: std::collections::HashMap::new(),
                option_colors: std::collections::HashMap::new(),
            },
            pipeline_stage: crate::projects::FieldMeta {
                field_id: "f3".to_owned(),
                options: std::collections::HashMap::new(),
                option_colors: std::collections::HashMap::new(),
            },
            agent: "f4".to_owned(),
            claimed_at: "f5".to_owned(),
            story_points: "f6".to_owned(),
            defer_until: "f7".to_owned(),
        };
        m.set_field_ids(ids).await;
        assert_eq!(m.calls().set_field_ids(), 1);
    }

    #[test]
    fn reset_zeros_all_counters() {
        let m = mock();
        m.calls.resolve_project_info.fetch_add(3, Ordering::SeqCst);
        m.calls.field_ids.fetch_add(2, Ordering::SeqCst);
        m.calls().reset();
        assert_eq!(m.calls().resolve_project_info(), 0);
        assert_eq!(m.calls().field_ids(), 0);
    }

    #[test]
    fn mock_not_stubbed_status_code_is_500() {
        let err = Error::MockNotStubbed { method: "x" };
        assert_eq!(err.status_code(), 500);
    }

    #[tokio::test]
    async fn reopen_issue_counter_and_queue_pop() {
        let m = mock();
        assert_eq!(m.calls().reopen_issue(), 0);

        // Empty queue → MockNotStubbed fallback.
        let first = m.reopen_issue(123).await;
        assert!(matches!(
            first,
            Err(Error::MockNotStubbed {
                method: "reopen_issue"
            })
        ));
        assert_eq!(m.calls().reopen_issue(), 1);

        // Queued Ok(()) must pop.
        m.push_reopen_issue(Ok(()));
        m.reopen_issue(123).await.expect("queued Ok should succeed");
        assert_eq!(m.calls().reopen_issue(), 2);
    }

    #[tokio::test]
    async fn search_issues_counter_and_queue_pop() {
        let m = mock();
        assert_eq!(m.calls().search_issues(), 0);

        // Empty queue → MockNotStubbed fallback.
        let first = m.search_issues("q", Some(10)).await;
        assert!(matches!(
            first,
            Err(Error::MockNotStubbed {
                method: "search_issues"
            })
        ));
        assert_eq!(m.calls().search_issues(), 1);

        // Queued Ok(vec) must pop.
        m.push_search_issues(Ok(Vec::new()));
        let second = m
            .search_issues("q", None)
            .await
            .expect("queued Ok should succeed");
        assert!(second.is_empty());
        assert_eq!(m.calls().search_issues(), 2);
    }

    // ── add_comment_ref / add_comment_in_repo (unblock-eos.13) ────────────

    #[tokio::test]
    async fn add_comment_in_repo_counter_and_queue_pop() {
        let m = mock();
        assert_eq!(m.calls().add_comment_in_repo(), 0);

        // Empty queue → MockNotStubbed fallback named by the method.
        let first = m
            .add_comment_in_repo("acme", "widgets", 42, "body".to_owned())
            .await;
        assert!(matches!(
            first,
            Err(Error::MockNotStubbed {
                method: "add_comment_in_repo"
            })
        ));
        assert_eq!(m.calls().add_comment_in_repo(), 1);

        // Queued Ok pops — returns the queued html_url.
        m.push_add_comment_in_repo(Ok("https://x.invalid/c".to_owned()));
        let second = m
            .add_comment_in_repo("other", "repo", 99, "body".to_owned())
            .await
            .expect("queued Ok should succeed");
        assert_eq!(second, "https://x.invalid/c");
        assert_eq!(m.calls().add_comment_in_repo(), 2);
    }

    #[tokio::test]
    async fn add_comment_ref_records_arguments_in_order() {
        let m = mock();
        assert!(
            m.add_comment_ref_calls().is_empty(),
            "no calls yet → empty log"
        );

        // Exercise the two variants in a deterministic sequence so tests
        // can assert the call LOG preserves insertion order.
        m.push_add_comment_ref(Ok("h1".to_owned()));
        m.push_add_comment_ref(Ok("h2".to_owned()));

        let _ = m
            .add_comment_ref(&IssueRef::Local(5), "body-local".to_owned())
            .await
            .expect("stubbed Ok");
        let _ = m
            .add_comment_ref(
                &IssueRef::CrossRepo {
                    owner: "other".to_owned(),
                    repo: "repo".to_owned(),
                    number: 99,
                },
                "body-cross".to_owned(),
            )
            .await
            .expect("stubbed Ok");

        // Counter reflects both dispatches.
        assert_eq!(m.calls().add_comment_ref(), 2);
        // Argument-aware log preserves order AND ref identity.
        let log = m.add_comment_ref_calls();
        assert_eq!(
            log,
            vec![
                IssueRef::Local(5),
                IssueRef::CrossRepo {
                    owner: "other".to_owned(),
                    repo: "repo".to_owned(),
                    number: 99,
                },
            ],
            "add_comment_ref_calls() MUST record every IssueRef in call order"
        );
    }

    #[tokio::test]
    async fn add_comment_ref_empty_queue_falls_back_to_mock_not_stubbed() {
        let m = mock();
        let result = m
            .add_comment_ref(&IssueRef::Local(1), "body".to_owned())
            .await;
        assert!(matches!(
            result,
            Err(Error::MockNotStubbed {
                method: "add_comment_ref"
            })
        ));
        // Even on the MockNotStubbed fallback the argument IS recorded —
        // this guarantees argument-aware assertions work regardless of
        // whether the test chose to stub the return value.
        assert_eq!(m.add_comment_ref_calls(), vec![IssueRef::Local(1)]);
    }
}
