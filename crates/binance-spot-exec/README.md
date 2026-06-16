# binance-spot-exec

Binance spot execution and account bookkeeping. **Inbound** order and account updates are driven by WebSocket user-data streams (WP-008). **Outbound** order placement uses signed REST `POST /api/v3/order` (WP-015); fills and rejects are still reconciled via the existing `executionReport` user-data path — no duplicate state machine in this crate.

## Outbound orders (REST)

We use the Binance **REST** trading API (not the WebSocket trading API) for place-order because the CLI and strategy egress adapters issue one-shot signed HTTP requests. User-data ingest remains WebSocket-only.

- Production: `https://api.binance.com/api/v3/order`
- Testnet: `https://testnet.binance.vision/api/v3/order`

Signed query params: `symbol`, `side`, `type` (`MARKET` / `LIMIT`), `quantity`, optional `price` + `timeInForce` (`GTC`, `IOC`, `FOK`) for limits, `timestamp`, `signature` (HMAC-SHA256). Header: `X-MBX-APIKEY`.

[`NewOrderRequest`](src/order.rs) builds market/limit requests. [`SpotOrderClient::place_order`](src/outbound.rs) submits them. [`SpotExecEgress`](src/outbound.rs) implements [`trolly_strategy::StreamEgress`](https://docs.rs/trolly-strategy) and maps [`OutboundMessage::OrderRequest`](https://docs.rs/trolly-strategy) to REST calls.

CLI example (requires `DEMO_BINANCE_KEY` / `DEMO_BINANCE_SECRET` or `BINANCE_API_KEY` / `BINANCE_SECRET_KEY`):

```bash
cargo run -- execute place-order --symbol BTCUSDT --side BUY --quantity 0.01 --price 100.0
```

## Stream subscription (user-data ingest)

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
