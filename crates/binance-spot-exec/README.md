# binance-spot-exec

Binance **spot** order execution bookkeeping and account updates from **user-data websocket streams only**. No REST trading endpoints live in this crate.

## Subscription flow

1. Connect to the Binance WebSocket API: `wss://ws-api.binance.com:443/ws-api/v3`.
2. Subscribe to the user-data stream:
   - **Authenticated session:** send `userDataStream.subscribe` after `session.logon`.
   - **Signature subscription:** use [`BinanceSpotUserStream::subscribe_signature_message`] with API key + HMAC-SHA256 signature (see Binance docs for `userDataStream.subscribe.signature`).
3. Push each received text frame into [`trolly_stream::MessageIngress`] (or call [`parse_and_push`] to validate first).
4. Build a [`MonitorMultiplexor`] with [`SpotExecHandler`] instances:
   - One handler per traded symbol (e.g. `BTCUSDT`) for `executionReport` / `listStatus`.
   - One handler registered under [`ACCOUNT_ROUTE_ID`] (`"__account__"`) for `outboundAccountPosition`, `balanceUpdate`, and related account events.

The `symbols` argument to [`Endpoints::ws_subscriptions`] is ignored for user-data setup (account-scoped stream); symbols configure multiplexor routing instead.

## Parsed events

| Binance `e` field           | Rust variant                         | Routes to        |
|----------------------------|--------------------------------------|------------------|
| `executionReport`          | `SpotUserDataEvent::ExecutionReport` | order symbol     |
| `listStatus`               | `SpotUserDataEvent::ListStatus`      | order symbol     |
| `outboundAccountPosition`  | `SpotUserDataEvent::OutboundAccountPosition` | `__account__` |
| `balanceUpdate`            | `SpotUserDataEvent::BalanceUpdate`   | `__account__`    |
| `externalLockUpdate`       | `SpotUserDataEvent::ExternalLockUpdate` | `__account__` |
| `eventStreamTerminated`    | `SpotUserDataEvent::EventStreamTerminated` | `__account__` |

Both WebSocket API envelopes (`{ "subscriptionId", "event": { ... } }`) and legacy listen-key payloads (`{ "e": "...", ... }`) are accepted by the parser.

## Tests

Fixture JSON lives under `tests/fixtures/`. Run:

```bash
cargo test -p binance-spot-exec
```

[`BinanceSpotUserStream::subscribe_signature_message`]: https://docs.rs/binance-spot-exec/latest/binance_spot_exec/struct.BinanceSpotUserStream.html#method.subscribe_signature_message
[`parse_and_push`]: https://docs.rs/binance-spot-exec/latest/binance_spot_exec/fn.parse_and_push.html
[`MonitorMultiplexor`]: https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html
[`SpotExecHandler`]: https://docs.rs/binance-spot-exec/latest/binance_spot_exec/struct.SpotExecHandler.html
[`Endpoints::ws_subscriptions`]: https://docs.rs/trolly-stream/latest/trolly_stream/trait.Endpoints.html#tymethod.ws_subscriptions
[`ACCOUNT_ROUTE_ID`]: https://docs.rs/binance-spot-exec/latest/binance_spot_exec/constant.ACCOUNT_ROUTE_ID.html
