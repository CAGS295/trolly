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

## PPO and WoLF-PPO (`torch` feature)

Actor–critic training lives in [`src/ppo/`](src/ppo/) and is exported from the crate root when `--features torch` is enabled:

- [`PpoConfig`](src/ppo/config.rs) / [`WolfPpoConfig`](src/ppo/config.rs) — clip ε, value coef c1, entropy coef c2, PPO epochs, hidden layers (default `[20, 20]`), optimizer (SGD default; Adam optional).
- [`ActorCritic`](src/ppo/actor_critic.rs) — shared MLP trunk with stochastic categorical policy head and scalar value head.
- [`PpoTrainer::policy_update`](src/ppo/trainer.rs) — clipped surrogate (L^CLIP), value loss (L^VF), entropy bonus (S) per Eq. 1 in Ratcliffe et al. (2019).
- [`WolfPpoTrainer::policy_update`](src/ppo/trainer.rs) — WoLF extension: rolling **average payoff** as an estimated Nash-equilibrium payoff; dual learning rates α_WIN = α_LOSE / 4, selecting α_WIN when the current batch payoff exceeds the estimate (otherwise α_LOSE).

### Why WoLF-PPO?

In multi-agent settings, plain PPO can oscillate away from a **Nash equilibrium strategy (NES)** when the equilibrium is not the maximum-entropy policy (see weighted Matching Pennies / Rock–Paper–Scissors in the paper). **Win or Learn Fast (WoLF)** slows learning when the agent is “winning” (doing better than the estimated equilibrium payoff) and speeds up when losing, improving convergence toward a stable NES. WoLF-PPO combines this schedule with PPO’s clipped policy update.

**Reference:** D. S. Ratcliffe, K. Hofmann, S. Devlin, *Win or Learn Fast Proximal Policy Optimisation*, IEEE CoG 2019. [paper_176](https://ieee-cog.org/2019/papers/paper_176.pdf)

### Hyperparameters (matrix-game defaults)

| Parameter | Symbol | Default | Notes |
|-----------|--------|---------|-------|
| Clip range | ε | `0.2` | PPO probability-ratio clip |
| Value coef | c1 | `0.5` | `PpoConfig::value_coef` |
| Entropy coef | c2 | `0.01` | `PpoConfig::entropy_coef` |
| PPO epochs | — | `4` | Updates per rollout batch |
| Hidden layers | — | `[20, 20]` | Paper feed-forward net |
| Optimizer | — | SGD | Set `optimizer: OptimizerKind::Adam` for Adam |
| WoLF lose rate | α_LOSE | `0.01` | `WolfPpoConfig::alpha_lose` |
| WoLF win rate | α_WIN | α_LOSE / 4 | `WolfPpoConfig::alpha_win()` |

```bash
export LIBTORCH=/path/to/libtorch   # or LIBTORCH_USE_PYTORCH=1
export CXX=g++                      # if default `c++` is clang without libstdc++ headers
cargo test -p trolly-gym --features torch
```

## Architecture

- **Observations** — normalized [`StreamEvent`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) values from `trolly-stream` ingress are converted to feature vectors and kept in a rolling [`ObservationWindow`](src/observation.rs).
- **Actions** — discrete [`Action`](src/action.rs) values map to [`OutboundMessage`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) commands and dispatch through [`StreamEgress`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy).
- **Replay** — [`ReplayBuffer`](src/replay.rs) ring buffer stores flattened observation windows and step transitions; [`RolloutCollector`](src/replay.rs) holds on-policy PPO trajectories (log-prob + value).
- **PPO / WoLF-PPO** — optional [`ppo`](src/ppo/) module (`--features torch`) for on-policy actor–critic updates; see above.
- **Matrix games** — optional [`games`](src/games/) module validates PPO and WoLF-PPO against known Nash equilibria from Ratcliffe et al. (2019); see below.
- **Env** — [`Env`](src/env.rs) ties ingest → window → step → egress; see `tests/smoke.rs` for an offline mock flow.

## Training loop and checkpoint I/O (WP-020)

On-policy rollout collection, training drivers, and actor–critic checkpoint save/load (`torch` feature):

- [`RolloutStep`](src/replay.rs) / [`RolloutCollector`](src/replay.rs) — parallel on-policy buffer alongside [`ReplayBuffer`](src/replay.rs) [`Transition`](src/replay.rs) rows; stores observation, action, behaviour log-prob, value, reward, and done for PPO updates.
- [`rollout_collector_to_batch`](src/train/collector.rs) — converts collected steps into [`RolloutBatch`](src/ppo/batch.rs) (returns = rewards, advantages = returns − value).
- [`collect_env_rollout_step`](src/train/env_rollout.rs) — samples from [`ActorCritic`](src/ppo/actor_critic.rs), calls [`Env::step`](src/env.rs), and appends to a collector (mock stream events via [`Env::ingest_event`](src/env.rs); reward stub unchanged).
- [`WolfPpoTrainingDriver`](src/train/driver.rs) / [`PpoTrainingDriver`](src/train/driver.rs) — collect rollouts → multi-epoch PPO/WoLF-PPO updates → [`TrainStepMetrics`](src/train/driver.rs) (policy loss, value loss, entropy, mean reward, active WoLF LR, optional NES distance).
- **Checkpoint format** — libtorch [`VarStore`](https://docs.rs/tch/latest/tch/nn/struct.VarStore.html) binary (`.ot`); [`save_checkpoint`](src/train/checkpoint.rs) / [`load_checkpoint`](src/train/checkpoint.rs) on [`PpoTrainer`](src/ppo/trainer.rs) and [`WolfPpoTrainer`](src/ppo/trainer.rs). Round-trip restores forward-pass outputs on CPU.

```bash
export LIBTORCH=/path/to/libtorch   # or LIBTORCH_USE_PYTORCH=1
cargo test -p trolly-gym --features torch
```

## Matrix-game validation harness (WP-019)

Offline two-player zero-sum matrix games reproduce the paper’s experimental setup before stream latency and reward engineering:

| Game | Variant | Known NES |
|------|---------|-----------|
| Matching Pennies | standard | `P(H) = 0.5` |
| Matching Pennies | weighted (Table IIa) | `P(H) = 0.4` |
| Rock–Paper–Scissors | standard | uniform `1/3` |
| Rock–Paper–Scissors | weighted (Table IIb) | `P(R)=0.2`, `P(P)=0.4` |

Self-play drives [`PpoTrainer`](src/ppo/trainer.rs) and [`WolfPpoTrainer`](src/ppo/trainer.rs) with shared hyperparameters (SGD, hidden `[20, 20]`, clip ε=`0.2`). The harness reports **Euclidean distance** of the learned policy from the known NES; per run it takes the **max distance over the last 10 policy updates** (paper Table I), then averages over independent seeds (50-run capability; CI uses fewer).

Pure game definitions and metrics compile without libtorch:

```bash
cargo test -p trolly-gym --test matrix_games
```

Self-play smoke and benchmarks require the `torch` feature:

```bash
export LIBTORCH=/path/to/libtorch   # or LIBTORCH_USE_PYTORCH=1
cargo test -p trolly-gym --features torch --test matrix_games
```

Extended benchmark (weighted Matching Pennies, reproduces Table I trend — WoLF-PPO closer to NES than PPO):

```bash
export LIBTORCH=/path/to/libtorch
# optional: full paper 50-run average
export TROLLY_MATRIX_BENCHMARK_RUNS=50
cargo test -p trolly-gym --features torch --test matrix_games \
  weighted_matching_pennies_wolf_closer_to_nes_than_ppo -- --ignored --nocapture
```

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
