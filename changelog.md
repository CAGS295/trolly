# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

_(none — all workplan items complete)_

## change log
+ RPI overlay verified end-to-end: `binance-usd-m:RPI:SYMBOL` routing, TUI Δ tab tests, canonical merge isolation documented in WORKPLAN.md.
+ Provider expansion scaffold: binance split into `providers/binance/{spot,usd_m}`; arbitrary `--sources provider:SYMBOL` labels parse via `Provider::Other`.
+ Hot-path allocation: `merge_aggregate_refs` avoids full-book clones on multi-source `refresh_merged_for`; criterion bench added.
+ CLI cleanup: wire `Hook::new`/`register` in book server; replace `long_about` TODO with project description.
+ Integration test hygiene: document live global-book test flow (`.env.example` → `.env`, `RUN_GLOBAL_BOOK_INTEGRATION=1`); fixture tests always run, live test opt-in via env.
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
