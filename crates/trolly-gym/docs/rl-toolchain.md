# ADR-001: RL training and inference toolchain for `trolly-gym`

| Field | Value |
|-------|-------|
| Status | **Accepted** (WP-016) |
| Date | 2026-06-17 |
| Scope | Stream-fed RL over `trolly-stream` / `trolly-strategy`; analysis only — no training loop in this WP |

## Context

WP-011 landed a feature-gated gym scaffold: [`Env`](../src/env.rs) ingests normalized [`StreamEvent`](../../trolly-strategy/src/lib.rs) values, maintains an [`ObservationWindow`](../src/observation.rs), records [`Transition`](../src/replay.rs) rows, and dispatches discrete [`Action`](../src/action.rs) values through [`StreamEgress`](../../trolly-strategy/src/egress.rs). Optional [`libtorch`](../src/libtorch.rs) hooks exist behind `--features torch`.

Before committing GPU CI, checkpoint formats, or a production inference path, we need a toolchain decision that respects:

1. **Default workspace builds** — `cargo check --workspace` must not require libtorch, CUDA, or Python.
2. **Stream hot path** — depth and execution events arrive asynchronously; the policy must not block ingress or amplify backpressure.
3. **Market-making / execution RL** — mix of on-policy (PPO), off-policy (DQN, SAC), and offline batch training from replay logs.
4. **Split lifecycle** — heavy offline training vs sub-ms to low-ms online inference with model hot-swap and deterministic fallbacks.

## Integration surface (shared by all candidates)

```text
trolly-stream ingress
        │
        ▼
  Env::ingest_event / ingest_message
        │  features_from_event → ObservationWindow::push
        │  ReplayBuffer::push_observation_window (prefill stub)
        ▼
  flattened Vec<f32>  ──►  policy forward (training / inference hook)
        │
        ▼
  Env::step(Action)  ──►  Action::dispatch → StreamEgress → OutboundMessage
        │                  ReplayBuffer::push_step
        ▼
  StepResult { observation, reward, done }
```

| Hook | Type today | Policy / training touchpoint |
|------|------------|------------------------------|
| Observation build | `features_from_event` → 7-float depth frame or kind tag | Feature engineering; normalization stats; multi-symbol tensors |
| Model input | `ObservationWindow::flattened()` → `Vec<f32>` | Fixed-shape tensor `(window_frames × feature_dim)` |
| Action egress | `Action::{Hold,Buy,Sell}` → `OutboundMessage` | Logits → argmax / sampled discrete action; continuous sizing is a follow-on WP |
| Replay | `ReplayBuffer` ring, `Transition` rows | Off-policy sampling, offline dataset export, prioritized replay |
| Reward | `reward_stub` in `env.rs` | PnL, inventory, spread capture — separate WP |
| Torch stub | `libtorch::observation_tensor`, `stub_forward` | `torch` feature only |

### Stream latency and backpressure

- **Ingress is synchronous** — `ingest_event` runs on the stream handler thread (or caller). Feature extraction is O(1) per event; window roll is bounded by `window_frames`.
- **`step` clones the last observation** — acceptable for scaffold; inference should read `window.flattened()` by reference or a pre-allocated buffer to avoid alloc on the action loop.
- **Policy latency budget** — target **&lt;1 ms** p99 on CPU for small MLPs (inference); **1–10 ms** tolerable for transformer-ish models if event rate is ≤100 Hz per symbol. Anything slower must run off the ingress thread (dedicated inference task with latest-observation-wins semantics).
- **Backpressure** — if `step` or egress `dispatch` blocks (network, venue rate limits), the gym must not stall `ingest_event`. Production layout: decouple **observe** (always ingest) from **act** (rate-limited egress queue) with explicit drop/ coalesce policy.
- **Deterministic fallback** — when inference fails, times out, or model version mismatches, fall back to `Action::Hold` or a rule-based policy wired outside ML (no panic on hot path).

## Candidate stacks

### 1. `tch` / libtorch (current `torch` feature)

| Aspect | Assessment |
|--------|------------|
| **Role** | Hybrid Rust runtime + LibTorch for training-adjacent work and TorchScript inference |
| **GPU / CPU** | Full CUDA / CPU via LibTorch; MPS on macOS with matching libtorch build |
| **Install burden** | **High** — download libtorch (~2 GB), set `LIBTORCH` or `LIBTORCH_USE_PYTORCH=1`, align CUDA toolkit with PyTorch wheel. Breaks headless CI unless cached or skipped. |
| **CI feasibility** | **Poor by default** — keep behind `torch` feature; optional manual/scheduled job with libtorch cache. Unit tests in `libtorch.rs` only run with `--features torch`. |
| **Training algorithms** | No built-in RL — roll your own or call into C++ LibTorch optimizers. Ecosystem parity with PyTorch for **custom** PPO/SAC loops in Rust is possible but immature vs Python. |
| **Inference** | Load `torch.jit` / traced modules; GPU batching supported. Latency competitive with PyTorch C++ frontend. |
| **Integration fit** | **Excellent** — `observation_tensor` already maps `flattened()` → `Tensor`. Extend with `Policy` trait impl. Training loop would live in Rust using `tch::nn` or export weights from Python. |

