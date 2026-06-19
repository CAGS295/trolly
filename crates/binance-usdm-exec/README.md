# binance-usdm-exec

Binance USD-M futures execution and account bookkeeping. **Inbound** order lifecycle, positions, and balances are driven by WebSocket user-data streams (`ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, etc.). **Outbound** order placement uses signed REST `POST /fapi/v1/order` (fills and rejects still reconcile on the user-data stream — no duplicate state machine).

## Outbound order placement (REST)

This crate uses the Binance USDM REST API for signed order submission:

- Endpoint: `POST https://fapi.binance.com/fapi/v1/order`
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
| `time_in_force` (`GTC`, `IOC`, `FOK`, `GTX`) | — | required (defaults to `GTC` via [`new_order_from_outbound`](src/order.rs)) |
| `position_side` (`LONG`, `SHORT`, `BOTH`) | optional | optional |

In **hedge mode**, Binance requires `positionSide` on every order. In one-way mode, omit it (Binance defaults to `BOTH`).

```rust
use binance_usdm_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, PositionSide, TimeInForce, UsdmOrderClient,
};

let client = UsdmOrderClient::new(ApiCredentials { api_key, secret_key });
let limit = NewOrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.01", "50000", TimeInForce::Gtc)
    .with_position_side(PositionSide::Long);
let ack = client.place_order(&limit)?;
// ack.status is REST acknowledgement only; ORDER_TRADE_UPDATE reconciles fills/rejects
```

### Strategy egress integration

[`UsdmOrderEgress`](src/egress.rs) implements [`trolly_strategy::StreamEgress`](../../trolly-strategy/src/egress.rs) and converts [`OutboundMessage::OrderRequest`](../../trolly-strategy/src/egress.rs) into signed REST placement:

```rust
use binance_usdm_exec::{ApiCredentials, UsdmOrderEgress};
use trolly_strategy::{OutboundMessage, StreamEgress};

let mut egress = UsdmOrderEgress::new(credentials);
egress.dispatch(
    OutboundMessage::limit_order("BTCUSDT", "BUY", "0.01", "100", Some("GTC"))
        .with_position_side("LONG"),
)?;
```

## User-data stream (listen key)

Private USDM events require a **listen key** from REST:

1. `POST https://fapi.binance.com/fapi/v1/listenKey` with `X-MBX-APIKEY` (no body) → returns `{ "listenKey": "..." }`
2. Keep alive: `PUT /fapi/v1/listenKey` every ~30 minutes (same header)
3. Connect: `wss://fstream.binance.com/private/ws/<listenKey>`

**Caller responsibilities:** this crate does not create or renew listen keys automatically. Supply an active key to [`UsdmUserDataStream`](src/endpoints.rs) and run keepalive in your process supervisor. Constants [`LISTEN_KEY_CREATE_PATH`](src/endpoints.rs) and [`LISTEN_KEY_KEEPALIVE_PATH`](src/endpoints.rs) document the REST paths.

## Multi-symbol routing (`trolly-stream`)

User-data events are injected through [`MonitorMultiplexor::ingest_message`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html#method.ingest_message):

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE` balances | `__account__` |
| `ACCOUNT_UPDATE` positions | symbol from position row |
| `MARGIN_CALL` | `__account__` |

Use [`build_hub`](src/ingress.rs) or register handlers per symbol plus the account route id [`ACCOUNT_ROUTING_ID`](src/handler.rs).
