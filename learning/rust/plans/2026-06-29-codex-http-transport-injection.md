# Codex HTTP Transport Injection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Add a small, tested injection seam to Codex 0.142.4 so a Catalyst-owned Rust transport can receive native Codex Responses requests and return native Responses SSE bytes without changing the agent turn loop.

**Architecture:** Keep the existing RPITIT-based HttpTransport trait. Add a concrete, cloneable HttpTransportHandle that erases a custom transport behind two boxed async closures, then carry an optional handle through ThreadManagerRuntimeOptions → CodexSpawnArgs → Session → ModelClient. The default path continues constructing ReqwestTransport exactly where it does today.

**Tech Stack:** Rust 1.95.0, Tokio, futures BoxFuture/BoxStream, Codex codex-client/codex-api/codex-core crates, cargo-nextest through just, tracing, LLDB on macOS.

---

## Scope Boundary

This is Plan 1 of four deliberately separate deliverables:

1. **This plan:** the public Codex transport injection seam.
2. **Plan 2:** private catalyst-runtime-rs workspace, normalized protocol, and one complete DeepSeek request/tool-call vertical slice.
3. **Plan 3:** GLM, Kimi, and MiniMax policies using the shared provider contract suite.
4. **Plan 4:** private skill packaging, entitlement, and extraction-risk controls.

This plan does not translate any provider protocol and does not embed private skills. Its success condition is a native Codex turn completed by an injected in-memory transport.

## Repository Organization

Keep the public compatibility patch and private product logic in separate repositories:

```text
/Users/luoruipu/projects/other_projects/codex
  RupertLuo/codex fork
  small upstream-facing transport seam only
  fork-local learning/rust companion material

/Users/luoruipu/projects/other_projects/catalyst-runtime-rs
  private repository created in Plan 2
  provider normalization, credentials, policy, and private skill runtime

/Users/luoruipu/projects/other_projects/IB-Tools
  current Catalyst product shell
  migrates incrementally after the Rust runtime vertical slice works
```

The fork is necessary because this plan changes Codex internals. Keeping the actual provider adapters and private skills out of the fork makes future upstream rebases smaller and prevents a public fork from becoming the secret-bearing product repository.

## Source Map

The model path to keep intact is:

```text
ThreadManager
  -> CodexSpawnArgs
  -> Session::new
  -> ModelClient
  -> ModelClientSession::stream_responses_api
  -> codex_api::ResponsesClient<HttpTransportHandle>
  -> injected closure
  -> StreamResponse raw SSE bytes
  -> native Codex SSE parser
  -> ResponseEvent
  -> native agent turn loop
```

Files changed by this plan:

- Create: codex-rs/codex-client/src/transport_handle.rs
- Create: codex-rs/codex-client/src/transport_handle_tests.rs
- Modify: codex-rs/codex-client/src/lib.rs
- Modify: codex-rs/codex-api/src/lib.rs
- Modify: codex-rs/core/src/client.rs
- Modify: codex-rs/core/src/thread_manager.rs
- Modify: codex-rs/core/src/lib.rs
- Modify: codex-rs/core-api/src/lib.rs
- Modify: codex-rs/core/src/session/mod.rs
- Modify: codex-rs/core/src/session/session.rs
- Modify: codex-rs/core/src/codex_delegate.rs
- Modify: codex-rs/core/src/session/tests.rs
- Modify: codex-rs/core/src/session/tests/guardian_tests.rs
- Modify: codex-rs/core/tests/common/test_codex.rs
- Create: codex-rs/core/tests/suite/transport_injection.rs
- Modify: codex-rs/core/tests/suite/mod.rs

No Cargo dependency or lockfile change is required.

### Task 0: Learn the local development loop

**Files:** None.

- [ ] **Step 1: Confirm the branch and toolchain**

Run from the repository root:

```bash
cd /Users/luoruipu/projects/other_projects/codex
git status --short --branch
cd codex-rs
rustc --version
cargo --version
just --version
```

Expected:

```text
The branch is codex/catalyst-transport-v0.142.4 and is up to date with its origin branch.
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
just 1.54.0
```

Teaching checkpoint:

- The Git branch fixes the Codex source baseline.
- rust-toolchain.toml fixes the compiler baseline.
- Cargo.toml defines crates; Cargo.lock fixes dependency versions.

- [ ] **Step 2: Install the missing test runner**

Run:

```bash
cargo install --locked cargo-nextest
cargo nextest --version
```

Expected: a cargo-nextest version line and no "no such command" error.

- [ ] **Step 3: Run a read-only build check**

