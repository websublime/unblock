//! Public-API guard: the documented cross-crate contract resolves and the pure builders are usable
//! without a `Session` (FR-12). Each `use`/reference only compiles if the item is public.

use unblock_mcp::{
    CONTRACT_VERSION, Capabilities, ErrorCodeDescriptor, McpServerError, PromptDescriptor, Quotas,
    ResourceDescriptor, SchemaBundle, ServeOptions, ToolDescriptor, capabilities, schema_bundle,
    serve,
};

#[test]
fn public_types_and_consts_resolve() {
    // Consts + builders are usable offline.
    assert_eq!(CONTRACT_VERSION, "unblock.mcp.v1");
    let caps: Capabilities = capabilities();
    let bundle: SchemaBundle = schema_bundle();
    assert_eq!(caps.contract_version, CONTRACT_VERSION);
    assert_eq!(bundle.contract_version, CONTRACT_VERSION);

    // Descriptor types are nameable from the public surface.
    let _tool: Option<ToolDescriptor> = caps.tools.into_iter().next();
    let _resource: Option<ResourceDescriptor> = caps.resources.into_iter().next();
    let _prompt: Option<PromptDescriptor> = caps.prompts.into_iter().next();
    let _err: Option<ErrorCodeDescriptor> = caps.error_codes.into_iter().next();

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
