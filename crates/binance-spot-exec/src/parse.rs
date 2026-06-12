use serde_json::Value;
use trolly_stream::Message;

use crate::events::{
    AssetBalance, BalanceUpdate, EventStreamTerminated, ExecutionReport, ExternalLockUpdate,
    ListStatus, ListStatusOrder, OutboundAccountPosition, SpotUserEvent,
};

#[derive(Debug)]
pub enum ParseError {
    InvalidMessage,
    Json(serde_json::Error),
    UnknownEvent(String),
    MissingField(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMessage => write!(f, "invalid websocket message"),
            Self::Json(e) => write!(f, "json parse error: {e}"),
            Self::UnknownEvent(t) => write!(f, "unknown event type: {t}"),
            Self::MissingField(field) => write!(f, "missing field {field}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<serde_json::Error> for ParseError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn parse_user_data_message(msg: Message) -> Result<Option<SpotUserEvent>, ParseError> {
    let Message::Text(text) = msg else {
        return Ok(None);
    };

    let root: Value = serde_json::from_str(&text)?;

    if root.get("result").is_some() && root.get("id").is_some() {
        return Ok(None);
    }

    let payload = if let Some(event) = root.get("event") {
        event
    } else {
        &root
    };

    let event_type = payload
        .get("e")
        .and_then(Value::as_str)
        .ok_or(ParseError::MissingField("e"))?;

    let event = match event_type {
        "executionReport" => SpotUserEvent::ExecutionReport(parse_execution_report(payload)?),
        "outboundAccountPosition" => {
            SpotUserEvent::OutboundAccountPosition(parse_outbound_account_position(payload)?)
        }
        "balanceUpdate" => SpotUserEvent::BalanceUpdate(parse_balance_update(payload)?),
        "listStatus" => SpotUserEvent::ListStatus(parse_list_status(payload)?),
        "externalLockUpdate" => {
            SpotUserEvent::ExternalLockUpdate(parse_external_lock_update(payload)?)
        }
        "eventStreamTerminated" => {
            SpotUserEvent::EventStreamTerminated(parse_event_stream_terminated(payload)?)
        }
        other => return Err(ParseError::UnknownEvent(other.into())),
    };

    Ok(Some(event))
}

fn str_field(value: &Value, key: &'static str) -> Result<String, ParseError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(ParseError::MissingField(key))
}

fn u64_field(value: &Value, key: &'static str) -> Result<u64, ParseError> {
    value
        .get(key)
        .and_then(|v| v.as_u64())
        .ok_or(ParseError::MissingField(key))
}

fn i64_field(value: &Value, key: &'static str) -> Result<i64, ParseError> {
    value
        .get(key)
        .and_then(|v| v.as_i64())
        .ok_or(ParseError::MissingField(key))
}

fn bool_field(value: &Value, key: &'static str) -> Result<bool, ParseError> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(ParseError::MissingField(key))
}

fn parse_execution_report(value: &Value) -> Result<ExecutionReport, ParseError> {
    Ok(ExecutionReport {
        event_time: u64_field(value, "E")?,
        symbol: str_field(value, "s")?,
        client_order_id: str_field(value, "c")?,
        side: str_field(value, "S")?,
        order_type: str_field(value, "o")?,
        time_in_force: str_field(value, "f")?,
        quantity: str_field(value, "q")?,
        price: str_field(value, "p")?,
        execution_type: str_field(value, "x")?,
        order_status: str_field(value, "X")?,
        order_id: i64_field(value, "i")?,
        last_qty: str_field(value, "l")?,
        cumulative_qty: str_field(value, "z")?,
        last_price: str_field(value, "L")?,
        transaction_time: u64_field(value, "T")?,
        trade_id: i64_field(value, "t")?,
        is_on_book: bool_field(value, "w")?,
    })
}

fn parse_outbound_account_position(value: &Value) -> Result<OutboundAccountPosition, ParseError> {
    let balances = value
        .get("B")
        .and_then(Value::as_array)
        .ok_or(ParseError::MissingField("B"))?
        .iter()
        .map(|entry| {
            Ok(AssetBalance {
                asset: str_field(entry, "a")?,
                free: str_field(entry, "f")?,
                locked: str_field(entry, "l")?,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(OutboundAccountPosition {
        event_time: u64_field(value, "E")?,
        last_update_time: u64_field(value, "u")?,
        balances,
    })
}

fn parse_balance_update(value: &Value) -> Result<BalanceUpdate, ParseError> {
    Ok(BalanceUpdate {
        event_time: u64_field(value, "E")?,
        asset: str_field(value, "a")?,
        delta: str_field(value, "d")?,
        clear_time: u64_field(value, "T")?,
    })
}

fn parse_list_status(value: &Value) -> Result<ListStatus, ParseError> {
    let orders = value
        .get("O")
        .and_then(Value::as_array)
        .ok_or(ParseError::MissingField("O"))?
        .iter()
        .map(|entry| {
            Ok(ListStatusOrder {
                symbol: str_field(entry, "s")?,
                order_id: i64_field(entry, "i")?,
                client_order_id: str_field(entry, "c")?,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(ListStatus {
        event_time: u64_field(value, "E")?,
        symbol: str_field(value, "s")?,
        list_status_type: str_field(value, "l")?,
        list_order_status: str_field(value, "L")?,
        orders,
    })
}

fn parse_external_lock_update(value: &Value) -> Result<ExternalLockUpdate, ParseError> {
    Ok(ExternalLockUpdate {
        event_time: u64_field(value, "E")?,
        asset: str_field(value, "a")?,
        delta: str_field(value, "d")?,
        transaction_time: u64_field(value, "T")?,
    })
}

fn parse_event_stream_terminated(value: &Value) -> Result<EventStreamTerminated, ParseError> {
    Ok(EventStreamTerminated {
        event_time: u64_field(value, "E")?,
    })
}
