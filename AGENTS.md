# Catalyst Codex Fork

本文件只记录本仓库相对官方 OpenAI Codex 的补充约定。官方 Rust/Codex 规范完整保存在
[`AGENTS.origin.md`](AGENTS.origin.md)；处理任务前先阅读原文，除本文件明确说明的内容外，
原规范继续有效。

## 项目身份和基线

- `OpenAI Codex`：官方上游项目，基线来自 `openai/codex`。
- `Catalyst Codex Fork`：本仓库，在上游实现上提供宿主可注入的 Runtime 能力和必要适配。
- 当前主线对应的官方基线是 `rust-v0.149.0`。
- `codex-rs/` 是 Rust 实现目录，不是 patch 目录；一个文件可以同时包含上游实现、适配和少量 fork 行为。

## 文档边界

- `docs/` 保持 OpenAI Codex 原版内容，不加入 Catalyst 专属说明。
- Catalyst 的 patch 地图、Runtime 接入、同步规则和差异证据统一放在 [`docs-cata/`](docs-cata/)。
- 构建输入、schema、snapshot、Cargo 文件和同步脚本留在它们被代码/工具发现的原路径，不为文档分类而移动。
- 不把一次性 handoff、机器路径、凭据、缓存或 token 写入长期文档。

## Runtime-first 原则

长期目标是尽量使用原版 Codex，并让宿主 Runtime 在边界处注入差异：

- 优先复用上游 crate、生命周期、协议和安全不变量，不复制整段上游实现。
- 新能力优先设计成 process/thread/turn scoped 的显式 override、provider 或 extension。
- 只有当 Runtime 无法在现有 boundary 接入时，才修改上游核心路径；修改必须有调用方和 focused test。
- 若新上游已经提供等价能力，删除 fork duplicate，而不是继续维护两套实现。
- 代码变更相对 `rust-v0.149.0` 的完整盘点称为 `fork delta`；其中仍属于 Catalyst 产品契约的部分才称为 `Catalyst patch`。

## 当前重点入口

- Runtime 注入：`codex-rs/core/src/thread_manager.rs`、`codex-rs/core/src/client.rs`、`codex-rs/app-server/src/lib.rs`。
- HTTP transport：`codex-rs/codex-client/src/transport_handle.rs` 和 Core client 的 transport override。
- Model/TUI runtime：`codex-rs/tui/src/model_runtime.rs` 及 TUI startup/model picker 流程。
- App Server 扩展：`codex-rs/app-server/src/rpc_extension.rs`、`message_processor.rs` 和 `extensions.rs`。
- Skill provider：`codex-rs/ext/skills/src/sources.rs` 及其 catalog/tool boundary。
- Thread title：`codex-rs/thread-store/src/title_generator.rs`、`live_thread.rs`。
- 增量请求和图片迁移：`codex-rs/core/src/client.rs`、`core/src/session/turn.rs`、`core/tests/suite/incremental_http.rs`。
- Windows 隔离：`codex-rs/windows-sandbox-rs/src/desktop.rs` 及 unified-exec 调用方。

完整归类和验证入口见 `docs-cata/01-Patch地图.md`、`docs-cata/04-官方基线差异索引.md`。

## 同步和验证

- 每次同步从准确、不可变的 upstream tag 或 commit 开始，先统计完整 fork delta，再按能力重接最小 patch。
- 不使用 blanket `ours`/`theirs` 解决 core、protocol、state、security 或 app-server 冲突。
- 代码、配置、协议或 UI 行为变化必须遵循 `AGENTS.origin.md` 中对应的测试、schema、snapshot 和格式要求。
- 同步流程见 `docs-cata/02-上游同步手册.md`。当前 patch 的代码证据和边界见 `docs-cata/01-Patch地图.md`。