Run:

```bash
cargo check -p codex-client
```

Expected:

```text
The command exits with status 0 and ends with a Finished dev profile message.
```

The release tag may rewrite workspace package versions in Cargo.lock from 0.0.0 to 0.142.4. Do not commit that release-tag artifact. Before editing source, verify:

```bash
cd ..
git status --short
```

Expected: no tracked changes. If Cargo.lock changed, stop and restore only that generated difference before continuing.

- [ ] **Step 4: Run the source-built CLI**

Run:

```bash
cd codex-rs
just codex --version
just codex --help
```

Expected: Codex 0.142.4 version output followed by CLI help.

- [ ] **Step 5: Locate the transport call chain**

Run:

```bash
rg -n "trait HttpTransport|ReqwestTransport::new|stream_responses_api|pub async fn stream" \
  codex-client/src/transport.rs \
  core/src/client.rs
```

Expected hits:

- HttpTransport in codex-client/src/transport.rs.
- Four ReqwestTransport construction sites in core/src/client.rs.
- stream_responses_api and ModelClientSession::stream.

Teaching checkpoint: the signature fn stream(request: Request) -> impl Future<Output = Result<StreamResponse, TransportError>> + Send uses RPITIT. The hidden return type is chosen by each implementation, so HttpTransport is not directly dyn-compatible.

### Task 1: Add a type-erased HttpTransportHandle

**Files:**

- Create: codex-rs/codex-client/src/transport_handle.rs
- Create: codex-rs/codex-client/src/transport_handle_tests.rs
- Modify: codex-rs/codex-client/src/lib.rs

- [ ] **Step 1: Write the failing delegation test**

Create codex-rs/codex-client/src/transport_handle_tests.rs:

```rust
use std::sync::Arc;
use std::sync::Mutex;

use bytes::Bytes;
use futures::StreamExt;
use http::HeaderMap;
use http::Method;
use http::StatusCode;
use pretty_assertions::assert_eq;

use super::*;

#[tokio::test]
async fn delegates_execute_and_stream_requests() {
    let execute_urls = Arc::new(Mutex::new(Vec::new()));
    let stream_urls = Arc::new(Mutex::new(Vec::new()));

    let handle = HttpTransportHandle::new(
        {
            let execute_urls = Arc::clone(&execute_urls);
            move |request: Request| {
                let execute_urls = Arc::clone(&execute_urls);
                async move {
                    execute_urls
                        .lock()
                        .expect("execute URL lock")
                        .push(request.url);
                    Ok(Response {
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        body: Bytes::from_static(b"execute-ok"),
                    })
                }
            }
        },
        {
            let stream_urls = Arc::clone(&stream_urls);
            move |request: Request| {
                let stream_urls = Arc::clone(&stream_urls);
                async move {
                    stream_urls
                        .lock()
                        .expect("stream URL lock")
                        .push(request.url);
                    Ok(StreamResponse {
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        bytes: Box::pin(futures::stream::iter([Ok(
                            Bytes::from_static(b"stream-ok"),
                        )])),
                    })
                }
            }
        },
    );

    let response = handle
        .execute(Request::new(
            Method::GET,
            "https://example.com/execute".to_string(),
        ))
        .await
        .expect("execute request should succeed");
    let stream = handle
        .stream(Request::new(
            Method::POST,
            "https://example.com/stream".to_string(),
        ))
        .await
        .expect("stream request should succeed");
    let chunks = stream
        .bytes
        .collect::<Vec<Result<Bytes, TransportError>>>()
        .await;

    assert_eq!(response.body, Bytes::from_static(b"execute-ok"));
    assert_eq!(
        chunks
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("stream chunks"),
        vec![Bytes::from_static(b"stream-ok")]
    );
    assert_eq!(
        execute_urls.lock().expect("execute URL lock").as_slice(),
        ["https://example.com/execute"]
    );
    assert_eq!(
        stream_urls.lock().expect("stream URL lock").as_slice(),
        ["https://example.com/stream"]
    );
}
```

Create the initial codex-rs/codex-client/src/transport_handle.rs:

```rust
#[cfg(test)]
#[path = "transport_handle_tests.rs"]
mod tests;
```

Add to codex-rs/codex-client/src/lib.rs:

```rust
mod transport_handle;
```

and add this public export beside the transport exports:

```rust
pub use crate::transport_handle::HttpTransportHandle;
```

- [ ] **Step 2: Run the test and verify red**

Run:

```bash
just test -p codex-client transport_handle --no-capture
```