### 2. Candle (Hugging Face)

| Aspect | Assessment |
|--------|------------|
| **Role** | Pure-Rust tensor library; strongest story for **inference** and small fine-tuning experiments |
| **GPU / CPU** | CUDA and Metal backends; CPU always available |
| **Install burden** | **Low** — crates.io dependency only; no external shared libs |
| **CI feasibility** | **Good** — CPU builds in default CI; GPU jobs optional |
| **Training algorithms** | General autograd + optimizers; **no** first-class RL library. PPO/SAC from scratch is weeks of work. |
| **Inference** | Fast CPU inference for small models; good for embedded policies |
| **Integration fit** | Map `Vec<f32>` → `Tensor::from_slice`; implement forward in Rust. Checkpoint format differs from PyTorch — need conversion pipeline or train in Candle natively. |

### 3. Burn

| Aspect | Assessment |
|--------|------------|
| **Role** | Pure-Rust deep learning with training-first ergonomics |
| **GPU / CPU** | WGPU (cross-vendor), CUDA, Metal |
| **Install burden** | **Low–medium** — crates.io; WGPU may pull heavier dev deps |
| **CI feasibility** | **Good** on CPU backend |
| **Training algorithms** | Training loops are first-class; RL still **bring-your-own** (no Stable-Baselines equivalent). Feasible for custom PPO with more boilerplate than Python. |
| **Inference** | Supported via same graph; less production track record than ONNX for frozen deploy |
| **Integration fit** | Similar to Candle — new `Policy` backend, own checkpoint format. Attractive if we commit to **Rust-only** training long term; high upfront cost for MM RL. |

### 4. ONNX Runtime (`ort`)

| Aspect | Assessment |
|--------|------------|
| **Role** | **Inference-only** runtime for frozen policies exported from PyTorch / other trainers |
| **GPU / CPU** | CPU, CUDA, TensorRT EPs; `ort` crate links prebuilt ORT binaries |
| **Install burden** | **Medium** — ORT shared library or `ort` download script; smaller than full libtorch |
| **CI feasibility** | **Good** — CPU EP runs in CI; CUDA EP optional |
| **Training algorithms** | **None** — train elsewhere, export `.onnx` |
| **Inference** | **Best fit for production** — optimized graphs, predictable latency, versioned model files, easy hot-swap (reload session) |
| **Integration fit** | New feature-gated module (proposed `onnx`): `flattened()` → `ort::Value`; outputs map to `Action` indices. Decouples training stack from runtime. |

### 5. Python / PyTorch sidecar (IPC bridge)

| Aspect | Assessment |
|--------|------------|
| **Role** | **Offline training** and optional **remote inference** via IPC (Unix socket, gRPC, shared memory) |
| **GPU / CPU** | Full PyTorch + CUDA; Ray, SB3, CleanRL, d3rlpy for offline RL |
| **Install burden** | **Outside Rust workspace** — Python venv, `requirements-rl.txt`; no impact on `cargo check --workspace` |
| **CI feasibility** | **Excellent for training** — separate Python job; Rust CI unchanged. Inference sidecar adds ops complexity. |
| **Training algorithms** | **Best coverage** — PPO, SAC, DQN, TD3, offline CQL/IQL from replay Parquet without rewriting algorithms |
| **Inference** | **Too slow / fragile for sub-ms loop** if per-step IPC + GIL; acceptable for research sims, not prod hot path |
| **Integration fit** | Export `ReplayBuffer::snapshot()` → JSON/Parquet/Arrow; import checkpoints as ONNX or TorchScript for Rust runtime. Sidecar **does not** sit on `Env::step` in production. |

## RL algorithm families vs stack

| Family | MM / execution use | Needs | `tch` | Candle | Burn | ONNX (`ort`) | Python sidecar |
|--------|-------------------|-------|-------|--------|------|--------------|----------------|
| On-policy (PPO, A2C) | Sim-to-live policy updates, stable baselines | Vectorized rollouts, advantage est. | Custom loop | Custom | Custom | Infer only | **SB3 / CleanRL** |
| Off-policy (DQN, SAC, TD3) | Sample-efficient from `ReplayBuffer` | Replay sampling, target nets | Custom | Custom | Custom | Infer only | **SB3 / Ray** |
| Offline / batch from replay | Train on logged stream transitions | Static dataset, no live env | Export/import | Export/import | Export/import | Infer only | **d3rlpy, RLlib offline** |
| Imitation / BC warm-start | Bootstrap from rule strategy | Supervised loss | Yes | Yes | Yes | Yes | **Easiest in PyTorch** |

**Without full rewrite:** Python sidecar covers all families today. `tch` can mirror a single algorithm if we port one trainer. Candle/Burn require owning the full RL loop. `ort` is inference-only — train anywhere, deploy one graph.

