# ADR: RL training and inference toolchain for `trolly-gym`

| Field | Value |
|-------|-------|
| Status | Accepted (WP-016) |
| Date | 2026-06-13 |
| Scope | `crates/trolly-gym` — stream-fed RL over `trolly-stream` / `trolly-strategy` |
| Supersedes | WP-011 “toolchain choice deferred” note |

## Context

`trolly-gym` (WP-011) provides a **stream-fed RL scaffold**: normalized market events become feature vectors, a rolling observation window feeds policy decisions, discrete actions dispatch through `trolly-strategy` egress, and a ring replay buffer records transitions. Libtorch hooks exist behind the optional `torch` feature; the default workspace build must not link libtorch.

This document compares candidate ML stacks for **offline training** and **live inference** on that scaffold, maps each to concrete integration points, and records a toolchain decision plus follow-on work items. **No training loop, checkpoints, or new default dependencies** are introduced here.

### Problem statement

Market-making and execution RL needs two distinct pipelines:

1. **Offline training** — batch updates from replay (or exported logs), experiment tracking, GPU batching, checkpointing. Latency tolerance: seconds to hours.
2. **Online inference** — sub-millisecond to low-millisecond action selection on every depth or account tick, with model hot-swap and deterministic fallbacks when inference fails or lags. Must not stall the stream ingest path.

A single stack rarely optimizes both. The decision therefore splits **training** from **production inference**, with feature gates keeping CI lean.

### Current integration surface (WP-011)

```
trolly-stream ingress
        │
        ▼
  Env::ingest_event / ingest_message
        │
        ├─► ObservationWindow::push → flattened Vec<f32>
        ├─► ReplayBuffer::push_observation_window
        │
  Env::step(Action)
        │
        ├─► Action::dispatch → StreamEgress → OutboundMessage
        ├─► ReplayBuffer::push_step
        └─► StepResult { observation, reward, done }
```

| Component | Role | ML touchpoint |
|-----------|------|---------------|
| [`Env`](../src/env.rs) | Symbol-filtered ingest, step, egress | Policy calls `step` or a future `Policy::act` between ingest and dispatch |
| [`ObservationWindow`](../src/observation.rs) | Rolling frames → `flattened()` `Vec<f32>` | Universal model input tensor |
| [`ReplayBuffer`](../src/replay.rs) | Ring buffer of `Transition` | Offline export, batch sampling, sidecar training feed |
| [`Action`](../src/action.rs) | Discrete {Hold, Buy, Sell} → `OutboundMessage` | Policy output head (logits → argmax / sample) |
| [`libtorch`](../src/libtorch.rs) | `observation_tensor`, `stub_forward` | Existing `torch` feature hook |

### Stream latency and backpressure

- **Ingest is on the hot path.** `ingest_event` runs synchronously when the multiplexor delivers a message. Feature extraction (`features_from_event`) and window push must stay allocation-bounded and avoid GPU sync.
- **Inference belongs off the ingest thread** or must be bounded. Recommended pattern: ingest updates `last_observation` atomically; a strategy task or dedicated inference thread reads the latest window and posts an `Action` to `step`. Never block websocket read on `forward()`.
- **Backpressure.** If inference cannot keep pace with tick rate, coalesce to latest observation (drop stale frames), fall back to `Action::Hold` or a rule-based policy, and emit metrics on inference queue depth.
- **Egress coupling.** `Action::dispatch` is synchronous today. Order placement (WP-014/015) adds network RTT outside the gym; the policy loop budget is inference + dispatch enqueue only.

---

## Candidates evaluated

### Summary matrix

| Stack | Training | Live inference | GPU | Default CI | Install burden | RL ecosystem |
|-------|----------|----------------|-----|------------|----------------|--------------|
| **tch / libtorch** (`torch` feature) | Strong | Good (in-process) | CUDA, CPU | Poor (needs libtorch) | High — `LIBTORCH` or PyTorch wheel | DIY in Rust; full PyTorch if training external |
| **Candle** | Moderate (DIY) | Good (small MLPs) | CUDA, CPU | Good (pure Rust) | Low — crates.io | Immature — implement PPO/DQN yourself |
| **Burn** | Moderate (DIY) | Good | WGPU, CUDA, Metal | Good (pure Rust) | Low — crates.io | Immature — no SB3 equivalent |
| **ONNX Runtime (`ort`)** | None (export only) | Excellent | CUDA, TensorRT, CPU | Good (prebuilt ORT) | Low — `ort` crate + model file | Train elsewhere; deploy ONNX |
| **Python / PyTorch sidecar** | Excellent | Moderate (IPC latency) | Full PyTorch | Good (Rust CI separate) | Medium — Python env + IPC | Stable-Baselines3, CleanRL, RLlib, etc. |

