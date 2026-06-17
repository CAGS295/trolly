# binance-spot-exec

Binance spot execution and account bookkeeping. **Inbound** state (fills, balances, positions) is driven by WebSocket user-data streams only. **Outbound** order placement uses signed REST `POST /api/v3/order`; acknowledgements and fills are still reconciled via the existing `executionReport` user-data path (no duplicate order state machine in this crate).

## Outbound order placement (REST)

Signed orders are sent to `POST https://api.binance.com/api/v3/order` with `X-MBX-APIKEY` and an HMAC-SHA256 signature over the form body (`symbol`, `side`, `type`, `quantity`, optional `price` / `timeInForce`, `timestamp`, `signature`). The WebSocket trading API is not used here so placement stays a simple one-shot HTTP call while user-data remains on the existing WS API connection.

[`PlaceOrderRequest`](src/order.rs) covers market and limit basics:

| Field | Market | Limit |
|-------|--------|-------|
| `symbol` | required | required |
| `side` (`BUY` / `SELL`) | required | required |
| `quantity` | required | required |
| `price` | — | required |
| `timeInForce` (`GTC` / `IOC` / `FOK`) | — | required (defaults to `GTC` from strategy egress) |

[`SpotOrderClient`](src/client.rs) performs the HTTP call. [`SpotExecEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`](https://docs.rs/trolly-strategy): strategy runtimes dispatch `OutboundMessage::OrderRequest`, which is converted into a `PlaceOrderRequest` and queued for an async executor.

CLI entrypoint (root `trolly` binary):

```bash
export DEMO_BINANCE_KEY=... DEMO_BINANCE_SECRET=...
cargo run -- execute place-order BTCUSDT BUY 0.001 --price 100000
```

## Stream subscription (user data)

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
