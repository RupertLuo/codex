#!/usr/bin/env bash
set -euo pipefail

# Read-only preflight for the Catalyst ↔ openai/codex sync workflow.
# It intentionally does not fetch, install, mutate config, or clean caches.

repo_root=$(git rev-parse --show-toplevel)
stable_tag=${1:-rust-v0.151.0}
stable_commit=$(git rev-list -n 1 "$stable_tag^{commit}")

printf 'branch: '; git branch --show-current
printf 'head: '; git rev-parse HEAD
printf 'stable_tag: %s\n' "$stable_tag"
printf 'stable_commit: %s\n' "$stable_commit"

for command_name in git rustc cargo just; do
    command -v "$command_name" >/dev/null || {
        printf 'missing_tool: %s\n' "$command_name" >&2
        exit 2
    }
done

rustc --version
cargo --version
just --version

if ! git merge-base --is-ancestor "$stable_commit" HEAD; then
    printf 'stable_ancestry: fail\n' >&2
    exit 3
fi
printf 'stable_ancestry: pass\n'

if ! git diff --check; then
    printf 'diff_check: fail\n' >&2
    exit 4
fi
printf 'diff_check: pass\n'

if ! (cd "$repo_root/codex-rs" && cargo metadata --no-deps --locked --offline --format-version 1 >/dev/null); then
    printf 'locked_metadata: fail\n' >&2
    exit 5
fi
printf 'locked_metadata: pass\n'

for suffix in rej orig snap.new; do
    if find "$repo_root" -type f -name "*.$suffix" -print -quit | grep -q .; then
        printf 'process_artifacts: fail (%s)\n' "$suffix" >&2
        exit 6
    fi
done
printf 'process_artifacts: pass\n'

df -h "$repo_root"
printf 'preflight: pass (compile/test still require adequate disk and dependency cache)\n'
