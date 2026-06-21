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

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
