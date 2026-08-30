# 2026-08-30 RPC Registry Injection Handoff

- Added an explicit `Arc<AppServerRpcRegistry>` dependency to `MessageProcessorArgs` and `MessageProcessor`.
- In-process startup now accepts trusted process-local `rpc_extensions` and builds one registry for that runtime; stdio startup supplies an empty registry until public process overrides are wired.
- No registry/config/cache global was introduced. Existing typed in-process requests remain native-only until a raw request API is added.
- Validation: direct rustfmt and `git diff --check`; compilation is still blocked by the full root filesystem during Cargo git dependency fetch.
- Next action: adapt upstream injection/dispatch changes, resolving against the current Catalyst runtime options and preserving both JSON and in-process paths.
