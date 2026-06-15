# RL Training and Inference Toolchain Analysis (WP-016)

**Status:** Architecture decision record (analysis only)  
**Scope:** `trolly-gym` stream-fed RL — offline training vs live inference  
**Related:** [`Env`](../src/env.rs), [`ObservationWindow`](../src/observation.rs), [`ReplayBuffer`](../src/replay.rs), [`Action`](../src/action.rs), [`libtorch`](../src/libtorch.rs)

## Context

`trolly-gym` is a scaffold that connects normalized market stream events to a discrete action space and strategy egress. The hot path today is:

```
StreamEvent → features_from_event → ObservationWindow → flattened Vec<f32>
                                              ↓
                         policy (stub) → Action → StreamEgress → OutboundMessage
                                              ↓
                                    ReplayBuffer (transitions)
```

Training loops, checkpoints, and GPU policies are intentionally out of scope until a toolchain is chosen. WP-011 landed an optional `torch` feature (`tch` + libtorch) with stub hooks in `libtorch.rs`. This document compares candidate stacks and records a decision before follow-on implementation work.

## Integration points (all candidates must map here)

| Integration point | Role | Latency / backpressure notes |
|-------------------|------|------------------------------|
| **`Env::ingest_event` / `ingest_message`** | Stream ingress; filters by symbol; grows window | Runs on every WS tick. Must not block the stream reader on heavy compute. Policy inference should be bounded (µs–low ms). |
| **`ObservationWindow`** | Rolling deque of `FeatureVector`; `flattened()` → model input | Default 8 frames × 7 depth features = 56-dim vector (smoke test). Tensor layout must match training export. |
| **`Env::step`** | Policy chooses `Action`, dispatches egress, records transition | Action loop target: **sub-ms to low-ms** for live trading; includes `ReplayBuffer::push_step`. |
| **`ReplayBuffer`** | Ring buffer of `Transition { observation, action, reward, done }` | Offline training source. Batch export must preserve insertion order (`snapshot()`). Capacity default 256 — training pipelines need periodic flush to disk. |
| **`Action` → `trolly-strategy`** | Discrete {Hold, Buy, Sell} → `OutboundMessage` | Inference output is argmax / sample over 3 logits (extendable). Fallback: rule-based policy when model unavailable. |
| **Stream backpressure** | `trolly-stream` multiplexor feeds handlers | If inference or training blocks ingest, events queue or drop per stream policy. **Training belongs off the hot path**; inference must stay non-blocking. |

## Candidate stacks

### Summary matrix

| Stack | Training (batch RL) | Live inference | GPU | CPU | Install / CI | Fit for trolly-gym |
|-------|--------------------|--------------------|-----|-----|--------------|-------------------|
| **`tch` / libtorch** (`torch` feature) | Possible but manual; no mature Rust RL libs | Good in-process | Yes (CUDA) | Yes | **Heavy** — libtorch download or PyTorch env; breaks default CI | Already scaffolded; good for prototyping |
| **Candle** | Possible (custom loops) | In-process | CUDA (limited) | Yes | Medium — pure Rust deps; no libtorch | Immature RL ecosystem |
| **Burn** | Possible (custom loops) | In-process | WGPU/CUDA/CPU | Yes | Medium — pure Rust | Immature RL ecosystem |
| **ONNX Runtime (`ort`)** | **Not for training** | **Excellent** inference-only | CUDA/TensorRT EPs | Yes | **Light** — prebuilt ORT wheels/binaries; CI-friendly | Best production inference path |
| **Python / PyTorch sidecar or IPC** | **Best** — SB3, CleanRL, RLlib | Poor (IPC ms+) unless isolated | Yes | Yes | Training: PyTorch env; sidecar adds ops burden | Best offline training path |

---

### 1. `tch` / libtorch (`torch` feature — current)

**What exists:** `libtorch.rs` exposes `observation_tensor()` and `stub_forward()` behind `#[cfg(feature = "torch")]`. Default workspace build does not link libtorch.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | Full libtorch backends (CUDA, CPU). |
| Install burden | High: `LIBTORCH` or `LIBTORCH_USE_PYTORCH=1`; version skew with `tch` crate (0.16). |
| CI | Not feasible in default `cargo check --workspace`; requires optional job with libtorch cache. |
| Training | No first-class PPO/DQN/SAC in Rust. Would require porting or hand-rolling optimizers on `tch::Tensor`. |
| Inference | Single-process, low overhead once loaded; `flattened()` → `Tensor::from_slice` → forward → argmax → `Action`. |

**Integration mapping:**