---

### 1. tch / libtorch (`torch` feature)

**What it is.** Rust bindings to libtorch via the optional `tch` dependency. Already wired in [`libtorch.rs`](../src/libtorch.rs) behind `#[cfg(feature = "torch")]`.

**GPU / CPU.** CUDA and CPU backends match PyTorch. MPS on Apple via libtorch builds.

**Install burden.** High. Developers need a libtorch distribution (`LIBTORCH` path) or `LIBTORCH_USE_PYTORCH=1` with a local PyTorch install. CI must download ~2 GB artifacts or skip `torch` tests (current approach). Cross-compiling for deployment images is painful.

**CI feasibility.** Keep **feature-gated**; default `cargo check --workspace` passes without libtorch. Optional nightly or manual job with `cargo test -p trolly-gym --features torch`.

**Integration mapping.**

| Hook | Fit |
|------|-----|
| `ObservationWindow::flattened()` | `observation_tensor()` already converts to `tch::Tensor` |
| `ReplayBuffer` | Manual batching into `Tensor` stacks; no built-in DataLoader |
| `Env::step` | Inline `forward` + argmax → `Action` |
| Egress | Same process — lowest IPC overhead |
| Latency | CPU forward for small MLP: ~0.05–2 ms; GPU kernel launch may dominate for tiny models |

**RL algorithms.**

| Family | Support |
|--------|---------|
| PPO (on-policy) | Implement in Rust or train in Python and load `torch.jit` / ONNX |
| DQN / SAC (off-policy) | DIY — no mature Rust RL lib on tch |
| Offline / batch from replay | Feasible with custom training loop |

**Verdict.** Best for **in-crate prototyping** when researchers already have libtorch installed. Not the default production inference path due to deploy weight and CI cost.

---

### 2. Candle

**What it is.** Hugging Face’s Rust tensor library (no libtorch). Pure Rust + optional CUDA.

**GPU / CPU.** `candle-core` with `cuda` feature; CPU default.

**Install burden.** Low — crates.io only. CUDA needs toolkit on build host for GPU.

**CI feasibility.** Good for CPU-only tests. GPU CI optional.

**Integration mapping.**

| Hook | Fit |
|------|-----|
| `flattened()` | `Tensor::from_slice` on CPU; copy to device if needed |
| `ReplayBuffer` | Custom `Dataset` iterator for minibatches |
| `Env::step` | `forward` in Rust; suitable for small policies |
| Latency | Competitive for small models on CPU; avoid per-tick GPU upload for 7×8 floats |

**RL algorithms.** All families require **custom implementations** or porting reference code. No first-class PPO/SAC crates at production maturity (as of 2026).

**Verdict.** Attractive for **fully Rust, libtorch-free inference** of small networks if we invest in algorithm code. Poor choice as sole training stack for fast research iteration.

---

### 3. Burn

**What it is.** Rust-native deep learning framework (autodiff, training loops, multiple backends).

**GPU / CPU.** WGPU (cross-vendor), CUDA, Metal via feature flags.

**Install burden.** Low — crates.io. Backend features add compile time.

**CI feasibility.** Good CPU/WGPU tests in CI; CUDA optional.

**Integration mapping.** Similar to Candle: `ObservationWindow::flattened()` → `Tensor`, custom replay sampler, `Env::step` calls `Module::forward`. Burn’s training API helps **if** we commit to Rust-native training long-term.

**RL algorithms.** Burn can train MLPs/CNNs, but **no standard RL library**. PPO/SAC/DQN are weeks of engineering per algorithm.

**Verdict.** Strong long-term bet for **unified Rust train+infer** only if the team prioritizes Rust over Python research velocity. Not recommended as WP-016 primary for near-term MM/execution experiments.

---

### 4. ONNX Runtime (`ort`)

**What it is.** Microsoft’s inference engine for ONNX models. Rust via `ort` crate (safe wrapper over ONNX Runtime C API).

**GPU / CPU.** CPU default; CUDA and TensorRT execution providers optional.