Expected: compile failure because transport_handle::HttpTransportHandle does not exist.

Teaching checkpoint: a compile failure is the Rust form of a red TDD test. Read the first compiler error first; later errors are often cascading consequences.

- [ ] **Step 3: Implement the minimal type-erased handle**

Replace codex-rs/codex-client/src/transport_handle.rs with:

```rust
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::HttpTransport;
use crate::Request;
use crate::Response;
use crate::StreamResponse;
use crate::TransportError;

type ExecuteFn = dyn Fn(
        Request,
    ) -> BoxFuture<'static, Result<Response, TransportError>>
    + Send
    + Sync;
type StreamFn = dyn Fn(
        Request,
    ) -> BoxFuture<'static, Result<StreamResponse, TransportError>>
    + Send
    + Sync;

/// Cloneable, type-erased HTTP transport used by runtime hosts.
///
/// The handle keeps Codex API clients generic over one concrete type while
/// allowing a host to provide transport behavior at process startup.
#[derive(Clone)]
pub struct HttpTransportHandle {
    execute: Arc<ExecuteFn>,
    stream: Arc<StreamFn>,
}

impl HttpTransportHandle {
    pub fn new<Execute, ExecuteFuture, Stream, StreamFuture>(
        execute: Execute,
        stream: Stream,
    ) -> Self
    where
        Execute: Fn(Request) -> ExecuteFuture + Send + Sync + 'static,
        ExecuteFuture:
            Future<Output = Result<Response, TransportError>> + Send + 'static,
        Stream: Fn(Request) -> StreamFuture + Send + Sync + 'static,
        StreamFuture:
            Future<Output = Result<StreamResponse, TransportError>> + Send + 'static,
    {
        Self {
            execute: Arc::new(move |request| Box::pin(execute(request))),
            stream: Arc::new(move |request| Box::pin(stream(request))),
        }
    }

    pub fn from_transport<T>(transport: T) -> Self
    where
        T: HttpTransport + 'static,
    {
        let transport = Arc::new(transport);
        let execute_transport = Arc::clone(&transport);

        Self::new(
            move |request| {
                let transport = Arc::clone(&execute_transport);
                async move { transport.execute(request).await }
            },
            move |request| {
                let transport = Arc::clone(&transport);
                async move { transport.stream(request).await }
            },
        )
    }
}

impl fmt::Debug for HttpTransportHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpTransportHandle")
            .finish_non_exhaustive()
    }
}

impl HttpTransport for HttpTransportHandle {
    async fn execute(&self, request: Request) -> Result<Response, TransportError> {
        (self.execute)(request).await
    }

    async fn stream(&self, request: Request) -> Result<StreamResponse, TransportError> {
        (self.stream)(request).await
    }
}

#[cfg(test)]
#[path = "transport_handle_tests.rs"]
mod tests;
```

- [ ] **Step 4: Run the test and verify green**

Run:

```bash
just test -p codex-client transport_handle --no-capture
```

Expected: one passing test.

Teaching checkpoint:

- Arc gives shared ownership of each closure.
- BoxFuture erases the concrete async block type.
- HttpTransportHandle itself is concrete, so generic ResponsesClient<T> still monomorphizes normally.
- This is type erasure at one explicit boundary, not dynamic typing throughout the program.

- [ ] **Step 5: Commit the handle**

Run:

```bash
cd ..
git add codex-rs/codex-client/src/lib.rs \
  codex-rs/codex-client/src/transport_handle.rs \
  codex-rs/codex-client/src/transport_handle_tests.rs
git commit -m "feat(client): add type-erased HTTP transport handle"
```

Expected: one commit containing only codex-client changes.

### Task 2: Let ModelClient select the injected handle

**Files:**

- Modify: codex-rs/codex-api/src/lib.rs
- Modify: codex-rs/core/src/client.rs
- Modify: codex-rs/core/src/client_tests.rs

- [ ] **Step 1: Re-export the handle through codex-api**

In codex-rs/codex-api/src/lib.rs, add beside ReqwestTransport:

```rust
pub use codex_client::HttpTransportHandle;
pub use codex_client::Request;
pub use codex_client::Response;
pub use codex_client::StreamResponse;
```

This lets codex-core consume the transport through its existing codex-api dependency.

- [ ] **Step 2: Write a failing ModelClient test helper**

In codex-rs/core/src/client_tests.rs, add these imports:

```rust
use bytes::Bytes;
use codex_api::HttpTransportHandle;
use codex_api::Request;
use codex_api::Response;
use codex_api::StreamResponse;
use http::HeaderMap;
use http::StatusCode;
```

