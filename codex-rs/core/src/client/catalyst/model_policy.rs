//! CATALYST: host-selected model policy contract, independent of request/session state.
//!
//! Runtime supplies these values. Core owns when they apply, baseline lifetime and recovery.
//! The existing `codex_core::client::ModelRuntimePolicy` public path is retained by re-export.

/// Host-supplied behavior for a model endpoint that is not part of the public model catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRuntimePolicy {
    pub supports_http_incremental_requests: bool,
    pub relocates_tool_output_images: bool,
    pub max_request_body_bytes: Option<u64>,
    pub supports_parallel_tool_calls: Option<bool>,
}
