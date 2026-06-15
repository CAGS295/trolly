# binance-usdm-exec

Binance USD-M futures execution: **outbound order placement** via signed REST, plus **account bookkeeping** driven by WebSocket user-data streams (fills/rejects reconcile through `ORDER_TRADE_UPDATE` — no duplicate order state machine).

## Outbound orders (REST)

Order placement uses Binance USDM REST [`POST /fapi/v1/order`](https://developers.binance.com/docs/derivatives/usds-margined-futures/trade/rest-api/New-Order). WebSocket trading API was not chosen so the same HMAC query signing used elsewhere maps cleanly to form-encoded POST bodies.

Request builder covers market/limit basics:

| Field | REST param | Notes |
|-------|------------|-------|
| symbol | `symbol` | uppercased |
| side | `side` | `BUY` / `SELL` |
| quantity | `quantity` | string decimal |
| price | `price` | limit orders only |
| time-in-force | `timeInForce` | default `GTC` for limits |
| position side | `positionSide` | required in hedge mode (`LONG` / `SHORT` / `BOTH`); omit in one-way mode |

```rust
use binance_usdm_exec::{
    ApiCredentials, OrderSide, PositionSide, UsdmOrderClient, UsdmOrderRequest, UsdmRestEgress,
    DEFAULT_REST_BASE_URL,
};
use trolly_strategy::{OutboundMessage, StreamEgress};

let client = UsdmOrderClient::new(
    DEFAULT_REST_BASE_URL,
    ApiCredentials { api_key: "...".into(), secret_key: "...".into() },
);
let request = UsdmOrderRequest::limit_with_position(
    "BTCUSDT",
    OrderSide::Buy,
    "0.01",
    "100000",
    PositionSide::Long,
);
let ack = client.place_order(request).await?;

// Strategy egress adapter (fills still arrive on user-data stream):
let mut egress = UsdmRestEgress::new(DEFAULT_REST_BASE_URL, credentials);
egress.dispatch(OutboundMessage::order_request(
    "BTCUSDT",
    "BUY",
    "0.01",
    Some("100000".into()),
    Some("LONG".into()),
))?;
```

Demo REST base: `https://demo-fapi.binance.com` (see `.env.example` `DEMO_BINANCE_KEY` / `DEMO_BINANCE_SECRET`).

## Stream subscription (user-data)

Private USDM execution events are delivered on a **private** user-data websocket keyed by `listenKey` (created via `POST /fapi/v1/listenKey` on the REST host). This crate does not call listen-key endpoints; callers supply an active key.

Recommended private URL:

```text
wss://fstream.binance.com/private/ws/<listenKey>
```

Optional event filtering:

```text
wss://fstream.binance.com/private/ws?listenKey=<key>&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE
```

`UsdmUserDataStream` builds the websocket URL via [`VenueEndpoints::websocket_url`](https://docs.rs/trolly-stream/latest/trolly_stream/trait.VenueEndpoints.html). Fan execution events into a shared [`MonitorMultiplexor`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html) via [`ingest_user_data`](src/ingress.rs).

## Multi-symbol routing (`trolly-stream`)

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE`, `MARGIN_CALL` | `__account__` |

Use [`build_multiplexor`](src/ingress.rs) to register per-symbol handlers plus the account-wide handler.