Add this test after summarize_memories_returns_empty_for_empty_input:

```rust
#[tokio::test]
async fn model_client_uses_injected_http_transport() -> anyhow::Result<()> {
    let request_urls = Arc::new(Mutex::new(Vec::new()));
    let sse_body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-injected\"}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-injected\"}}\n\n",
    );

    let transport = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build(
                "execute should not be called".to_string(),
            ))
        },
        {
            let request_urls = Arc::clone(&request_urls);
            move |request: Request| {
                let request_urls = Arc::clone(&request_urls);
                async move {
                    request_urls
                        .lock()
                        .expect("request URL lock")
                        .push(request.url);
                    Ok(StreamResponse {
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        bytes: Box::pin(futures::stream::iter([Ok(Bytes::from_static(
                            sse_body.as_bytes(),
                        ))])),
                    })
                }
            }
        },
    );

    let client =
        test_model_client(SessionSource::Cli).with_http_transport(transport);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-injected"),
        format!("{}:0", client.state.thread_id),
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::Turn,
    );
    let mut client_session = client.new_session();
    let mut stream = client_session
        .stream_responses_api(
            &crate::Prompt::default(),
            &model_info,
            &session_telemetry,
            /*effort*/ None,
            ReasoningSummaryConfig::Auto,
            /*service_tier*/ None,
            &responses_metadata,
            &InferenceTraceContext::disabled(),
        )
        .await?;

    let mut completed_response_id = None;
    while let Some(event) = stream.next().await {
        if let ResponseEvent::Completed { response_id, .. } = event? {
            completed_response_id = Some(response_id);
        }
    }

    assert_eq!(
        completed_response_id.as_deref(),
        Some("resp-injected")
    );
    assert_eq!(
        request_urls.lock().expect("request URL lock").as_slice(),
        ["https://example.com/v1/responses"]
    );
    Ok(())
}
```

Also import:

```rust
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
```

- [ ] **Step 3: Run the test and verify red**

Run:

```bash
just test -p codex-core model_client_uses_injected_http_transport --no-capture
```

Expected: compile failure because ModelClient::with_http_transport does not exist.

- [ ] **Step 4: Add transport state and selection**

In codex-rs/core/src/client.rs:

1. Import HttpTransportHandle from codex_api.
2. Add this field to ModelClientState:

```rust
http_transport: Option<HttpTransportHandle>,
```

3. Initialize it in ModelClient::new:

```rust
http_transport: None,
```

4. Add these methods inside impl ModelClient:

```rust
pub(crate) fn with_http_transport(
    mut self,
    http_transport: HttpTransportHandle,
) -> Self {
    Arc::get_mut(&mut self.state)
        .expect("model client state should be uniquely owned during construction")
        .http_transport = Some(http_transport);
    self
}

fn http_transport(&self) -> HttpTransportHandle {
    self.state.http_transport.clone().unwrap_or_else(|| {
        HttpTransportHandle::from_transport(ReqwestTransport::new(
            build_reqwest_client(),
        ))
    })
}

pub(crate) fn http_transport_override(
    &self,
) -> Option<HttpTransportHandle> {
    self.state.http_transport.clone()
}
```

5. Replace each of the four occurrences of:

```rust
let transport = ReqwestTransport::new(build_reqwest_client());
```

with:

```rust
let transport = self.http_transport();
```

For stream_responses_api, self is ModelClientSession, so use:

```rust
let transport = self.client.http_transport();
```

Do not change websocket construction. Catalyst providers will set supports_websockets = false so model traffic uses the injected HTTP path.

- [ ] **Step 5: Run focused tests**

Run:

```bash
just test -p codex-core model_client_uses_injected_http_transport --no-capture
just test -p codex-client transport_handle --no-capture
```

Expected: both tests pass.

- [ ] **Step 6: Commit ModelClient injection**

Run:

```bash
cd ..
git add codex-rs/codex-api/src/lib.rs \
  codex-rs/core/src/client.rs \
  codex-rs/core/src/client_tests.rs
git commit -m "feat(core): allow model client transport injection"
```

### Task 3: Carry transport from ThreadManager to Session

**Files:**

- Modify: codex-rs/core/src/thread_manager.rs
- Modify: codex-rs/core/src/lib.rs
- Modify: codex-rs/core-api/src/lib.rs
- Modify: codex-rs/core/src/session/mod.rs
- Modify: codex-rs/core/src/session/session.rs
- Modify: codex-rs/core/src/codex_delegate.rs
- Modify: codex-rs/core/src/session/tests.rs
- Modify: codex-rs/core/src/session/tests/guardian_tests.rs

