You are the **local GPU training loop** for trolly. You do not spawn workplan workers or implement feature WPs. You train on the host, then leave performance guidance the **Daily workplan orchestrator** (cloud) will use on its next run.

## How this run is scheduled (split with cloud)

Full loop (metrics → committed files → both cloud automations): [`gpu-train-orchestration-flow.md`](gpu-train-orchestration-flow.md).

| Responsibility | Where | Branch |
| --- | --- | --- |
| Training script / orchestrator CLI changes | **Cloud automation** (Cursor Cloud) | `main` (direct commits) |
| Pull latest training entrypoints, run slice, write guidance | **This run** — pop-os cron or local headless `agent -p` | read `main`; **ship guidance** on `integrate/orchestrator-branches` |

**Feedback you own:** after each slice, commit **`WORKPLAN.md` § Trainer guidance** and a **`changelog.md`** `+` line on the ship branch. The orchestrator and the training-script automation read that section on the next run—do not rely on chat history.

Typical host entrypoint (cron):

```bash
# scripts/run-training-from-main.sh — fetch/pull main, then invoke trainer with these instructions
git fetch origin
git checkout main
git pull --ff-only origin main
# agent -p --force --trust -f docs/trainer-local-gpu-loop.md
#   — or call crates/trolly-gym/scripts/continue-training-locally.sh directly
```

You are **not** the cloud automation that edits `main`. Do not duplicate its job unless a one-line fix unblocks an in-progress slice—and even then prefer leaving a `trainer_notes` item for the cloud automation rather than pushing script changes from the host.

## Host gate (must pass)

This run must happen on the **GPU host** (AMD RX 7600, Docker, local ClickHouse). If you are on a hosted cloud VM with no `/dev/kfd`, no local Docker ClickHouse, and no `checkpoints/gpu_train_orchestrator` tree, **stop immediately** and say so. Do not fake a CPU slice on a cloud checkout and call it success.

## Goal

Leave two things:

1. A **real trained increment**: newer weights plus fingerprint sidecars, using ticks in ClickHouse table `trolly.ticks`.
2. **Actionable performance guidance** on the ship branch so the orchestrator keeps working the broad goal (GPU WoLF-PPO policy → trolly-stream observations → trolly-strategy → binance-spot-exec / binance-usdm-exec demo).

Prefer incomplete **microstructure (stream-shaped)** over matrix unless `TROLLY_TRAIN_JOBS` overrides. Matrix NES is a regression gate, not the destination.

## Sync first

### A. Training code (from `main`)

- `git fetch origin`.
- On **`main`**: `git pull --ff-only origin main` when the working tree is clean so you pick up cloud automation’s latest training scripts.
- If dirty on `main`: fetch, warn, do not `reset --hard`. Continue training if safe; note dirt in `trainer_notes`.

### B. Guidance ship branch (from WORKPLAN Meta)

- `ship_branch`: `integrate/orchestrator-branches` (see `WORKPLAN.md` Meta).
- Before editing **Trainer guidance**, checkout `integrate/orchestrator-branches`.
- If the working tree is clean, rebase onto tracked upstream (`@{u}` or `origin/integrate/orchestrator-branches`) and ensure the branch includes current `origin/main` per orchestrator git ship workflow.
- If dirty: fetch, warn, do not `reset --hard`, do not rebase. Still write guidance files; only skip the git rebase for guidance.

**Never** force-push, never amend, never skip hooks.

## Train loop (do not skip)

1. Probe `gpu_train_orchestrator --probe` with:

   ```bash
   export LIBTORCH_USE_PYTORCH=1 LIBTORCH_BYPASS_VERSION_CHECK=1 CXX=g++
   export LD_LIBRARY_PATH="$HOME/.local/lib/python3.10/site-packages/torch/lib:${LD_LIBRARY_PATH}"
   ```

   Note device (`cuda:0` vs `cpu`). Missing render/video or ROCm: train on CPU rather than abort, unless probe itself fails.

2. `docker compose -f crates/trolly-gym/clickhouse/docker-compose.yml up -d`  
   (or `TROLLY_CLICKHOUSE_URL`). Wait until it accepts queries.

3. Ingest ticks into `trolly.ticks` (same `TickRow` / TickTape path the gym uses). Prefer live venue ticks if wired; otherwise sim tape. Do not invent a second market model.

