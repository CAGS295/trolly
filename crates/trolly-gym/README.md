# trolly-gym

Training gym scaffold for reinforcement learning over trolly market streams.

## Default build (no libtorch)

CI and the default workspace build do **not** link libtorch:

```bash
cargo check -p trolly-gym
cargo test -p trolly-gym
```

## Libtorch build (`torch` feature)

Libtorch integration lives in `src/libtorch.rs` and is compiled only with the
`torch` feature. You must install [libtorch](https://pytorch.org/get-started/locally/)
and point the build at it (typically via `LIBTORCH` or `LIBTORCH_USE_PYTORCH=1`).

```bash
export LIBTORCH_USE_PYTORCH=1   # or LIBTORCH=/path/to/libtorch
cargo check -p trolly-gym --features torch
cargo test -p trolly-gym --features torch
```

The optional `tch` crate is pulled in only when `--features torch` is set.

## Architecture

- **Observations** — normalized [`StreamEvent`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) values from `trolly-stream` ingress are converted to feature vectors and kept in a rolling [`ObservationWindow`](src/observation.rs).
- **Actions** — discrete [`Action`](src/action.rs) values map to [`OutboundMessage`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) commands and dispatch through [`StreamEgress`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy).
- **Replay** — [`ReplayBuffer`](src/replay.rs) ring buffer stores flattened observation windows and step transitions (training loop stub).
- **Env** — [`Env`](src/env.rs) ties ingest → window → step → egress; see `tests/smoke.rs` for an offline mock flow.

Training loops, checkpoints, and GPU policies are out of scope for this crate scaffold.

## WoLF-PPO (WP-018)

### Rationale

Market-making is a multi-agent game; vanilla PPO can cycle or fail to converge
near Nash equilibria because a fixed learning rate cannot distinguish between
"already at equilibrium" and "need to adapt". **WoLF-PPO** (Win-or-Learn-Fast
Policy Proximal Optimization, [Ratcliffe et al., IEEE CoG 2019][paper_176])
adds a simple dual-rate mechanism that provably drives convergence to
Nash-Equilibrium Seeking (NES) strategies in matrix games, and generalises to
multi-agent trading environments.

### Algorithm overview

1. **PPO clipped surrogate** (Schulman et al., 2017):

   ```
   L^CLIP(θ) = E[min(r_t(θ)·Â_t, clip(r_t(θ), 1−ε, 1+ε)·Â_t)]
   L^VF(θ)  = MSE(V_θ(s_t), R_t)
   L(θ)     = L^CLIP − c1·L^VF + c2·H[π_θ(·|s_t)]
   ```

2. **WoLF dual learning rates** — rolling average payoff as NES payoff proxy:

   | Condition | Learning rate |
   |-----------|---------------|
   | `V > V̄` (winning — above rolling average) | `α_WIN = α_LOSE / 4` |
   | `V ≤ V̄` (losing — at or below rolling average) | `α_LOSE` |

   Smaller `α_WIN` prevents overshooting at equilibrium; larger `α_LOSE`
   allows fast recovery when losing.

3. **Actor-critic MLP** — shared trunk with tanh activations, categorical
   policy head (softmax over discrete actions), scalar value head.
   Default hidden sizes `[20, 20]` match the matrix-game experiments in
   Ratcliffe et al.

### Hyperparameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `clip_epsilon` (ε) | `f64` | `0.2` | PPO clipping range |
| `entropy_coef` (c2) | `f64` | `0.01` | Entropy bonus weight |
| `value_coef` (c1) | `f64` | `0.5` | Value loss weight |
| `ppo_epochs` | `usize` | `4` | Gradient epochs per rollout |
| `hidden_sizes` | `Vec<i64>` | `[20, 20]` | Shared MLP hidden layer widths |
| `lr` | `f64` | `0.01` | Base learning rate (PPO-only mode) |
| `use_adam` | `bool` | `false` | Adam optimizer; default is SGD |
| `alpha_lose` | `f64` | `0.01` | WoLF losing-regime learning rate |
| `alpha_win` | `f64` | `0.0025` | WoLF winning-regime rate (`= alpha_lose / 4`) |
| `payoff_window` | `usize` | `100` | Rolling average window for NES payoff estimate |

### Public API

All types live in `trolly_gym::ppo` (requires `--features torch`):

```rust
use trolly_gym::ppo::{PpoConfig, WolfPpoConfig, PpoTrainer, WolfPpoTrainer, RolloutBatch};

// Pure PPO
let mut trainer = PpoTrainer::new(obs_dim, num_actions, PpoConfig::default());
let loss = trainer.policy_update(&batch);

// WoLF-PPO
let cfg = WolfPpoConfig::default().with_alpha_lose(0.02);
let mut wolf = WolfPpoTrainer::new(obs_dim, num_actions, cfg);
let loss = wolf.policy_update(&batch, episode_return);
let winning = wolf.is_winning();   // true when V > rolling_avg
let lr = wolf.active_lr();         // α_WIN or α_LOSE
```

Collecting a `RolloutBatch`:

```rust
// Collect observations, actions, log_probs, returns, advantages
// then wrap in RolloutBatch (tch Tensors, CPU, Kind::Float / Int64)
let batch = RolloutBatch {
    observations: Tensor::randn(&[T, obs_dim], (Kind::Float, Device::Cpu)),
    actions: action_tensor,          // Kind::Int64 [T]
    old_log_probs: log_prob_tensor,  // Kind::Float [T]
    returns: returns_tensor,         // Kind::Float [T]
    advantages: advantage_tensor,    // Kind::Float [T]
};
```

### Optimizer note

SGD is the default per the paper's matrix-game experiments. Adam can be
selected via `PpoConfig { use_adam: true, .. }`. When using WoLF-PPO, the
dual LR selection overrides whatever base `lr` is set; `alpha_lose` and
`alpha_win` are the effective rates.

### Running the torch tests

```bash
export LIBTORCH_USE_PYTORCH=1
export LIBTORCH_BYPASS_VERSION_CHECK=1
export LD_LIBRARY_PATH="$(python3 -c 'import torch, os; print(os.path.dirname(torch.__file__))')/lib:$LD_LIBRARY_PATH"
export RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"

cargo test -p trolly-gym --features torch
```

### Citation

Ratcliffe, A., Byde, A., Sherlock, A. (2019).
**WoLF-PPO: A Novel Multi-agent Reinforcement Learning Approach to Algorithmic Trading**.
*IEEE Conference on Games (CoG) 2019.*
[paper_176](https://ieee-cog.org/2019/papers/paper_176.pdf)

---

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
