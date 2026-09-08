# 敏感值分层与 Cata 测试设计

状态：设计已批准，实现与验证完成。基于 fork `a4e491828b`、Runtime `d7b095ee`；官方基线保持 0.153.4。

## 整理前已确认的问题

`codex-tui::SensitiveInput` 同时用于交互输入、Provider 请求凭据和凭据存储。整理前 Runtime 的 `catalyst-provider-core` 与 `catalyst-credentials` 因这个类型直接依赖 `codex-tui`；`CatalystControlService` 也使用该类型。基础能力因此反向依赖展示层，类型名称也不能准确表达非交互用途。

当前类型具有固定脱敏 Debug、显式 `expose_secret`、Drop 时 zeroize 的行为。官方 `RedactedString` 具有 Serialize、Clone、Deref 等不同语义且没有相同的 Drop 实现，不能直接作为等价替代。

## 方案比较

| 方案 | 收益 | 代价 |
| --- | --- | --- |
| 推荐：共享基础类型下沉 | Fork TUI 与 Runtime 使用同一个类型，消除基础层到 TUI 的依赖，无需在适配层复制敏感文本 | 增加一个很小的基础 crate；两仓修改依赖和 import |
| Runtime 使用自己的 SecretString，TUI 保留原类型 | 不增加共享 crate，Runtime 也能去掉 TUI 依赖 | 边界上保留两种敏感值表示及转换，调用方迁移范围依然存在 |

## 推荐设计

1. Fork 新增 `codex-utils-sensitive-string`，目录 `codex-rs/utils/sensitive-string/`，提供 `SensitiveString`。这是通用基础类型；源码标明 Catalyst fork 所增，不放入业务 Provider、模型策略或 UI 行为。
2. 从 TUI 原样下沉敏感值的存储、显式读取、脱敏和 zeroize 逻辑。保持既有 Debug 输出，避免额外引入 Clone、Display、Serialize 或 Deref。类型本身不负责凭据有效性和权限。
3. TUI 的宿主扩展契约集中到 `tui/src/catalyst/`，按模型 readiness、凭据交互、敏感值来源明确职责。保留现有 `codex_tui::SensitiveInput` 别名供旧源码调用方迁移；不在本批删除兼容别名。
4. Runtime 的 Provider、credentials、control 和适配层改用 `SensitiveString`。删除 `catalyst-provider-core`、`catalyst-credentials` 对 `codex-tui` 的直接依赖。TUI 状态、交互结果与领域结果的转换继续归 `tui_model_runtime` 适配层。
5. 将该适配层约 400 行测试从生产实现文件分出，保留成熟的 fixture 和行为断言。命名按职责调整，不批量给原生函数加前缀。

依赖方向为：TUI / Runtime 适配层 → credentials / provider-core → 敏感值基础类型。基础类型不依赖 TUI、Core 或 Runtime。原生 Turn/session、持久化格式、工具 wire 名称及 Workbench 均不在本批改动范围。

## Cata 测试与验收

测试按能力和 owner 组织，复用已有用例，不增加只为重命名或字段常量服务的测试。

| 层 | 具体行为 | 验收证据 |
| --- | --- | --- |
| 基础类型 | 已有脱敏 Debug 和显式读取行为在下沉后保留；使用 zeroize 的原实现 | 迁移原有测试并核验实现等价；不通过读取已释放内存测试 Drop |
| Runtime credentials | 认证拒绝保留旧凭据、暂时失败保存未验证状态、警告不泄露候选值、环境覆盖拒绝修改 | 先精确运行认证拒绝用例，再运行剩余 credentials 单测，覆盖上述行为 |
| Runtime TUI 适配 | 模型按 Provider 获取 readiness；凭据操作的结果与错误映射正确 | 运行 `tui_model_runtime::tests::` 全部 5 项适配用例 |
| Fork TUI Cata 接入 | 待凭据准备完成后才提交模型 Turn；交互结果到达时仍遵守当前模型归属 | 复用现有 readiness 测试，新增按事件顺序执行的 stale-result 用例 |
| 跨仓装配 | Runtime 实际装配仍向 Fork 注入对应服务 | TUI 入口装配用例精确运行；App Server runtime_composition 目标验证实际消费路径 |
| 依赖边界 | provider-core 与 credentials 的依赖图不再到达 codex-tui | 用 Cargo metadata/tree 核验实际图，不能只检查两个 Cargo.toml 的文本 |

