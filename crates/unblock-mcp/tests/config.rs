//! FR-13 full over the live MCP surface (PRD §4 D57, `ub-lp9.2`): the `issue` tool's `config`
//! action driven end to end through the real server over a duplex transport, the per-op field rule
//! enforced at the adapter with ZERO writes as `RecordingStorage` observes them, the clause (2a)
//! fail-closed gate as tools and resources surface it (while `capabilities`, `schema` and the
//! prompts stay served), and the D57 criterion (o) contract checks.

mod common;

use std::sync::Arc;

use common::recording::RecordingStorage;
use common::{call_tool, connect, session_recording, tester_ctx};
use rmcp::model::{GetPromptRequestParams, ReadResourceRequestParams};
use rmcp::service::ServiceError;
use serde_json::{Value, json};
use unblock_engine::{Session, SessionConfig};
use unblock_storage::{LibsqlStorage, Storage};

/// The `issue` description both copies publish (D57 clause (11)).
const ISSUE_DESCRIPTION: &str = "Create, show, update, close, reopen, delete, restore, relabel, or close_eligible issues, or get, set, unset and list runtime configuration with config. Where an action accepts the optional agent_name, harness and model fields, unblock currently discards them.";

/// A session over a `RecordingStorage` spy whose `config` table holds `rows`, opened with the
/// snapshot the storage-bearing facade would resolve from them (the D57 clause (2a) faults included).
async fn session_with_rows(rows: &[(&str, &str)]) -> (Arc<Session>, Arc<RecordingStorage>) {
    let inner = LibsqlStorage::open_in_memory()
        .await
        .expect("open in-memory");
    inner.migrate().await.expect("migrate");
    for (key, value) in rows {
        inner.set_config(key, value).await.expect("seed row");
    }
    let owned: Vec<(String, String)> = rows
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let inner: Arc<dyn Storage> = Arc::new(inner);
    let spy = RecordingStorage::new(inner);
    let spy_dyn: Arc<dyn Storage> = spy.clone();
    let mut ctx = tester_ctx(spy_dyn);
    ctx.config.runtime.db_faults = unblock_config::decode_db_rows(&owned);
    let session = Session::open(ctx, SessionConfig::default())
        .await
        .expect("open session");
    (Arc::new(session), spy)
}