- **Observations:** Direct — `observation_tensor(&window.flattened())`.
- **Env::step:** Synchronous forward in `step` before `action.dispatch`; keep on same thread as egress for determinism.
- **ReplayBuffer:** Store raw `Vec<f32>`; training can replay in Rust (hard) or export for Python.
- **Egress:** Map output tensor indices to `Action` enum.
- **Backpressure:** Forward time adds to step latency; batch size 1 keeps this acceptable on CPU for small MLPs.

**Verdict:** Keep as **optional fallback / dev feature**, not primary production path.

---

### 2. Candle

Pure Rust ML framework (Hugging Face). No libtorch dependency.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | CUDA backend available; CPU always. Ecosystem smaller than PyTorch. |
| Install burden | Low — crates.io only. |
| CI | Good — `cargo check` with feature gate. |
| Training | Custom autograd + manual PPO/DQN loops possible; no Stable-Baselines equivalent. |
| Inference | In-process; similar hook to libtorch. |

**Integration mapping:** Same tensor path as libtorch; would add `candle` feature module mirroring `libtorch.rs`. Replay export unchanged.

**Verdict:** Viable for **small in-Rust experiments** only. Not recommended as primary until RL recipes exist.

---

### 3. Burn

Pure Rust deep learning with pluggable backends (WGPU, NdArray, CUDA).

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | Flexible backends; good for cross-platform CPU inference. |
| Install burden | Low — pure Rust for NdArray backend. |
| CI | Good with feature gate. |
| Training | Custom training loops; Burn Train crate helps but no RL-specific stack. |
| Inference | In-process; model hot-swap via reload from checkpoint file. |

**Integration mapping:** Identical hook shape to Candle/libtorch. `ObservationWindow::flattened()` → `Tensor` → policy head → `Action`.

**Verdict:** Same as Candle — **research alternative**, not primary for market-making RL at current maturity.

---

### 4. ONNX Runtime (`ort`)

Inference-only runtime; models exported from PyTorch (or other trainers).

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | CPU default; CUDA / TensorRT execution providers optional. |
| Install burden | Moderate — `ort` crate bundles or links prebuilt ORT; lighter than full libtorch. |
| CI | **Feasible** — optional feature + small `.onnx` fixture in tests. |
| Training | **Out of scope** — train elsewhere, export ONNX. |
| Inference | Optimized graphs; typical MLP policy **< 1 ms** on CPU for 56-dim input. |

**Integration mapping:**

- **Observations:** `flattened()` → `ndarray` / ORT input tensor `(1, obs_dim)`.
- **Env::step:** `session.run()` → logits → `Action::from_index`.
- **ReplayBuffer:** Unchanged; training uses Python exports, not ORT.
- **Hot-swap:** Reload `Session` from new `.onnx` path on SIGHUP or config watch; fall back to Hold/rule policy on load failure.
- **Backpressure:** Bounded sync inference; async batching not needed for discrete 3-action head.

**Verdict:** **Primary live inference** path.

---

### 5. Python / PyTorch sidecar or IPC bridge

Separate process (or offline batch job) running PyTorch training; optional gRPC/Unix socket for inference.

| Dimension | Assessment |
|-----------|------------|
| GPU / CPU | Full PyTorch ecosystem. |
| Install burden | Python venv + PyTorch; sidecar deployment separate from Rust binary. |
| CI | Training jobs as optional workflow; not in default `cargo check`. |
| Training | **Best option:** PPO (Stable-Baselines3, CleanRL), DQN, SAC, offline RL (CQL, IQL) on exported replay Parquet/Arrow. |
| Inference | IPC adds **milliseconds** — unacceptable for sub-ms action loop co-located with stream handler. |

**Integration mapping:**

- **Offline:** Periodic `ReplayBuffer::snapshot()` → serialize → Python dataset → train → export `.onnx` + metrics.
- **Online inference:** **Not recommended** on critical path; use exported ONNX in Rust instead.
- **Optional sidecar:** Acceptable for **paper / research** modes with relaxed latency.

**Verdict:** **Primary offline training** pipeline (export + Python), not live inference.

---

## RL algorithm families vs stack support

| Algorithm family | Use case in trolly | `tch`/libtorch | Candle/Burn | Python/PyTorch | ONNX (inference only) |
|------------------|-------------------|----------------|-------------|----------------|----------------------|
| **PPO** (on-policy) | Online fine-tuning from rolling env | Manual impl | Manual impl | **SB3, CleanRL, RLlib** | Deploy exported actor |
| **DQN / SAC** (off-policy) | Sample-efficient discrete control | Manual impl | Manual impl | **Mature libs** | Deploy exported Q-network / actor |
| **Offline / batch** from replay | Train on logged `Transition`s without live env | Poor ergonomics | Poor | **CQL, IQL, BC** on exported buffer | N/A (train in Python) |
| **Behavior cloning warm-start** | Bootstrap from rule strategy | Possible | Possible | **Easy** | Export BC policy to ONNX |

