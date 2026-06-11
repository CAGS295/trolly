use crate::types::{
    AssetBalance, BalanceUpdate, EventStreamTerminated, ExecutionReport, ExternalLockUpdate,
    ListStatus, ListStatusOrder, OrderSide, OutboundAccountPosition, SpotUserDataEvent,
};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected text websocket frame")]
    NotText,
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing event type field `e`")]
    MissingEventType,
    #[error("unsupported event type: {0}")]
    UnsupportedEventType(String),
    #[error("invalid field `{field}`: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

/// Parse a Binance spot user-data websocket text frame into a normalized event.
///
/// Accepts both WebSocket API envelopes (`{ "subscriptionId", "event": { ... } }`)
/// and legacy listen-key payloads (`{ "e": "executionReport", ... }`).
pub fn parse_user_data_message(text: &str) -> Result<Option<SpotUserDataEvent>, ParseError> {
    let root: Value = serde_json::from_str(text)?;
    let subscription_id = root
        .get("subscriptionId")
        .and_then(Value::as_u64);

    let payload = root
        .get("event")
        .cloned()
        .unwrap_or(root);

    let event_type = payload
        .get("e")
        .and_then(Value::as_str)
        .ok_or(ParseError::MissingEventType)?;

    let event = match event_type {
        "executionReport" => SpotUserDataEvent::ExecutionReport(parse_execution_report(
            subscription_id,
            &payload,
        )?),
        "outboundAccountPosition" => {
            SpotUserDataEvent::OutboundAccountPosition(parse_outbound_account_position(
                subscription_id,
                &payload,
            )?)
        }
        "balanceUpdate" => {
            SpotUserDataEvent::BalanceUpdate(parse_balance_update(subscription_id, &payload)?)
        }
        "listStatus" => {
            SpotUserDataEvent::ListStatus(parse_list_status(subscription_id, &payload)?)
        }
        "externalLockUpdate" => SpotUserDataEvent::ExternalLockUpdate(parse_external_lock_update(
            subscription_id,
            &payload,
        )?),
        "eventStreamTerminated" => SpotUserDataEvent::EventStreamTerminated(
            parse_event_stream_terminated(subscription_id, &payload)?,
        ),
        other => return Err(ParseError::UnsupportedEventType(other.to_string())),
    };

    Ok(Some(event))
}

