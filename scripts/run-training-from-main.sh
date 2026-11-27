#!/usr/bin/env bash
# Host cron entry: sync training code from main, then run the local GPU trainer prompt.
set -euo pipefail

REPO="${TROLLY_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
REMOTE="${TROLLY_REMOTE:-origin}"
BRANCH="${TROLLY_BRANCH:-main}"
LOG="${TROLLY_LOG:-$HOME/logs/trolly-train.log}"
LOCK="${TROLLY_TRAIN_LOCK:-/tmp/trolly-train.lock}"
PROMPT="${TROLLY_TRAINER_PROMPT:-$REPO/docs/trainer-local-gpu-loop.md}"

mkdir -p "$(dirname "$LOG")"
exec >>"$LOG" 2>&1
echo "=== $(date -Is) run-training-from-main ==="

exec 9>"$LOCK"
flock -n 9 || { echo "Another training run in progress"; exit 0; }

cd "$REPO"
git fetch "$REMOTE" "$BRANCH"
git checkout "$BRANCH"
if git diff --quiet && git diff --cached --quiet; then
  git pull --ff-only "$REMOTE" "$BRANCH"
else
  echo "WARN: dirty working tree on $BRANCH; skipping pull --ff-only"
fi

if [[ -n "${TROLLY_TRAIN_DURATION_SECS:-}" ]]; then
  export TROLLY_TRAIN_DURATION_SECS
fi

if command -v agent >/dev/null 2>&1 && [[ -f "$PROMPT" ]]; then
  exec agent -p --force --trust --workspace "$REPO" -f "$PROMPT"
elif [[ -x "$REPO/crates/trolly-gym/scripts/continue-training-locally.sh" ]]; then
  exec "$REPO/crates/trolly-gym/scripts/continue-training-locally.sh" --continue
else
  echo "ERROR: no agent CLI and no continue-training-locally.sh"
  exit 1
fi