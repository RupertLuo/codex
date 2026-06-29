# Catalyst Provider Transport Design

## Status

- Baseline: Codex `rust-v0.128.0`
- Development branch: `codex/catalyst-transport-poc`
- Upstream repository: `openai/codex`
- Fork: `RupertLuo/codex`

## Objective

Allow a Catalyst-owned Rust runtime to translate Codex Responses API traffic to multiple LLM provider APIs without replacing the native Codex agent turn loop. The first supported providers are DeepSeek, GLM, Kimi, and MiniMax.

The Codex fork must contain only a small, reviewable dependency-injection patch. Provider-specific protocol logic and private skills belong in a separate private repository and are linked into the Catalyst binary at build time.

## Non-goals

- Reimplementing the Codex agent loop.
- Adding provider-specific request fields to Codex core types.
- Treating text-only chat as sufficient Codex agent support.
- Creating an app-server MCP compatibility layer.
- Guaranteeing that compiled private skills cannot be reverse engineered.
- Supporting runtime-loaded third-party provider plugins in the first version.

## Repository Topology

### Public Codex fork

`RupertLuo/codex` tracks `openai/codex` through an `upstream` remote. It owns only:

- the `HttpTransportFactory` and `RuntimeExtensions` injection point;
- builder plumbing needed to inject that extension;
- tests proving the default Codex transport remains unchanged;
- tests proving a supplied transport reaches model requests.

It must not contain provider API keys, private skills, provider fixtures copied from production traffic, or proprietary translation rules.

### Private Catalyst runtime

The private `catalyst-runtime-rs` repository is a Cargo workspace with this initial shape:

```text
catalyst-runtime-rs/
├── Cargo.toml
├── crates/
│   ├── catalyst-llm-protocol/
│   ├── catalyst-openai-compat/
│   ├── catalyst-codex-transport/
│   ├── catalyst-provider-deepseek/
│   ├── catalyst-provider-glm/
│   ├── catalyst-provider-kimi/
│   ├── catalyst-provider-minimax/
│   └── catalyst-skills/
├── fixtures/
│   ├── deepseek/
│   ├── glm/
│   ├── kimi/
│   └── minimax/
└── tests/
    └── agent-contract/
```

Provider crates may share an OpenAI Chat Completions codec, but each provider retains an explicit policy layer for endpoints, authentication, reasoning fields, caching, tool-call quirks, errors, and capability reporting.

## Codex Patch Boundary

Codex currently exposes `codex_client::HttpTransport`, while core model-client construction creates `ReqwestTransport` directly. The patch will make transport construction injectable from the public core API composition root.

The selected shape is:

1. Add a documented `HttpTransportFactory` abstraction beside the neutral `HttpTransport` layer.
2. Add a `RuntimeExtensions` value that owns an `Arc<dyn HttpTransportFactory>`.
3. Add `ThreadManager::new_with_runtime_extensions`; keep `ThreadManager::new` and make it delegate with default extensions.
4. Store the extensions in thread-manager state and pass the selected factory to every session-scoped `ModelClient`.
5. Add a model-client constructor that accepts the factory; keep the existing constructor as the default path.
6. Preserve `ReqwestTransport` construction in the default factory.
7. Keep request serialization, SSE parsing, tool dispatch, turn continuation, and event handling native to Codex.

The patch must not add provider branches to `run_turn`, `run_sampling_request`, or the tool-call loop. If upstream later offers a stable factory, the fork should delete its patch and adopt the upstream interface.

## Private Runtime Architecture

The runtime translates through a provider-neutral intermediate representation:

```text
Codex Responses request
    -> Codex ingress decoder
    -> NormalizedRequest
    -> ProviderPolicy + provider wire codec
    -> provider HTTP/SSE API
    -> provider event decoder
    -> NormalizedEvent stream
    -> Responses SSE encoder
    -> native Codex SSE parser and agent loop
```

`NormalizedRequest` represents messages, tools, tool results, reasoning intent, output limits, response format, and stable request metadata. `NormalizedEvent` represents response start, text delta, reasoning delta, tool-call start, tool-argument delta, usage, completion, and failure.

The normalized protocol must not attempt to expose the union of every provider field. Provider-only options live in typed provider policy/configuration, and unsupported capabilities produce explicit errors or documented degradation.

## Provider Contract

All four initial providers must pass the minimum Codex agent contract:

- streaming text deltas;
- multi-turn conversation reconstruction;
- tool definitions;
- streamed or non-streamed tool calls;
- tool result submission on the next model request;
- stable tool-call ID correlation;
- cancellation and stream termination;
- normalized authentication, rate-limit, timeout, and provider errors.

Reasoning output, prompt caching, vision, structured output, and parallel tool calls are capability-gated. A provider may be released without an optional capability, but not without the minimum agent contract.

## Provider Selection

Provider selection is explicit configuration resolved at process startup. The first implementation should use a closed Rust enum for built-in providers rather than runtime plugin loading. This keeps matching exhaustive and packaging deterministic while the protocol is still evolving.

Each provider exposes a capability record. Request validation happens before network I/O so unsupported combinations fail with a useful provider-specific message.

## Error and Retry Behavior

The private runtime maps provider failures into a small stable taxonomy: authentication, invalid request, unsupported capability, rate limit, timeout, network, malformed stream, provider service failure, and cancellation.

The transport must not implement an independent broad retry loop when Codex already owns retry behavior. In particular, it must not replay a request after response bytes or tool-call deltas have been emitted. Provider retry hints may be preserved as structured metadata for the Codex retry layer.

Logs redact authorization headers, cookies, request secrets, and private skill contents. Full protocol dumps are opt-in, stored outside the repository, and disabled in packaged builds by default.

## Testing Strategy

### Public fork tests

- Default construction still selects `ReqwestTransport`.
- A test factory is invoked for model traffic.
- Injecting a transport does not bypass the native SSE parser or agent loop.

### Private runtime tests

- Unit tests for request and event conversions.
- Redacted golden fixtures for every provider.
- Chunk-boundary tests that split SSE and JSON at arbitrary byte positions.
- Contract tests shared by DeepSeek, GLM, Kimi, and MiniMax.
- Mock-server integration tests for text, tool call, tool result, usage, errors, and cancellation.
- Optional live smoke tests gated by provider API-key environment variables.

Live credentials and unredacted traffic are never committed.

## Delivery Sequence

1. Add and test the thin transport injection seam in the Codex fork.
2. Create the private Cargo workspace and normalized protocol crate.
3. Implement one complete DeepSeek vertical slice.
4. Extract the shared OpenAI-compatible codec from proven behavior.
5. Add GLM, Kimi, and MiniMax policies one at a time using the shared contract suite.
6. Compose Codex and the private runtime into the Catalyst binary.
7. Add upstream-rebase CI and a compatibility matrix keyed by Codex tag.

## Upstream Maintenance

The fork records the Codex tag tested by each Catalyst release. Upstream updates are first rebased onto a dedicated integration branch, then validated against the public injection tests and private provider contract suite.

Expected adaptation points are the transport trait, model-client construction, Responses request shape, and SSE event shape. The agent turn-loop behavior should remain upstream-owned.

## Security Boundary

Keeping provider adapters and skills in a private repository prevents source disclosure through the public fork. Static linking raises the cost of casual extraction but does not make embedded prompts or assets impossible to recover. Sensitive skills should therefore avoid embedding long-lived credentials and may later use encryption, entitlement checks, or server-side execution where stronger protection is required.
