# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Live demo place + user-stream reconcile of a Gaussian-quantized policy on Binance demo (keys / `RUN_BINANCE_DEMO_ORDERS=1`). Offline load-and-dispatch, captured/injected/subscribed books, ONNX Gaussian μ, snapshot+diff rebuild, multi-symbol joined observations, per-symbol `Action::dispatch`, extra-symbol reconcile, and book-local inventory scoring are in.
- Extra-symbol user-data fills are not written back into `Env::position_for` (WP-051).
- Keep local GPU training checkpoint metrics improving via ClickHouse `trolly.ticks`, trajectory FIFO replay, daily `--continue` slices, and checkpoint fingerprints.
- Add more exchange providers beyond Binance spot + USDM (stub scaffold landed; full venues remain).

## change log
+ `trolly-gym`: extra-pair Buy/Sell scores that book's mid/spread; `Env::position()` stays primary (WP-050). Trainer guidance missing (silent).
+ `execute policy-demo`: user-data hubs register dispatched `OrderRequest` symbols so extra-pair fills reconcile offline (WP-049). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: `PolicyProvider::decide` / `--dispatch-symbol` can `Action::dispatch` a tracked extra pair; qty stays `--qty`, side stays Buy/Sell/Hold, default remains primary (WP-048). Trainer guidance missing (silent).
+ `execute policy-demo`: `public_depth_snapshot_json` accepts a JSON array of REST-style snapshots (one local book per symbol) so two-symbol subscribe rebuilds stay offline-testable (WP-047). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: Gaussian sources keep the primary-symbol `V×5` ladder when extra books are listed so weekday `mu.onnx` / `gaussian_mlp` dim still matches (WP-046). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: Env joins per-symbol 7-D or `V×5` frames (`--symbol BTCUSDT,ETHUSDT`); Buy/Sell still dispatch the primary pair; synthetic tape stays single-symbol (WP-045). Trainer guidance missing (silent).
+ `execute policy-demo`: subscribed public depth seeds a local book from a demo REST-style snapshot and applies WS diffs (qty `0` removes; stale `u` skipped) so Env sees reconstructed top-of-book (WP-044). Trainer guidance missing (silent).
+ `execute policy-demo`: `--subscribe-public-depth` plus required `--public-depth-timeout-secs` collects demo public depth through the WP-042 hook; report prints `depth=subscribed`; `--depth-json` still wins (WP-043). Trainer guidance missing (silent).
+ `execute policy-demo`: injectable public-depth source (`run_policy_demo_with_public_depth`); report prints `depth=synthetic|captured-json|injected`; `--depth-json` still wins (WP-042). Trainer guidance missing (silent).
+ `execute policy-demo`: `--depth-json` feeds captured StreamEvent / Binance depth / REST snapshot frames into Env; unset keeps the synthetic tape (WP-041). Trainer guidance missing (silent).
+ `execute policy-demo`: auto-load `mu.onnx` from `GAUSSIAN_CHECKPOINT_DIR` or weekday `microstructure/gaussian_mlp` when `ONNX_GAUSSIAN_MODEL_PATH` is unset; recorded mean-actions still win (WP-040). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: offline export of a static `[1, V×5] → [1,1]` Gaussian μ ONNX graph (recorded-mean stand-in or weekday `gaussian_mlp`) for `ONNX_GAUSSIAN_MODEL_PATH` (WP-039). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: optional ONNX Gaussian μ head (`[1, V×5] → [1]` / `[1,1]`) quantizes via WP-035 onto `Action::dispatch`; `ONNX_GAUSSIAN_MODEL_PATH` selects it and the ladder frame; 3-logit `ONNX_MODEL_PATH` unchanged (WP-038). Trainer guidance missing (silent).
+ `trolly-gym` / policy-demo: Gaussian sources consume WP-032 `V×5` ladder frames from ingested depth (half-spread → δ) plus inventory; Hold/ONNX stay 7-D (WP-037). Trainer guidance missing (silent).
+ `execute policy-demo`: load recorded Gaussian mean-action vectors or `microstructure/gaussian_mlp` (torch), quantize via WP-035 onto `Action::dispatch` (WP-036). Trainer guidance missing (silent).
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
