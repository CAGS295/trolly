# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Add more exchange providers beyond Binance spot + USDM (scaffold in place; wire additional venues).

## change log
+ WP-002: document global book integration test flow; `.env.example`, README Testing section, one-shot dotenv load in `tests/global_book.rs`.
+ WP-001: incremental merge on global book refresh hot path (`merge_into_aggregate` / `merge_aggregate_refs`).
+ WP-003: provider layout `depth::binance::spot`; `Provider::Registered` for third-venue `--sources` parsing.
+ WP-005: Hook dead_code fix; CLI `long_about` documents project goals.
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