- [ ] **Step 1: Define runtime options**

In codex-rs/core/src/thread_manager.rs, import HttpTransportHandle and add:

```rust
#[derive(Clone, Debug, Default)]
pub struct ThreadManagerRuntimeOptions {
    http_transport: Option<HttpTransportHandle>,
}

impl ThreadManagerRuntimeOptions {
    pub fn with_http_transport(
        mut self,
        http_transport: HttpTransportHandle,
    ) -> Self {
        self.http_transport = Some(http_transport);
        self
    }

    pub(crate) fn http_transport(&self) -> Option<HttpTransportHandle> {
        self.http_transport.clone()
    }
}
```

Add to ThreadManagerState:

```rust
runtime_options: ThreadManagerRuntimeOptions,
```

- [ ] **Step 2: Preserve the existing constructor**

Keep ThreadManager::new public and source-compatible. Change it to delegate to a new constructor:

```rust
#[allow(clippy::too_many_arguments)]
pub fn new(
    config: &Config,
    auth_manager: Arc<AuthManager>,
    session_source: SessionSource,
    environment_manager: Arc<EnvironmentManager>,
    extensions: Arc<ExtensionRegistry<Config>>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
    analytics_events_client: Option<AnalyticsEventsClient>,
    thread_store: Arc<dyn ThreadStore>,
    state_db: Option<StateDbHandle>,
    installation_id: String,
    attestation_provider: Option<Arc<dyn AttestationProvider>>,
    external_time_provider: Option<Arc<dyn TimeProvider>>,
) -> Self {
    Self::new_with_runtime_options(
        config,
        auth_manager,
        session_source,
        environment_manager,
        extensions,
        user_instructions_provider,
        analytics_events_client,
        thread_store,
        state_db,
        installation_id,
        attestation_provider,
        external_time_provider,
        ThreadManagerRuntimeOptions::default(),
    )
}
```

Rename the current constructor body to new_with_runtime_options, keep every current initialization, and add runtime_options to ThreadManagerState:

```rust
#[allow(clippy::too_many_arguments)]
pub fn new_with_runtime_options(
    config: &Config,
    auth_manager: Arc<AuthManager>,
    session_source: SessionSource,
    environment_manager: Arc<EnvironmentManager>,
    extensions: Arc<ExtensionRegistry<Config>>,
    user_instructions_provider: Arc<dyn UserInstructionsProvider>,
    analytics_events_client: Option<AnalyticsEventsClient>,
    thread_store: Arc<dyn ThreadStore>,
    state_db: Option<StateDbHandle>,
    installation_id: String,
    attestation_provider: Option<Arc<dyn AttestationProvider>>,
    external_time_provider: Option<Arc<dyn TimeProvider>>,
    runtime_options: ThreadManagerRuntimeOptions,
) -> Self {
    let codex_home = config.codex_home.clone();
    let restriction_product = session_source.restriction_product();
    let (thread_created_tx, _) =
        broadcast::channel(THREAD_CREATED_CHANNEL_CAPACITY);
    let plugins_manager = Arc::new(PluginsManager::new_with_options(
        codex_home.to_path_buf(),
        restriction_product,
        auth_manager.get_api_auth_mode(),
    ));
    let mcp_manager = Arc::new(McpManager::new_with_extensions(
        Arc::clone(&plugins_manager),
        Arc::clone(&extensions),
    ));
    let skills_service = Arc::new(
        SkillsService::new_with_restriction_product(
            codex_home,
            config.bundled_skills_enabled(),
            restriction_product,
        ),
    );

    Self {
        state: Arc::new(ThreadManagerState {
            threads: Arc::new(RwLock::new(HashMap::new())),
            thread_created_tx,
            models_manager: build_models_manager(
                config,
                Arc::clone(&auth_manager),
            ),
            environment_manager,
            skills_service,
            plugins_manager,
            mcp_manager,
            extensions,
            user_instructions_provider,
            thread_store,
            attestation_provider,
            external_time_provider,
            auth_manager,
            session_source,
            installation_id,
            analytics_events_client,
            state_db,
            runtime_options,
            ops_log: should_use_test_thread_manager_behavior()
                .then(|| Arc::new(std::sync::Mutex::new(Vec::new()))),
        }),
        _test_codex_home_guard: None,
    }
}
```

In the existing with_models_provider_home_and_state_for_tests constructor, add this field to its ThreadManagerState literal:

```rust
runtime_options: ThreadManagerRuntimeOptions::default(),
```