fn parse_execution_report(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<ExecutionReport, ParseError> {
    Ok(ExecutionReport {
        subscription_id,
        event_time: require_u64(payload, "E")?,
        symbol: require_string(payload, "s")?,
        client_order_id: require_string(payload, "c")?,
        side: parse_side(require_string(payload, "S")?.as_str())?,
        order_type: require_string(payload, "o")?,
        time_in_force: require_string(payload, "f")?,
        order_quantity: require_string(payload, "q")?,
        order_price: require_string(payload, "p")?,
        stop_price: require_string(payload, "P")?,
        iceberg_quantity: require_string(payload, "F")?,
        order_list_id: require_i64(payload, "g")?,
        original_client_order_id: require_string(payload, "C")?,
        execution_type: require_string(payload, "x")?,
        order_status: require_string(payload, "X")?,
        reject_reason: require_string(payload, "r")?,
        order_id: require_u64(payload, "i")?,
        last_executed_quantity: require_string(payload, "l")?,
        cumulative_filled_quantity: require_string(payload, "z")?,
        last_executed_price: require_string(payload, "L")?,
        commission_amount: require_string(payload, "n")?,
        commission_asset: optional_string(payload, "N"),
        transaction_time: require_u64(payload, "T")?,
        trade_id: require_i64(payload, "t")?,
        prevented_match_id: optional_i64(payload, "v"),
        execution_id: require_u64(payload, "I")?,
        is_on_book: require_bool(payload, "w")?,
        is_maker: require_bool(payload, "m")?,
        ignore_field: require_bool(payload, "M")?,
        order_creation_time: require_u64(payload, "O")?,
        cumulative_quote_quantity: require_string(payload, "Z")?,
        last_quote_quantity: require_string(payload, "Y")?,
        quote_order_quantity: require_string(payload, "Q")?,
        working_time: require_u64(payload, "W")?,
        self_trade_prevention_mode: require_string(payload, "V")?,
    })
}

fn parse_outbound_account_position(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<OutboundAccountPosition, ParseError> {
    let balances = payload
        .get("B")
        .and_then(Value::as_array)
        .ok_or(ParseError::InvalidField {
            field: "B",
            reason: "expected array".into(),
        })?
        .iter()
        .map(|entry| {
            Ok(AssetBalance {
                asset: require_string(entry, "a")?,
                free: require_string(entry, "f")?,
                locked: require_string(entry, "l")?,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(OutboundAccountPosition {
        subscription_id,
        event_time: require_u64(payload, "E")?,
        last_account_update: require_u64(payload, "u")?,
        balances,
    })
}

fn parse_balance_update(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<BalanceUpdate, ParseError> {
    Ok(BalanceUpdate {
        subscription_id,
        event_time: require_u64(payload, "E")?,
        asset: require_string(payload, "a")?,
        balance_delta: require_string(payload, "d")?,
        clear_time: require_u64(payload, "T")?,
    })
}

fn parse_list_status(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<ListStatus, ParseError> {
    let orders = payload
        .get("O")
        .and_then(Value::as_array)
        .ok_or(ParseError::InvalidField {
            field: "O",
            reason: "expected array".into(),
        })?
        .iter()
        .map(|entry| {
            Ok(ListStatusOrder {
                symbol: require_string(entry, "s")?,
                order_id: require_u64(entry, "i")?,
                client_order_id: require_string(entry, "c")?,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(ListStatus {
        subscription_id,
        event_time: require_u64(payload, "E")?,
        symbol: require_string(payload, "s")?,
        order_list_id: require_i64(payload, "g")?,
        contingency_type: require_string(payload, "c")?,
        list_status_type: require_string(payload, "l")?,
        list_order_status: require_string(payload, "L")?,
        list_reject_reason: require_string(payload, "r")?,
        list_client_order_id: require_string(payload, "C")?,
        transaction_time: require_u64(payload, "T")?,
        orders,
    })
}

fn parse_external_lock_update(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<ExternalLockUpdate, ParseError> {
    Ok(ExternalLockUpdate {
        subscription_id,
        event_time: require_u64(payload, "E")?,
        asset: require_string(payload, "a")?,
        delta: require_string(payload, "d")?,
        transaction_time: require_u64(payload, "T")?,
    })
}

fn parse_event_stream_terminated(
    subscription_id: Option<u64>,
    payload: &Value,
) -> Result<EventStreamTerminated, ParseError> {
    Ok(EventStreamTerminated {
        subscription_id,
        event_time: require_u64(payload, "E")?,
    })
}

fn parse_side(raw: &str) -> Result<OrderSide, ParseError> {
    match raw {
        "BUY" => Ok(OrderSide::Buy),
        "SELL" => Ok(OrderSide::Sell),
        other => Err(ParseError::InvalidField {
            field: "S",
            reason: format!("unknown side `{other}`"),
        }),
    }
}

fn require_string(value: &Value, field: &'static str) -> Result<String, ParseError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ParseError::InvalidField {
            field,
            reason: "expected string".into(),
        })
}

fn optional_string(value: &Value, field: &'static str) -> Option<String> {
    value
        .get(field)
        .and_then(|v| match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
}

fn require_u64(value: &Value, field: &'static str) -> Result<u64, ParseError> {
    value.get(field).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
    }).ok_or(ParseError::InvalidField {
        field,
        reason: "expected unsigned integer".into(),
    })
}

fn optional_i64(value: &Value, field: &'static str) -> Option<i64> {
    value.get(field).and_then(|v| v.as_i64())
}

fn require_i64(value: &Value, field: &'static str) -> Result<i64, ParseError> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or(ParseError::InvalidField {
            field,
            reason: "expected integer".into(),
        })
}

fn require_bool(value: &Value, field: &'static str) -> Result<bool, ParseError> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(ParseError::InvalidField {
            field,
            reason: "expected bool".into(),
        })
}
