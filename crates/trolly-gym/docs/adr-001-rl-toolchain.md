# ADR-001: RL Training and Inference Toolchain for trolly-gym

**Status:** Proposed  
**Date:** 2026-06-18  
**Authors:** workplan WP-016  
**Scope:** `crates/trolly-gym/` — ML backend selection for stream-fed reinforcement learning

---

## Context

`trolly-gym` provides a stream-fed RL environment scaffold: an `Env` that ingests normalized
`StreamEvent` values from `trolly-stream`, maintains a rolling `ObservationWindow` of feature
vectors, records `Transition` tuples in a `ReplayBuffer`, and dispatches discrete `Action` values
through `StreamEgress` to `trolly-strategy`. The current `src/libtorch.rs` module wraps `tch`
(PyTorch C++ bindings) behind `--features torch`, but no training loop or live inference path is
wired in.

We need a decision on which ML stack(s) to adopt for:

1. **Offline training** — batch/replay-driven gradient updates, potentially on GPU, outside the
   hot path.
2. **Live inference** — loading a trained policy and calling it on each `ObservationWindow::flattened()`
   result, on the critical `Env::step` path (target: sub-ms to low-ms action latency, deterministic
   fallbacks, model hot-swap without downtime).

The decision must be compatible with no new runtime dependency in `cargo check --workspace` (default
features), and must not break the existing feature-gated `torch` path.

---

## Integration Points

Before evaluating candidates, it is useful to map the data flow through `trolly-gym` to the
interface points where any ML backend must plug in:

| Integration point | Type / interface | Constraint |
|---|---|---|
| **Feature extraction** | `observation::features_from_event` → `FeatureVector(Vec<f32>)` | Runs on every ingested event; must be allocation-minimal on hot path |
| **Observation window** | `ObservationWindow::flattened() → Vec<f32>` | Fixed-length flat `f32` slice; this is the model's raw input tensor |
| **Replay store** | `ReplayBuffer::snapshot() → Vec<Transition>` | Produces `(obs, action, reward, done)` tuples for off-policy or offline training batches |
| **Policy call site** | `Env::step(action: Action)` | Caller decides the action today; a policy provider would replace or wrap this call |
| **Egress / action dispatch** | `Action::dispatch → StreamEgress::dispatch(OutboundMessage)` | Must remain synchronous and infallible from the `Env::step` caller's perspective |
| **Stream backpressure** | `Env::ingest_event` is called on every stream tick | Model inference must not block the ingest loop; async or lock-free handoff needed if inference is slow |

Any ML backend integration must:

- Accept a `&[f32]` slice (or convert from it without an extra copy on the hot path).
- Return a discrete action index or probability distribution over `{Hold, Buy, Sell}`.
- Tolerate hot-swap (loading a new checkpoint at runtime without restarting the process).
- Provide a deterministic fallback (e.g. `Action::Hold`) when no model is loaded or inference fails.

---

## Candidate Evaluation

### 1. `tch` / libtorch (current `torch` feature)