- [ ] **Step 3: Export runtime options**

In codex-rs/core/src/lib.rs:

```rust
pub use thread_manager::ThreadManagerRuntimeOptions;
```

In codex-rs/core-api/src/lib.rs:

```rust
pub use codex_core::ThreadManagerRuntimeOptions;
```

- [ ] **Step 4: Add transport to CodexSpawnArgs**

In codex-rs/core/src/session/mod.rs, add to CodexSpawnArgs:

```rust
pub(crate) http_transport: Option<HttpTransportHandle>,
```

Import HttpTransportHandle, destructure http_transport in Codex::spawn_internal, and pass it to Session::new immediately before attestation_provider.

In ThreadManagerState::spawn_thread_with_source, add:

```rust
http_transport: self.runtime_options.http_transport(),
```

to the CodexSpawnArgs literal.

The direct delegate path bypasses ThreadManager. In codex-rs/core/src/codex_delegate.rs, add this field to its CodexSpawnArgs literal so delegated agents inherit the same override:

```rust
http_transport: parent_session
    .services
    .model_client
    .http_transport_override(),
```

In codex-rs/core/src/session/tests/guardian_tests.rs, add the explicit default to its direct CodexSpawnArgs test literal:

```rust
http_transport: None,
```

The file codex-rs/core/src/session/tests.rs has three tests that call Session::new directly. Add the explicit default immediately before each attestation_provider argument:

```rust
/*http_transport*/ None,
```

- [ ] **Step 5: Apply transport during Session construction**

In codex-rs/core/src/session/session.rs:

1. Import HttpTransportHandle.
2. Add this Session::new argument immediately before attestation_provider:

```rust
http_transport: Option<HttpTransportHandle>,
```

3. Immediately before SessionServices construction, create the model client:

```rust
let mut model_client = ModelClient::new(
    Some(Arc::clone(&auth_manager)),
    thread_id,
    session_configuration.provider.clone(),
    session_configuration.session_source.clone(),
    config.model_verbosity,
    config
        .features
        .enabled(Feature::EnableRequestCompression),
    config.features.enabled(Feature::RuntimeMetrics),
    Self::build_model_client_beta_features_header(config.as_ref()),
    /*item_ids_enabled*/ config.features.enabled(Feature::ItemIds),
    attestation_provider.clone(),
);
if let Some(http_transport) = http_transport {
    model_client = model_client.with_http_transport(http_transport);
}
```

Replace the existing inline model_client field expression with:

```rust
model_client: model_client.with_prompt_cache_key_override(
    crate::guardian::prompt_cache_key_override_for_review_session(
        &session_configuration.session_source,
        session_configuration.parent_thread_id,
    ),
),
```

- [ ] **Step 6: Compile before adding integration tests**

Run:

```bash
just test -p codex-core model_client_uses_injected_http_transport --no-capture
```

Expected: compilation succeeds and the focused test passes. Compiler errors about a missing CodexSpawnArgs field identify every construction site that needs the new field.

Teaching checkpoint: this task is ownership plumbing. HttpTransportHandle is cheap to clone because each clone increments Arc reference counts; it does not clone provider state or network buffers.

- [ ] **Step 7: Commit runtime plumbing**

Run:

```bash
cd ..
git add codex-rs/core/src/thread_manager.rs \
  codex-rs/core/src/lib.rs \
  codex-rs/core-api/src/lib.rs \
  codex-rs/core/src/session/mod.rs \
  codex-rs/core/src/session/session.rs \
  codex-rs/core/src/codex_delegate.rs \
  codex-rs/core/src/session/tests.rs \
  codex-rs/core/src/session/tests/guardian_tests.rs
git commit -m "feat(core): inject HTTP transport through thread manager"
```

### Task 4: Prove an injected transport completes a native agent turn

**Files:**

- Modify: codex-rs/core/tests/common/test_codex.rs
- Create: codex-rs/core/tests/suite/transport_injection.rs
- Modify: codex-rs/core/tests/suite/mod.rs

- [ ] **Step 1: Extend the integration-test builder**

In TestCodexBuilder add:

```rust
runtime_options: ThreadManagerRuntimeOptions,
```

Import ThreadManagerRuntimeOptions from codex_core in this test helper.

Add this method:

```rust
pub fn with_runtime_options(
    mut self,
    runtime_options: ThreadManagerRuntimeOptions,
) -> Self {
    self.runtime_options = runtime_options;
    self
}
```

Initialize it in test_codex():

```rust
runtime_options: ThreadManagerRuntimeOptions::default(),
```

