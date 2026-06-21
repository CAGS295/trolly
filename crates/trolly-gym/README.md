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
- **Replay** — [`ReplayBuffer`](src/replay.rs) ring buffer stores flattened observation windows and step transitions; [`OnPolicyRolloutBuffer`](src/replay.rs) holds PPO fields (log-prob, value) for on-policy updates.
- **Env** — [`Env`](src/env.rs) ties ingest → window → step → egress; see `tests/smoke.rs` for an offline mock flow.

Training loops and checkpoints live behind the `torch` feature (WP-020).

## WoLF-PPO (WP-018)

WoLF-PPO ([Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf)) extends clipped PPO with **Win or Learn Fast** learning rates for Nash-equilibrium-seeking self-play. Implementation lives behind the `torch` feature in `src/ppo/`:

- **`PpoConfig`** — clip ε, value coef `c1`, entropy coef `c2`, PPO epochs, SGD/Adam learning rate.
- **`WolfPpoConfig`** — adds `alpha_lose` (α_LOSE); α_WIN = α_LOSE / 4; selects α_WIN when current expected payoff exceeds the rolling NES estimate, else α_LOSE.
- **`ActorCritic`** — MLP actor–critic with default hidden layers `[20, 20]` (paper matrix-game setup), categorical policy head + scalar value head.
- **`PpoTrainer::policy_update`** / **`WolfPpoTrainer::policy_update`** — multi-epoch clipped surrogate (L^CLIP), value loss (L^VF), and entropy bonus on a [`RolloutBatch`](src/ppo/trainer.rs).

```bash
export LIBTORCH_USE_PYTORCH=1
cargo test -p trolly-gym --features torch
```

Matrix-game validation (WP-019) and the full training driver (WP-020) build on this module.

## Training loop and checkpoints (WP-020)

The `torch` feature adds `src/train/` — rollout collection, WoLF-PPO driver, and checkpoint I/O:

- **`OnPolicyRolloutBuffer`** — on-policy trajectories (obs, action, log-prob, value, reward, done); converts to [`RolloutBatch`](src/ppo/trainer.rs) via [`rollout_buffer_to_batch`](src/train/rollout.rs).
- **`WolfPpoTrainingDriver`** — collect rollouts → multi-epoch WoLF-PPO update → log scalar metrics (policy/value loss, entropy, NES distance when a reference is supplied, active WoLF LR).
- **`collect_env_rollout`** — feed mock [`StreamEvent`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) slices through [`Env::ingest_event`](src/env.rs) / [`Env::step`](src/env.rs) (reward stub ok; no live Binance in CI).
- **Checkpoints** — `save_checkpoint` / `load_checkpoint` write `*.safetensors` (VarStore weights) plus `*.meta.json` sidecar (`format_version`, `obs_dim`, `action_count`, `hidden_layers`).

```bash
export RUSTUP_TOOLCHAIN=stable
unset LIBTORCH_USE_PYTORCH
export LIBTORCH=/tmp/libtorch
export LD_LIBRARY_PATH=/tmp/libtorch/lib
cargo test -p trolly-gym --features torch
```

Integration tests in `tests/train_loop.rs` cover checkpoint round-trip and a short env-backed train loop.

## Matrix-game validation (WP-019)

Offline two-player zero-sum matrix games from [Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf) validate WoLF-PPO before stream-backed training:

| Game | Variant | NES |
|------|---------|-----|
| Matching Pennies | standard | P(H)=0.5 |
| Matching Pennies | weighted (Table IIa) | P(H)=0.4 |
| Rock–Paper–Scissors | standard | uniform 1/3 |
| Rock–Paper–Scissors | weighted (Table IIb) | P(R)=0.2, P(P)=0.4, P(S)=0.4 |

Implementation lives in `src/games/`:

- **`MatrixGame`** — payoff matrix + known NES probabilities.
- **`euclidean_distance` / `max_distance_last_n`** — Table I metric (max distance over the last 10 policy updates per run).
- **`run_matrix_experiment`** (`torch` feature) — self-play loop driving `PpoTrainer` or `WolfPpoTrainer` with shared setup (50-run capable via `run_matrix_experiments`).

Default CI runs offline metric/game tests without libtorch:

```bash
cargo test -p trolly-gym
```

Torch-backed smoke and benchmark tests:

```bash
export LIBTORCH=/tmp/libtorch
export LD_LIBRARY_PATH=/tmp/libtorch/lib
export RUSTUP_TOOLCHAIN=stable
unset LIBTORCH_USE_PYTORCH
cargo test -p trolly-gym --features torch
```

Optional extended benchmark (weighted Matching Pennies, reproduces paper trend — WoLF-PPO closer to NES than PPO):

```bash
cargo test -p trolly-gym --features torch weighted_matching_pennies_wolf_beats_ppo -- --ignored --nocapture
```

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