/// D57 (j) over the wire: `list` on an empty table is the three layer keys in key order, each
/// `value: null`, `fault: null`, with its resolved source; `set` returns the entry after the write
/// and the reload; `get` reads it back; `unset` restores the next layer — every op CD-2 wrapped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_action_round_trips_every_op() {
    let (session, _spy) = session_recording().await;
    let (client, server, _cancel) = connect(session).await;

    let (is_error, payload) =
        call_tool(&client, "issue", json!({"action":"config","op":"list"})).await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"config":[
            {"key":"actor","value":null,"effective":"tester","source":"os_user","fault":null},
            {"key":"output_format","value":null,"effective":"json","source":"default","fault":null},
            {"key":"search_cap","value":null,"effective":"50","source":"default","fault":null}
        ]})
    );

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"set","key":"search_cap","value":"7"}),
    )
    .await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"config":[{"key":"search_cap","value":"7","effective":"7","source":"db","fault":null}]})
    );

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"get","key":"search_cap"}),
    )
    .await;
    assert!(!is_error, "{payload}");
    assert_eq!(payload["config"][0]["value"], json!("7"));

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"unset","key":"search_cap"}),
    )
    .await;
    assert!(!is_error, "{payload}");
    assert_eq!(
        payload,
        json!({"config":[{"key":"search_cap","value":null,"effective":"50","source":"default","fault":null}]})
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// D57 (o): every per-op field violation, and an unknown argument, is rejected in-band as
/// `VALIDATION_FAILED` naming the field — with ZERO writes as `RecordingStorage` observes them. The
/// key/value VALUES are the engine's business (`CONFIG_ERROR`, also zero writes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_field_rule_rejects_in_band_with_zero_writes() {
    let (session, spy) = session_recording().await;
    spy.set_config("search_cap", "9")
        .await
        .expect("a row the rejections must leave byte-identical");
    let table_before = spy.read_config().await.expect("read");
    let snapshot_before = format!("{:?}", session.runtime());
    let mutations_before = spy.mutation_count();
    let (client, server, _cancel) = connect(Arc::clone(&session)).await;

    for (args, field) in [
        (json!({"action":"config","op":"get"}), "key"),
        (json!({"action":"config","op":"unset"}), "key"),
        (json!({"action":"config","op":"set","value":"7"}), "key"),
        (json!({"action":"config","op":"list","key":"actor"}), "key"),
        (
            json!({"action":"config","op":"set","key":"search_cap"}),
            "value",
        ),
        (
            json!({"action":"config","op":"get","key":"actor","value":"x"}),
            "value",
        ),
        (
            json!({"action":"config","op":"unset","key":"actor","value":"x"}),
            "value",
        ),
        (json!({"action":"config","op":"list","value":"x"}), "value"),
    ] {
        let (is_error, payload) = call_tool(&client, "issue", args.clone()).await;
        assert!(is_error, "{args} must be rejected: {payload}");
        assert_eq!(payload["code"], json!("VALIDATION_FAILED"), "{args}");
        assert_eq!(
            payload["context"]["fields"][0]["field"],
            json!(field),
            "{args}"
        );
        assert!(
            payload["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("depend on `op`"),
            "{args}: the hint states the per-op rule: {payload}"
        );
    }

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"set","key":"search_cap","value":"7","bogus":1}),
    )
    .await;
    assert!(is_error, "an unknown argument is rejected: {payload}");
    assert_eq!(payload["code"], json!("VALIDATION_FAILED"));

    for args in [
        json!({"action":"config","op":"set","key":"search_cap","value":"0"}),
        json!({"action":"config","op":"set","key":"db_filename","value":"x.db"}),
        json!({"action":"config","op":"set","key":"jsonl_export","value":"true"}),
        json!({"action":"config","op":"set","key":"nope","value":"1"}),
    ] {
        let (is_error, payload) = call_tool(&client, "issue", args.clone()).await;
        assert!(is_error, "{args}: {payload}");
        assert_eq!(payload["code"], json!("CONFIG_ERROR"), "{args}");
    }
    let (_, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"set","key":"db_filename","value":"x.db"}),
    )
    .await;
    assert_eq!(payload["context"]["layer"], json!("db"));

    assert_eq!(
        spy.mutation_count(),
        mutations_before,
        "no rejected call wrote anything"
    );
    assert_eq!(
        spy.read_config().await.expect("read"),
        table_before,
        "D57 (i): the config table is byte-identical after every rejection"
    );
    assert_eq!(
        format!("{:?}", session.runtime()),
        snapshot_before,
        "D57 (i): the live snapshot is unchanged after every rejection"
    );
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// The discovery surface that never touches the `Session`: `unblock://capabilities`,
/// `unblock://schema` and ALL THREE prompts, each rendered as JSON for comparison.
async fn discovery_surface(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
) -> Vec<(String, Value)> {
    let mut served = Vec::new();
    for uri in ["unblock://capabilities", "unblock://schema"] {
        let read = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .unwrap_or_else(|e| panic!("{uri} is served: {e:?}"));
        served.push((uri.to_string(), serde_json::to_value(read).expect("json")));
    }
    for prompt in ["triage", "plan_next_work", "close_with_suggestions"] {
        let got = client
            .get_prompt(GetPromptRequestParams::new(prompt))
            .await
            .unwrap_or_else(|e| panic!("prompt {prompt} is served: {e:?}"));
        served.push((prompt.to_string(), serde_json::to_value(got).expect("json")));
    }
    served
}

/// D57 (d)/(d2): the three Session-backed resources answer `-32603` with `data.code:
/// CONFIG_ERROR` and the hint; `capabilities`, `schema` and every prompt stay served UNCHANGED —
/// byte-identical to what an UNFAULTED session serves.
async fn assert_resources_gated_and_discovery_served(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
    hint_call: &str,
) {
    for uri in [
        "unblock://issues/ub-x",
        "unblock://issues/ready",
        "unblock://issues/blocked",
    ] {
        let err = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .expect_err("a gated resource read fails");
        let ServiceError::McpError(data) = err else {
            panic!("{uri}: expected an rmcp McpError, got {err:?}");
        };
        assert_eq!(data.code.0, -32603, "{uri}");
        let payload = data.data.expect("structured payload attached as data");
        assert_eq!(payload["code"], json!("CONFIG_ERROR"), "{uri}");
        assert!(
            payload["hint"]
                .as_str()
                .unwrap_or_default()
                .contains(hint_call),
            "{uri}: {payload}"
        );
    }

    let gated = discovery_surface(client).await;
    let (clean_session, _spy) = session_recording().await;
    let (clean, clean_server, _cancel) = connect(clean_session).await;
    let baseline = discovery_surface(&clean).await;
    for ((name, under_gate), (_, unfaulted)) in gated.iter().zip(&baseline) {
        assert_eq!(
            under_gate, unfaulted,
            "{name} is served UNCHANGED under the gate"
        );
    }
    let _ = clean.cancel().await;
    let _ = clean_server.cancel().await;
}

