//! Public-API guard: the documented cross-crate contract resolves and the pure builders are usable
//! without a `Session` (FR-12). Each `use`/reference only compiles if the item is public.

use unblock_mcp::{
    AgentsDigest, CONTRACT_HASH, CONTRACT_VERSION, Capabilities, ErrorCodeDescriptor,
    ErrorCodeDigest, McpServerError, McpServerOptions, PromptDescriptor, PromptDigest, Quotas,
    ResourceDescriptor, ResourceDigest, SchemaBundle, ToolAction, ToolDescriptor, ToolDigest,
    ToolSchemas, agents_digest, capabilities, run_mcp_server, schema_bundle,
};

#[test]
fn public_types_and_consts_resolve() {
    // Consts + builders are usable offline.
    assert_eq!(CONTRACT_VERSION, "unblock.mcp.v1.3");
    assert_eq!(
        CONTRACT_HASH.len(),
        64,
        "the hash-coupled drift pin is exported"
    );
    let caps: Capabilities = capabilities();
    let bundle: SchemaBundle = schema_bundle();
    assert_eq!(caps.contract_version, CONTRACT_VERSION);
    assert_eq!(bundle.contract_version, CONTRACT_VERSION);

    // The per-tool `{input, output}` bundle + the shared error schema are objects (T2.6/D25).
    // Naming `ToolSchemas` in a type ascription is a compile-witness that the type is public.
    let issue_schemas: &ToolSchemas = &bundle.issue;
    assert!(issue_schemas.input.is_object());
    assert!(issue_schemas.output.is_object());
    assert!(bundle.error.is_object());

    // Descriptor types are nameable from the public surface.
    let _tool: Option<ToolDescriptor> = caps.tools.into_iter().next();
    let _resource: Option<ResourceDescriptor> = caps.resources.into_iter().next();
    let _prompt: Option<PromptDescriptor> = caps.prompts.into_iter().next();
    // `hint_shape` is readable (proves `HintShape` is public via unblock-error, spine §1.10).
    let first_err: Option<ErrorCodeDescriptor> = caps.error_codes.into_iter().next();
    let _shape = first_err.map(|d| d.hint_shape);

    // Options are constructible.
    let _opts = McpServerOptions::default();
    let _quotas = Quotas::default();

    // `agents_digest()` (D33) is a public, pure, typed digest — NOT part of the hashed contract
    // tuple (it derives neither `Serialize` nor `JsonSchema`).
    let digest: AgentsDigest = agents_digest();
    assert_eq!(digest.contract_version, CONTRACT_VERSION);
    let first_tool: Option<ToolDigest> = digest.tools.into_iter().next();
    let _action: Option<ToolAction> = first_tool.and_then(|t| t.actions.into_iter().next());
    let _resource: Option<ResourceDigest> = digest.resources.into_iter().next();
    let _prompt: Option<PromptDigest> = digest.prompts.into_iter().next();
    let _error_code: Option<ErrorCodeDigest> = digest.error_codes.into_iter().next();
}

#[test]
fn run_mcp_server_is_a_named_public_fn() {
    // A compile-only witness that `run_mcp_server` is the public async entry point with the expected
    // signature; we do not call it (it would bind real stdio). Passing it to a bound that names its
    // exact parameter/return types proves the arity without invoking it.
    fn takes_run_mcp_server_shape<F, Fut>(_f: F)
    where
        F: Fn(std::sync::Arc<unblock_engine::Session>, McpServerOptions) -> Fut,
        Fut: std::future::Future<Output = Result<(), McpServerError>>,
    {
    }
    takes_run_mcp_server_shape(run_mcp_server);
}

#[test]
fn mcp_server_error_is_a_public_error_type() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<McpServerError>();
}
