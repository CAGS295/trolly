# binance-spot-exec

Binance spot order execution and account bookkeeping driven **only** by WebSocket user-data streams. This crate does not call REST trading endpoints.

## Stream subscription

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
