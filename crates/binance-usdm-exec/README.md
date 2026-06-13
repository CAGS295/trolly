# binance-usdm-exec

Binance USD-M futures execution and account bookkeeping. **Inbound** order lifecycle updates (fills, rejects, balances, positions) are driven by WebSocket user-data streams. **Outbound** order placement uses signed REST `POST /fapi/v1/order` (fills/rejects are still reconciled via `ORDER_TRADE_UPDATE` on the user-data stream — no duplicate state machine).

## Outbound order placement (REST)

This crate places orders via the [Binance USDM REST API](https://developers.binance.com/docs/derivatives/usds-margined-futures/trade/rest-api/New-Order):

- **Endpoint:** `POST https://fapi.binance.com/fapi/v1/order`
- **Auth:** `X-MBX-APIKEY` header + HMAC-SHA256 signature over form params (`timestamp` + `signature`)
- **Types:** `MARKET` and `LIMIT` (symbol, side, quantity, price, time-in-force, `positionSide`)

`positionSide` defaults to `BOTH` when omitted (one-way mode). In hedge mode, set `LONG` or `SHORT` explicitly via [`OutboundMessage::OrderRequest::position_side`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) or [`PlaceOrderRequest::with_position_side`](src/order.rs).

```rust
use binance_usdm_exec::{
    ApiCredentials, NativeTlsTransport, OrderSide, PlaceOrderRequest, PositionSide, TimeInForce,
    UsdmOrderClient,
};

let client = UsdmOrderClient::new(
    ApiCredentials { api_key: "...", secret_key: "..." },
    NativeTlsTransport::new(),
);

let ack = client
    .place_order(
        PlaceOrderRequest::limit(
            "BTCUSDT",
            OrderSide::Buy,
            "0.01",
            "50000.00",
            TimeInForce::Gtc,
        )
        .with_position_side(PositionSide::Long),
    )
    .await?;
```

Use [`UsdmOrderEgress`](src/egress.rs) to consume normalized [`OutboundMessage::OrderRequest`](https://github.com/CAGS295/trolly/tree/main/crates/trolly-strategy) commands from `trolly-strategy`. The queued egress enqueues requests for [`run_order_executor`](src/egress.rs); the direct egress places immediately (CLI/tests).

## Stream subscription (user-data)

1. Create a `listenKey` via `POST /fapi/v1/listenKey` (caller responsibility — this crate does not manage listen keys).
2. Connect to the private user-data websocket:

```text
wss://fstream.binance.com/private/ws/<listenKey>
```

Optional event filtering:

```text
wss://fstream.binance.com/private/ws?listenKey=<key>&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE
```

`UsdmUserDataStream` builds the websocket URL via [`VenueEndpoints::websocket_url`](https://docs.rs/trolly-stream/latest/trolly_stream/trait.VenueEndpoints.html). Fan execution events into a shared [`trolly_stream::MonitorMultiplexor`] via [`crate::ingress::ingest_user_data`].

## Multi-symbol routing (`trolly-stream`)

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE`, `MARGIN_CALL` | `__account__` |

Pass trading symbols plus the account route when building handlers. One websocket connection receives all account events; the multiplexor fans order updates to per-symbol handlers and account events to the `__account__` handler.