**Install burden.** Low — `ort` downloads prebuilt ORT binaries on supported platforms. Models shipped as `.onnx` files (no libtorch at runtime).

**CI feasibility.** Excellent. CPU inference tests in default CI with tiny fixture models. No compiler toolchain beyond C++ runtime ORT bundles.

**Integration mapping.**

| Hook | Fit |
|------|-----|
| `flattened()` | `Array1<f32>` / `ndarray` → ORT input tensor |
| `ReplayBuffer` | Not used at inference; training exports ONNX from Python/tch/Burn |
| `Env::step` | `session.run()` → output logits → `Action` |
| Hot-swap | Reload `Session` from disk on SIGHUP or watch channel — ORT supports session replacement |
| Deterministic fallback | On `run` error or timeout, return `Action::Hold` |
| Latency | Small MLP on CPU: typically sub-ms to ~1 ms; TensorRT for larger models |

**RL algorithms.** **Inference only.** Training: PyTorch → `torch.onnx.export`, or Stable-Baselines3 export pipelines. Covers all algorithm families indirectly.

**Verdict.** **Recommended production inference stack** — fast, deployable, CI-friendly, aligns with train-in-Python / infer-in-Rust split common in trading.

---

### 5. Python / PyTorch sidecar (IPC bridge)

**What it is.** Separate Python process (or container) running PyTorch / SB3 / CleanRL. Rust `trolly-gym` communicates via gRPC, Unix socket, shared memory, or Redis streams.

**GPU / CPU.** Full PyTorch ecosystem.

**Install burden.** Medium — Python venv, `requirements.txt`, optional GPU drivers. Rust binary stays free of libtorch.

**CI feasibility.** Split pipelines: Rust `cargo test` default; Python integration tests optional (`#[ignore]` or separate workflow).

**Integration mapping.**

| Hook | Fit |
|------|-----|
| `ObservationWindow::flattened()` | Serialize as JSON, MessagePack, or raw `f32` bytes over IPC |
| `ReplayBuffer` | Stream transitions to sidecar for online training; or periodic Parquet export |
| `Env::step` | RPC `predict(obs)` → `Action` — adds **0.1–5+ ms** depending on transport |
| Egress | Remains in Rust — sidecar advises, Rust executes (safer for keys) |
| Backpressure | Async RPC with timeout; stale request cancellation |

**RL algorithms.**

| Family | Support |
|--------|---------|
| PPO | Stable-Baselines3, CleanRL — excellent |
| DQN / SAC | SB3, Ray RLlib — excellent |
| Offline / batch | d3rlpy, SB3 offline, custom replay loaders |

**Verdict.** **Recommended primary training path** for research throughput. Use for heavy GPU training and hyperparameter sweeps; export ONNX for production Rust inference.

---

## RL algorithm families (market making / execution)

| Family | Typical use | Data source | Best stack today |
|--------|-------------|-------------|------------------|
| **PPO** (on-policy) | Adaptive quoting, spread capture with stable updates | Live or sim rollouts via `Env::step` | Python sidecar or offline sim export → train → ONNX |
| **DQN** (off-policy, discrete) | Matches current `Action` enum | `ReplayBuffer` + ε-greedy | Python training on exported replay; ONNX for `Action` logits |
| **SAC** (off-policy, continuous) | Size / price skew (future continuous action space) | Replay + relabeling | Python; requires extending `Action` or separate dims |
| **Offline / batch** | Train on historical stream captures without live risk | `ReplayBuffer::snapshot`, Parquet logs | Python (d3rlpy, SB3) or custom tch loop if `torch` enabled |
| **Imitation / BC** | Bootstrap from rule-based or human trader | Logged (obs, action) pairs | Python — often fastest path to first policy |

**Discrete actions today.** `Action::{Hold, Buy, Sell}` maps cleanly to a 3-way softmax (DQN/PPO discrete). Continuous control (limit price, quantity scaling) is a follow-on schema change, not a toolchain blocker.

---

## Recommendations

### Offline training (batch replay, checkpoints, experiment tracking)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | **Python / PyTorch sidecar** (or offline trainer on exported replay) | Mature PPO/DQN/SAC, W&B/MLflow, GPU utilization, fastest iteration |
| **Optional** | **`torch` feature (tch)** | In-repo experiments without IPC; reuse `observation_tensor` |
| **Defer** | Burn / Candle as training backend | High engineering cost for algorithms; revisit if Python split becomes operational burden |

