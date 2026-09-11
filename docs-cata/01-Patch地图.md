# Catalyst Patch 地图

本文按当前主线代码整理；2026-09-08 的固定身份与完整差异见 [差异索引](04-官方基线差异索引.md)，宿主装配差异和测试缺口见 [架构治理](05-宿主消费与架构治理.md)。当前基线是 `rust-v0.153.4`；地图只表示人工复核入口，不表示整个文件都由 Catalyst 编写。

## 一、Runtime 注入主链

```text
host
  -> AppServerProcessOverrides
  -> ThreadManagerRuntimeOptions
  -> ThreadManagerState
  -> ModelClient / Session / ThreadStore / Extensions
```

| 能力 | 代码入口 | 重点证据 |
| --- | --- | --- |
| 进程级 Runtime options | `codex-rs/core/src/thread_manager.rs` | `ThreadManagerRuntimeOptions` 的 builder 与创建 session 的传递 |
| App Server 宿主接入 | `codex-rs/app-server/src/lib.rs`、`in_process.rs` | `AppServerProcessOverrides`、`run_main_with_transport_options_and_overrides` |
| 可复用 App Server client | `codex-rs/app-server-client/src/lib.rs` | `start_with_thread_manager_options`；Runtime 中用于 dev 测试，不是生产主入口 |
| HTTP transport override | `codex-rs/codex-client/src/transport_handle.rs`、`core/src/client.rs` | 注入后请求统一走 host transport，避免隐式切换 WebSocket |
| 静态 model catalog | `core/src/thread_manager.rs`、`models-manager` | `StaticModelsManager` 只在宿主提供 catalog 时替换默认 manager |

### 五类接线的直接验证路由

Runtime 的 App Server 装配入口为其仓库 `crates/catalyst-app-server/src/lib.rs::process_overrides`。
下表 Codex 路径相对 `codex-rs/`；Runtime 路径相对 Runtime 仓库根。测试是复核入口，不表示每次文档更新都执行了测试。

| 能力 | Runtime 注入 | Codex 消费 | 直接测试路由 |
| --- | --- | --- | --- |
| 模型目录 | `with_model_catalog`；目录来自 `crates/catalyst-codex/src/model_catalog.rs::build_model_catalog` | `core/src/thread_manager.rs::new_with_runtime_options` → `StaticModelsManager` | Runtime `crates/catalyst-app-server/tests/runtime_composition.rs::app_server_overrides_include_transport_catalog_and_tavily_extension` 验证装配；`crates/catalyst-codex/src/model_catalog.rs::tests::catalog_is_default_first_then_product_id` 验证目录投影 |
| HTTP / 模型策略 | `with_http_transport`、逐模型 `with_model_runtime_policy` | `core/src/thread_manager.rs` → `core/src/session/session.rs` → `core/src/client.rs::ModelClient`；HTTP override 禁用模型 WebSocket | `core/tests/suite/incremental_http.rs::host_policy_resends_full_history_when_previous_response_expires`；Runtime `crates/catalyst-codex/tests/qwen_incremental_fallback.rs::synthetic_policy_to_core_fallback_matrix` 覆盖真实 policy/transport 到 Core 的消费 |
| 工具 / Skill | `with_runtime_extension`、`with_skill_provider`；Browser 条件注入 | `core/src/tools/spec_plan.rs` 装配工具，`ext/skills/src/sources.rs` 按 authority/kind 路由 | `core/src/tools/spec_plan_tests.rs::hosted_web_search_and_standalone_image_generation_follow_runtime_gates`；`core/tests/suite/skills_extension.rs::production_turn_uses_provider_host_catalog_and_core_snapshot_injection`；Runtime `crates/catalyst-private-skills/tests/provider.rs::read_rejects_unentitled_unknown_legacy_and_invalid_handles_before_key_delivery` |
| 标题 | `with_title_generator` | `core/src/thread_manager.rs` → `thread-store/src/local/mod.rs` → `thread-store/src/live_thread.rs`；持久化前取得 metadata mutation gate | `thread-store/src/local/live_thread_title_tests.rs::manual_thread_rename_wins_over_in_flight_llm_title`、`rejected_metadata_mutation_gate_skips_llm_title_and_callback` |
| 产品 RPC | `AppServerProcessOverrides::with_rpc_extension` | `app-server/src/lib.rs` → `rpc_extension.rs::AppServerRpcRegistry` → `message_processor.rs` | Runtime `crates/catalyst-app-server/tests/rpc_router.rs` 的 duplicate、namespace、manifest 和 handler error 用例；这些验证 Runtime router；fork `rpc_extension_tests.rs` 另覆盖 native-method 防覆盖 |

