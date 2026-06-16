# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Add more exchange providers beyond Binance spot + USDM (stub scaffold landed; full venues remain).

## change log
+ `binance-usdm-exec`: account-wide position bookkeeping keyed by `(symbol, position_side)` with flatten semantics (WP-012).
+ `binance-spot-exec`: signed REST outbound order placement, strategy egress integration, `execute place-order` CLI (WP-015).
+ `trolly-gym`: ADR-001 RL toolchain analysis — ONNX Runtime inference, Python/PyTorch training, `torch` fallback (WP-016).
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