In build_from_config, replace ThreadManager::new with ThreadManager::new_with_runtime_options and pass:

```rust
self.runtime_options.clone(),
```

as the final argument.

- [ ] **Step 2: Write the integration test**

Add to codex-rs/core/tests/suite/mod.rs:

```rust
mod transport_injection;
```

Create codex-rs/core/tests/suite/transport_injection.rs:

```rust
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use bytes::Bytes;
use codex_api::HttpTransportHandle;
use codex_api::Request;
use codex_api::Response;
use codex_api::StreamResponse;
use codex_api::TransportError;
use codex_core::ThreadManagerRuntimeOptions;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::test_codex::test_codex;
use http::HeaderMap;
use http::StatusCode;
use pretty_assertions::assert_eq;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_transport_completes_native_agent_turn() -> Result<()> {
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let transport = HttpTransportHandle::new(
        |_request: Request| async {
            Err::<Response, TransportError>(TransportError::Build(
                "execute should not be called".to_string(),
            ))
        },
        {
            let request_bodies = Arc::clone(&request_bodies);
            move |request: Request| {
                let request_bodies = Arc::clone(&request_bodies);
                async move {
                    let prepared = request
                        .prepare_body_for_send()
                        .map_err(TransportError::Build)?;
                    request_bodies
                        .lock()
                        .expect("request body lock")
                        .push(prepared.body_bytes());

                    let body = sse(vec![
                        ev_response_created("resp-injected"),
                        ev_assistant_message(
                            "msg-injected",
                            "injected transport completed",
                        ),
                        ev_completed("resp-injected"),
                    ]);
                    Ok(StreamResponse {
                        status: StatusCode::OK,
                        headers: HeaderMap::new(),
                        bytes: Box::pin(futures::stream::iter([Ok(
                            Bytes::from(body),
                        )])),
                    })
                }
            }
        },
    );
    let runtime_options =
        ThreadManagerRuntimeOptions::default().with_http_transport(transport);

    let server = MockServer::start().await;
    let test = test_codex()
        .with_runtime_options(runtime_options)
        .with_config(|config| {
            config.model_provider.supports_websockets = false;
        })
        .build(&server)
        .await?;

    test.submit_turn("hello through injected transport").await?;

    let bodies = request_bodies.lock().expect("request body lock");
    assert_eq!(bodies.len(), 1);
    let request: serde_json::Value =
        serde_json::from_slice(&bodies[0])?;
    assert!(
        request
            .to_string()
            .contains("hello through injected transport")
    );
    Ok(())
}
```

- [ ] **Step 3: Run the end-to-end integration proof**

Run:

```bash
just test -p codex-core injected_transport_completes_native_agent_turn --no-capture
```

Expected: one passing integration test, one captured request body, and no request requirement on the WireMock server.

- [ ] **Step 4: Deliberately verify the assertion protects the seam**

Temporarily change with_runtime_options(runtime_options) to with_runtime_options(ThreadManagerRuntimeOptions::default()), then run:

```bash
just test -p codex-core injected_transport_completes_native_agent_turn --no-capture
```

Expected: FAIL because no request body reaches the injected transport. Undo that temporary edit immediately and rerun the command; expected: PASS.

- [ ] **Step 5: Commit the integration proof**

Run:

```bash
cd ..
git add codex-rs/core/tests/common/test_codex.rs \
  codex-rs/core/tests/suite/mod.rs \
  codex-rs/core/tests/suite/transport_injection.rs
git commit -m "test(core): cover injected transport agent turn"
```

### Task 5: Learn logging, test debugging, and LLDB

**Files:** None required.

- [ ] **Step 1: Run the focused test with transport tracing**

Run:

```bash
cd /Users/luoruipu/projects/other_projects/codex/codex-rs
RUST_LOG=codex_core::client=trace,codex_client::transport=trace \
  just test -p codex-core \
  injected_transport_completes_native_agent_turn \
  --no-capture
```

Expected: passing test plus trace output from the HTTP Responses path.

If output is too noisy, narrow it:

```bash
RUST_LOG=codex_core::client=debug \
  just test -p codex-core \
  injected_transport_completes_native_agent_turn \
  --no-capture
```

- [ ] **Step 2: Run Codex TUI from source with a plaintext log**

Terminal 1:

```bash
RUST_LOG=codex_core::client=debug,codex_client::transport=trace \
  just codex -c log_dir=./.codex-log
```

Terminal 2:

```bash
tail -F ./.codex-log/codex-tui.log
```

