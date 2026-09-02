# Catalyst Patch 地图

本文按当前主线代码整理。当前基线是 `rust-v0.149.0`；地图只表示人工复核入口，不表示整个文件都由 Catalyst 编写。

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
| 可复用 App Server client | `codex-rs/app-server-client/src/lib.rs` | `start_with_thread_manager_options` |
| HTTP transport override | `codex-rs/codex-client/src/transport_handle.rs`、`core/src/client.rs` | 注入后请求统一走 host transport，避免隐式切换 WebSocket |
| 静态 model catalog | `core/src/thread_manager.rs`、`models-manager` | `StaticModelsManager` 只在宿主提供 catalog 时替换默认 manager |

## 二、可保留的产品能力

| 能力 | 主要入口 | 复核边界 | 测试/证据入口 |
| --- | --- | --- | --- |
| TUI model runtime | `tui/src/model_runtime.rs`、`tui/src/app/`、`tui/src/chatwidget/` | TUI 只消费抽象 runtime，不把 provider 业务硬编码进 UI | `tui/src/app/tests/`、`chatwidget/tests/`、敏感输入单测 |
| Credential workflow | `tui/src/model_runtime.rs`、`bottom_pane/sensitive_prompt_view.rs`、`chatwidget/credential_popups.rs` | secret 输入、redacted Debug、zeroize、失败恢复 | `sensitive_prompt_view_tests.rs` 与 TUI snapshots |
| App Server RPC extension | `app-server/src/rpc_extension.rs`、`message_processor.rs` | 方法必须 namespaced，不得覆盖 native RPC；transport context 影响信任边界 | app-server message processor tests |
| Native turn/plugin bridge | `app-server/src/extensions.rs`、`request_processors/plugins.rs`、`rpc_extension.rs` | 只暴露受控 gateway；plugin selection 不能绕过现有生命周期 | app-server integration tests |
| Host Skill provider | `ext/skills/src/sources.rs`、`catalog.rs`、`tools/` | provider 按 authority/kind 路由；读取失败不能静默跨 authority | `core/tests/suite/skills_extension.rs` |
| Thread title generator | `thread-store/src/title_generator.rs`、`live_thread.rs`、`local/mod.rs` | best-effort 异步生成；metadata mutation 必须通过 gate，不能覆盖手工标题 | thread-store/session tests |
| HTTP incremental requests | `core/src/client.rs`、`session/turn.rs` | baseline 与 wire request 必须一致；previous response 失效时回退 full request | `core/tests/suite/incremental_http.rs` 与 client suite |
| 工具图片迁移 | `core/src/client.rs` | 先从 function output 移出图片，再捕获 baseline；保留 output array 形状和图片顺序 | incremental request shape tests |
| Windows private desktop | `windows-sandbox-rs/src/desktop.rs`、`unified_exec/` | 仅复用相同账户和有效权限；非法名称、权限变化和并发创建需隔离 | `desktop_tests.rs`、unified exec tests |

## 三、不要误判为 patch 的内容

- `codex-rs/` 是上游实现和 fork 变化的混合目录。
- `Cargo.lock`、协议类型、snapshot 和测试可能只是某个代码变化的构建或验证结果。
- `app-server`、`core/session`、`protocol`、`thread-store` 中的生命周期和持久化逻辑默认仍由上游拥有，除非能指出明确的
  宿主契约和测试证据。
- Catalyst Runtime 或 Workbench 自己的业务 crate 不属于本仓库的 patch。

## 四、审查一个差异的顺序

1. 对照 `rust-v0.149.0` 确认差异，而不是对照旧 fork 或旧同步分支。
2. 读取引入差异的提交，区分产品能力、上游适配、测试/生成物和同步基础设施。
3. 找到调用方、公开 API 或运行时制品中的实际消费者。
4. 检查 focused test、schema、snapshot 或 wire body 证据。
5. 如果上游已提供等价能力，删除本地 duplicate，并保留上游不变量。

## 五、常用命令

```bash
git diff --stat rust-v0.149.0..HEAD
git diff --name-status rust-v0.149.0..HEAD -- codex-rs/
git log --follow --oneline -- codex-rs/core/src/client.rs
git blame <commit> -- codex-rs/core/src/thread_manager.rs
git diff --check
```
