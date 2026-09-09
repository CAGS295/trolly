# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Wire Gaussian/Liquid ladder checkpoints into `execute policy-demo` (load `gaussian_mlp` / `gaussian_liquid`, quantize, demo place). Live 3-way ONNX/Hold path already works.
- Keep local GPU training checkpoint metrics improving via ClickHouse `trolly.ticks`, trajectory FIFO replay, daily `--continue` slices, and checkpoint fingerprints.
- Add more exchange providers beyond Binance spot + USDM (stub scaffold landed; full venues remain).

## change log
+ `trolly-gym`: tanh-Gaussian inventory policy on the WP-032 ladder (`action: f32`, `step_target`, `microstructure/gaussian_mlp` mean-action vs Hold); Liquid-on-rungs FA (`gaussian_liquid`); quantize `a` onto `Action::dispatch` (WP-033–WP-035). Trainer guidance missing (silent).
+ `trolly-gym`: microstructure depth ladder `α(v)=δ+λv`, integral walk cost, parallel rung observations, resampled episode seeds (WP-032). Trainer guidance missing (silent); scheduled only ready item WP-032.
+ Policy demo runner: explicit guarded `--wait-for-user-data` mode waits on spot/USDM demo user-data streams after placement and fills typed reconciliation rows through existing exec ingest paths (WP-031).
+ Policy demo runner: typed receipt-to-user-stream reconciliation rows from captured spot/USDM user-data frames, CLI `--reconcile-user-data-json`, and offline mock reconciliation tests (WP-030).
+ Policy demo runner: deterministic demo client order IDs, typed spot/USDM placement receipts, CLI receipt output, and offline mock placement tests for the guarded demo path (WP-029).
+ Root policy demo runner: `execute policy-demo` dry-runs checkpoint/ONNX/hold policy output through `OrderOnlyEgress` into spot/USDM exec adapters, with demo REST placement guarded by `RUN_BINANCE_DEMO_ORDERS=1` (WP-028).
+ `trolly-gym`: ONNX Runtime policy provider behind `--features ort`, `ONNX_MODEL_PATH` harness selection, dynamic runtime loading, and docs for exported microstructure actors (WP-027).
+ `trolly-strategy` / `trolly-gym`: order-only egress bridge lets checkpoint policy harness output reach spot/USDM execution queues while ignoring Hold/Subscribe side effects (WP-026).
+ `trolly-gym`: checkpoint-or-hold policy harness loads saved safetensors under `--features torch`, feeds injected stream observations into Env, and dispatches normalized strategy messages (WP-025).
+ `trolly-gym`: PolicyProvider / HoldPolicy / torch-gated CheckpointPolicy, configurable stream reward, episode horizon, and Env policy stepping through Action::dispatch (WP-023).
+ Binance demo execution: guarded ignored spot/USDM market-order reconcile tests route fills through existing user-stream bookkeeping; default workspace tests stay offline (WP-024).
+ `trolly-gym`: local continue/weekday GPU paths `git fetch` then rebase onto `@{u}` before training so this host picks up cloud-agent pushes (dirty tree: fetch + warn, no reset).
+ `trolly-gym`: ClickHouse tick ingest/train join, freshness-aware FIFO replay, checkpoint hash fingerprints, `--continue-local` daily train slice.
+ Orchestrator run (2026-08-14): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-13): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-11): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-09): ready set empty - all workplan items are done; no worker wave scheduled.
+ Orchestrator run (2026-08-08): ready set empty - all workplan items are done; no worker wave scheduled.
+ Orchestrator run (2026-08-07): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-04): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-03): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-02): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-08-01): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-31): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-30): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-28): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-27): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-26): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-25): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-24): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-23): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-22): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-21): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-20): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-19): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-18): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-17): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-16): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-14): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-13): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-12): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-11): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-10): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-09): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-08): ready set empty - WP-001-WP-022 all done; no worker wave scheduled.
+ `trolly-gym`: synthetic microstructure training benchmark — stream-shaped obs, mark-to-market reward, checkpoint driver (WP-022).
+ `trolly-gym`: Liquid Neural Network actor-critic backend for WoLF-PPO, LNN matrix-game/checkpoint/train-loop smoke coverage, and MLP-vs-LNN docs (WP-021).
+ Orchestrator run (2026-07-06): ready set empty — WP-001–WP-020 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-05): ready set empty — WP-001–WP-020 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-04): ready set empty — WP-001–WP-020 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-03): ready set empty — WP-001–WP-020 all done; no worker wave scheduled.
+ Orchestrator run (2026-07-02): ready set empty — WP-001–WP-020 all done; no worker wave scheduled.
+ Orchestrator run (2026-06-30): ready set empty — WP-001–WP-020 all done; `cargo test --workspace --locked` pass.
+ `trolly-gym`: WoLF-PPO training loop, rollout collection, checkpoint I/O (WP-020).
+ `trolly-gym`: matrix-game validation harness for WoLF-PPO paper reproduction (WP-019).
+ `trolly-gym`: PPO and WoLF-PPO actor-critic primitives behind `torch` feature (WP-018).
+ Binance demo integration tests: opt-in spot/USDM demo streams (`tests/binance_demo.rs`), demo URL helpers, `.env.example` flags.
+ `binance-spot-exec`: signed REST order placement, strategy egress adapter, `execute place-order` CLI entrypoint.
+ `trolly-gym`: RL training/inference toolchain ADR (`docs/rl-toolchain-analysis.md`); primary inference ONNX, training Python sidecar.
+ `trolly-gym`: stream-fed `Env`, observation windows, replay buffer stub, `torch` feature gate.
+ `binance-spot-exec`: spot user-data stream parsing, account book, trolly-stream ingress.
+ `binance-usdm-exec`: USDM user-data stream parsing, order/position tracking, trolly-stream ingress.
+ `trolly-strategy`: strategy runtime, normalized events, recording test double, synthetic integration tests.
+ Extract `trolly-stream`: multiplexor, `EventHandler`, ws adapter, injectable `ingest_message` ingress.
+ Binance USDM RPI overlay: end-to-end routing, TUI Δ tab (`@depth − @rpiDepth`), global merge isolation.
+ Cargo workspace scaffold: `trolly-stream`, `binance-spot-exec`, `binance-usdm-exec`, `trolly-strategy`, `trolly-gym` stub crates.
+ Document global-book integration test flow: offline fixtures by default, opt-in live REST via `.env` + `--ignored`.
+ Reduce full-book clones on global book merge hot path via `merge_aggregate_absorb`.
+ Refactor Binance spot to `depth::binance::spot`; add `stub` provider scaffold for `--sources`.
+ Fix Hook dead_code warnings; replace CLI `long_about` TODO with project goals.
+ Wire global book into depth monitor (`--output global --sources`).
+ Prometheus `/metrics` on book server; dynamic book registry for late-registered merged instruments.
+ Global book hub: multi-provider WebSocket feeds, `BookSource`, `merge_aggregate` via patched `lob`.
+ Wire `patches/lob` submodule at `patches/lob` for reproducible clones.
+ Update Binance API endpoints per 2026-04-19 changelog review:
  - Replace deprecated `wss://data-stream.binance.com` with primary `wss://stream.binance.com:9443`.
  - Add `limit=5000` to depth snapshot request per Binance best-practices for local order book management.
+ Add multi-symbol support.
+ add a DockerFile
+ Added a client example consumig the orderbook.
+ Add a client benchmark.
+ Serve the LOB through gRPC.
+ decouple web socket base url from the event subscription.
+ define a new subcommand depth for monitor
+ it should work as follows ./bin monitor <metric> [sources, symbol]
+ secure web socket streams
+ Graceful shutdown
+ pretty panics
+ basic logging
