# binance-usdm-exec

Binance USD-M futures execution and account bookkeeping. **Inbound** order and account updates are driven by WebSocket user-data streams (WP-009). **Outbound** order placement uses signed REST `POST /fapi/v1/order` (WP-014); fills and rejects are still reconciled via the existing `ORDER_TRADE_UPDATE` user-data path — no duplicate state machine in this crate.

## Outbound orders (REST)

We use the Binance **REST** trading API (not the WebSocket trading API) for place-order because strategy egress adapters issue one-shot signed HTTP requests. User-data ingest remains WebSocket-only.

- Production: `https://fapi.binance.com/fapi/v1/order`
- Testnet: `https://testnet.binancefuture.com/fapi/v1/order`
- Demo: `https://demo-fapi.binance.com/fapi/v1/order`

Signed query params: `symbol`, `side`, `type` (`MARKET` / `LIMIT`), `quantity`, `positionSide` (`BOTH` one-way default, `LONG` / `SHORT` in hedge mode), optional `price` + `timeInForce` (`GTC`, `IOC`, `FOK`, `GTX`) for limits, `timestamp`, `signature` (HMAC-SHA256). Header: `X-MBX-APIKEY`.

[`NewOrderRequest`](src/order.rs) builds market/limit requests. [`UsdmOrderClient::place_order`](src/outbound.rs) submits them. [`UsdmExecEgress`](src/outbound.rs) implements [`trolly_strategy::StreamEgress`] and maps [`OutboundMessage::OrderRequest`] to REST calls (defaults `positionSide=BOTH`; set explicitly via [`NewOrderRequest::position_side`](src/order.rs) for hedge mode).

## User-data stream (listen key)

Private execution events require an active `listenKey`:

1. **Caller responsibility:** create via `POST /fapi/v1/listenKey`, keepalive via `PUT /fapi/v1/listenKey` every ~30 minutes, and recreate on `listenKeyExpired`. This crate does not manage listen-key lifecycle.
2. Connect with [`UsdmUserDataStream`](src/endpoints.rs):

```text
wss://fstream.binance.com/private/ws/<listenKey>
```

Optional event filter:

```text
wss://fstream.binance.com/private/ws?listenKey=<key>&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE
```

Fan execution events into a shared [`MonitorMultiplexor`](https://docs.rs/trolly-stream/latest/trolly_stream/struct.MonitorMultiplexor.html) via [`ingest_user_data`](src/ingress.rs).

## Multi-symbol routing

| Event | Route key (`EventHandler::to_id`) |
|-------|-----------------------------------|
| `ORDER_TRADE_UPDATE` | trading symbol (e.g. `BTCUSDT`) |
| `ACCOUNT_UPDATE`, `MARGIN_CALL`, `listenKeyExpired` | `__account__` |

Pass trading symbols plus [`ACCOUNT_ROUTING_ID`](src/handler.rs) when building the multiplexor via [`build_multiplexor`](src/ingress.rs).

[`trolly_strategy::StreamEgress`]: https://docs.rs/trolly-strategy
[`OutboundMessage::OrderRequest`]: https://docs.rs/trolly-strategy
