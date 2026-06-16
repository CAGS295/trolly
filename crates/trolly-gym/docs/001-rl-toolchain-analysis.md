# ADR-001: RL Training and Inference Toolchain for `trolly-gym`

| Field | Value |
|-------|-------|
| Status | Accepted |
| Date | 2026-06-16 |
| Workplan | WP-016 |
| Scope | Analysis only — no new default runtime dependencies |

## Context

`trolly-gym` (WP-011) provides a stream-fed RL scaffold:

- **Observations** — `StreamEvent` → `FeatureVector` → rolling [`ObservationWindow`](../src/observation.rs) → flattened `Vec<f32>` model input.
- **Actions** — discrete [`Action`](../src/action.rs) → [`OutboundMessage`](../../trolly-strategy/src/egress.rs) via [`StreamEgress`](../../trolly-strategy/src/egress.rs).
- **Replay** — [`ReplayBuffer`](../src/replay.rs) ring buffer of [`Transition`](../src/replay.rs) for offline/batch training.
- **Env** — [`Env::ingest_event`](../src/env.rs) / [`Env::step`](../src/env.rs) ties ingest, policy, egress, and replay.

Libtorch hooks live behind the optional `torch` feature in [`libtorch.rs`](../src/libtorch.rs). Default `cargo check --workspace` must remain free of ML runtime deps.

We need a toolchain decision covering **both**:

1. **Offline training** — batch replay sampling, GPU training, checkpoints, experiment tracking.
2. **Online inference** — sub-ms to low-ms action selection on live streams, model hot-swap, deterministic fallbacks when the policy is unavailable.

Relevant RL families for market making / execution:

| Family | Examples | Typical use |
|--------|----------|-------------|
| On-policy | PPO, A2C | Policy gradient from live or simulated rollouts |
| Off-policy | DQN, SAC, TD3 | Sample-efficient learning from replay |
| Offline / batch | CQL, BC, replay-only fine-tuning | Learn from logged transitions without live exploration |

## Constraints

| Constraint | Implication |
|------------|-------------|
| Stream-fed, latency-sensitive | Inference must not block ingest; policy eval should stay in the sub-ms–low-ms range for small MLPs on CPU |
| Backpressure | `Env` is synchronous today; bursty depth updates can pile up if training/inference shares the hot path — policy hook should be bounded-time or offloaded |
| CI default | No libtorch/GPU in default workspace check; ML features are opt-in |
| Discrete action space (stub) | Current `Action::{Hold,Buy,Sell}` maps cleanly to argmax / softmax over 3 logits; continuous control (SAC) is a future extension |
| Hot-swap + fallback | Production needs reloadable checkpoints and a rule-based fallback (e.g. `Action::Hold`) without process restart |

## Integration point map

How each candidate connects to existing `trolly-gym` surfaces:

```
StreamEvent ──► Env::ingest_event ──► ObservationWindow::push / flattened()
                                              │
                                              ▼
                                    ┌─────────────────┐
                                    │  Policy hook    │  ◄── THIS ADR
                                    │  (not yet impl) │
                                    └────────┬────────┘
                                             │ Action
                                             ▼
                              Env::step ──► Action::dispatch ──► StreamEgress
                                             │
                                             ▼
                                    ReplayBuffer::push_step
```

| Integration point | Role | Candidate touchpoint |
|-------------------|------|----------------------|
| `Env::ingest_event` | Updates window on each stream tick; must stay fast | Policy may run here (reactive) or defer to `step` |
| `ObservationWindow::flattened` | Fixed-size `f32` tensor input (e.g. 7 features × N frames) | All stacks consume this slice; no copy ideally |
| `ReplayBuffer::snapshot` / sampling | Offline batch construction | Training-only; export to Parquet/Arrow or in-proc tensors |
| `Env::step` | Action + reward + transition | **Primary inference hook** — call policy, then dispatch |
| `Action` → `StreamEgress` | Order egress | Unchanged; policy outputs `Action` or logits → argmax |
| Stream latency / backpressure | WS ingress can outpace policy | Bound inference time; drop/skip frames or use latest-window-only; never block egress on training |

## Candidate evaluation

### 1. `tch` / libtorch (current `torch` feature)

**What it is:** Rust bindings to libtorch (PyTorch C++ API). Already wired in [`libtorch.rs`](../src/libtorch.rs) with `observation_tensor` and `stub_forward`.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | Both via libtorch; CUDA/MPS depends on libtorch build |
| Install burden | **High** — download libtorch or `LIBTORCH_USE_PYTORCH=1`; linker + rpath pain; version pin with `tch` crate |
| CI feasibility | **Poor for default CI** — opt-in job only; CPU libtorch tarball adds ~500MB+ and flaky network |
| Training | Possible in Rust via `tch` autograd, but no mature Rust RL library; custom loops only |
| Inference | **Good** — sub-ms for small nets on CPU; GPU optional |
| Hot-swap | Load `tch::CModule` or `tch::jit` from disk; reload on signal |
| Deterministic fallback | Easy — wrap policy trait, fall back to `Hold` on error |

