# trolly-gym

libtorch.rs training gym scaffold over trolly stream events. Feeds normalized
[`StreamEvent`](https://docs.rs/trolly-strategy/latest/trolly_strategy/struct.StreamEvent.html)
updates into observation windows, records transitions in a replay buffer stub,
and dispatches outbound [`Command`](https://docs.rs/trolly-strategy/latest/trolly_strategy/enum.Command.html)
values through the strategy egress API.

Training loops and model checkpoints are out of scope for this crate; it provides
layout and stream integration hooks only.

## Default build (no libtorch)

The default feature set does not depend on libtorch or GPU libraries:

```bash
cargo check -p trolly-gym
cargo test -p trolly-gym
```

Core types:

- [`Env`](src/env.rs) — ingest stream events, produce observation vectors, step with actions
- [`RingBuffer`](src/buffer.rs) / [`ReplayBuffer`](src/buffer.rs) — rolling feature windows and transition stubs
- [`event_to_features`](src/observation.rs) — map a stream event to a fixed `f64` feature row

## libtorch integration (`torch` feature)

Enable the optional torch module for the libtorch.rs hook point:

```bash
cargo check -p trolly-gym --features torch
cargo test -p trolly-gym --features torch
```

The `torch` feature gates [`TorchPolicy`](src/torch.rs). The current implementation is a
pure-Rust stub so CI does not require libtorch installed. To wire real inference:

1. Install [libtorch](https://pytorch.org/get-started/locally/) (CPU or CUDA build).
2. Set environment variables expected by [`tch`](https://docs.rs/tch), typically
   `LIBTORCH` pointing at the libtorch directory and `LD_LIBRARY_PATH` including
   `$LIBTORCH/lib`.
3. Add an optional `tch` dependency behind `torch` in `Cargo.toml` and replace
   `TorchPolicy::infer` with `tch::CModule` checkpoint loading and tensor forward.

## Tests

Offline smoke tests push mock observations (no live network or GPU):

```bash
cargo test -p trolly-gym
```