空 overrides 使用默认装配，但 Skill roots、流式 item 等默认行为补丁仍生效，不能据此认定等同官方；工具最终集合由实际装配及 gate 决定，不以固定工具数量作为长期不变量。

Runtime 装配补充证据：`runtime_composition.rs::image_capable_model_plans_product_tools_without_hosted_duplicates` 验证实际 Qwen 请求；TUI `main.rs::tests::catalyst_tui_assembly_preserves_runtime_extensions_and_optional_catalog`（需 `test-support`）验证生产 helper 的 extension 引用身份与可选 catalog。范围与结果见[升级验证](06-0.153.4升级验证.md)和[分层验证](08-敏感值分层与Cata测试设计.md)。

## 二、可保留的产品能力

| 能力 | 主要入口 | 复核边界 | 测试/证据入口 |
| --- | --- | --- | --- |
| 敏感值基础类型 | `utils/sensitive-string/src/lib.rs` | Runtime 与 TUI 共用；旧 `SensitiveInput` 是同类型别名，基础 crate 不依赖 UI | `sensitive_string_tests.rs`；Runtime credentials 与适配测试 |
| TUI model runtime | `tui/src/catalyst/model_runtime.rs`、`tui/src/app/`、`tui/src/chatwidget/` | TUI 只消费抽象 runtime，不把 provider 业务硬编码进 UI | `tui/src/app/tests/`、`chatwidget/tests/`、敏感输入单测 |
| Credential workflow | `tui/src/catalyst/credentials.rs`、`bottom_pane/sensitive_prompt_view.rs`、`chatwidget/credential_popups.rs` | secret 输入、redacted Debug、zeroize、失败恢复 | `sensitive_prompt_view_tests.rs` 与 TUI snapshots |
| App Server RPC extension | `app-server/src/catalyst/rpc_extension.rs`、`message_processor.rs` | 方法必须 namespaced，不得覆盖 native RPC；transport context 影响信任边界 | fork `rpc_extension_tests.rs` 与 `message_processor_rpc_tests.rs` 覆盖 registry/initialize/dispatch；native bridge 完整生命周期仍不能由 router 单测替代 |
| Native turn/plugin bridge | `app-server/src/extensions.rs`、`request_processors/plugins.rs`、`rpc_extension.rs` | 只暴露受控 gateway；plugin selection 不能绕过现有生命周期 | Runtime `runtime_composition.rs::catalyst_turn_start_enters_the_native_turn_lifecycle_once`；fork plugin/queue 边界证据待补 |
| Host Skill provider | `ext/skills/src/sources.rs`、`catalog.rs`、`tools/` | provider 按 authority/kind 路由；读取失败不能静默跨 authority | `core/tests/suite/skills_extension.rs` |
| Thread title generator | `thread-store/src/title_generator.rs`、`live_thread.rs`、`local/mod.rs` | best-effort 异步生成；metadata mutation 必须通过 gate，不能覆盖手工标题 | thread-store/session tests |
| HTTP incremental requests | `core/src/client.rs`、`session/turn.rs` | baseline 与 wire request 必须一致；previous response 失效时回退 full request | `core/tests/suite/incremental_http.rs` 与 client suite |
| 工具图片迁移 | `core/src/client/catalyst/host_http.rs` | 先从 function output 移出图片，再捕获 baseline；保留 output array 形状和图片顺序 | incremental request shape tests |

### 当前必须单独跟踪的补充能力

