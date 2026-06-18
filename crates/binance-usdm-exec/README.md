# binance-usdm-exec

Binance USD-M futures execution for the trolly strategy runtime.

## Overview

This crate provides two complementary capabilities:

1. **Inbound** – parses `ORDER_TRADE_UPDATE`, `ACCOUNT_UPDATE`, `MARGIN_CALL`, and related
   private user-data stream events into typed Rust structs and fans them into a
   [`trolly_stream::MonitorMultiplexor`].

2. **Outbound** – places signed REST orders on `POST /fapi/v1/order` and integrates with the
   [`trolly_strategy::StreamEgress`] trait so a strategy runtime can dispatch normalized
   [`OutboundMessage::OrderRequest`](trolly_strategy::OutboundMessage) commands consumed by the
   USDM execution adapter.

## Order Placement API (REST)

Orders are submitted via **`POST /fapi/v1/order`** (REST, not WebSocket).

| Environment | Base URL |
|---|---|
| Production | `https://fapi.binance.com` |
| Demo / testnet | `https://demo-fapi.binance.com` |

Fills and rejects flow back asynchronously through the `ORDER_TRADE_UPDATE` user-data stream
(handled by the existing [`UsdmExecHandler`] / [`ingest_user_data`] path); this crate only
*places* the order.

## Request Signing

Every request is signed with **HMAC-SHA256** over the sorted query parameters
(`key=value&key=value` sorted by key, consistent with the Binance REST signing spec).
The `X-MBX-APIKEY` header carries the API key; the `signature` parameter carries the hex digest.

## Position Side (Hedge Mode)

In Binance USDM **hedge mode** every order must include an explicit `positionSide`
(`LONG` or `SHORT`).  In **one-way mode** the field is omitted (defaults to `BOTH`
server-side).

The [`trolly_strategy::OutboundMessage::OrderRequest`] variant carries an optional
`position_side` field.  Set it to `Some("LONG")` / `Some("SHORT")` for hedge mode, or `None`
for one-way mode.

## Quick Start

```rust,ignore
use binance_usdm_exec::{
    ApiCredentials, HttpOrderClient, UsdmExecEgress, run_order_worker,
    usdm_demo_endpoints,
};

// Credentials
let credentials = ApiCredentials {
    api_key: std::env::var("BINANCE_API_KEY").unwrap(),
    secret_key: std::env::var("BINANCE_SECRET_KEY").unwrap(),
};

// Wire egress to strategy runtime
let (egress, rx) = UsdmExecEgress::new_with_receiver();
// strategy_runtime.set_egress(egress);  // depends on your runtime wiring

// Spawn the order worker (targets demo endpoint)
let client = HttpOrderClient::with_base_url(usdm_demo_endpoints::REST_BASE_URL);
tokio::spawn(run_order_worker(credentials, client, rx));
```

## Building Orders Directly

```rust,ignore
use binance_usdm_exec::{ApiCredentials, HttpOrderClient, OrderRequest, OrderSide, place_order};

let credentials = ApiCredentials { api_key: "..".into(), secret_key: "..".into() };
let client = HttpOrderClient::new();

// One-way mode MARKET order
let ack = place_order(&client, &credentials,
    OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001")
).await?;

// Hedge mode LIMIT order (LONG position)
let ack = place_order(&client, &credentials,
    OrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.001", "60000.00")
        .with_position_side("LONG")
).await?;
```

## Running Tests

```sh
cargo test -p binance-usdm-exec
```

No live API keys are required; tests use an in-process mock client.