Expected: the TUI starts and the second terminal follows the source-built process log.

- [ ] **Step 3: Run non-interactive Codex from source**

Run:

```bash
RUST_LOG=codex_core::client=debug \
  just exec --ephemeral "Reply with exactly: source build works"
```

Expected: a non-interactive turn. This still uses the default provider until the private Catalyst composition binary is built in Plan 2.

- [ ] **Step 4: Build a debug binary for LLDB**

Run:

```bash
cargo build --bin codex
rust-lldb target/debug/codex
```

At the LLDB prompt:

```text
(lldb) breakpoint set --func-regex stream_responses_api
(lldb) breakpoint set --func-regex HttpTransportHandle
(lldb) settings set target.env-vars RUST_LOG=codex_core::client=trace
(lldb) run exec --ephemeral "debug transport"
(lldb) bt
(lldb) frame variable
(lldb) next
(lldb) continue
```

Expected: LLDB stops in the HTTP Responses path. Rust async functions generate state-machine symbols, so --func-regex is more reliable than an exact symbol name.

- [ ] **Step 5: Know which debugging layer to choose**

Use this order:

1. Compiler diagnostics for type, ownership, and Send/lifetime errors.
2. Focused nextest test with --no-capture.
3. tracing with a narrow RUST_LOG filter.
4. LLDB only when state or control flow is still unclear.

For an ownership error, do not add clone immediately. First answer: who should own the value, and how long must it live?

### Task 6: Format, lint, verify, and hand off

**Files:** All files changed above.

- [ ] **Step 1: Inspect the complete patch**

Run:

```bash
cd /Users/luoruipu/projects/other_projects/codex
git diff --stat rust-v0.142.4...HEAD
git diff --check
git status --short
```

Expected: only the planned transport and test files.

- [ ] **Step 2: Run focused verification**

Run from codex-rs:

```bash
just test -p codex-client transport_handle --no-capture
just test -p codex-core model_client_uses_injected_http_transport --no-capture
just test -p codex-core injected_transport_completes_native_agent_turn --no-capture
```

Expected: all focused tests pass.

- [ ] **Step 3: Apply repository formatting and scoped fixes**

Run:

```bash
just fix -p codex-client
just fix -p codex-core
just fmt
```

Per repository instructions, do not rerun tests after fix/fmt. Review the formatter diff instead:

```bash
cd ..
git diff --check
git status --short
```

- [ ] **Step 4: Commit formatter and Clippy changes if they changed files**

Run:

```bash
cd ..
git status --short
git add codex-rs/codex-client/src/lib.rs \
  codex-rs/codex-client/src/transport_handle.rs \
  codex-rs/codex-client/src/transport_handle_tests.rs \
  codex-rs/codex-api/src/lib.rs \
  codex-rs/core/src/client.rs \
  codex-rs/core/src/client_tests.rs \
  codex-rs/core/src/thread_manager.rs \
  codex-rs/core/src/lib.rs \
  codex-rs/core-api/src/lib.rs \
  codex-rs/core/src/session/mod.rs \
  codex-rs/core/src/session/session.rs \
  codex-rs/core/src/codex_delegate.rs \
  codex-rs/core/src/session/tests.rs \
  codex-rs/core/src/session/tests/guardian_tests.rs \
  codex-rs/core/tests/common/test_codex.rs \
  codex-rs/core/tests/suite/mod.rs \
  codex-rs/core/tests/suite/transport_injection.rs
git diff --cached --stat
```

If the staged stat is non-empty, run:

```bash
git commit -m "chore: format transport injection patch"
```

Expected: either no staged formatter changes or one small formatting commit.

- [ ] **Step 5: Run full workspace tests only with explicit approval**

The patch touches codex-core, so the repository requests a complete suite. Ask before running:

```bash
just test
```

Expected: this can be long and should not be interrupted.

- [ ] **Step 6: Push the completed public patch branch**

Run:

```bash
cd ..
git push origin codex/catalyst-transport-v0.142.4
```

Expected: the fork branch advances with the focused, reviewable commits.

## Definition of Done

- The default Codex CLI still uses ReqwestTransport.
- A host can supply HttpTransportHandle through ThreadManagerRuntimeOptions.
- An injected transport receives a native /responses Request.
- Its raw SSE bytes pass through the existing Codex parser.
- The existing agent loop emits TurnComplete.
- WebSocket model transport is explicitly disabled for Catalyst provider configs.
- No provider-specific code exists in the public fork.
- No dependency or Cargo.lock change is committed.
- Focused codex-client and codex-core tests pass.