**RL algorithms:** No first-class PPO/DQN/SAC in Rust/tch. Practical path: train in Python PyTorch, export TorchScript (`.pt`) or ONNX, load in Rust.

**Integration:** `flattened()` → `observation_tensor` → forward → argmax → `Action`. Training reads `ReplayBuffer::snapshot()` externally or via export.

---

### 2. Candle (Hugging Face)

**What it is:** Rust-native ML framework with CPU/CUDA backends.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | CPU default; CUDA feature for GPU; no libtorch required |
| Install burden | **Low** — pure Cargo deps; CUDA needs toolkit for GPU build |
| CI feasibility | **Good** — CPU-only builds in CI; no external tarball |
| Training | Custom autograd loops; no RL ecosystem; suitable for small policy nets |
| Inference | **Very good** — pure Rust, predictable latency |
| Hot-swap | Load weights from safetensors/ckpt; re-init `VarMap` |
| Deterministic fallback | Same trait pattern as tch |

**RL algorithms:** Implement PPO/DQN manually or train elsewhere and import weights. Candle is stronger as inference backend than training orchestrator.

**Integration:** Copy `flattened()` to `Tensor`; forward pass; argmax. Heavier dependency than ORT for inference-only path.

---

### 3. Burn

**What it is:** Rust-native DL with pluggable backends (NdArray, WGPU, Candle, etc.).

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | NdArray (CPU), WGPU (GPU compute), optional Candle backend |
| Install burden | **Low–medium** — Cargo-only for CPU; WGPU avoids CUDA but adds GPU driver deps |
| CI feasibility | **Good** on CPU backend |
| Training | Flexible training API; still no RL library; good for bespoke small models |
| Inference | **Good** on NdArray; competitive with Candle |
| Hot-swap | Reload `Record` / module weights from file |
| Deterministic fallback | Same as above |

**RL algorithms:** Same as Candle — roll your own or train externally.

**Integration:** Similar to Candle. Burn vs Candle is largely ergonomics; neither beats ORT for deploy-only inference.

---

### 4. ONNX Runtime (`ort`)

**What it is:** Cross-platform inference engine for ONNX models.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | CPU EP default; CUDA/TensorRT EPs optional |
| Install burden | **Medium** — `ort` crate bundles or downloads prebuilt ORT; simpler than libtorch |
| CI feasibility | **Good** — CPU EP in opt-in CI job; no Python required |
| Training | **None** — inference only |
| Inference | **Excellent** — optimized kernels, low latency, batching |
| Hot-swap | Reload `Session` from new `.onnx` file; minimal downtime |
| Deterministic fallback | ORT deterministic mode + fixed seeds; fallback trait unchanged |

**RL algorithms:** N/A for training. Export trained policy (PyTorch → ONNX, or Candle → ONNX) for deployment.

**Integration:** `flattened()` → `ndarray` / direct tensor → `Session::run` → logits → `Action`. Best fit for **production inference hook** in `Env::step`.

---

### 5. Python / PyTorch sidecar or IPC bridge

**What it is:** Separate Python process (PyTorch, Stable-Baselines3, CleanRL) communicating via IPC (gRPC, Unix socket, shared memory, or stdin/JSON).

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | Native PyTorch CUDA; best training ergonomics |
| Install burden | **High ops** — Python venv, CUDA drivers, separate deploy artifact |
| CI feasibility | **Separate pipeline** — not in default `cargo check`; optional nightly GPU job |
| Training | **Best** — SB3 (PPO), CleanRL, RLlib, custom SAC/DQN; W&B/MLflow integration |
| Inference | **Poor in hot path** — IPC adds ~0.1–5ms+ per step; GIL and serialization overhead |
| Hot-swap | Reload model in sidecar; Rust client unchanged |
| Deterministic fallback | Rust-side fallback essential when sidecar is down |

**RL algorithms:**

| Algorithm | Python ecosystem | Rust-native stacks |
|-----------|------------------|-------------------|
| PPO | SB3, CleanRL, RLlib | Manual only (tch/Candle/Burn) |
| DQN / SAC / TD3 | SB3, CleanRL, d3rlpy | Manual only |
| Offline / batch (CQL, BC) | d3rlpy, RLlib | Export logs → Python train → ONNX deploy |

**Integration:** Sidecar receives flattened observation (JSON/protobuf), returns action index. **Not recommended for live inference** on stream hot path; ideal for **offline training** and batch replay export.

---

## Comparative summary

| Stack | Training | Live inference | GPU train | Default CI | Hot-swap | libtorch burden |
|-------|----------|----------------|-----------|------------|----------|-----------------|
| `tch`/libtorch | Manual / import TorchScript | Good | Yes | Opt-in only | Yes | **High** |
| Candle | Manual | Very good | Yes (CUDA) | CPU opt-in | Yes | None |
| Burn | Manual | Good | Yes (WGPU) | CPU opt-in | Yes | None |
| ONNX Runtime | No | **Best** | N/A (infer EP) | CPU opt-in | **Best** | None |
| Python sidecar | **Best** | Poor | Yes | Separate job | Yes (sidecar) | None (Rust side) |

## Recommendations