Training flow (target):

```
ReplayBuffer / Parquet logs
        │
        ▼
  Python trainer (SB3/CleanRL/custom)
        │
        ├─► checkpoints (.pt)
        └─► ONNX export (.onnx)
```

### Online inference (sub-ms to low-ms, hot-swap, fallbacks)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | **ONNX Runtime (`ort`)** — new `onnx` feature | Low latency, no libtorch in prod, deterministic CPU path, session reload for hot-swap |
| **Fallback 1** | Rule-based / `Action::Hold` | Already available; use on ORT timeout or invalid output |
| **Fallback 2** | **`torch` feature** | Dev/staging parity with research notebooks |
| **Avoid hot path** | Python sidecar RPC | IPC jitter too high for tick-synchronized MM unless co-located with aggressive batching |

Inference flow (target):

```
ingest_event (hot path, no ML)
        │
        ▼
 latest ObservationWindow::flattened()
        │
        ▼
 Policy::act (async or bounded sync)
        │
        ├─► ort::Session::run → Action
        └─► on failure → Action::Hold
        │
        ▼
 Env::step(action) → StreamEgress
```

---

## Decision

| Role | Toolchain | Cargo feature |
|------|-----------|---------------|
| **Primary training** | Python / PyTorch (sidecar or offline scripts) consuming replay exports | None in `trolly-gym` (separate `training/` or repo) |
| **Primary inference** | ONNX Runtime (`ort`) | `onnx = ["dep:ort", …]` (future WP) |
| **Optional research** | tch / libtorch | Existing `torch` feature — unchanged |
| **Not selected** | Burn, Candle as primary | Monitor for Rust RL maturity; no feature gate now |
| **Default build** | No ML runtime | `default = []` — scaffold only |

### Feature-gating policy

```toml
# Current (WP-011)
[features]
default = []
torch = ["dep:tch"]

# Planned (follow-on WPs — not implemented in WP-016)
# onnx = ["dep:ort"]
# policy-ort = ["onnx"]
```

- `cargo check --workspace` — **no** `tch`, **no** `ort`.
- CI smoke tests — mock observations only (`tests/smoke.rs`).
- Optional jobs — `--features torch` and future `--features onnx` on demand.

### Determinism and safety

- ONNX Runtime CPU EP with fixed thread count and graph optimizations disabled for reproducibility when required.
- Validate policy output: finite logits, argmax within `Action` enum, rate-limit orders at egress (strategy layer).
- Model hot-swap: load new `Session` atomically behind `Arc<RwLock<_>>`; in-flight `run` completes on old session or times out.

---

## Follow-on work items (not in WP-016)

| ID | Title | Scope |
|----|-------|-------|
| **WP-018** | `Policy` trait + rule-based default | `trolly-gym`: `Policy::act(&[f32]) -> Action`, wire into `Env` without ML deps |
| **WP-019** | ONNX inference hook (`onnx` feature) | `ort` session wrapper, `observation` → `Action`, timeout + `Hold` fallback |
| **WP-020** | Replay export + batch sampling API | Parquet/JSON Lines export from `ReplayBuffer`, minibatch iterator for trainers |
| **WP-021** | Python training sidecar protocol | gRPC/MessagePack spec: `Predict`, `TrainStep`, health; example SB3 script |
| **WP-022** | Training loop stub (offline) | Feature-gated epoch loop reading exported replay — no live trading |
| **WP-023** | Checkpoint and model hot-swap | Atomic ORT session reload, version metadata, metrics |
| **WP-024** | Reward shaping + continuous actions | Replace `reward_stub`, extend `Action` for size/price (SAC-ready) |
| **WP-025** | GPU CI matrix | Optional workflow: `torch` + CUDA smoke; ORT CUDA EP smoke |

---

## References

- WP-011 scaffold: [`crates/trolly-gym/README.md`](../README.md)
- [`tch` crate](https://github.com/LaurentMazare/tch-rs) / [libtorch](https://pytorch.org/get-started/locally/)
- [Candle](https://github.com/huggingface/candle)
- [Burn](https://github.com/tracel-ai/burn)
- [ort (ONNX Runtime Rust)](https://github.com/pykeio/ort)
- [Stable-Baselines3](https://stable-baselines3.readthedocs.io/)
