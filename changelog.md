# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- Add more exchange providers beyond Binance spot + USDM (scaffold in place; need real venues).

## change log
+ WP-004: RPI overlay routing end-to-end; TUI Δ tab groups overlay under base instrument without polluting canonical merge.
+ WP-001: Merge source books by reference on `refresh_merged_for` hot path (`merge_aggregate` accepts `&Self`).
+ WP-003: Provider scaffold under `depth::binance::spot` / `depth::usdm` with extension registry.
+ WP-005: Wire `Hook` API into production server; replace CLI `long_about` placeholder.
+ Document global book integration test flow: `.env.example`, README Testing section, opt-in live REST merge via `RUN_GLOBAL_BOOK_INTEGRATION`.
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
