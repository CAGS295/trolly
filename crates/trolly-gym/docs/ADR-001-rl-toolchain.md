# ADR-001: RL training and inference toolchain for `trolly-gym`

| Field | Value |
|-------|-------|
| Status | Accepted |
| Date | 2026-06-19 |
| Workplan | WP-016 |
| Depends on | WP-011 (gym scaffold) |

## Context

`trolly-gym` is a stream-fed reinforcement-learning gym scaffold over trolly market data. WP-011 landed the integration hooks without committing to a full ML stack:

- **[`Env`](../../src/env.rs)** — ingests normalized [`StreamEvent`](../../trolly-strategy) values, maintains an observation window, steps with discrete [`Action`](../../src/action.rs) values, dispatches via [`StreamEgress`](../../trolly-strategy), and records transitions in a replay buffer.
- **[`ObservationWindow`](../../src/observation.rs)** — rolling deque of feature frames flattened to a `Vec<f32>` model input (default 8 frames × 7 depth features = 56 floats).
- **[`ReplayBuffer`](../../src/replay.rs)** — fixed-capacity ring buffer of [`Transition`](../../src/replay.rs) records for offline/batch training export.
- **[`libtorch.rs`](../../src/libtorch.rs)** — optional `torch` feature: `tch` tensor wrapper and stub forward pass; not linked in default builds.

Market-making and execution RL workloads impose distinct latency and toolchain constraints:

| Phase | Latency budget | Determinism | GPU | CI default |
|-------|----------------|-------------|-----|------------|
| **Offline training** | seconds–hours per batch | relaxed | desirable | CPU-only smoke; GPU optional nightly |
| **Live inference** | sub-ms to low-ms per action | required fallback path | optional | CPU-only |

This ADR compares candidate stacks, maps them to gym integration points, evaluates RL algorithm fit, and records an explicit toolchain decision plus follow-on work items. **No new runtime dependencies are introduced by this document.**

## Integration surface (all candidates)

Every stack must interact with the same hooks:

```text
trolly-stream Message
       │
       ▼
  Env::ingest_event / ingest_message
       │
       ├──► ObservationWindow::push → flattened Vec<f32>
       │
       ├──► ReplayBuffer::push_observation_window (prefill)
       │
       ▼
  policy(obs: &[f32]) → Action   ◄── inference hook (WP-020)
       │
       ▼
  Env::step(action) → Action::dispatch → StreamEgress → OutboundMessage
       │
       └──► ReplayBuffer::push_step (obs, action, reward, done)
```

**Stream latency / backpressure constraints**

- **Ingest path** — `ingest_event` is synchronous and O(window_frames); it must not block the stream reader. Heavy work (training, large tensor copies) belongs off the hot path.
- **Step path** — `step` dispatches orders synchronously through egress; inference must complete within the action loop budget or defer to a deterministic fallback (`Action::Hold`).
- **Replay export** — `ReplayBuffer::snapshot()` is suitable for batch handoff to a training process; no candidate should require holding the stream lock during export.
- **Observation shape** — fixed-size `Vec<f32>` today; stacks that expect NCHW or sequence tensors need a thin adapter at the inference hook, not changes to stream parsing.

## Candidate evaluation

### Summary matrix

| Stack | Training (batch/offline) | Live inference | GPU | CPU inference | libtorch / native install | Default CI `cargo check --workspace` | RL algo libraries |
|-------|--------------------------|----------------|-----|---------------|---------------------------|--------------------------------------|-------------------|
| **tch / libtorch** (`torch` feature) | Good (manual loops) | Good | CUDA, MPS | Yes | **Heavy** — libtorch download or PyTorch env | ✅ passes (feature off) | Manual / port from Python |
| **Candle** | Experimental | Good | CUDA (limited) | Yes | **Light** — pure Rust crate | ✅ passes (not added) | Immature; custom PPO/DQN |
| **Burn** | Experimental | Good | WGPU, CUDA, ROCm | Yes | **Light** — pure Rust crate | ✅ passes (not added) | Immature; custom training |
| **ONNX Runtime (`ort`)** | N/A (export only) | **Excellent** | CUDA, TensorRT, DirectML | **Excellent** | **Medium** — prebuilt ORT binaries via `ort` crate | ✅ passes (not added) | Train elsewhere; infer here |
| **Python / PyTorch sidecar (IPC)** | **Excellent** | Poor–Fair (IPC) | Full PyTorch | Via sidecar | **Heavy** (sidecar only) | ✅ passes (out of process) | SB3, CleanRL, RLlib, etc. |

