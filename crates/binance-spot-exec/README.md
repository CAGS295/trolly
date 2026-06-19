# binance-spot-exec

Binance spot execution and account bookkeeping. **Inbound** order lifecycle and balances are driven by WebSocket user-data streams (`executionReport`, account events). **Outbound** order placement uses signed REST `POST /api/v3/order` (fills and rejects still reconcile on the user-data stream — no duplicate state machine).

## Outbound order placement (REST)

This crate uses the Binance Spot REST API for signed order submission:

- Endpoint: `POST https://api.binance.com/api/v3/order`
- Auth: `X-MBX-APIKEY` header + HMAC-SHA256 signature over form parameters (`timestamp`, `signature`)

WebSocket user-data remains the source of truth for fills, rejects, and account book updates after placement.

### Request builder

[`NewOrderRequest`](src/order.rs) covers market and limit basics:

| Field | Market | Limit |
|-------|--------|-------|
| `symbol` | required | required |
| `side` (`BUY` / `SELL`) | required | required |
| `quantity` | required | required |
| `price` | — | required |
| `time_in_force` (`GTC`, `IOC`, `FOK`) | — | required (defaults to `GTC` via [`new_order_from_outbound`](src/order.rs)) |

```rust
use binance_spot_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, SpotOrderClient, TimeInForce,
};

let client = SpotOrderClient::new(ApiCredentials { api_key, secret_key });
let limit = NewOrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.01", "50000", TimeInForce::Gtc);
let ack = client.place_order(&limit)?;
// ack.status is REST acknowledgement only; stream executionReport reconciles fills/rejects
```

### Strategy egress integration

[`SpotOrderEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`](../../trolly-strategy/src/egress.rs) and converts [`OutboundMessage::OrderRequest`](../../trolly-strategy/src/egress.rs) into signed REST placement:

```rust
use binance_spot_exec::{ApiCredentials, SpotOrderEgress};
use trolly_strategy::{OutboundMessage, StreamEgress};

let mut egress = SpotOrderEgress::new(credentials);
egress.dispatch(OutboundMessage::limit_order("BTCUSDT", "BUY", "0.01", "100", Some("GTC")))?;
```

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

## CLI

From the root `trolly` binary:

```bash
export DEMO_BINANCE_KEY=...
export DEMO_BINANCE_SECRET=...
trolly execute spot-order --symbol BTCUSDT --side BUY --qty 0.01 --price 50000
```

Omit `--price` for a market order. REST placement returns an acknowledgement; monitor the user-data stream for `executionReport` reconciliation.

## Demo / testnet endpoints

Use Binance **demo** credentials only (`DEMO_BINANCE_KEY`, `DEMO_BINANCE_SECRET`). Never use production keys against these hosts.

| Host | URL |
|------|-----|
| REST | `https://demo-api.binance.com/api` |
| WebSocket API (signed user-data) | `wss://demo-ws-api.binance.com/ws-api/v3` |
| Market streams | `wss://demo-stream.binance.com/ws` |

[`BinanceSpotUserStream::demo`](src/endpoints.rs) targets the demo WebSocket API; [`BinanceSpotUserStream::demo_rest_depth_url`](src/endpoints.rs) builds demo REST depth URLs. Opt-in integration tests live in [`tests/binance_demo_integration.rs`](../../tests/binance_demo_integration.rs) (`RUN_BINANCE_DEMO_INTEGRATION=1`).