**Description:** The `tch` crate wraps libtorch (PyTorch's C++ library). `src/libtorch.rs` already
contains an `observation_tensor` helper and a `stub_forward` identity function.

**Pros:**
- Full PyTorch operator coverage; any PyTorch-trained model serialized as TorchScript
  (`.pt`) can be loaded with `tch::CModule::load`.
- GPU support via CUDA (same device selection API as Python PyTorch).
- PPO, DQN, SAC, and offline/batch algorithms can all be implemented in Python and exported.
- Python-trained checkpoints are immediately portable.

**Cons:**
- **libtorch install burden is severe.** The libtorch shared library bundle is ~600 MB (CPU) to
  ~2 GB (CUDA). It must be present at both build time and runtime. `tch-rs` requires setting
  `LIBTORCH` or `LIBTORCH_USE_PYTORCH=1` env vars; CI runners need either a pre-cached tarball or
  a Docker image with libtorch pre-installed.
- **CI feasibility: poor without a custom runner.** Default GitHub Actions / free-tier runners have
  slow network, no GPU, and no libtorch pre-installed. A minimal CI step with `--features torch`
  requires ~5-10 min download or a custom Docker image.
- Linking is dynamic by default; runtime `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` management is
  fragile in containerized deployments.
- `tch` version lock-step with libtorch version; upgrading either is an ecosystem-wide event.
- No safe Rust API; most of the surface is `unsafe` under the hood.

**RL algorithm support:**
- Unlimited (any algorithm implemented in Python can be exported as TorchScript).
- On-policy (PPO): export actor-critic networks.
- Off-policy (DQN, SAC): export Q-network or policy network.
- Offline/batch: load a pre-trained checkpoint from `ReplayBuffer` dumps.

**Recommendation for trolly-gym:** Keep as an optional, feature-gated path. Do not make it the
default or CI-required path. Suitable for a researcher who has a local libtorch install and wants
to evaluate a trained PyTorch policy without a Python process.

---

### 2. Candle (Hugging Face)

**Description:** Pure-Rust tensor library from Hugging Face. No C/C++ binding; computes via
BLAS/LAPACK on CPU, or via `candle-kernels` CUDA kernels on GPU.

**Pros:**
- **Zero native linking burden on CPU.** `cargo check` and `cargo test` work out of the box on any
  platform without additional system libraries.
- Safe Rust API; no `unsafe` blocks in user code.
- Good for inference-only use cases: load a model from SafeTensors or GGUF format.
- Small binary footprint for CPU-only builds.
- CI feasibility: excellent for CPU; CUDA requires `nvcc` but is opt-in.

**Cons:**
- **Training support is limited and experimental.** Candle's autograd is partial; implementing
  PPO or SAC training from scratch is non-trivial and unsupported by the project officially.
  It is primarily a **inference library**.
- No built-in RL algorithm implementations; all training code must be written in Rust or
  offloaded to Python.
- GPU CUDA support requires CUDA toolkit at build time and a custom feature flag, similar to
  `tch` but lighter.
- Smaller op coverage than PyTorch; some custom ops needed for advanced architectures.
- Ecosystem still maturing; breaking API changes are more frequent than libtorch.

**RL algorithm support:**
- Inference: PPO/DQN/SAC actor networks can be exported from Python and loaded in Candle
  (SafeTensors format). The inference forward pass is fully supported.
- Training: not recommended in Rust. Candle lacks a production-grade autograd engine for
  large RL training loops.

**Recommendation for trolly-gym:** Excellent choice for the **live inference path**. A Python
training harness produces a checkpoint in SafeTensors format; Candle loads and evaluates it
inside `Env::step`. Feature-gate behind `candle` feature. Not suitable for in-process training.

---

### 3. Burn

**Description:** Modular deep learning framework in Rust, with multiple backends (NdArray for
CPU, `tch` for libtorch, `wgpu` for GPU via WebGPU API, and others).

**Pros:**
- **First-class training support in Rust**, including autograd, optimizers, and a training
  runner abstraction.
- Backend-agnostic: the same model code runs on CPU (NdArray), libtorch (GPU), or wgpu (GPU
  without CUDA dependency).
- Native RL-relevant primitives (replay buffers, environment traits) are in active development
  in `burn-rl` (experimental).
- CI feasibility: excellent for NdArray (CPU) backend — zero native deps. CUDA requires
  libtorch or CUDA toolkit.
- Burn's `AutodiffBackend` trait allows writing training loops entirely in Rust.

**Cons:**
- **Ecosystem immaturity.** Burn is moving fast; APIs break between minor versions.
- `burn-rl` is pre-alpha as of 2026; PPO and SAC implementations are not yet stable.
- wgpu GPU backend is slower than CUDA libtorch for large batches due to WebGPU overhead.
- Interop with Python-trained checkpoints is possible via SafeTensors (Burn can load them),
  but ONNX import is partial.
- Larger dependency tree than Candle; compile times are longer.

**RL algorithm support:**
- On-policy (PPO): possible with Burn's autograd (experimental `burn-rl`).
- Off-policy (DQN): possible but requires manual implementation of the replay sampling loop.
- SAC: not yet in `burn-rl`; requires custom implementation.
- Offline/batch: load replay snapshots from `ReplayBuffer::snapshot()` and train with Burn
  optimizers.
- **Most viable Rust-native option for in-process training**, but with significant prototype
  risk.

**Recommendation for trolly-gym:** Promising for a future all-Rust training path. Feature-gate
behind `burn` feature. Adopt for offline training experiments with the NdArray CPU backend.
Do not block on it for the first production inference path.

---

### 4. ONNX Runtime (`ort`)

**Description:** The `ort` crate binds to Microsoft's ONNX Runtime, which can execute any ONNX
model on CPU, CUDA, TensorRT, or other execution providers.

**Pros:**
- **Broad model portability.** Any model trained in PyTorch, TensorFlow, JAX, or Scikit-Learn
  can be exported to ONNX and executed by `ort`.
- CPU inference is fast (optimized kernels, thread pool management).
- CUDA and TensorRT execution providers give near-native GPU inference speed for static graphs.
- `ort` crate bundles the ONNX Runtime shared library automatically on some platforms
  (fetched at build time); no separate install for common targets.
- CI feasibility: good for CPU. `ort` can download the ONNX Runtime DLL/so during `cargo build`
  via the `load-dynamic` feature or bundled feature.
- Model hot-swap: reload a new `Session` from disk at runtime without restarting the process.
- Graph-level optimizations (constant folding, fusion) are applied automatically.

**Cons:**
- **Inference only.** ONNX Runtime does not perform gradient-based training; training must happen
  in Python.
- Dynamic shapes require care; static shapes preferred for sub-ms latency.
- ONNX model export from PyTorch sometimes requires `torch.export` wrangling (dynamic indexing,
  custom ops not exported cleanly).
- `ort` crate API has changed significantly across versions (0.x → 1.x → 2.x); pinning is important.
- Bundled runtime adds ~10-40 MB to binary size.

**RL algorithm support (inference):**
- Any trained PPO/DQN/SAC policy that can be exported to ONNX (almost all standard architectures)
  is supported.
- Stochastic policies: export the actor's deterministic or argmax path; sampling logic lives in Rust.

**Recommendation for trolly-gym:** **Best default choice for live inference.** Excellent CI story,
no libtorch dependency, fast CPU inference, straightforward model hot-swap. Feature-gate behind
`ort` feature. Training stays in Python.

---

### 5. Python / PyTorch Sidecar (IPC Bridge)

**Description:** Run a Python process (torch, stable-baselines3, RLlib, or custom) alongside the
Rust trading engine. Communicate via Unix socket, named pipe, shared memory (e.g. Arrow IPC /
`arrow2`), or gRPC/REST.

**Pros:**
- **Maximum algorithm coverage.** Stable-baselines3, RLlib, CleanRL, and similar libraries
  provide production-grade PPO, DQN, SAC, TD3, and offline RL (CQL, IQL) out of the box.
- Python training harness for the full RL loop: episode management, reward shaping, hyperparameter
  search (Optuna, Ray Tune), checkpoint management, W&B/MLflow experiment tracking.
- No Rust ML code required; the Rust engine is the environment, Python is the policy.
- GPU: trivially available in Python; no Rust CUDA plumbing.
- Algorithm upgrades: update the Python sidecar without touching Rust.

**Cons:**
- **IPC latency floor.** Even a Unix socket round-trip adds ~10–100 µs of latency per step.
  For sub-ms action loops this is the dominant cost. Named pipes are similar. Shared memory
  (mmap / Arrow) can reduce this to ~1–5 µs but requires more engineering.
- **Deployment complexity.** The production image must carry both a Rust binary and a Python
  virtualenv. Process supervision (e.g. systemd, Docker Compose) is needed for both.
- **Backpressure and synchronization.** If the Python inference process falls behind the stream
  ingest rate, the Rust engine must buffer or drop observations. This must be explicitly designed
  (async channel, ring buffer, or `EAGAIN` handling).
- Latency is non-deterministic; garbage collection pauses in Python can cause tail latency spikes.
- Testing: the smoke/integration test suite must mock or stub the Python side.

**IPC protocol options:**

| Protocol | Latency | Throughput | Complexity |
|---|---|---|---|
| Unix domain socket (msgpack/JSON) | ~30-100 µs RTT | Low | Low |
| Unix socket (Flatbuffers/Cap'n Proto) | ~10-30 µs RTT | Medium | Medium |
| Shared memory (mmap + semaphore) | ~1-5 µs RTT | High | High |
| `arrow-ipc` (shared mem) | ~2-10 µs RTT | High | Medium (ecosystem support) |
| gRPC (local loopback) | ~50-200 µs RTT | Medium | Medium |

**RL algorithm support:**
- Full: PPO, DQN, SAC, TD3, IQL, CQL, and any future algorithm implemented in Python.
- Offline/batch: serialize `ReplayBuffer::snapshot()` to Arrow/Parquet, read from Python.

**Recommendation for trolly-gym:** **Primary training path.** Use Python/PyTorch for the offline
and online training loop. Export the trained model to ONNX for live inference in Rust (combining
this option with `ort`). The IPC bridge is an optional real-time training mode for environments
where online learning is required.

---

## RL Algorithm Families and Stack Support

| Algorithm family | Representative algos | tch | Candle | Burn | ort | Python sidecar |
|---|---|---|---|---|---|---|
| **On-policy** | PPO, A2C | ✓ export | inference only | experimental | inference only | ✓ full |
| **Off-policy** | DQN, SAC, TD3 | ✓ export | inference only | experimental | inference only | ✓ full |
| **Offline / batch RL** | CQL, IQL, BCQ | ✓ export | inference only | partial | inference only | ✓ full |
| **Model-based** | Dreamer, MuZero | ✓ export | inference only | not yet | inference only | ✓ full |
| **Imitation learning** | BC, GAIL, DAGGER | ✓ export | inference only | not yet | inference only | ✓ full |

The `ReplayBuffer::snapshot()` interface in `trolly-gym` provides the data substrate for
all offline and off-policy algorithms. Online training (updating weights during live trading)
requires either the Python sidecar IPC pattern or, eventually, Burn with a stable autograd backend.

---

## GPU / CPU Considerations

| Stack | CPU | CUDA GPU | Apple Metal / wgpu |
|---|---|---|---|
| `tch` / libtorch | ✓ (BLAS) | ✓ (CUDA required) | ✗ (libtorch CUDA only) |
| Candle | ✓ (BLAS) | ✓ (`candle-kernels`, CUDA required) | ✓ (Metal feature) |
| Burn NdArray | ✓ (pure Rust) | ✗ | ✗ |
| Burn + tch backend | ✓ | ✓ | ✗ |
| Burn + wgpu backend | ✓ | ✓ (via Vulkan/CUDA) | ✓ (Metal) |
| `ort` | ✓ (optimized) | ✓ (CUDA EP) | ✗ (Core ML EP partial) |
| Python sidecar | ✓ | ✓ (trivial) | ✓ (MPS in PyTorch) |

For live inference (the action loop), **CPU is sufficient for small MLP / shallow LSTM policies**.
Sub-ms latency on a modern CPU core is achievable with `ort` or Candle for a 3–4 layer network with
a 50–200-element input vector (which is what `ObservationWindow::flattened()` produces with 8 frames
× 7 features = 56 elements).

For offline training, GPU matters for larger models and longer histories. The Python sidecar on GPU
is the path of least resistance.

---

## CI Feasibility Summary

| Stack | Default `cargo check` | CI without extras | CI with GPU |
|---|---|---|---|
| `tch` (feature-gated) | ✓ (no tch in default) | ✗ (needs libtorch) | Needs custom runner |
| Candle (feature-gated) | ✓ | ✓ (CPU) | Needs CUDA toolkit |
| Burn NdArray (feature-gated) | ✓ | ✓ (CPU) | — |
| `ort` (feature-gated) | ✓ | ✓ (bundled runtime) | Needs CUDA EP |
| Python sidecar | ✓ | ✓ (mock in Rust tests) | Python + torch install |

**All ML backends must be feature-gated.** `cargo check --workspace` (default features) must remain
clean and fast. This is already the case for `tch` and must be preserved for any new backend.

---

## Offline Training vs. Live Inference: Separate Recommendations

### Offline Training

**Recommended stack: Python / PyTorch (sidecar or standalone script)**

- Use `trolly-gym`'s `ReplayBuffer::snapshot()` to export transition batches (serialize to Arrow /
  Parquet / HDF5 or stream via IPC).
- Train PPO, SAC, or DQN using stable-baselines3, CleanRL, or a custom PyTorch loop.
- Export the trained policy to ONNX (`torch.onnx.export`) or SafeTensors format.
- Optionally checkpoint TorchScript for use via `tch` if a researcher needs live fine-tuning.

**Why not Rust-native training?**  
Burn's `burn-rl` is pre-alpha. Implementing PPO or SAC in Burn from scratch carries high
maintenance risk and no ecosystem support. The Python ecosystem for RL training is mature and
far more productive. The cost of cross-language model export is low and one-time.

### Live Inference

**Recommended stack: ONNX Runtime (`ort`)**

- Load a pre-trained ONNX policy at startup via a future `PolicyProvider` trait in `trolly-gym`.
- Call `Session::run()` with `ObservationWindow::flattened()` as input.
- Decode the output (logits or argmax) to an `Action` variant.
- A `None`-policy fallback returns `Action::Hold` deterministically.
- Hot-swap: build a `Arc<RwLock<Option<Session>>>` wrapper; a background watcher reloads the
  session when the model file changes (inotify on Linux).

**Sub-ms action loop budget (56-element input, 3-layer MLP):**

| Step | Estimated latency |
|---|---|
| `ObservationWindow::flattened()` | ~100 ns |
| `ort` session run (CPU, MKL, 3-layer MLP) | ~50–200 µs |
| `Action` decode + `StreamEgress::dispatch` | ~1 µs |
| **Total** | **< 300 µs** |

This is within the low-ms budget for market-making strategies. For sub-100 µs action loops,
a hand-vectorized Rust inference path (no framework) or a Candle model with a pre-allocated
tensor would be needed.

**Backpressure and ingest decoupling:**  
`Env::ingest_event` runs on the stream read loop. Inference should be decoupled via a
`crossbeam-channel` or `tokio::sync::watch` channel carrying the latest observation; a
separate task calls the policy and posts the resulting `Action` back. This prevents model
inference jitter from blocking stream ingestion and introducing update-ID gaps.

---

## Decision

### Primary toolchain: ONNX Runtime (`ort`) for inference + Python / PyTorch for training

**Rationale:**

1. **No new default dependency.** `ort` is feature-gated; `cargo check --workspace` remains clean.
2. **Best CI story for inference.** `ort`'s bundled runtime downloads automatically; no libtorch
   install required in CI.
3. **Algorithm coverage via Python.** The full RL algorithm ecosystem (PPO, SAC, DQN, offline RL)
   is available through Python training, with ONNX export as the hand-off mechanism.
4. **Model portability.** ONNX is framework-agnostic; switching the training backend (e.g. from
   PyTorch to JAX or Burn later) does not require changes to the Rust inference path.
5. **Production hardening.** ONNX Runtime is production-grade, maintained by Microsoft, and used
   in high-frequency and low-latency systems.

### Optional fallback: Candle

For use cases where ONNX export is not feasible (e.g. custom ops, research prototypes), Candle
provides a pure-Rust inference path that is CI-friendly and has no native deps. Feature-gate
behind `candle`.

### Feature-gated in trolly-gym

| Feature flag | Backend | Purpose |
|---|---|---|
| `torch` (existing) | `tch` / libtorch | Researcher local eval, fine-tuning |
| `ort` (to add) | ONNX Runtime | Primary production inference |
| `candle` (to add) | Candle | Fallback pure-Rust inference |
| `burn` (to add) | Burn | Future in-process training experiments |

All flags are off by default. The `PolicyProvider` trait (a future abstraction) will dispatch
to whichever backend is compiled in, with a `Hold` stub as the default.

### What stays out of scope for trolly-gym

- Model architecture definitions (stay in the Python training codebase).
- Training loops, optimizers, gradient computation.
- Hyperparameter search and experiment tracking.
- ONNX model export scripts (live in a sibling `tools/` or `scripts/` directory).

---

## Follow-on Implementation Work Items (not implemented here)

The following WPs are scoped out of this ADR and must be implemented separately:

| ID | Title | Description |
|---|---|---|
| WP-017 | `PolicyProvider` trait and `Hold` stub | Define `trait PolicyProvider { fn act(&self, obs: &[f32]) -> Action; }` with a `HoldPolicy` default; wire into `Env::step`. |
| WP-018 | `ort` feature integration | Add `ort` as an optional dependency; implement `OnnxPolicy: PolicyProvider`; write a smoke test loading a trivial ONNX model. |
| WP-019 | Replay export for Python training | Implement `ReplayBuffer::to_arrow()` or a serialization adapter so the Python training harness can consume `Transition` batches. |
| WP-020 | Python training harness scaffold | Minimal `trolly-gym-py` stub: a Python environment wrapper that calls into the Rust gym via PyO3 or serialized IPC for online training. |
| WP-021 | Candle fallback inference | Add `candle` as an optional dependency; implement `CandlePolicy: PolicyProvider` loading SafeTensors checkpoints. |
| WP-022 | Model hot-swap watcher | Background task using `notify`/inotify to reload a `PolicyProvider` when the model file changes; `Arc<RwLock<Box<dyn PolicyProvider>>>` pattern. |
| WP-023 | Reward shaping and episode termination | Replace `reward_stub` in `env.rs` with a configurable `RewardFn` trait; define episode terminal conditions for market-making tasks. |
| WP-024 | Backpressure and ingest/inference decoupling | Decouple `Env::ingest_event` from policy calls using a `crossbeam-channel`; define latency budget and overflow policy (drop or block). |

---

## Consequences

**If this ADR is accepted:**

- `cargo check --workspace` continues to pass with no new default dependencies.
- `src/libtorch.rs` remains as-is under `--features torch`.
- A `docs/` directory is established as the home for architecture decisions in `trolly-gym`.
- Future WPs (WP-017 through WP-024) implement the `PolicyProvider` trait and inference backends
  as feature-gated additions.
- The Python training workflow is explicitly out-of-crate; a separate `tools/training/` subtree
  or sibling crate will own training code.

**If this ADR is rejected:**

- Alternative: adopt Burn with NdArray backend for all-Rust training. This is viable but carries
  higher implementation risk and algorithm coverage gaps until `burn-rl` stabilizes.
- Alternative: make `tch` the primary inference path and require libtorch in CI. This gives the
  richest single-process experience but significantly increases CI complexity and onboarding friction.

---

## References

- [`tch-rs` crate](https://github.com/LaurentMazare/tch-rs) — PyTorch Rust bindings
- [`candle` crate](https://github.com/huggingface/candle) — Hugging Face pure-Rust ML framework
- [`burn` crate](https://github.com/tracel-ai/burn) — Modular Rust deep learning framework
- [`ort` crate](https://github.com/pykeio/ort) — ONNX Runtime Rust bindings
- [ONNX Runtime documentation](https://onnxruntime.ai/docs/)
- [stable-baselines3](https://stable-baselines3.readthedocs.io/) — PPO/DQN/SAC/TD3 reference implementations
- [CleanRL](https://github.com/vwxyzjn/cleanrl) — single-file RL implementations
- `crates/trolly-gym/src/env.rs` — stream-fed `Env` stepping logic
- `crates/trolly-gym/src/observation.rs` — `ObservationWindow`, `FeatureVector`
- `crates/trolly-gym/src/replay.rs` — `ReplayBuffer`, `Transition`
- `crates/trolly-gym/src/action.rs` — `Action` enum and egress dispatch
- `crates/trolly-strategy/src/egress.rs` — `StreamEgress` trait
