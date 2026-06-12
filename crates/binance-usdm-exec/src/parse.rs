use serde::Deserialize;
use thiserror::Error;
use trolly_stream::Message;

use crate::types::{
    BalanceChange, MarginCall, MarginCallPosition, OrderTradeUpdate, PositionChange, UsdmExecUpdate,
};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected text websocket message")]
    NotText,
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported or unknown event: {event}")]
    UnknownEvent { event: String },
    #[error("missing field {field} in {context}")]
    MissingField { field: String, context: &'static str },
}

/// Parse one user-data websocket payload into zero or more routed updates.
///
/// `ACCOUNT_UPDATE` may fan out into balance rows (account handler) and one
/// [`UsdmExecUpdate::PositionChange`] per affected symbol.
pub fn parse_user_events(msg: Message) -> Result<Vec<UsdmExecUpdate>, ParseError> {
    let Message::Text(text) = msg else {
        return Err(ParseError::NotText);
    };

    let root: serde_json::Value = serde_json::from_str(&text)?;
    let payload = unwrap_combined_envelope(&root);

    if payload.get("result").is_some() && payload.get("id").is_some() {
        return Ok(vec![]);
    }

    let event_type = payload
        .get("e")
        .and_then(|v| v.as_str())
        .ok_or(ParseError::UnknownEvent {
            event: "(missing)".into(),
        })?;

    match event_type {
        "ORDER_TRADE_UPDATE" => {
            let update = parse_order_trade_update(payload)?;
            Ok(vec![UsdmExecUpdate::OrderTrade(update)])
        }
        "ACCOUNT_UPDATE" => Ok(parse_account_update(payload)?),
        "listenKeyExpired" => Ok(vec![UsdmExecUpdate::ListenKeyExpired]),
        "MARGIN_CALL" => {
            let call = parse_margin_call(payload)?;
            Ok(vec![UsdmExecUpdate::MarginCall(call)])
        }
        other => Err(ParseError::UnknownEvent {
            event: other.into(),
        }),
    }
}

fn unwrap_combined_envelope<'a>(root: &'a serde_json::Value) -> &'a serde_json::Value {
    root.get("data").unwrap_or(root)
}

fn parse_order_trade_update(payload: &serde_json::Value) -> Result<OrderTradeUpdate, ParseError> {
    let event_time = required_u64(payload, "E", "ORDER_TRADE_UPDATE")?;
    let transaction_time = required_u64(payload, "T", "ORDER_TRADE_UPDATE")?;
    let order = payload
        .get("o")
        .ok_or(ParseError::MissingField {
            field: "o".into(),
            context: "ORDER_TRADE_UPDATE",
        })?;

    Ok(OrderTradeUpdate {
        event_time,
        transaction_time,
        symbol: required_str(order, "s", "order")?,
        client_order_id: optional_str(order, "c"),
        side: required_str(order, "S", "order")?,
        order_type: required_str(order, "o", "order")?,
        time_in_force: optional_str(order, "f"),
        original_qty: optional_str(order, "q"),
        original_price: optional_str(order, "p"),
        average_price: optional_str(order, "ap"),
        execution_type: required_str(order, "x", "order")?,
        order_status: required_str(order, "X", "order")?,
        order_id: required_i64(order, "i", "order")?,
        last_filled_qty: optional_str(order, "l"),
        accumulated_filled_qty: optional_str(order, "z"),
        last_filled_price: optional_str(order, "L"),
        trade_id: order.get("t").and_then(|v| v.as_i64()).unwrap_or(0),
        position_side: optional_str(order, "ps"),
        realized_profit: optional_str(order, "rp"),
    })
}

fn parse_account_update(payload: &serde_json::Value) -> Result<Vec<UsdmExecUpdate>, ParseError> {
    let event_time = required_u64(payload, "E", "ACCOUNT_UPDATE")?;
    let account = payload
        .get("a")
        .ok_or(ParseError::MissingField {
            field: "a".into(),
            context: "ACCOUNT_UPDATE",
        })?;
    let reason = optional_str(account, "m");

    let mut out = Vec::new();

    if let Some(balances) = account.get("B").and_then(|v| v.as_array()) {
        for balance in balances {
            out.push(UsdmExecUpdate::BalanceChange(BalanceChange {
                event_time,
                reason: reason.clone(),
                asset: required_str(balance, "a", "balance")?,
                wallet_balance: optional_str(balance, "wb"),
                cross_wallet_balance: optional_str(balance, "cw"),
                balance_change: optional_str(balance, "bc"),
            }));
        }
    }

    if let Some(positions) = account.get("P").and_then(|v| v.as_array()) {
        for position in positions {
            out.push(UsdmExecUpdate::PositionChange(PositionChange {
                event_time,
                reason: reason.clone(),
                symbol: required_str(position, "s", "position")?,
                position_amount: optional_str(position, "pa"),
                entry_price: optional_str(position, "ep"),
                unrealized_pnl: optional_str(position, "up"),
                margin_type: optional_str(position, "mt"),
                isolated_wallet: optional_str(position, "iw"),
                position_side: optional_str(position, "ps"),
            }));
        }
    }

    Ok(out)
}

#[derive(Deserialize)]
struct MarginCallPositionRaw {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "ps", default)]
    position_side: String,
    #[serde(rename = "pa", default)]
    position_amount: String,
    #[serde(rename = "mt", default)]
    margin_type: String,
    #[serde(rename = "iw", default)]
    isolated_wallet: String,
    #[serde(rename = "mp", default)]
    mark_price: String,
    #[serde(rename = "up", default)]
    unrealized_pnl: String,
    #[serde(rename = "mm", default)]
    maintenance_margin_required: String,
}

fn parse_margin_call(payload: &serde_json::Value) -> Result<MarginCall, ParseError> {
    let event_time = required_u64(payload, "E", "MARGIN_CALL")?;
    let cross_wallet_balance = optional_str(payload, "cw");
    let positions = payload
        .get("p")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value::<MarginCallPositionRaw>(row.clone()).ok())
                .map(|p| MarginCallPosition {
                    symbol: p.symbol,
                    position_side: p.position_side,
                    position_amount: p.position_amount,
                    margin_type: p.margin_type,
                    isolated_wallet: p.isolated_wallet,
                    mark_price: p.mark_price,
                    unrealized_pnl: p.unrealized_pnl,
                    maintenance_margin_required: p.maintenance_margin_required,
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(MarginCall {
        event_time,
        cross_wallet_balance,
        positions,
    })
}

fn required_str(value: &serde_json::Value, field: &str, context: &'static str) -> Result<String, ParseError> {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or(ParseError::MissingField {
            field: field.to_string(),
            context,
        })
}

fn optional_str(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn required_u64(value: &serde_json::Value, field: &str, context: &'static str) -> Result<u64, ParseError> {
    value
        .get(field)
        .and_then(|v| v.as_u64())
        .ok_or(ParseError::MissingField {
            field: field.to_string(),
            context,
        })
}

fn required_i64(value: &serde_json::Value, field: &str, context: &'static str) -> Result<i64, ParseError> {
    value
        .get(field)
        .and_then(|v| v.as_i64())
        .ok_or(ParseError::MissingField {
            field: field.to_string(),
            context,
        })
}
