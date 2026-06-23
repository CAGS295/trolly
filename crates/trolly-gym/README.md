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

## WoLF-PPO (`ppo` module)

When built with `--features torch`, `trolly-gym` exposes [`ppo`](src/ppo/mod.rs): a
**Proximal Policy Optimization (PPO)** implementation and its **WoLF-PPO** extension from
[Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf).

### Rationale

In multi-agent settings the learning problem is non-stationary: opponent policies change over
time, so single-agent MDP guarantees no longer apply. A **Nash equilibrium strategy (NES)**
provides a stable policy with a known minimum payoff against rational opponents. Standard PPO
can drift away from the NES when the equilibrium is not the maximum-entropy policy (e.g.
weighted matrix games). **Win or Learn Fast (WoLF)** varies the learning rate: learn **fast**
when underperforming the estimated NES payoff (`α_LOSE`), and **slow** when outperforming it
(`α_WIN`), encouraging convergence toward the NES rather than exploitative cycles.

WoLF-PPO combines PPO’s clipped surrogate with this dual-rate schedule. The implementation
tracks a **rolling average payoff** as an estimated NES payoff (per the paper’s matrix-game
experiments) and selects `α_WIN = α_LOSE / 4` when current expected payoff exceeds the
estimate, otherwise `α_LOSE`.

### Objective (Eq. 1)

The policy update maximizes:

`L = E[L^CLIP − c1·L^VF + c2·S]`

where `L^CLIP` is the clipped probability-ratio surrogate, `L^VF` is the squared value error,
and `S` is the policy entropy bonus.

### Public API

| Type | Role |
|------|------|
| [`PpoConfig`](src/ppo/config.rs) | Clip ε, value coef `c1`, entropy coef `c2`, PPO epochs, hidden layers, optimizer |
| [`WolfPpoConfig`](src/ppo/config.rs) | Wraps `PpoConfig` plus `α_LOSE` and win/lose ratio |
| [`ActorCritic`](src/ppo/actor_critic.rs) | Shared-trunk MLP; default hidden `[20, 20]`; categorical policy + value head |
| [`PpoTrainer`](src/ppo/trainer.rs) | Multi-epoch vanilla PPO `policy_update` |
| [`WolfPpoTrainer`](src/ppo/trainer.rs) | WoLF learning-rate selection + PPO update |
| [`RolloutBatch`](src/ppo/rollout.rs) | On-policy tensors for one update step (WP-020 will collect these) |

**Optimizer:** SGD is the default (paper choice). Set `PpoConfig.optimizer = OptimizerKind::Adam`
for Adam; note that Adam’s adaptive moments can interact with WoLF’s explicit LR switching.

### Hyperparameters (paper defaults)

| Parameter | Default | Notes |
|-----------|---------|-------|
| `clip_epsilon` | `0.2` | PPO clip ε |
| `value_coef` (`c1`) | `0.5` | Value loss weight |
| `entropy_coef` (`c2`) | `0.01` | Entropy bonus weight |
| `ppo_epochs` | `4` | Passes over each rollout |
| `alpha_lose` | `0.1` | WoLF “lose fast” rate (paper Table I) |
| `win_lose_ratio` | `4` | `α_WIN = α_LOSE / 4` |
| `hidden_layers` | `[20, 20]` | Matrix-game MLP in paper |

### Example

```rust
use trolly_gym::ppo::{PpoConfig, RolloutBatch, WolfPpoConfig, WolfPpoTrainer};

let mut trainer = WolfPpoTrainer::new(obs_dim, n_actions, WolfPpoConfig::default());
// Build RolloutBatch from collected trajectories (see WP-020).
let metrics = trainer.policy_update(&batch);
assert!(metrics.wolf_lr.lr > 0.0);
```

### Citation

```bibtex
@inproceedings{RatcliffeHD19,
  author    = {Dino Stephen Ratcliffe and Katja Hofmann and Sam Devlin},
  title     = {Win or Learn Fast Proximal Policy Optimisation},
  booktitle = {IEEE Conference on Games (CoG)},
  year      = {2019},
  doi       = {10.1109/CIG.2019.8848100}
}
```

## Matrix-game validation harness (`games` module)

When built with `--features torch`, `trolly_gym::games` implements the offline
two-player zero-sum matrix games from the WoLF-PPO paper for algorithm validation
before stream-backed trading policies (WP-019).

### Games

| Variant | NES (row player) |
|---------|------------------|
| Matching Pennies (standard) | `P(H) = P(T) = 0.5` |
| Matching Pennies (weighted, Table IIa) | `P(H) = 0.4` |
| Rock–Paper–Scissors (standard) | uniform `1/3` |
| Rock–Paper–Scissors (weighted, Table IIb) | `P(R)=0.2`, `P(P)=0.4`, `P(S)=0.4` |

### Self-play loop

[`run_self_play`](src/games/self_play.rs) drives two learning agents (row and column)
with shared [`PpoTrainer`](src/ppo/trainer.rs) or [`WolfPpoTrainer`](src/ppo/trainer.rs)
configuration. Each policy update collects on-policy rollouts against the opponent's
current policy and applies the WP-018 update step. The harness supports up to 50 independent
seeds via [`run_benchmark`](src/games/self_play.rs) (CI may use fewer).

### NES distance metric

Following paper Table I: after each update, compute the Euclidean distance between the
row player's learned action probabilities and the known NES. Report the **maximum**
distance over the last 10 policy updates per run; aggregate with the mean over runs.

### Tests

Default CI (no libtorch):

```bash
cargo test -p trolly-gym
```

Matrix-game smoke and unit tests (offline, requires libtorch):

```bash
export LIBTORCH_USE_PYTORCH=1   # or LIBTORCH=/path/to/libtorch
cargo test -p trolly-gym --features torch
```

Optional extended benchmark (reproduces paper trend — WoLF-PPO closer to NES than PPO
on **weighted** matching pennies at `α_LOSE ∈ {0.1, 0.01}`):

```bash
export LIBTORCH_USE_PYTORCH=1
cargo test -p trolly-gym --features torch matrix_games -- --ignored --test-threads=1
```

The benchmark uses fewer runs than the paper's 50 for local practicality; increase
`NUM_RUNS` in [`tests/matrix_games.rs`](tests/matrix_games.rs) for tighter reproduction.

## Architecture

- **Observations** — normalized [`StreamEvent`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) values from `trolly-stream` ingress are converted to feature vectors and kept in a rolling [`ObservationWindow`](src/observation.rs).
- **Actions** — discrete [`Action`](src/action.rs) values map to [`OutboundMessage`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) commands and dispatch through [`StreamEgress`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy).
- **Replay** — [`ReplayBuffer`](src/replay.rs) ring buffer stores flattened observation windows and step transitions (training loop stub).
- **Env** — [`Env`](src/env.rs) ties ingest → window → step → egress; see `tests/smoke.rs` for an offline mock flow.

Training loops, checkpoints, and GPU policies are out of scope for this crate scaffold.

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
