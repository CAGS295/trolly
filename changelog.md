# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Add more exchange providers beyond Binance spot + USDM (stub scaffold landed; full venues remain).
- Workspace crates: stream ingress refactor, Binance spot/USDM execution, strategy runtime, libtorch gym scaffold (see [`WORKPLAN.md`](WORKPLAN.md) WP-006 … WP-011).

## change log
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