| 能力 | 代码/消费入口 | 验证入口与限制 |
| --- | --- | --- |
| process MCP 整表替换 | `app-server/src/cli.rs` → overrides → `config_manager.rs` → `core/src/config/mod.rs`；Runtime flatten 共享 CLI | `process_mcp_server_replacement_discards_stale_fields_across_rebuild`、`app_server_accepts_process_mcp_server_replacement`；`app-server/src/catalyst/config_manager_tests.rs` 验证 refresh、request override 优先级、requirements 禁用和 typed validation；`cli_tests.rs` 验证 CLI 整表解析及非法输入 |
| 流式 item ID 归属 | `codex-api/src/sse/responses.rs` → `core/src/session/turn.rs` | `preserves_stream_delta_item_ids`、`output_text_delta_before_output_item_added_is_buffered`、`interleaved_response_items_keep_delta_ownership` |
| Skill roots 默认限制 | `ext/skills/src/host_roots.rs`；不自动扫描 home/repo `.agents/skills` | `resolved_roots_preserve_configured_sources_and_ignore_agents_dirs`；不等同仅允许私有 Skill |
| Skill 清单输出过滤与完整性 | Runtime 原生 `skills/list` → `ext/skills/src/loader/discovery.rs`、`host.rs`；每次 walk 显式请求 `WalkOptions.file_names`，默认省略仍返回全部文件。仅减少返回清单，不隐藏资源或放宽遍历预算；host 将扫描 warning 传入既有 errors 并保留成功项 | `discovery_tests.rs::discovers_nested_skills_when_resource_inventory_exceeds_response_budget`、`host_tests.rs::reports_truncated_host_inventory_with_and_without_discovered_skills`；exec-server `file_system/shared.rs` 的 local/remote 过滤用例及 protocol 旧 wire 兼容用例。旧执行端可忽略过滤，截断仍须作为不完整处理；合并热点为 walk、discovery 与 host 错误汇总 |
| 独立搜索能力 | `tools/src/tool_executor.rs::is_standalone_web_search` → `core/src/tools/spec_plan.rs`；Runtime `WebRunTool` 声明 | 任意宿主名称/gate、注册冲突、真实工具 schema 测试；保留原生名称兼容与历史 wire 名称 |
| 基础指令与 prompt debug | `core/src/thread_manager.rs`、`prompt_debug.rs`；Runtime 两条 main 和 debug prompt 消费 | Runtime `system_prompt.rs`、`private_skills_runtime.rs`；prompt snapshot 不代替真实工具执行 |
| 标题 metadata gate | `core/src/session/session.rs`、`session/handlers.rs` | `metadata_mutation_gate_tests`；gate 由 Core 创建，不是 Runtime update-install 门禁 |

## 三、不要误判为 patch 的内容

当前 `windows-sandbox-rs` 与 0.153.4 官方基线无差异，private desktop 作为上游平台能力维护，不再列为 Catalyst patch；这不代表已取得 Windows 运行验收。

- `codex-rs/` 是上游实现和 fork 变化的混合目录。
- `Cargo.lock`、协议类型、snapshot 和测试可能只是某个代码变化的构建或验证结果。
- `app-server`、`core/session`、`protocol`、`thread-store` 中的生命周期和持久化逻辑默认仍由上游拥有，除非能指出明确的
  宿主契约和测试证据。
- Catalyst Runtime 或 Workbench 自己的业务 crate 不属于本仓库的 patch。

## 四、审查一个差异的顺序

1. 对照 `rust-v0.153.4` 确认差异，而不是对照旧 fork 或旧同步分支。
2. 读取引入差异的提交，区分产品能力、上游适配、测试/生成物和同步基础设施。
3. 找到调用方、公开 API 或运行时制品中的实际消费者。
4. 检查 focused test、schema、snapshot 或 wire body 证据。
5. 如果上游已提供等价能力，删除本地 duplicate，并保留上游不变量。

## 五、常用命令

```bash
git diff --stat rust-v0.153.4..HEAD
git diff --name-status rust-v0.153.4..HEAD -- codex-rs/
git log --follow --oneline -- codex-rs/core/src/client.rs
git blame <commit> -- codex-rs/core/src/thread_manager.rs
git diff --check
```

## HTTP 请求补丁的内部边界

`core/src/client/catalyst/host_http.rs` 只承担宿主 policy 启用的请求图片迁移与 previous response 失效识别；`client.rs` 保留 session baseline、delta 选择和重试顺序。迁移发生在保存 typed baseline 前，不改写持久化历史。公开 `ModelRuntimePolicy` 路径和 Runtime 注入接口不变。

验证入口：fork `client::host_http::tests::recognizes_qwen_unknown_previous_response_error`，以及 Runtime `qwen_incremental_fallback::synthetic_policy_to_core_fallback_matrix`。

### 长耗时宿主 RPC 显式并发分发

`AppServerRpcExtension::concurrent_methods` 是 process-scoped 的宿主显式 opt-in，默认空集合保持原有分发顺序。Runtime 为受限后台推理及账号状态读取启用该声明；原生 MessageProcessor 在初始化门禁后投递独立请求任务，继续接收对话与文件请求。授权、并发上限、超时由 Runtime handler 保持。合并热点是 message_processor.rs 的宿主扩展分支；直接测试 slow_background_extension_does_not_block_following_native_reads 将后台 Future 挂起，再验证后续原生 thread/list 仍可返回，另保留初始化与错误契约测试。
