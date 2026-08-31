# Catalyst ↔ openai/codex 稳定版同步运行手册

这是一份可重复使用的同步手册。每次把 Catalyst fork 合并到新的
`openai/codex` 稳定版时，以本文件为流程基线，并在
`docs/superpowers/plans/YYYY-MM-DD-handoff-round<N>.md` 留下本轮事实记录。
本文件描述不变的流程；日期 handoff 只记录会变化的 commit、冲突和测试结果。

## 0. 成功标准与不可变原则

- 同步目标必须是明确的 release tag（优先）或经维护者确认的 upstream commit，不能默默跟随移动的 `main`。
- 每轮操作前创建可恢复 tag；完成后保留最终 tag，删除临时 scratch 文件和失败尝试产生的临时分支。
- Catalyst 行为契约优先于“机械接受 ours/theirs”。涉及安全、权限、上下文、持久化和 API 的冲突必须逐项说明不变量。
- 任何新增或改变 agent 行为都要有集成测试；UI 改动必须有 insta snapshot。
- 不修改 `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` 或 `CODEX_SANDBOX_ENV_VAR` 相关代码，也不把环境/网络阻塞误报为源代码回归。

## 1. 同步前基础设施检查

在开始 rebase/merge 前，在干净或已明确记录的工作树中确认：

```text
git --version                 # >= 2.40，支持 rebase/restore 所需功能
rustc --version && cargo --version
just --version
bazel --version               # 若仓库任务使用 Bazel
cargo insta --version         # TUI snapshot；缺失时安装 cargo-insta
```

Rust toolchain 应以仓库 `rust-toolchain.toml` 为准。若工具缺失，先安装并记录版本，
不要在未完成安装时进入冲突解决。确认可访问 crates.io、GitHub release artifact
（尤其 rusty-v8）以及上游 Git remote；必要时换到具备网络的执行环境。网络失败时保留
错误摘要，但不要为了让测试“通过”而改变网络禁用检查。

当前仓库的具体 pins 为：Rust `1.95.0`（`codex-rs/rust-toolchain.toml`）、Bazel
`9.0.0`（`.bazelversion`）、Bazelisk `1.28.1`（CI）。测试命令依赖
`cargo-nextest`，TUI snapshot 依赖 `cargo-insta`；Linux 还应按 CI 准备
`pkg-config`、`libcap-dev`、`bubblewrap`，musl/Bazel 路径可能额外需要 clang/lld
等工具。Python 辅助脚本要求至少 3.10，建议使用 3.11/3.12。基础设施 gate
应记录版本、安装路径、缓存位置和失败原因，再进入源码迁移。

同时盘点全局配置与缓存：Rustup toolchains、`~/.cargo/config*`、Cargo
registry/git 缓存、Bazel output/cache，以及 rusty-v8 归档。测试和构建尽量使用
任务专用的 `CARGO_HOME`/Bazel output root，或先确认全局缓存来源可信且版本匹配；
不要清空或覆盖用户的全局缓存。任何 handoff 只能记录脱敏后的路径类别、版本和
命中/未命中结果，禁止写入凭证、token、代理 URL 或完整配置内容。
若全局缓存中存在 provider/account/connected-providers 等类别，视为敏感数据，
只记录“存在”这一事实，禁止读取、搬运或清理；为本次同步单独设置缓存根目录。

注意：仅设置 `CARGO_TARGET_DIR` 不能隔离 Cargo 的 git 依赖缓存；如果
`CARGO_HOME` 仍指向全局目录，依赖更新仍会把 packfile 写入根盘。测试前应同时
把 `CARGO_HOME` 指向任务专用目录，并只以只读/链接方式复用已核验 registry；若一次
失败尝试在全局 git cache 产生明确的临时 packfile，先记录来源，再只清理该次产生的
目录，不能泛化清空全局缓存。

即使 `CARGO_HOME` 使用 tmpfs，也要为 registry `src`/`cache` 预留空间；`just test`
会先对整个 workspace 做 all-features metadata，可能在真正编译前解压大量平台无关
crate。若 tmpfs 不足，应停止并保留失败原因，不要让 Cargo 自动回写根盘或修改
`Cargo.lock`；将意外的 lockfile 改写恢复后再继续。

离线模式只能作为快速缓存完整性探针：它应在缺少 git checkout 时快速失败，不能被
当作测试通过或源码回归。确认依赖齐全后，再在有足够磁盘的环境中执行正常联网测试。

建议恢复环境后的最小检查顺序：

