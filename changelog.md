# Changelog

Project journal for shipped work. Active backlog lives in [`WORKPLAN.md`](WORKPLAN.md).

## WIP

- CLI/servers cleanup — WP-005

## change log
+ Binance USDM RPI overlays: `binance-usd-m:RPI:SYMBOL` routing, isolated merge lanes, TUI `Δ` tab (`@depth − @rpiDepth`); documented in WORKPLAN WP-004.
+ Provider expansion scaffold: `depth::binance::spot` layout and `other` third-venue registration for `--sources`.
+ Hot-path merge: `refresh_merged_for` uses `merge_into` instead of cloning every source book before `merge_aggregate`.
+ Document and gate global-book live REST integration test (`RUN_GLOBAL_BOOK_INTEGRATION`, `.env.example`, README); loopback REST stub fallback when Binance is geo-blocked.
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
