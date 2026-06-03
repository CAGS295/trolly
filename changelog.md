# Goals
- Have a command to build a global order book.

## WIP

- optimize allocations
- prometheus server.
- Wire global book into `monitor depth --output serve` (gRPC / SCALE).
- Add more exchange providers beyond Binance spot + USDM.

## Workplan — global order book

- **Cross-source merge (done):** [`BookSource`](src/monitor/global_book.rs) (`provider:SYMBOL`) feeds a shared [`GlobalBookHub`](src/monitor/global_book.rs); books merge by canonical instrument via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs) (spot + USDM is the first pair).
- **Intra-provider overlays (Binance-only):** RPI `@rpiDepth` stays an optional USDM leg and TUI `Δ` tab — not part of the portable merge key.
- **Integration tests:** copy [`.env.example`](.env.example) → `.env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`, run `cargo test --test global_book global_book_live_rest_merge -- --ignored`.

## change log
+ Global book hub: multi-provider WebSocket feeds, `BookSource`, `merge_aggregate` via patched `lob`.
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
