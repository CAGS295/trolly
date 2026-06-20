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

See the **WP-020 training loop** section below for rollout collection, the
`WolfPpoTrainDriver`, and checkpoint save/load.

## WoLF-PPO training loop and checkpoints (WP-020)

### Modules (`trolly_gym::train`, requires `--features torch`)

| Module | Contents |
|--------|---------|
| `train::rollout` | `OnPolicyTransition`, `RolloutCollector`, `compute_gae` |
| `train::driver` | `WolfPpoTrainDriver`, `TrainMetrics`, `TrainDriverConfig` |
| `train::checkpoint` | `save_checkpoint`, `load_checkpoint` |

### Rollout collection

[`RolloutCollector`](src/train/rollout.rs) steps a caller-supplied closure for
`horizon` timesteps, recording observations, sampled actions, log-probabilities,
and critic values.  After collection, `into_batch(bootstrap_value)` computes
GAE(γ, λ) returns and advantages and wraps everything into a [`RolloutBatch`]
compatible with `PpoTrainer`/`WolfPpoTrainer`.

```rust
use trolly_gym::train::rollout::{RolloutCollector, StepOutput};
use trolly_gym::ppo::{ActorCritic, PpoConfig};
use tch::nn;

let vs = nn::VarStore::new(tch::Device::Cpu);
let ac = ActorCritic::new(&vs, 4, 3, &PpoConfig::default());

let mut collector = RolloutCollector::new(/*horizon=*/64, /*gamma=*/0.99, /*lambda=*/0.95);
collector.collect(&ac, vec![0.0_f32; 4], |obs, _action| StepOutput {
    next_observation: obs,   // advance your env here
    reward: 0.0,
    done: false,
});
let batch = collector.into_batch(0.0);
```

The env-step closure receives the **current observation** and the **sampled
action index** and must return a `StepOutput` (next observation, reward, done).
In CI/tests a mock closure is used; in production wrap `Env::step`.

### Training driver

[`WolfPpoTrainDriver`](src/train/driver.rs) wraps `WolfPpoTrainer` with a
single `train_step` entry point that collects, computes GAE, and runs the
multi-epoch PPO/WoLF-PPO update:

```rust
use trolly_gym::train::{WolfPpoTrainDriver, TrainDriverConfig, StepOutput};
use trolly_gym::ppo::WolfPpoConfig;

let cfg = TrainDriverConfig {
    obs_dim: 4,
    num_actions: 3,
    horizon: 64,
    gamma: 0.99,
    gae_lambda: 0.95,
};
let mut driver = WolfPpoTrainDriver::new(cfg, WolfPpoConfig::default());

for _step in 0..100 {
    let metrics = driver.train_step(
        vec![0.0_f32; 4],               // initial observation
        |obs, _action| StepOutput {     // env closure
            next_observation: obs,
            reward: 0.0,
            done: false,
        },
        0.0,                            // bootstrap value
        None,                           // optional NES target for distance logging
    );
    println!(
        "policy_loss={:.4}  value_loss={:.4}  entropy={:.4}  lr={}",
        metrics.policy_loss, metrics.value_loss, metrics.entropy, metrics.active_lr
    );
}
```

#### `TrainMetrics` fields

| Field | Type | Description |
|-------|------|-------------|
| `policy_loss` | `f64` | Mean combined PPO loss (L^CLIP − c1·L^VF + c2·S) |
| `value_loss` | `f64` | Diagnostic value-MSE loss (same gradient; separate readout) |
| `entropy` | `f64` | Mean per-sample policy entropy H[π] over the rollout |
| `nes_distance` | `Option<f64>` | Euclidean distance to the provided NES target |
| `active_lr` | `f64` | WoLF learning rate used (α_WIN or α_LOSE) |
| `steps_collected` | `usize` | Env steps in this training step |

### Checkpoint save / load