---

### 1. tch / libtorch (`torch` feature — current)

**What it is:** Rust bindings to libtorch (same C++ backend as PyTorch). Already scaffolded in [`libtorch.rs`](../../src/libtorch.rs) behind `#[cfg(feature = "torch")]`.

**Integration mapping**

| Hook | Fit |
|------|-----|
| `ObservationWindow::flattened` | `observation_tensor(&flat)` → `tch::Tensor` |
| `Env::step` | Policy forward in-process; argmax/sample → `Action` |
| `ReplayBuffer` | Manual batching into tensors; no built-in RL |
| `Action` → egress | Direct — same process, lowest IPC overhead |
| Stream backpressure | Keep forward on dedicated thread or budgeted; clone `Vec<f32>` is cheap |

**GPU / CPU:** CUDA and CPU backends via libtorch; MPS on macOS. Performance matches PyTorch for equivalent graphs.

**Install burden:** High. Developers need libtorch headers/libs (`LIBTORCH`, `LIBTORCH_USE_PYTORCH=1`, or bundled download). CI cannot enable `torch` in default workspace checks without caching large artifacts or skipping GPU agents.

**RL algorithms:** No first-class RL crate. PPO, SAC, DQN require porting training loops from Python or using `tch` as a thin inference loader for TorchScript checkpoints trained elsewhere.

**Verdict:** Strong for **in-process inference** of TorchScript/JIT models and developer parity with PyTorch exports. Weak as a **primary training** environment in Rust.

---

### 2. Candle

**What it is:** Minimalist ML framework in pure Rust (Hugging Face ecosystem).

**Integration mapping**

| Hook | Fit |
|------|-----|
| Observations | `Tensor::from_slice(flat)` on CPU; optional CUDA feature |
| `Env::step` | Custom `Module` forward; map logits → `Action` |
| `ReplayBuffer` | Manual batch construction |
| Egress | In-process, same as tch |
| Backpressure | Pure Rust, no FFI to libtorch; predictable alloc patterns |

**GPU / CPU:** CPU first-class; CUDA feature exists but ecosystem and ops coverage lag PyTorch.

**Install burden:** Low — Cargo dependency only. CI-friendly.

**RL algorithms:** No mature PPO/SAC/DQN implementations. Would require substantial custom training code or research-grade ports. Suitable for small MLP policies only in near term.

**Verdict:** Attractive long-term for all-Rust deployment, but **not ready** as primary training stack for execution RL without a full algorithm rewrite.

---

### 3. Burn

**What it is:** Flexible deep learning framework in Rust with pluggable backends (NdArray, WGPU, CUDA, ROCm).

**Integration mapping**

| Hook | Fit |
|------|-----|
| Observations | `Tensor` from slice; backend selected at compile time |
| `Env::step` | `Module` forward + argmax |
| `ReplayBuffer` | Custom dataset trait over `snapshot()` |
| Egress | In-process |
| Backpressure | Similar to Candle; WGPU backend unsuitable for sub-ms CPU inference on server |

**GPU / CPU:** Multiple backends; training on GPU feasible but RL examples are sparse.

**Install burden:** Low for CPU/NdArray; CUDA/ROCm feature flags add compile time but no libtorch.

**RL algorithms:** Burn has community RL examples (DQN-style) but nothing production-grade for continuous control (SAC) or distributed PPO. Expect custom implementation effort comparable to Candle.

**Verdict:** Best pure-Rust option for **future** unified train+infer if the ecosystem matures; today it duplicates Candle’s gaps for RL.

---

### 4. ONNX Runtime (`ort`)

**What it is:** High-performance inference engine for ONNX models. Training happens elsewhere; Rust loads `.onnx` for forward passes.

**Integration mapping**

