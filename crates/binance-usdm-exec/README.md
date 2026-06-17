# binance-usdm-exec

Binance USD-M futures execution and account bookkeeping. **Inbound** state (fills, balances, positions) is driven by WebSocket user-data streams only. **Outbound** order placement uses signed REST `POST /fapi/v1/order`; acknowledgements and fills are still reconciled via the existing `ORDER_TRADE_UPDATE` user-data path (no duplicate order state machine in this crate).

## Outbound order placement (REST)

Signed orders are sent to `POST https://fapi.binance.com/fapi/v1/order` with `X-MBX-APIKEY` and an HMAC-SHA256 signature over the form body (`symbol`, `side`, `type`, `quantity`, optional `price` / `timeInForce` / `positionSide`, `timestamp`, `signature`). The WebSocket trading API is not used here so placement stays a simple one-shot HTTP call while user-data remains on the private listen-key stream.

[`PlaceOrderRequest`](src/order.rs) covers market and limit basics:

| Field | Market | Limit |
|-------|--------|-------|
| `symbol` | required | required |
| `side` (`BUY` / `SELL`) | required | required |
| `quantity` | required | required |
| `price` | — | required |
| `timeInForce` (`GTC` / `IOC` / `FOK`) | — | required (defaults to `GTC` from strategy egress) |
| `positionSide` (`LONG` / `SHORT` / `BOTH`) | optional | optional |

`positionSide` is required in **hedge mode** (`LONG` / `SHORT` per leg). In one-way mode omit the field or use `BOTH`.

[`UsdmOrderClient`](src/client.rs) performs the HTTP call. [`UsdmExecEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`]: strategy runtimes dispatch `OutboundMessage::OrderRequest` (including optional `position_side`), which is converted into a `PlaceOrderRequest` and queued for an async executor.

## Stream subscription (user data)

1. Create a listen key via `POST /fapi/v1/listenKey` (caller responsibility; not implemented in this crate).
2. Connect to the private user-data WebSocket:

```text
wss://fstream.binance.com/private/ws/<listenKey>
```

Optional event filtering:

```text
wss://fstream.binance.com/private/ws?listenKey=<key>&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE
```

`UsdmUserDataStream` builds the URL via [`VenueEndpoints::websocket_url`](https://docs.rs/trolly-stream/latest/trolly_stream/trait.VenueEndpoints.html). Fan execution events into a shared [`MonitorMultiplexor`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html) via [`ingest_user_data`](src/ingress.rs).

## Multi-symbol routing (`trolly-stream`)

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE` balances | `__account__` |
| `ACCOUNT_UPDATE` positions | per-symbol handler |
| `MARGIN_CALL` | `__account__` |

One WebSocket connection receives all account events; the multiplexor fans order updates to per-symbol handlers and account-wide events to the `__account__` handler.