Checkpoints are written with `tch::nn::VarStore::save`.  The recommended
extension is `.safetensors` (cross-language, readable from Python with the
[`safetensors`](https://github.com/huggingface/safetensors) library).  All
named parameters are stored; the network architecture must match on load.

```rust
use trolly_gym::train::checkpoint::{save_checkpoint, load_checkpoint};
use trolly_gym::ppo::{PpoTrainer, PpoConfig};

let mut trainer = PpoTrainer::new(4, 3, PpoConfig::default());
// … train …
save_checkpoint(&trainer.vs, "/tmp/actor_critic.safetensors").unwrap();

// ── Restore into a fresh network with the same architecture ───────
let mut trainer2 = PpoTrainer::new(4, 3, PpoConfig::default());
load_checkpoint(&mut trainer2.vs, "/tmp/actor_critic.safetensors").unwrap();
// trainer2.actor_critic now has the same weights as trainer.actor_critic
```

**Format notes:**

| Extension | Format | Notes |
|-----------|--------|-------|
| `.safetensors` | [SafeTensors](https://github.com/huggingface/safetensors) | **Recommended.** Cross-language; readable from Python via `safetensors.torch.load_file`. |
| `.ckpt` (or any non-`.pt`/`.bin`) | libtorch C++ module format | tch-native binary; not directly readable from Python. |

> **Warning:** do **not** use `.pt` or `.bin` extensions with `VarStore::save` /
> `VarStore::load`.  tch uses the libtorch C++ module format when *saving*
> (regardless of extension), but routes `.pt`/`.bin` to a pickle-based loader
> when *loading* — causing a format mismatch and a runtime error.

- Architecture must match: variable names and tensor shapes are fixed at
  network construction time and must align with the checkpoint.
- Device: saved/loaded on CPU in the current implementation.

### Env integration hook

`RolloutCollector::collect` accepts any `FnMut(Vec<f32>, i64) -> StepOutput`
closure, making it straightforward to wrap `Env::step`:

```rust
// Pseudocode — reward is still the stub in env.rs
let mut gym_env = Env::new(EnvConfig::new("BTCUSDT"), egress);
collector.collect(&ac, gym_env.observation_window().flattened(), |_obs, action_i| {
    let a = Action::from_index(action_i);  // adapt index → Action
    let step_result = gym_env.step(a).unwrap();
    StepOutput {
        next_observation: step_result.observation,
        reward: step_result.reward as f32,
        done: step_result.done,
    }
});
```

No live Binance connection is needed in CI — mock the closure with synthetic
observations and rewards (see unit tests in `src/train/`).

### Running the train tests

```bash
export LIBTORCH_USE_PYTORCH=1
export LIBTORCH_BYPASS_VERSION_CHECK=1
export RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"

cargo test -p trolly-gym                        # no libtorch (env, replay, games NES)
cargo test -p trolly-gym --features torch       # all tests including train + checkpoint
```

---

## Matrix-game validation harness (WP-019)

Validates WoLF-PPO on offline two-player zero-sum matrix games before any
live-stream integration. Games and NES targets are taken from the payoff tables
in Ratcliffe et al. (2019).

### Games

| Variant | Payoff matrix (row player) | Known NES |
|---------|---------------------------|-----------|
| Matching Pennies — standard | `[[1,−1],[−1,1]]` | P(H) = 0.5, P(T) = 0.5 |
| Matching Pennies — weighted (Table IIa) | `[[2,−1],[−1,1]]` | **P(H) = 0.4**, P(T) = 0.6 |
| Rock–Paper–Scissors — standard | skew-symmetric, ±1 | P(R)=P(P)=P(S) = 1/3 |
| Rock–Paper–Scissors — weighted (Table IIb) | `[[0,−2,2],[2,0,−1],[−2,1,0]]` | **P(R)=0.2, P(P)=0.4, P(S)=0.4** |

### Metric (paper Table I)

After a training run of N update steps, the metric is:

```
max_distance = max{ d(πₖ, NES) : k ∈ {N−9, …, N} }
```

where `d` is the Euclidean distance and πₖ are the row player's policy
probabilities extracted from the softmax of the actor logits.

### Self-play training loop

Both players are initialised with independent networks. At each update step:

1. A batch of `batch_size` interactions is sampled from the current joint policy.
2. Advantages are centred: `adv_t = r_t − mean(r)`.
3. PPO (or WoLF-PPO) gradient steps are applied to each player independently.
4. The row player's policy probabilities are recorded and compared to the NES.

The column player's payoff is `−row_payoff` (zero-sum); WoLF-PPO's episode
return is the mean batch payoff for that player.

### Public API

All types live in `trolly_gym::games` (requires `--features torch`):

```rust
use trolly_gym::games::{
    matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
    run_wolf_ppo_self_play, run_ppo_self_play, SelfPlayConfig,
};
use trolly_gym::ppo::WolfPpoConfig;

let game = matching_pennies_weighted();

// WoLF-PPO
let result = run_wolf_ppo_self_play(
    &game,
    &WEIGHTED_NES,
    SelfPlayConfig::default(),        // 200 updates, batch 64
    WolfPpoConfig::default().with_alpha_lose(0.1),
);
println!("max NES distance (last 10): {:.4}", result.max_distance_last_10);

// Standard PPO
let ppo_result = run_ppo_self_play(&game, &WEIGHTED_NES, SelfPlayConfig::default());
```

### Default test (always offline)

```bash
cargo test -p trolly-gym                        # no libtorch needed
cargo test -p trolly-gym --features torch       # includes torch-gated smoke tests
```

The always-run tests verify NES arithmetic and the distance metric with no
libtorch dependency. The torch-gated smoke tests prove a WoLF-PPO training
step completes and returns a finite NES distance.

### Extended benchmark (`#[ignore]`)

Reproduce the paper trend: WoLF-PPO converges closer to the NES than standard
PPO on weighted Matching Pennies with `α_LOSE ∈ {0.1, 0.01}`.

```bash
export LIBTORCH_USE_PYTORCH=1
export LIBTORCH_BYPASS_VERSION_CHECK=1
export RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"
cargo test -p trolly-gym --features torch -- --include-ignored \
    benchmark_wolf_ppo_closer_to_nes_weighted_matching_pennies
```

Expected output: `WoLF-PPO (0.1)` and/or `WoLF-PPO (0.01)` mean max-distance
≤ PPO mean max-distance over 10 seeds × 200 updates.

---

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