/// D57 (d)/(d2) over MCP: with a blocking startup-key row, every non-`config` tool call answers
/// `CONFIG_ERROR` in-band with `context.keys` and the `unset` hint, the three Session-backed
/// resources answer `-32603` with `data.code: CONFIG_ERROR` and the hint, while `capabilities`,
/// `schema` and the prompts stay SERVED and all four `config` ops run; `config unset` of the row
/// re-opens the SAME session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_fault_gate_over_tools_resources_and_prompts() {
    let (session, spy) = session_with_rows(&[("db_filename", "elsewhere.db")]).await;
    let (client, server, _cancel) = connect(session).await;
    let hint_call = r#"issue {"action":"config","op":"unset","key":"db_filename"}"#;

    for (tool, args) in [
        ("issue", json!({"action":"create","title":"gated"})),
        ("issue", json!({"action":"show","id":"ub-x"})),
        ("query", json!({"kind":"ready"})),
        ("query", json!({"kind":"labels"})),
        ("claim", json!({"id":"ub-x","assignee":"me"})),
        (
            "defer",
            json!({"action":"defer","id":"ub-x","until":"2099-01-01T00:00:00Z"}),
        ),
        ("defer", json!({"action":"undefer","id":"ub-x"})),
        ("dep", json!({"action":"list","id":"ub-x"})),
        ("comment", json!({"action":"list","issue_id":"ub-x"})),
        ("diagnostics", json!({"kind":"stats"})),
        ("sync", json!({"action":"export"})),
        ("sync", json!({"action":"import","path":"issues.jsonl"})),
    ] {
        let (is_error, payload) = call_tool(&client, tool, args.clone()).await;
        assert!(is_error, "{tool} {args}: {payload}");
        assert_eq!(payload["code"], json!("CONFIG_ERROR"), "{tool} {args}");
        assert_eq!(
            payload["context"]["keys"],
            json!(["db_filename"]),
            "{tool} {args}"
        );
        assert!(
            payload["hint"]
                .as_str()
                .unwrap_or_default()
                .contains(hint_call),
            "{tool} {args}: {payload}"
        );
    }
    assert_eq!(spy.mutation_count(), 0, "the gate wrote nothing");

    assert_resources_gated_and_discovery_served(&client, hint_call).await;

    let (is_error, payload) =
        call_tool(&client, "issue", json!({"action":"config","op":"list"})).await;
    assert!(!is_error, "{payload}");
    let row = payload["config"]
        .as_array()
        .expect("list")
        .iter()
        .find(|e| e["key"] == json!("db_filename"))
        .cloned()
        .expect("the faulty row is listed");
    assert_eq!(row["fault"]["kind"], json!("startup_key"));
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"get","key":"db_filename"}),
    )
    .await;
    assert!(!is_error, "get runs under the gate: {payload}");
    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"set","key":"output_format","value":"plain"}),
    )
    .await;
    assert!(!is_error, "set runs under the gate: {payload}");

    let (is_error, payload) = call_tool(
        &client,
        "issue",
        json!({"action":"config","op":"unset","key":"db_filename"}),
    )
    .await;
    assert!(!is_error, "the hinted repair runs: {payload}");
    let (is_error, payload) = call_tool(&client, "query", json!({"kind":"ready"})).await;
    assert!(
        !is_error,
        "the very next call on the SAME session succeeds: {payload}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

/// D57 (o): the `config` arms sit LAST in the `issue` input and output unions, the tool count stays
/// 8, both description copies carry the new sentence (and the live `tools/list` serves it),
/// `CONFIG_ERROR`'s hint shape reads `contextual_text`, and the live id is `unblock.mcp.v1.12`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_contract_arms_last_and_v1_12() {
    let bundle = unblock_mcp::schema_bundle();
    let input_arms = bundle.issue.input["oneOf"].as_array().expect("issue oneOf");
    let last_input = input_arms.last().expect("an arm");
    let action = &last_input["properties"]["action"];
    assert_eq!(
        action.get("const").or_else(|| action["enum"].get(0)),
        Some(&json!("config")),
        "the config input arm is APPENDED LAST: {last_input}"
    );
    let output = &bundle.issue.output;
    let output_arms = output
        .get("anyOf")
        .or_else(|| output.get("oneOf"))
        .and_then(Value::as_array)
        .expect("issue output union");
    assert!(
        output_arms
            .last()
            .expect("an arm")
            .to_string()
            .contains("ConfigList"),
        "the Config(ConfigList) output arm is APPENDED LAST"
    );

    let capabilities = unblock_mcp::capabilities();
    assert_eq!(capabilities.tools.len(), 8, "no tool is added");
    let issue = capabilities
        .tools
        .iter()
        .find(|t| t.name == "issue")
        .expect("issue descriptor");
    assert_eq!(issue.description, ISSUE_DESCRIPTION);
    let config_error = capabilities
        .error_codes
        .iter()
        .find(|e| e.code == "CONFIG_ERROR")
        .expect("CONFIG_ERROR");
    assert_eq!(config_error.hint_shape.as_str(), "contextual_text");
    assert_eq!(unblock_mcp::CONTRACT_VERSION, "unblock.mcp.v1.12");

    let (session, _spy) = session_recording().await;
    let (client, server, _cancel) = connect(session).await;
    let tools = client.list_all_tools().await.expect("tools/list");
    let live = tools
        .iter()
        .find(|t| t.name == "issue")
        .expect("issue tool");
    assert_eq!(live.description.as_deref(), Some(ISSUE_DESCRIPTION));
    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