| Hook | Fit |
|------|-----|
| Observations | `Session::run` with input shape `[1, flat.len()]` or `[1, frames, features]` |
| `Env::step` | Sync inference; map output logits/probs → `Action` |
| `ReplayBuffer` | Not used at inference time; training exports ONNX from PyTorch |
| Egress | In-process, minimal overhead |
| Backpressure | **Best-in-class** CPU latency; session reuse; optional IO binding |

**GPU / CPU:** CPU EP is production-proven for small MLPs/CNNs; CUDA/TensorRT EPs for larger models. For discrete 3-action policies on ~56-dim input, CPU inference is typically **sub-ms**.

**Install burden:** Medium. The `ort` crate downloads platform ORT binaries (cacheable in CI). No libtorch. Smaller than full PyTorch.

**CI feasibility:** Feature-gated `ort` dependency; default workspace check unchanged. Add optional CI job with `--features ort` and cached ORT artifacts.

**RL algorithms:** Training not in ORT. Standard path: train PPO/SAC/DQN in Python → `torch.onnx.export` → load in Rust. Supports all algorithm families **at inference** without rewrite.

**Model hot-swap:** Reload `Session` from disk on SIGHUP or file watcher — natural fit for production policy updates.

**Deterministic fallbacks:** If `Session::run` fails or exceeds deadline, return `Action::Hold` (already maps to benign `Subscribe` heartbeat in scaffold).

**Verdict:** **Primary choice for live inference.**

---

### 5. Python / PyTorch sidecar (IPC bridge)

**What it is:** Separate Python process running PyTorch (+ SB3/CleanRL/etc.) communicating over IPC (Unix socket, gRPC, Redis, shared memory, or periodic replay file export).

**Integration mapping**

| Hook | Fit |
|------|-----|
| Observations | Serialize `flattened` JSON/msgpack/Arrow over socket |
| `Env::step` | **Blocking RPC** — latency = IPC + Python GIL + forward |
| `ReplayBuffer` | **Excellent** — `snapshot()` → Parquet/Arrow → sidecar training loop |
| Egress | Rust retains egress; only **action index** crosses IPC (recommended) |
| Backpressure | Risk: slow sidecar stalls stream loop. Mitigate with async channel + last-action cache + timeout → `Hold` |

**GPU / CPU:** Full PyTorch stack in sidecar; Rust process stays lean.

**Install burden:** Heavy for training host only; not linked into Rust binary. CI runs Rust tests without Python unless optional job.

**RL algorithms:** **Best coverage.** PPO, SAC, DQN, offline RL (CQL, IQL), multi-agent — all available without Rust reimplementation.

**Verdict:** **Primary choice for offline training** and experiment tracking. **Not primary for live inference** due to IPC latency and operational complexity, unless action rate is very low (e.g. &lt;1 Hz rebalancing).

---

## RL algorithm families (market making / execution)

| Family | Examples | Typical use | Stack support without full rewrite |
|--------|----------|-------------|-------------------------------------|
| **On-policy** | PPO, A2C | Adaptive quoting, inventory-aware MM | Python sidecar: native. tch/Candle/Burn: custom loop. ORT: infer only post-export. |
| **Off-policy** | DQN, SAC, TD3 | Optimal execution, placement timing | Python sidecar: native. Others: significant custom code. ORT: infer only. |
| **Offline / batch from replay** | BC, CQL, IQL, AWAC | Train on logged `ReplayBuffer` snapshots | Python sidecar: native. Export ONNX for deploy. Rust buffers: export format only. |
| **Imitation / supervised warm-start** | BC on expert logs | Bootstrap before online RL | Python training → ONNX deploy. |

**Recommendation:** Implement algorithm diversity **once** in Python (sidecar or batch trainer). Deploy **one** inference format (ONNX) to Rust. Avoid maintaining parallel PPO implementations in Candle/Burn unless a future WP explicitly targets all-Rust training.

## Offline training vs online inference

### Offline training (batch replay, checkpoints, experiment tracking)