依赖和锁文件同步后运行要求的 Bazel lock 更新；Bazel crate 接线随新增基础 crate 一起维护。不跑全 workspace 测试，不使用真实凭据或远程 Provider。

## 实施顺序与停止条件

先完成基础类型及 TUI 接线，再联动 Runtime import 和依赖，最后整理直接测试并执行一次最低充分验证。修改前确认两仓仍在同一升级分支且工作区干净。只处理直接阻断本批验收的问题，通过后分别提交两仓。

预计 20–30 分钟，主要成本是两仓受依赖变化影响的增量编译。若发现持久化、公开协议或凭据语义必须改变，停止扩展，单独列出证据与选择。

## 实际迁移范围

Runtime 共 7 个 crate 迁移敏感类型；provider-core、credentials、providers、Anthropic adapter、tool-search、app-server 六个纯类型消费者删除直接 TUI 依赖，catalyst-codex 保留真正的 TUI 装配依赖。Cargo resolved graph 已核验前五者无传递 TUI 路径；app-server 仍经 catalyst-codex 到达 TUI，未将整个 App Server 宣称为无 TUI 依赖。

新增 Cata 测试 `stale_model_readiness_rechecks_current_model_before_submission`：A 的结果晚到时不能放行已切到 B 的提交，B 就绪后原文和远程图片完整提交一次，重复结果不重复提交。原生生产控制流未修改。基础类型迁移原脱敏测试；Runtime 适配测试保留原逻辑名称和 fixture。

## 2026-09-08 验证记录

两位子 agent 分别检查类型迁移与调用边界、Cata 测试与兼容性；集中 review 无 Blocker / Required。生产变更保持类型行为与公开 TUI 别名，新增测试验证旧模型结果晚到后重新检查当前模型，避免错误放行和重复提交。

| 验证 | 结果 |
| --- | --- |
| Fork 最小集合：基础类型脱敏、新增模型切换竞态 | 2 passed |
| Fork 扩大集合：Cata readiness、credentials、onboarding、敏感输入 | 23 passed，排除已运行的新竞态用例 |
| Runtime credentials | 精确 1 + 其余 29 passed，无重复 |
| Runtime TUI 适配 | 5 passed |
| Runtime TUI 入口装配 | 1 passed |
| Runtime App Server runtime_composition 全目标 | 18 passed，编译 3m07s、执行 5.68s |

Fork 首次受影响编译 3m59s，扩大集合增量编译 2.03s、执行 0.21s；Runtime credentials 首次受影响编译 1m33s，适配测试编译 48.60s。编译耗时不计为用例执行耗时。

依赖图来自两仓离线 Cargo metadata 的完整 resolved graph；两个仓库各自使用本地 target。Fork formatter、Runtime 改动文件 rustfmt 和 `just bazel-lock-update` 通过，后者未产生额外锁文件差异。未运行全 workspace、真实 Provider、GUI、Windows 或发布验证；Bazel lock 更新不等于 Bazel 构建通过。

可复用的 Runtime 验证入口（仓库根目录，串行执行；整批重跑时 credentials 可一次运行全模块）：

```bash
cargo test --offline --locked -p catalyst-credentials --lib
cargo test --offline --locked -p catalyst-codex --lib tui_model_runtime::tests::
cargo test --offline --locked -p catalyst-codex --features test-support --bin catalyst-codex tests::catalyst_tui_assembly_preserves_runtime_extensions_and_optional_catalog -- --exact
cargo test --offline --locked -p catalyst-app-server --test runtime_composition
```

Fork 最小入口（仓库根目录）：

```bash
CARGO_NET_OFFLINE=true just test --locked -p codex-utils-sensitive-string -p codex-tui --lib -E 'test(=tests::sensitive_string_debug_is_redacted) | test(=chatwidget::tests::composer_submission::stale_model_readiness_rechecks_current_model_before_submission)'
```

本批合计 79 个不同用例通过（Fork 25、Runtime 54）。两仓差异检查及文档相对链接检查通过，Workbench 未修改。