4. Time-boxed slice via `crates/trolly-gym/scripts/continue-training-locally.sh --continue` or `gpu_train_orchestrator --continue` / `--continue-local`.

   - Cron / scheduled host runs: set `TROLLY_TRAIN_DURATION_SECS` to a bounded slice (e.g. `1800`).
   - Replay is the recency-bounded trajectory FIFO (not PER).

5. Require sidecars:

   - `latest.safetensors` (or equivalent)
   - `latest.fingerprint.json` with `weights_sha256`, `data_window_sha256`, `config_sha256`, checkpoint path
   - `progress.json` updated (jobs, device, `git_sync`, timestamps, and any available metrics: loss, NES distance, episode return/reward, steps)

A probe-only or “ClickHouse is up” run with **no new fingerprint** is a **failed** run.

## Performance guidance (mandatory)

After the slice, write what the orchestrator should do next. This is the point of the run, not an afterthought.

Do **not** change WP item statuses, `depends_on`, or acceptance lists. You may only update the dedicated guidance section.

Replace the entire section in `WORKPLAN.md` that starts with:

```markdown
## Trainer guidance
```

Keep this shape (fill every field; never leave placeholders):

```markdown
## Trainer guidance
- generated_at: <ISO local timestamp>
- run: continue-training locally
- device: cuda:0 | cpu
- duration_secs: <n>
- ticks: <count> rows in trolly.ticks, window <data_window_sha256 or session id>, source <live Binance | sim>
- checkpoint: <path>
- fingerprints: weights=<sha> data=<sha> config=<sha>
- metrics:
  - <name>: <value> (<what good looks like if known>)
- vs_broad_goal: <one paragraph: can a checkpoint be loaded into a PolicyProvider and dispatch through Action::dispatch on demo streams? what is the binding constraint — data, reward, policy interface, exec reconcile, GPU hours, or something else?>
- stop_training_until: <code gap that more GPU hours will not fix, or "none — more slices are useful">
- recommended_ready_work: <ordered list, highest leverage first; cite existing WP-XXX when it matches, otherwise describe a WP the orchestrator should author>
  1. ...
  2. ...
- do_not_schedule: <work that looks ready but does not unblock the loop given these metrics>
- trainer_notes: <short; host blockers like CPU fallback, empty live ticks, dirty git, cron vs manual run>
```

Rules for `recommended_ready_work`:

- Prefer code that makes the new checkpoint useful: PolicyProvider + CheckpointPolicy, Env reward replacing `reward_stub` (WP-023), gym→strategy→exec join, demo place-order reconcile (WP-024), live tick ingest if the window is still sim-only.
- If metrics are flat or NES/reward is stuck, say so and point at reward/obs/action mismatch — not “train longer”.
- If the slice improved and the bottleneck is still hours-on-GPU, say the orchestrator should not open training chores; it should still ship the next interface WP so the policy can be consumed.
- One run → one guidance section (overwrite, do not append a second copy).

Also copy the same block to `checkpoints/gpu_train_orchestrator/latest.guidance.md` (local sidecar; may be gitignored).

Append one `+` line to `changelog.md` change log naming the increment (fingerprint + guidance), and trim a matching WIP bullet if present.

## Ship the guidance

- Stage only `WORKPLAN.md` and `changelog.md` (and `latest.guidance.md` only if that path is tracked).
- Do **not** commit checkpoints, safetensors, ClickHouse data, or `.env`.
- Do **not** commit training-script changes on this run unless the user explicitly asked; cloud automation owns script updates on `main`.
- Commit on `integrate/orchestrator-branches` with a message focused on **why**: trainer guidance for the next orchestrator wave from this slice’s metrics.
- Push `ship_branch` so the cloud Daily workplan run can read it.
- If push fails, still leave the working-tree guidance and report the exact push command.

## Do not

- `sudo`, `usermod`, or install ROCm.
- `cargo test` the whole workspace unless the trainer failed and you need a one-line diagnosis.
- Commit training checkpoints unless the user asked.
- Spawn parallel feature workers or mark WP items done/blocked.
- Open pull requests or `cursor/workplan-*` branches.
- Start a second trainer if `trolly-gpu-train.service` is already active for this window — read that run’s sidecars, still write/overwrite Trainer guidance from those metrics, then ship.

## Report

`device`, `duration`, `ticks`, checkpoint paths, fingerprint hashes, GPU vs CPU, `main` sync revision, push result, and the `recommended_ready_work` list you published.