```text
rustup toolchain install 1.95.0 --profile minimal
rustup component add clippy rustfmt rust-src --toolchain 1.95.0
cargo install --locked cargo-nextest cargo-insta
just --version && cargo nextest --version && cargo insta --version
bazel --version                         # 应解析到 Bazel 9.x
```

如果宿主机默认 `python3` 低于 3.10，应在运行 `just` 前把任务专用 Python
放在 `PATH` 前面（例如 `PATH=/tmp/codex-python/bin:$PATH`）。`just fmt` 还会调用
仓库要求的 `dotslash` 和 `uv`；缺失时应记录 formatter 阶段阻塞，不能把只完成
Rust rustfmt 误报为完整格式化通过。

安装命令需在可写的用户环境执行；若使用 CI 或容器，优先复用仓库 CI 中
已固定版本的安装 action，而不是依赖宿主机的漂移版本。

先运行轻量基线（不直接调用 `cargo test`）：

```text
cd codex-rs
cargo fmt --all -- --check
just test -p codex-thread-store
just test -p codex-state
just test -p codex-api
just test -p codex-protocol
```

记录工具版本、每个 crate 的通过/失败/阻塞数量。若修改了共享 crate，完整 `just test`
需要在获得维护者确认后执行。

## 2. 识别目标并建立安全点

```text
git fetch upstream --tags
git show <stable-tag> --no-patch --decorate
git merge-base HEAD <stable-tag>
git tag pre-sync-YYYYMMDD
git branch sync/YYYYMMDD-<stable-tag> HEAD
```

把目标 tag、解析出的 commit、共同祖先、当前 HEAD 和 fork patch 数量写入 handoff。
若只能使用 `upstream/main`，先固定其 SHA 并由维护者确认；后续所有命令均引用 SHA/tag，
避免期间 remote 移动造成不可复现结果。

若 `git merge-base HEAD <stable-tag>` 等于 stable tag 的 peeled commit，且
`git merge-base --is-ancestor <stable-tag> HEAD` 成功，则说明当前分支已经以该
稳定版为基线；此时不要重复 cherry-pick 同一批 upstream commits，应转为审计
`<stable-tag>..HEAD` 的 Catalyst delta，并把此前的试迁移记录标为历史分析。

## 3. 建立 patch 清单和测试安全网

以共同祖先为范围生成清单，并按行为域分组，而不是按文件名分组：transport、model/TUI、
skills/extensions、incremental request、compaction、platform、thread metadata 等。
每组记录提交范围、触碰的高风险文件、上游可能替代的功能、迁移顺序和回滚点。

同步前先补齐 fork 新增行为的测试，至少覆盖：

- thread-store transaction/checkpoint/title/metadata durability；
- state/runtime SQL migration 和旧数据 round-trip；
- client wire-level incremental request、image cache、过期 response-id fallback；
- compaction commit/abort/replay/recovery；
- app-server v2 RPC、CLI 入口、extension/native-agent 公共路径；
- TUI provider/model onboarding snapshots。

测试应比较完整对象，测试模块放在独立 `*_tests.rs`（新模块），并使用仓库规定的
`TestCodexBuilder::build_with_auto_env()` / `TestAppServer::new_with_auto_env()`。

## 4. 分阶段迁移顺序

每阶段都遵循：创建 `pre-round-N` tag → 只处理一个行为组 → 解决冲突并运行该组测试 →
写 handoff → 再进入下一阶段。

1. **低风险**：平台修复、thread title、image extension、error classification、reasoning、housekeeping。
2. **Transport + Model runtime**：先 client/core transport threading，再 TUI model catalog、provider onboarding。
3. **Skills + Extensions**：host skills、mandatory instructions、RPC registry、native agent、plugin roots；重新检查上游安全边界。
4. **Incremental requests**：按 `previous_response_id`、baseline、image cache、byte estimation、fallback、rollback 的顺序逐提交迁移，并做 wire-level 断言。
5. **Compaction transactionality**：最后处理 compact 配置、checkpoint、持久化生命周期和 replay；每个提交先写不变量，再移植代码。

Compaction 的提交顺序固定为：prepare window → 在同一 session lock 内做
CAS 校验 → 单批 append rollout（失败立即返回）→ 更新 live history、world-state
baseline 和 window → 再发送 `RawResponseCompleted`/completion trace。provider 已经
消耗的 rollout budget 可在历史提交前记账，但不得重复记账。测试设施应提供 stale
window、append failure 和 cold-resume 三类可确定注入点；禁止用不可靠的网络延迟竞态
替代 commit-pause hook。