**Recommendation:** Implement **offline/batch training in Python** consuming replay exports. Use **on-policy PPO** or **off-policy SAC** there depending on sample efficiency needs. Deploy ** ONNX actor** to Rust for live inference. Avoid rewriting PPO/DQN in Rust unless dependency constraints change.

## Recommendations: offline training vs online inference

### Offline training (batch replay, checkpoints, experiment tracking)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | Python / PyTorch batch pipeline | RL libraries, GPU training, W&B/MLflow, rapid iteration. |
| **Interface** | `ReplayBuffer` export → Parquet/Arrow/JSONL on interval or env shutdown | Keeps default crate free of Python. |
| **Artifacts** | Checkpoint (`.pt`) + **ONNX export** for deployment | Single export path to production inference. |
| **Not primary** | In-crate `tch` training loop | High engineering cost; weak RL ecosystem in Rust. |

### Online inference (sub-ms action loop, hot-swap, fallbacks)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | ONNX Runtime (`ort` feature, future WP) | Fast CPU inference, CI-friendly, no libtorch in prod binary. |
| **Fallback 1** | Rule-based / Hold when model missing or ORT session fails | Deterministic safety; already have `Action::Hold`. |
| **Fallback 2** | Existing `torch` feature (`tch`) for dev / A-B tests | Reuse `libtorch.rs` hooks; feature-gated. |
| **Avoid** | Python sidecar on stream hot path | IPC latency and process failure domain. |

**Hot-swap:** Load ONNX from versioned path (e.g. `models/policy_v{N}.onnx`); atomic rename + session rebuild; metric hook for inference latency p99.

**Deterministic fallbacks:** On ORT error or NaN logits → `Action::Hold` + log; optional circuit breaker to disable RL egress.

## Decision

| Role | Toolchain | Feature gate |
|------|-----------|--------------|
| **Primary — offline training** | Python / PyTorch (external scripts consuming replay export) | None in `trolly-gym` default; optional `export` feature for serde replay I/O only |
| **Primary — live inference** | ONNX Runtime (`ort`) | Future `ort = ["dep:ort"]` |
| **Optional fallback — inference dev** | `tch` / libtorch | Existing `torch = ["dep:tch"]` |
| **Not selected** | Candle, Burn as primary | May revisit for all-Rust inference experiments |
| **Not selected for hot path** | Python IPC sidecar | Training-only |

**Default `cargo check --workspace`:** unchanged — no new required dependencies. Analysis-only in this WP.

## Feature-gating plan (`trolly-gym`)

```toml
[features]
default = []
torch = ["dep:tch"]           # existing — libtorch dev / fallback inference
# ort = ["dep:ort"]           # WP-018 — production inference
# export = ["dep:serde", ...]  # WP-019 — replay checkpoint export for Python training
```

Public API shape (future):

- `torch`: `libtorch::policy_forward(flat) -> Action` (extend stub).
- `ort`: `onnx::PolicySession::decide(flat) -> Action` with hot-swap.
- No feature: deterministic stub / rule policy for tests and CI.

## Stream latency and backpressure

```
┌─────────────┐     ingest      ┌──────────────────┐
│ trolly-stream│ ──────────────► │ Env (same task)  │
│  multiplexor │                 │  window + step   │
└─────────────┘                 └────────┬─────────┘
                                         │
                    ┌────────────────────┼────────────────────┐
                    ▼                    ▼                    ▼
              ORT inference      ReplayBuffer push      StreamEgress
              (sync, bounded)   (async flush OK)       (must succeed)
```

- **Ingest thread:** Must not run Python or multi-ms training steps.
- **Replay flush:** Background task or separate process reading exported files.
- **Inference budget:** Target p99 **< 1 ms** CPU for small policies; measure in WP-018.

## Follow-on work items (not implemented here)

| ID | Title | Depends on |
|----|-------|------------|
| **WP-018** | ONNX inference hook (`ort` feature, `PolicySession`, hot-swap, fallbacks) | WP-016 |
| **WP-019** | Replay export format + Python training script scaffold (PPO/SAC offline) | WP-016 |
| **WP-020** | Optional libtorch policy head (extend `stub_forward` → discrete `Action`) | WP-016 |
| **WP-021** | CI matrix: default check + optional `torch` / `ort` jobs | WP-018, WP-020 |
| **WP-022** | Reward shaping + done signals (replace `reward_stub`) | WP-011 |

## References

- [`crates/trolly-gym/README.md`](../README.md) — build features
- [`src/libtorch.rs`](../src/libtorch.rs) — current torch gate
- [PyTorch libtorch](https://pytorch.org/get-started/locally/)
- [ONNX Runtime](https://onnxruntime.ai/)
- [Candle](https://github.com/huggingface/candle)
- [Burn](https://github.com/tracel-ai/burn)
