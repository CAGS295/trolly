# binance-usdm-exec

Binance USD-M futures execution and account bookkeeping. **Inbound** order and account updates are driven by WebSocket user-data streams (WP-009). **Outbound** order placement uses signed REST `POST /fapi/v1/order` (WP-014); fills and rejects reconcile via the existing `ORDER_TRADE_UPDATE` user-data path — no duplicate order state machine in this crate.

## Outbound order API choice

We use **REST `POST /fapi/v1/order`** rather than the Binance WebSocket trading API because:

- One-shot placement from strategy egress maps naturally to a request/response HTTP call.
- HMAC signing reuses the same query-string pattern as other Binance REST signed endpoints.
- User-data streams already deliver `ORDER_TRADE_UPDATE` events for fill/reject reconciliation.

Production base URL: `https://fapi.binance.com`. Demo: `https://demo-fapi.binance.com` (`UsdmOrderClient::demo`).

Alternative considered: WebSocket API order placement on the futures WS API — viable for co-located low-latency strategies; can be added later without changing ingest bookkeeping.

## Placing orders

```rust
use binance_usdm_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, PositionSide, TimeInForce, UsdmOrderClient,
};

let client = UsdmOrderClient::demo(ApiCredentials {
    api_key: std::env::var("DEMO_BINANCE_KEY")?,
    secret_key: std::env::var("DEMO_BINANCE_SECRET")?,
});

let request = NewOrderRequest::limit(
    "BTCUSDT",
    OrderSide::Buy,
    "0.01",
    "100.0",
    TimeInForce::Gtc,
)
.with_position_side(PositionSide::Long);
let ack = client.place_order(&request).await?;
// ack.status is the REST acknowledgement only; watch user-data ORDER_TRADE_UPDATE for fills.
```

Request builder fields:

| Field | Market | Limit |
|-------|--------|-------|
| `symbol` | required | required |
| `side` (`BUY` / `SELL`) | required | required |
| `quantity` | required | required |
| `price` | — | required |
| `timeInForce` (`GTC`, `IOC`, `FOK`, `GTX`) | — | required (default `GTC` via strategy egress) |
| `positionSide` (`LONG`, `SHORT`, `BOTH`) | optional | optional |

Omit `positionSide` in one-way mode (Binance defaults to `BOTH`). In hedge mode, set `LONG` or `SHORT` on each leg.

## Strategy egress integration

[`UsdmOrderEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`](../../trolly-strategy/src/egress.rs) and translates [`OutboundMessage::OrderRequest`](../../trolly-strategy/src/egress.rs) into signed REST placement (`price: Some` → limit + `GTC`, `price: None` → market):

```rust
use binance_usdm_exec::{ApiCredentials, UsdmOrderEgress};
use trolly_strategy::{OutboundMessage, StreamEgress};

let mut egress = UsdmOrderEgress::demo(ApiCredentials { api_key, secret_key });
egress.dispatch(OutboundMessage::OrderRequest {
    symbol: "BTCUSDT".into(),
    side: "BUY".into(),
    qty: "0.01".into(),
    price: Some("100".into()),
    position_side: Some("LONG".into()),
})?;
```

Unsupported outbound variants return [`UsdmOrderError::UnsupportedOutbound`](src/order.rs) — no silent fallback.

## User-data stream (`listenKey`) — caller responsibilities

Private USDM user-data requires a REST `listenKey` before opening the websocket:

1. **Create** — `POST /fapi/v1/listenKey` (signed). Use [`ListenKeyClient::create`](src/listen_key.rs).
2. **Connect** — pass the key to [`UsdmUserDataStream::new`](src/endpoints.rs) → `wss://fstream.binance.com/private/ws/<listenKey>`.
3. **Keepalive** — `PUT /fapi/v1/listenKey` every ~30 minutes while connected ([`ListenKeyClient::keepalive`](src/listen_key.rs)).
4. **Fan-in** — push websocket text into [`ingest_user_data`](src/ingress.rs) on a shared [`MonitorMultiplexor`](../../trolly-stream/src/lib.rs).

This crate provides signed REST helpers for steps 1 and 3; the application owns websocket lifecycle, reconnect, and key rotation on expiry.

Demo REST host: `https://demo-fapi.binance.com` (same paths as production).

## Multi-symbol routing (`trolly-stream`)

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE` balance rows | `__account__` |
| `ACCOUNT_UPDATE` position rows | symbol + `__account__` |
| `MARGIN_CALL` | `__account__` |

One private websocket receives all account events; the multiplexor fans order updates to per-symbol handlers and account events to the `__account__` handler.