若高风险组与上游重构高度交织，停止逐提交 rebase，记录原因后采用“先合并上游、再按行为组重新应用补丁”的方案；不得在未记录决策的情况下混用两种历史策略。

## 5. 每阶段验收矩阵

先运行受影响 crate 的 `just test -p <crate>`，再按依赖顺序运行 protocol/state/store → client/core → app-server → tui。
Rust 代码变更结束后自动运行 `just fmt`；大型 Rust 变更在最终提交前运行对应的
`just fix -p <project>`，且 fix/fmt 后不重复测试。检查：

- `cargo insta pending-snapshots -p codex-tui` 并审阅每个 `.snap.new`；
- Config/API 形状变化时运行 `just write-config-schema` / `just write-app-server-schema`；
- Cargo 依赖变化时运行 `just bazel-lock-update`；
- API、CLI、配置、rollout resume、raw response item 进行 breaking-change review；
- 明确区分上游既有失败、环境阻塞和本轮新增失败。

## 6. 交接记录模板

复制以下模板为新的日期文件；每轮结束时更新 `AGENTS.md` 的 Upstream Sync Project Handoff
或链接到该文件：

```markdown
# Handoff — YYYY-MM-DD Round N

## Branch State
- Branch / HEAD:
- Stable target (tag + SHA):
- Common ancestor:
- Safety tag:
- Completed groups / remaining groups:

## Changes and Conflict Decisions
- Migrated commits or behavior groups:
- Conflicts and invariant-based resolutions:
- Upstream behavior intentionally retained/dropped:

## Validation
- Commands and pass/fail/blocked results:
- Snapshot/schema/lockfile status:
- Known upstream-only failures:

## Blockers and Exact Next Action
- Human/network/toolchain blockers:
- Next command or next round:
```

## 7. 最终清理与可复现交付

完成全部阶段后，确认工作树只包含预期源码、测试、schema/lockfile 和最终沉淀文档：

```text
git status --short
git diff --check
git log --oneline <stable-tag>..HEAD
```

若使用可重建的任务专用存储，清理前先确认挂载点和剩余空间；若存储不可用，先提醒维护者
准备替代环境，再进行任何构建。机器专属路径、缓存链接和配置只应记录在本地忽略文件中，
不得上传。卸载任务存储前，应恢复工具默认路径或重新建立兼容链接。

删除 `.rej`、临时 patch、冲突备份、未采用的 snapshot、调试日志和临时分支；保留
`pre-sync-*`/`pre-round-*` 安全 tag（除非维护者要求删除）。不要把命令输出、凭证、网络
代理或本机路径写入文档。最终 handoff 必须包含最终 HEAD、目标 SHA、每组处理方式、测试结果、
已知阻塞和下一次同步可直接执行的第一步。

## 8. 下一次同步的最短入口

新会话首先阅读本运行手册、最近一次日期 handoff 和 `AGENTS.md` 的 handoff 段落，然后执行：

1. 检查工具链与工作树；
2. fetch tags 并确认新的稳定 target；
3. 创建 `pre-sync-YYYYMMDD` 和同步分支；
4. 从 patch 清单和测试基线开始，不跳过安全网；
5. 按第 4 节顺序推进，并在每轮结束写 handoff。

仓库还提供只读 preflight：

```text
bash scripts/upstream-sync-preflight.sh rust-v0.151.0
```

它检查工具链、stable ancestry、`cargo metadata --no-deps --locked --offline`、
`git diff --check` 和残留过程文件；不会 fetch、安装工具、改写配置或清理全局缓存。
脚本还检查 `cargo-nextest`、`cargo-insta` 以及 Python 3.10+；可通过
`SYNC_PYTHON_BIN=/path/to/python3.11` 指定任务级 Python。`dotslash`/`uv` 只作
可选工具报告，不会被脚本悄悄安装。

在具备足够资源的执行环境中的第一轮命令建议为（将 `<cache-root>` 替换为该环境的任务缓存根目录）：

```text
df -h <cache-root>
SYNC_PYTHON_BIN=/usr/bin/python3.11 bash scripts/upstream-sync-preflight.sh rust-v0.151.0
cd codex-rs
CARGO_TARGET_DIR=<cache-root>/target CARGO_BUILD_JOBS=2 just test -p codex-thread-store
CARGO_TARGET_DIR=<cache-root>/target CARGO_BUILD_JOBS=2 just test -p codex-core
CARGO_TARGET_DIR=<cache-root>/target CARGO_BUILD_JOBS=2 just test -p codex-app-server
```

若任务缓存存储未准备好，停止执行并先创建/挂载替代存储；不要让 Cargo 回退到系统盘。
