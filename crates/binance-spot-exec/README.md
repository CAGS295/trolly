# binance-spot-exec

Binance spot order execution and account bookkeeping driven by WebSocket user-data
streams for inbound events and **REST `POST /api/v3/order`** for outbound order placement.

## Order placement: REST vs WebSocket

This crate uses the **REST API** (`POST https://api.binance.com/api/v3/order`) to submit
orders, not the WebSocket trading API.

**Rationale:**

| Criterion | REST | WebSocket trading API |
|-----------|------|-----------------------|
| Simplicity | Single signed POST per order | Requires a persistent WS session with JSON-RPC framing |
| Availability | Universally documented; all SDK examples use REST | Newer, fewer examples |
| Latency | Adequate for strategy-driven orders (not HFT) | Marginally lower round-trip |
| Error handling | Standard HTTP status codes + JSON error body | In-band JSON-RPC errors on the same connection |

For ultra-low-latency execution the WebSocket trading API (`wss://ws-api.binance.com:443/ws-api/v3`,
method `order.place`) should be preferred. The `OrderClient` trait in `order.rs` abstracts the
transport so the implementation can be swapped without changing call sites.

## Signing

Every REST order request is signed with **HMAC-SHA256** using the same `sign_hmac_sha256_hex`
helper used for WebSocket session subscriptions.  Parameters are sorted lexicographically
(via `BTreeMap`) before the payload is hashed, matching Binance's requirement.

## Stream subscription (inbound)

Execution reports, fills, and rejects flow back through the WebSocket user-data stream — the
REST call only _submits_ the order.

1. Connect to the Binance WebSocket API: `wss://ws-api.binance.com:443/ws-api/v3`.
2. Send a signed subscribe request (no REST listen key):

```json
{
  "id": "binance-spot-exec-subscribe",
  "method": "userDataStream.subscribe.signature",
  "params": {
    "apiKey": "<API_KEY>",
    "timestamp": 1747385641636,
    "signature": "<HMAC-SHA256 hex of apiKey=...&timestamp=...>"
  }
}
```

`BinanceSpotUserStream` builds this message via
[`VenueEndpoints::ws_subscriptions`](https://docs.rs/trolly-stream/latest/trolly_stream/trait.VenueEndpoints.html).
The `symbols` iterator is ignored for subscribe payloads because user-data is account-wide;
symbols are used for multiplexor routing only.

## Placing an order programmatically

```rust
use binance_spot_exec::{
    ApiCredentials, HttpOrderClient, OrderRequest, OrderSide, place_order,
};

let credentials = ApiCredentials {
    api_key:    std::env::var("BINANCE_API_KEY").unwrap(),
    secret_key: std::env::var("BINANCE_SECRET_KEY").unwrap(),
};

// MARKET order
let ack = place_order(
    &HttpOrderClient::new(),
    &credentials,
    OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001"),
).await?;

// LIMIT GTC order
let ack = place_order(
    &HttpOrderClient::new(),
    &credentials,
    OrderRequest::limit("ETHUSDT", OrderSide::Sell, "1.0", "3000.00"),
).await?;
```

## CLI

```sh
trolly place-order \
  --symbol BTCUSDT --side BUY --quantity 0.001   # MARKET
trolly place-order \
  --symbol ETHUSDT --side SELL --quantity 1.0 --price 3000.00  # LIMIT
```

Credentials are read from `BINANCE_API_KEY` / `BINANCE_SECRET_KEY` env vars.

## Strategy egress integration

`SpotExecEgress` implements `trolly_strategy::StreamEgress`.  It translates
`OutboundMessage::OrderRequest` messages from a strategy runtime into REST orders
submitted by an async worker:

```rust
use binance_spot_exec::{SpotExecEgress, HttpOrderClient, run_order_worker, ApiCredentials};

let (egress, rx) = SpotExecEgress::new_with_receiver();
tokio::spawn(run_order_worker(credentials, HttpOrderClient::new(), rx));

// Pass `egress` to StrategyHub::new(strategy, egress)
```

Fills and rejects are still reconciled by the `executionReport` user-data stream path —
no duplicate state machines.

## Multi-symbol routing (`trolly-stream`)

User-data events are injected through
[`MonitorMultiplexor::ingest_message`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html#method.ingest_message):

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `executionReport`, `listStatus` | trading symbol (e.g. `BTCUSDT`) |
| `outboundAccountPosition`, `balanceUpdate`, `externalLockUpdate` | `__account__` |

Pass trading symbols plus the account route when building handlers. Use
[`exec_subscription_symbols`](src/lib.rs) to append `__account__` automatically:

```rust
use binance_spot_exec::{exec_subscription_symbols, SpotExecHandler, SpotExecContext, BinanceSpotUserStream, ApiCredentials};

let symbols = exec_subscription_symbols(&["BTCUSDT", "ETHUSDT"]);
// symbols == ["BTCUSDT", "ETHUSDT", "__account__"]
```

One WebSocket connection receives all account events; the multiplexor fans execution
reports to per-symbol handlers and account events to the `__account__` handler.