## Recommendations

### Offline training (batch replay, checkpoints, experiment tracking)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | **Python / PyTorch sidecar** | Fastest path to PPO, SAC, DQN, and offline RL on exported `ReplayBuffer` data; W&B / MLflow / TensorBoard ecosystem; no new default Rust deps |
| **Secondary** | **`torch` feature (`tch`)** | Rust-native experiments that must share types with `trolly-gym` without IPC; reuse LibTorch optimizers for small custom loops |
| **Defer** | Candle, Burn | Pure-Rust training is a multi-month investment; revisit if Python ops burden becomes unacceptable |

**Checkpoint contract:** train in PyTorch → export **ONNX** (primary deploy artifact) and optionally **TorchScript** (fallback for `torch` feature debugging).

### Online inference (sub-ms to low-ms loop, hot-swap, fallbacks)

| Priority | Choice | Rationale |
|----------|--------|-----------|
| **Primary** | **ONNX Runtime** (proposed `onnx` feature) | No libtorch in prod; fast CPU inference; atomic model file swap + session reload; deterministic ORT CPU EP |
| **Fallback** | **`torch` feature** | Load TorchScript when ONNX export loses ops or for research parity on GPU |
| **Avoid on hot path** | Python sidecar | IPC + GIL adds ms–ms jitter; use only in lab / shadow mode |

**Hot-swap:** versioned model path (`policy-v3.onnx`); background thread builds new `ort::Session`; atomic pointer swap; on failure keep previous session and emit metric. **Deterministic fallback:** rule policy or `Action::Hold` when session is `None` or forward returns error.

## Decision

| Role | Toolchain | Cargo feature |
|------|-----------|---------------|
| **Primary offline training** | Python / PyTorch sidecar (replay export → train → ONNX export) | None in `trolly-gym` (separate `tools/` or `python/` tree in follow-on WP) |
| **Primary online inference** | ONNX Runtime (`ort`) | Proposed `onnx` feature (not implemented in WP-016) |
| **Optional fallback inference** | `tch` / libtorch | Existing `torch` feature — **keep** |
| **Not selected for near term** | Candle, Burn | Document only; no features until a Rust-only training WP is scheduled |

### Feature gating in `trolly-gym`

| Feature | Default | Purpose |
|---------|---------|---------|
| *(none)* | **yes** | Env, observation, replay, action — CI and production scaffold |
| `torch` | no | LibTorch tensor + TorchScript inference hook (`libtorch.rs`) |
| `onnx` | no | ORT session inference hook (follow-on WP) |
| `python-bridge` | no | Optional IPC client for lab / shadow inference (follow-on WP) |

No new features or dependencies are added in WP-016; the table records the intended layout.

## Follow-on work items (not in WP-016)

| ID | Title | Depends on | Notes |
|----|-------|------------|-------|
| WP-018 | `Policy` trait + deterministic fallback | WP-016 | `fn act(&mut self, obs: &[f32]) -> Action`; default rule policy |
| WP-019 | Replay export / import (Parquet or Arrow) | WP-016 | Sidecar training input; schema for `Transition` |
| WP-020 | Python training sidecar + IPC protocol | WP-019 | PPO/SAC templates; ONNX export script |
| WP-021 | `onnx` feature + ORT inference hook | WP-018 | Model hot-swap, CPU EP tests in CI |
| WP-022 | Off-policy training loop stub (Rust) | WP-018, WP-019 | Sample `ReplayBuffer`, no GPU required |
| WP-023 | Optional GPU CI matrix | WP-021 | Cached libtorch / CUDA ORT; `#[ignore]` local |
| WP-024 | Reward model + PnL-shaped reward | WP-010, WP-018 | Replace `reward_stub` |
| WP-025 | Async inference task (decouple ingest / act) | WP-018 | Latest-observation-wins; egress rate limit |

## Summary matrix

| Stack | Train | Infer (hot) | Default CI | libtorch | Best for trolly-gym |
|-------|-------|-------------|------------|----------|---------------------|
| `tch` | Possible, immature | Good | Feature-gated | **Required** | Research parity, TorchScript fallback |
| Candle | DIY | Good CPU | Easy | No | Future pure-Rust infer experiments |
| Burn | DIY | Good | Easy | No | Future pure-Rust training experiments |
| `ort` | No | **Best** | Easy (CPU EP) | No | **Production inference** |
| Python sidecar | **Best** | Poor (hot path) | Rust CI unaffected | No | **Offline training** |

## References

- [`crates/trolly-gym/README.md`](../README.md) — build instructions (`torch` feature)
- [PyTorch LibTorch](https://pytorch.org/get-started/locally/)
- [Candle](https://github.com/huggingface/candle)
- [Burn](https://github.com/tracel-ai/burn)
- [ort crate](https://docs.rs/ort/)
- [Stable-Baselines3](https://stable-baselines3.readthedocs.io/)
