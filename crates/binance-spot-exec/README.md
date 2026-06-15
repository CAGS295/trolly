# binance-spot-exec

Binance spot execution: **outbound order placement** via signed REST, plus **account bookkeeping** driven by WebSocket user-data streams (fills/rejects reconcile through `executionReport` — no duplicate order state machine).

## Outbound orders (REST)

Order placement uses Binance spot REST [`POST /api/v3/order`](https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints#new-order-trade). WebSocket trading API was not chosen so the same HMAC query signing used elsewhere maps cleanly to form-encoded POST bodies.

Request builder covers market/limit basics:

| Field | REST param | Notes |
|-------|------------|-------|
| symbol | `symbol` | uppercased |
| side | `side` | `BUY` / `SELL` |
| quantity | `quantity` | string decimal |
| price | `price` | limit orders only |
| time-in-force | `timeInForce` | default `GTC` for limits |

```rust
use binance_spot_exec::{
    ApiCredentials, OrderSide, SpotOrderClient, SpotOrderRequest, SpotRestEgress, DEFAULT_REST_BASE_URL,
};
use trolly_strategy::{OutboundMessage, StreamEgress};

let client = SpotOrderClient::new(
    DEFAULT_REST_BASE_URL,
    ApiCredentials { api_key: "...".into(), secret_key: "...".into() },
);
let request = SpotOrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.01", "100000");
let ack = client.place_order(request).await?;

// Strategy egress adapter (fills still arrive on user-data stream):
let mut egress = SpotRestEgress::new(DEFAULT_REST_BASE_URL, credentials);
egress.dispatch(OutboundMessage::order_request("BTCUSDT", "BUY", "0.01", Some("100000".into())))?;
```

Demo/testnet REST base: `https://demo-api.binance.com` (see `.env.example` `DEMO_BINANCE_KEY` / `DEMO_BINANCE_SECRET`).

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
