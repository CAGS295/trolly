#!/usr/bin/env bash
# Drive the local continue-training / weekday GPU loop.
# Always git-fetch first so this host picks up commits cloud agents pushed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

# Fetch origin, then rebase onto the tracked upstream when the tree is clean.
# Never reset --hard, never rebase -i, never --no-verify. Dirty tree: fetch + warn.
sync_repo_with_upstream() {
  if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "warn: not a git work tree; skipping fetch" >&2
    return 0
  fi
  if ! git fetch origin; then
    echo "warn: git fetch origin failed; continuing with local tree" >&2
    return 0
  fi
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "warn: uncommitted changes; fetched but skipping rebase so local work is not destroyed" >&2
    return 0
  fi
  if ! git rev-parse --abbrev-ref --symbolic-full-name '@{u}' >/dev/null 2>&1; then
    echo "warn: no upstream tracking branch; fetched but skipping rebase" >&2
    return 0
  fi
  if ! git rebase '@{u}'; then
    echo "warn: git rebase @{u} failed; aborting rebase and continuing with local tree" >&2
    git rebase --abort >/dev/null 2>&1 || true
    return 0
  fi
}

sync_repo_with_upstream

MODE="--continue"
case "${1:-}" in
  --once|--continue|--continue-local|--daemon|--probe) MODE="$1" ;;
  "") ;;
  *) echo "unknown argument: $1 (expected --once|--continue|--continue-local|--daemon|--probe)" >&2; exit 2 ;;
esac

export LIBTORCH_USE_PYTORCH="${LIBTORCH_USE_PYTORCH:-1}"
export LIBTORCH_BYPASS_VERSION_CHECK="${LIBTORCH_BYPASS_VERSION_CHECK:-1}"
export TROLLY_TRAIN_DURATION_SECS="${TROLLY_TRAIN_DURATION_SECS:-120}"
TORCH_LIB="${HOME}/.local/lib/python3.10/site-packages/torch/lib"
if [[ -d "$TORCH_LIB" ]]; then
  export LD_LIBRARY_PATH="${TORCH_LIB}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
CH_URL="${TROLLY_CLICKHOUSE_URL:-http://127.0.0.1:8123}"
CH_PING="${CH_URL%/}/ping"
COMPOSE="$ROOT/crates/trolly-gym/clickhouse/docker-compose.yml"
CLICKHOUSE_WAIT_SECS="${TROLLY_CLICKHOUSE_WAIT_SECS:-180}"

clickhouse_ping_ok() {
  curl -fsS --connect-timeout 2 --max-time 5 "$CH_PING" >/dev/null 2>&1
}

wait_for_clickhouse() {
  local attempts=$((CLICKHOUSE_WAIT_SECS * 2))
  local n=0
  while (( n < attempts )); do
    if clickhouse_ping_ok; then
      echo "ClickHouse reachable at $CH_URL (after $((n / 2))s)" >&2
      return 0
    fi
    sleep 0.5
    ((n++)) || true
  done
  return 1
}

clickhouse_diagnostics() {
  echo "error: ClickHouse is not reachable at $CH_URL after ${CLICKHOUSE_WAIT_SECS}s" >&2
  if command -v docker >/dev/null 2>&1; then
    docker compose -f "$COMPOSE" ps >&2 || true
    docker logs trolly-clickhouse --tail 40 >&2 || true
  fi
}

if ! clickhouse_ping_ok; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "error: ClickHouse is not reachable at $CH_URL and docker is not available" >&2
    exit 1
  fi
  echo "starting ClickHouse via docker compose..." >&2
  if ! docker compose -f "$COMPOSE" up -d --wait --wait-timeout "$CLICKHOUSE_WAIT_SECS" 2>/dev/null; then
    docker compose -f "$COMPOSE" up -d
  fi
fi
if ! wait_for_clickhouse; then
  clickhouse_diagnostics
  exit 1
fi
if [[ -x "$ROOT/target/release/gpu_train_orchestrator" ]]; then
  exec "$ROOT/target/release/gpu_train_orchestrator" "$MODE"
fi
exec cargo run -p trolly-gym --features torch --bin gpu_train_orchestrator -- "$MODE"
