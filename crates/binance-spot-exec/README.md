# binance-spot-exec

Binance spot execution and account bookkeeping. **Inbound** order lifecycle updates (fills, rejects, balances) are driven by WebSocket user-data streams. **Outbound** order placement uses signed REST `POST /api/v3/order` (fills/rejects are still reconciled via `executionReport` on the user-data stream — no duplicate state machine).

## Outbound order placement (REST)

This crate places orders via the [Binance Spot REST API](https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints#new-order-trade):

- **Endpoint:** `POST https://api.binance.com/api/v3/order`
- **Auth:** `X-MBX-APIKEY` header + HMAC-SHA256 signature over form params (`timestamp` + `signature`)
- **Types:** `MARKET` and `LIMIT` (symbol, side, quantity, price, time-in-force)

```rust
use binance_spot_exec::{
    ApiCredentials, OrderSide, PlaceOrderRequest, NativeTlsTransport, SpotOrderClient, TimeInForce,
};

let client = SpotOrderClient::new(
    ApiCredentials { api_key: "...", secret_key: "..." },
    NativeTlsTransport::new(),
);

let ack = client
    .place_order(PlaceOrderRequest::limit(
        "BTCUSDT",
        OrderSide::Buy,
        "0.01",
        "50000.00",
        TimeInForce::Gtc,
    ))
    .await?;
```

Use [`SpotOrderEgress`](src/egress.rs) to consume normalized [`OutboundMessage::OrderRequest`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) commands from `trolly-strategy`. The queued egress enqueues requests for [`run_order_executor`](src/egress.rs); the direct egress places immediately (CLI/tests).

## Stream subscription (user-data)

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

`BinanceSpotUserStream` builds this message via [`VenueEndpoints::ws_subscriptions`](https://docs.rs/trolly-stream/latest/trolly_stream/trait.VenueEndpoints.html). The `symbols` iterator is ignored for subscribe payloads because user-data is account-wide; symbols are used for multiplexor routing only.

## Multi-symbol routing (`trolly-stream`)

User-data events are injected through [`MonitorMultiplexor::ingest_message`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html#method.ingest_message):

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `executionReport`, `listStatus` | trading symbol (e.g. `BTCUSDT`) |
| `outboundAccountPosition`, `balanceUpdate`, `externalLockUpdate` | `__account__` |

Pass trading symbols plus the account route when building handlers. Use [`exec_subscription_symbols`](src/lib.rs) to append `__account__` automatically:

```rust
use binance_spot_exec::{exec_subscription_symbols, SpotExecHandler, SpotExecContext, BinanceSpotUserStream, ApiCredentials};
use trolly_stream::MonitorMultiplexor;

let symbols = exec_subscription_symbols(&["BTCUSDT", "ETHUSDT"]);
// symbols == ["BTCUSDT", "ETHUSDT", "__account__"]
```

One WebSocket connection receives all account events; the multiplexor fans execution reports to per-symbol handlers and account events to the `__account__` handler.

## Demo / testnet (WP-017)

| Host | URL |
|------|-----|
| REST depth | `https://demo-api.binance.com/api` |
| WebSocket API (user-data) | `wss://demo-ws-api.binance.com/ws-api/v3` |
| Market streams | `wss://demo-stream.binance.com/ws` |

Use [`BinanceSpotUserStream::demo`](src/endpoints.rs) and [`spot_depth_rest_url`](src/endpoints.rs) with [`SPOT_DEMO_REST_BASE_URL`](src/endpoints.rs). Opt-in integration tests live in [`tests/binance_demo.rs`](../../tests/binance_demo.rs).
