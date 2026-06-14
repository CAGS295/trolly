# binance-spot-exec

Binance spot execution and account bookkeeping. **Inbound** order and account updates are driven by WebSocket user-data streams (WP-008). **Outbound** order placement uses signed REST `POST /api/v3/order` (WP-015); fills and rejects reconcile via the existing `executionReport` user-data path — no duplicate order state machine in this crate.

## Outbound order API choice

We use **REST `POST /api/v3/order`** rather than the Binance WebSocket trading API because:

- One-shot placement from the CLI and strategy egress maps naturally to a request/response HTTP call.
- HMAC signing reuses the same query-string pattern as other Binance REST signed endpoints.
- User-data streams already deliver `executionReport` events for fill/reject reconciliation; a persistent trading WebSocket would not remove that requirement.

Production base URL: `https://api.binance.com`. Demo: `https://demo-api.binance.com` (`SpotOrderClient::demo` / `--demo` on the CLI).

Demo hosts for integration tests (WP-017):

| Service | Production | Demo |
|---------|------------|------|
| REST | `https://api.binance.com` | `https://demo-api.binance.com` |
| WebSocket API (user-data) | `wss://ws-api.binance.com/ws-api/v3` | `wss://demo-ws-api.binance.com/ws-api/v3` |
| Market streams | `wss://stream.binance.com/ws` | `wss://demo-stream.binance.com/ws` |

Run opt-in live tests: `cargo test --test binance_demo_integration -- --ignored` with `RUN_BINANCE_DEMO_INTEGRATION=1` and demo keys in `.env` (see root [`.env.example`](../../.env.example)).

Alternative considered: WebSocket API `order.place` on `wss://ws-api.binance.com/ws-api/v3` — viable for co-located low-latency strategies; can be added later without changing ingest bookkeeping.

## Placing orders

```rust
use binance_spot_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, SpotOrderClient, TimeInForce,
};

let client = SpotOrderClient::new(ApiCredentials {
    api_key: std::env::var("DEMO_BINANCE_KEY")?,
    secret_key: std::env::var("DEMO_BINANCE_SECRET")?,
});

let request = NewOrderRequest::limit(
    "BTCUSDT",
    OrderSide::Buy,
    "0.01",
    "100.0",
    TimeInForce::Gtc,
);
let ack = client.place_order(&request).await?;
// ack.status is the REST acknowledgement only; watch user-data executionReport for fills.
```

Request builder fields:

| Field | Market | Limit |
|-------|--------|-------|
| `symbol` | required | required |
| `side` (`BUY` / `SELL`) | required | required |
| `quantity` | required | required |
| `price` | — | required |
| `timeInForce` (`GTC`, `IOC`, `FOK`) | — | required (default `GTC` in CLI) |

## Strategy egress integration

[`SpotOrderEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`](../../trolly-strategy/src/egress.rs) and translates [`OutboundMessage::OrderRequest`](../../trolly-strategy/src/egress.rs) into signed REST placement (`price: Some` → limit + `GTC`, `price: None` → market):

```rust
use binance_spot_exec::{ApiCredentials, SpotOrderEgress};
use trolly_strategy::{OutboundMessage, StreamEgress};

let mut egress = SpotOrderEgress::new(ApiCredentials { api_key, secret_key });
egress.dispatch(OutboundMessage::OrderRequest {
    symbol: "BTCUSDT".into(),
    side: "BUY".into(),
    qty: "0.01".into(),
    price: Some("100".into()),
})?;
```

## CLI

```bash
export DEMO_BINANCE_KEY=... DEMO_BINANCE_SECRET=...
cargo run -- execute place-order BTCUSDT BUY 0.01 --price 100 --demo
```

Omit `--price` for market orders. Credentials fall back to `BINANCE_API_KEY` / `BINANCE_API_SECRET`.

## Stream subscription (inbound)

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
