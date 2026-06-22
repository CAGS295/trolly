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
- **Replay** — [`ReplayBuffer`](src/replay.rs) ring buffer stores flattened observation windows and step transitions (training loop stub).
- **PPO / WoLF-PPO** — optional [`ppo`](src/ppo/) module (`--features torch`) for on-policy actor–critic updates; see above.
- **Env** — [`Env`](src/env.rs) ties ingest → window → step → egress; see `tests/smoke.rs` for an offline mock flow.

Training drivers, matrix-game harnesses, and checkpoint I/O are follow-on work (WP-019 / WP-020).

## RL toolchain analysis (WP-016)

Stack comparison, integration mapping, and train vs inference recommendations:

- **[RL training and inference toolchain analysis](docs/rl-toolchain-analysis.md)** — ADR covering `tch`/libtorch, Candle, Burn, ONNX Runtime, and Python/PyTorch sidecar options; primary decision is Python/offline training with ONNX Runtime inference (`onnx` feature planned), existing `torch` feature retained for research.
