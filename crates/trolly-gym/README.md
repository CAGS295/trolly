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

Nash-equilibrium-oriented RL via **Win or Learn Fast PPO** ([Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf)) on the primary `tch`/libtorch stack.

Build and test with libtorch installed:

```bash
export LIBTORCH_USE_PYTORCH=1   # or LIBTORCH=/path/to/libtorch
cargo test -p trolly-gym --features torch
```

### API (feature `torch`)

| Type | Role |
|------|------|
| [`PpoConfig`](src/ppo/config.rs) | Clip ε, value/entropy coefficients, PPO epochs, hidden layers `[20, 20]`, SGD/Adam LR |
| [`WolfPpoConfig`](src/ppo/config.rs) | Wraps `PpoConfig` + α_LOSE (α_WIN = α_LOSE / 4) and payoff EMA for estimated NES |
| [`ActorCritic`](src/ppo/actor_critic.rs) | Shared MLP + categorical policy head + scalar value head |
| [`RolloutBatch`](src/ppo/batch.rs) | On-policy tensors for one update step |
| [`PpoTrainer`](src/ppo/trainer.rs) | Fixed-LR clipped surrogate updates |
| [`WolfPpoTrainer`](src/ppo/trainer.rs) | Dual LR: α_WIN when current payoff beats estimated NES, else α_LOSE |

Objective (paper Eq. 1): maximize `L^CLIP − c1·L^VF + c2·S` with clipped probability ratios, squared value error, and entropy bonus. WoLF tracks a rolling average payoff as the estimated NES return and switches learning rate before each update.

Default `cargo test -p trolly-gym` (no libtorch) is unchanged; torch-gated tests live under `src/ppo/` and `tests/ppo_torch.rs`.

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