| Requirement | Recommended approach |
|-------------|---------------------|
| Consume `ReplayBuffer::snapshot()` | WP-019: export Parquet/Arrow + metadata schema |
| PPO / SAC / DQN training | Python sidecar or batch script (PyTorch + SB3/CleanRL) |
| Checkpoints | PyTorch `.pt` + exported `.onnx` for production |
| Experiment tracking | MLflow/W&B in Python job (out of Rust crate) |
| GPU CI | Optional nightly workflow; not gated on default PR |
| Rust `torch` feature | Optional for in-Rust experiments loading same TorchScript |

### Online inference (sub-ms to low-ms action loop)

| Requirement | Recommended approach |
|-------------|---------------------|
| Policy forward on `flattened` obs | `ort` feature: `Session::run` (WP-020) |
| Latency target | &lt;1 ms CPU for small MLP; measure in benchmark WP |
| Model hot-swap | Reload ONNX file atomically (write temp → rename) |
| Deterministic fallback | Timeout or error → `Action::Hold`; log metric |
| GPU inference | Optional ORT CUDA EP; not required for v1 policy size |
| Stream backpressure | Never block ingest on inference; optional async policy task with latest-obs overwrite |

**Split stack rationale:** Training favors ecosystem breadth (Python). Inference favors latency, determinism, and CI simplicity (ONNX in Rust). tch remains a **dev fallback** for TorchScript without ONNX export step.

## Decision

### Primary toolchain

| Phase | Choice | Rationale |
|-------|--------|-----------|
| **Offline training** | **Python / PyTorch batch trainer or sidecar** | Mature RL libraries; trains on replay exports; no new default Rust deps |
| **Live inference** | **ONNX Runtime (`ort` feature, future WP)** | Sub-ms CPU inference, hot-swap, CI-cacheable binaries, algorithm-agnostic |

### Optional fallback

| Phase | Fallback | When to use |
|-------|----------|-------------|
| Inference | **`torch` / tch** (existing feature) | TorchScript models, rapid prototyping, parity checks against Python |
| Training | **Manual `tch` loops** | Small in-Rust experiments only; not production training |
| Long-term research | **Candle or Burn** (not scheduled) | Revisit when native RL crates stabilize |

### Feature gating in `trolly-gym`

| Feature | Default | Purpose |
|---------|---------|---------|
| *(none)* | **on** | Env, observation, replay, action — CI default |
| `torch` | off | `libtorch.rs`, `tch` dependency (WP-011) |
| `ort` | off | Future: ONNX inference hook (WP-020) |
| `candle` / `burn` | off | **Not planned** unless ADR revised; avoids dependency sprawl |

Python sidecar and training scripts live **outside** the Rust crate (e.g. `crates/trolly-gym/trainer/` or repo `tools/`) and do not affect `cargo check --workspace`.

## Consequences

- Default workspace builds remain libtorch-free and ORT-free.
- Production path: train in Python → export ONNX → load via future `ort` module in `Env` step path.
- `torch` feature preserved for developers who prefer TorchScript and for WP-011 compatibility.
- Candle/Burn evaluated but deferred; adopting them would require custom RL training infrastructure.
- Replay export schema and inference hook are explicit follow-on WPs (below).

## Follow-on work items (not implemented in WP-016)

| ID | Title | Scope |
|----|-------|-------|
| **WP-018** | Replay export I/O | Parquet/Arrow export from `ReplayBuffer::snapshot()`, import for batch training, schema versioning |
| **WP-019** | Python training loop scaffold | Sidecar or batch script: read replay export, PPO/SAC/DQN baseline, checkpoint + ONNX export |
| **WP-020** | ONNX inference hook | Optional `ort` feature, `policy.rs` with session load, hot-swap, timeout → `Action::Hold` |
| **WP-021** | Inference integration in `Env::step` | Inject `Policy` trait; wire observation → action before egress; deterministic fallback metrics |
| **WP-022** | Toolchain CI matrix | Optional jobs: `--features torch` (cached libtorch), `--features ort` (cached ORT), Python trainer smoke |

## References

- [`crates/trolly-gym/README.md`](../README.md) — build instructions and architecture overview
- [PyTorch libtorch](https://pytorch.org/get-started/locally/)
- [Candle](https://github.com/huggingface/candle)
- [Burn](https://burn.dev/)
- [ort crate (ONNX Runtime)](https://docs.rs/ort/)
- [Stable-Baselines3](https://stable-baselines3.readthedocs.io/)
