# Goals
- Have a command to build a global order book.

## WIP

- optimize allocations (incremental merge; reduce clone on hot path further)
- Add more exchange providers beyond Binance spot + USDM.

## Workplan — global order book

- **Cross-source merge (done):** [`BookSource`](src/monitor/global_book.rs) feeds [`GlobalBookHub`](src/monitor/global_book.rs); merge via [`LimitOrderBook::merge_aggregate`](patches/lob/src/limit_order_book/mod.rs).
- **CLI serve (done):** `monitor depth --output global --sources binance:BTCUSDT,binance-usd-m:BTCUSDT --server-port 50051`
- **Prometheus (done):** `GET /metrics` on the book server (`trolly_depth_updates_total`, `trolly_global_book_merge_refresh_total`).
- **Intra-provider overlays (Binance-only):** RPI stays optional; TUI `Δ` tab only.
- **Integration tests:** copy [`.env.example`](.env.example) → `.env`, set `RUN_GLOBAL_BOOK_INTEGRATION=1`.

## change log
+ Wire global book into depth monitor (`--output global --sources`).
+ Prometheus `/metrics` on book server; dynamic book registry for late-registered merged instruments.
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
