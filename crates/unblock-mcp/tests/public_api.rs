//! Public-API guard: the documented cross-crate contract resolves and the pure builders are usable
//! without a `Session` (FR-12). Each `use`/reference only compiles if the item is public.

use unblock_mcp::{
    CONTRACT_HASH, CONTRACT_VERSION, Capabilities, ErrorCodeDescriptor, McpServerError,
    PromptDescriptor, Quotas, ResourceDescriptor, SchemaBundle, ServeOptions, ToolDescriptor,
    ToolSchemas, capabilities, schema_bundle, serve,
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
    let _opts = ServeOptions::default();
    let _quotas = Quotas::default();
}

#[test]
fn serve_is_a_named_public_fn() {
    // A compile-only witness that `serve` is the public async entry point with the expected
    // signature; we do not call it (it would bind real stdio). Passing it to a bound that names its
    // exact parameter/return types proves the arity without invoking it.
    fn takes_serve_shape<F, Fut>(_f: F)
    where
        F: Fn(std::sync::Arc<unblock_engine::Session>, ServeOptions) -> Fut,
        Fut: std::future::Future<Output = Result<(), McpServerError>>,
    {
    }
    takes_serve_shape(serve);
}

#[test]
fn mcp_server_error_is_a_public_error_type() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<McpServerError>();
}