### Offline training (batch replay, checkpoints, experiment tracking)

**Primary: Python / PyTorch offline pipeline (outside default Rust binary).**

- Export transitions from `ReplayBuffer::snapshot()` (or Parquet logs from stream runs) into a Python training job.
- Use Stable-Baselines3 or CleanRL for PPO; d3rlpy or custom PyTorch for DQN/SAC/offline RL.
- Track experiments with W&B or MLflow; emit checkpoints as **ONNX** (preferred) and optionally TorchScript.
- Avoid coupling training to the live stream loop — training is batch/offline by design.

**Secondary: `torch` feature for Rust-native experimentation.**

- Keep [`libtorch.rs`](../src/libtorch.rs) for developers who want in-crate prototyping without Python.
- Not the primary training path; useful for sanity checks and small policy nets.

Candle/Burn are viable if the project later commits to **fully Rust-native training**, but they require bespoke RL loop code with no ecosystem advantage over Python today.

### Online inference (sub-ms–low-ms, hot-swap, deterministic fallback)

**Primary: ONNX Runtime (`onnx` feature — to be added in a follow-on WP).**

- Hook in `Env::step`: `flattened()` → ORT session → argmax → `Action`.
- Target <1ms for small MLP policies on CPU; profile on target hardware.
- Hot-swap: atomic replace of `Session` behind `Arc<RwLock<_>>` or reload on SIGHUP.
- **Deterministic fallback:** implement `Policy` trait; on ORT error or timeout, emit `Action::Hold` and log.

**Fallback: `torch` feature (TorchScript via `tch::CModule`).**

- For teams already shipping libtorch in prod or needing op coverage ONNX lacks.
- Same `Policy` trait; switch backend via feature flags.

**Do not use Python sidecar for live inference** on the stream action loop. Reserve IPC for offline training coordination only.

### Stream latency and backpressure

1. **Never run training backward pass on the ingest thread.**
2. **Inference in `Env::step` only** (or on a dedicated worker with latest-window snapshot) — training consumes exported replay asynchronously.
3. If inference exceeds a budget (e.g. 2ms), skip policy update for that tick and use fallback action.
4. `ReplayBuffer` prefill on ingest is cheap; cap `replay_capacity` to bound memory under backpressure.
5. For bursty venues, prefer **latest `ObservationWindow`** over queuing every tick through the policy.

## Decision

| Role | Choice | Feature gate |
|------|--------|--------------|
| **Primary training toolchain** | Python / PyTorch offline (export → ONNX) | None in Rust default build |
| **Primary inference toolchain** | ONNX Runtime (`ort`) | New `onnx` feature (follow-on WP) |
| **Optional inference fallback** | `tch`/libtorch TorchScript | Existing `torch` feature (existing) |
| **Experimental / future** | Candle or Burn | Defer; add `candle` or `burn` feature only if Rust-native training becomes a goal |
| **Explicitly not for hot path** | Python IPC sidecar | External scripts only |

**Default `cargo check --workspace`:** unchanged — no ML deps.

**Feature gates in `trolly-gym` (current + planned):**

| Feature | Deps | Purpose |
|---------|------|---------|
| _(default)_ | none | Scaffold, replay, env stepping |
| `torch` | `tch` | Libtorch tensor I/O, TorchScript inference (existing) |
| `onnx` _(planned)_ | `ort` | Production inference (follow-on WP) |

## Follow-on implementation workplans (not in WP-016)

These are proposed WPs — **not implemented here**:

| ID | Title | Scope |
|----|-------|-------|
| WP-018 | Policy trait + inference hook in `Env::step` | `crates/trolly-gym/src/policy.rs`, wire argmax → `Action` |
| WP-019 | `onnx` feature + ORT session loader | `src/onnx.rs`, hot-swap reload API |
| WP-020 | Replay export for offline training | Parquet/JSON export from `ReplayBuffer::snapshot` |
| WP-021 | Python training scripts + ONNX export | `crates/trolly-gym/python/` or repo `scripts/rl/` |
| WP-022 | Opt-in CI jobs (`torch`, `onnx` CPU) | `.github/workflows/` |
| WP-023 | Deterministic fallback policy + timeout budget | Integrate with `Env::step`, metrics |
| WP-024 | Continuous action space (SAC) extension | Extend `Action` or separate action head |

## Consequences

- Default builds and CI stay ML-free; production inference adds optional `onnx` feature.
- Training investment goes to Python ecosystem + export pipeline, not Rust RL frameworks.
- `torch` remains for dev/prototyping and TorchScript fallback, not primary training.
- Candle/Burn deferred unless project direction changes toward all-Rust training.
- Live stream path stays in Rust with bounded-time ORT inference and explicit fallbacks.

## References

- [`trolly-gym` README](../README.md)
- [PyTorch libtorch](https://pytorch.org/get-started/locally/)
- [Candle](https://github.com/huggingface/candle)
- [Burn](https://github.com/tracel-ai/burn)
- [ONNX Runtime](https://onnxruntime.ai/)
- [Stable-Baselines3](https://stable-baselines3.readthedocs.io/)

)
