//! Parse Binance USDM user-data websocket JSON into normalized events.

use crate::events::{
    AccountUpdate, BalanceUpdate, ListenKeyExpired, MarginCall, MarginCallPosition, OrderDetail,
    OrderTradeUpdate, PositionUpdate, UsdmStreamEvent,
};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug)]
pub enum ParseError {
    NotText,
    Json(serde_json::Error),
    Unexpected(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => write!(f, "expected text websocket frame"),
            Self::Json(e) => write!(f, "json parse error: {e}"),
            Self::Unexpected(s) => write!(f, "unexpected user-data payload: {s}"),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Deserialize)]
struct NullResponse {
    result: Option<serde_json::Value>,
    #[allow(dead_code)]
    id: Option<u64>,
}

#[derive(Deserialize)]
struct StreamEnvelope {
    #[allow(dead_code)]
    stream: String,
    data: RawUserEvent,
}

#[derive(Deserialize)]
#[serde(tag = "e", rename_all = "SCREAMING_SNAKE_CASE")]
enum RawUserEvent {
    OrderTradeUpdate {
        #[serde(rename = "E")]
        event_time: u64,
        #[serde(rename = "T")]
        transaction_time: u64,
        #[serde(rename = "o")]
        order: RawOrderDetail,
    },
    AccountUpdate {
        #[serde(rename = "E")]
        event_time: u64,
        #[serde(rename = "T")]
        transaction_time: u64,
        #[serde(rename = "a")]
        account: RawAccountData,
    },
    MarginCall {
        #[serde(rename = "E")]
        event_time: u64,
        #[serde(rename = "cw")]
        cross_wallet_balance: String,
        #[serde(rename = "p")]
        positions: Vec<RawMarginCallPosition>,
    },
    ListenKeyExpired {
        #[serde(rename = "E")]
        event_time: u64,
    },
}

#[derive(Deserialize)]
struct RawAccountData {
    #[serde(rename = "m")]
    reason: String,
    #[serde(rename = "B", default)]
    balances: Vec<RawBalanceUpdate>,
    #[serde(rename = "P", default)]
    positions: Vec<RawPositionUpdate>,
}

#[derive(Deserialize)]
struct RawBalanceUpdate {
    #[serde(rename = "a")]
    asset: String,
    #[serde(rename = "wb")]
    wallet_balance: String,
    #[serde(rename = "cw")]
    cross_wallet_balance: String,
    #[serde(rename = "bc")]
    balance_change: String,
}

#[derive(Deserialize)]
struct RawPositionUpdate {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "pa")]
    position_amount: String,
    #[serde(rename = "ep")]
    entry_price: String,
    #[serde(rename = "cr")]
    accumulated_realized: String,
    #[serde(rename = "up")]
    unrealized_pnl: String,
    #[serde(rename = "mt")]
    margin_type: String,
    #[serde(rename = "iw")]
    isolated_wallet: String,
    #[serde(rename = "ps")]
    position_side: String,
}

#[derive(Deserialize)]
struct RawMarginCallPosition {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "ps")]
    position_side: String,
    #[serde(rename = "pa")]
    position_amount: String,
    #[serde(rename = "mt")]
    margin_type: String,
    #[serde(rename = "iw")]
    isolated_wallet: String,
    #[serde(rename = "mp")]
    mark_price: String,
    #[serde(rename = "up")]
    unrealized_pnl: String,
    #[serde(rename = "mm")]
    maintenance_margin_required: String,
}

#[derive(Deserialize)]
struct RawOrderDetail {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "c")]
    client_order_id: String,
    #[serde(rename = "S")]
    side: String,
    #[serde(rename = "o")]
    order_type: String,
    #[serde(rename = "f")]
    time_in_force: String,
    #[serde(rename = "q")]
    original_qty: String,
    #[serde(rename = "p")]
    original_price: String,
    #[serde(rename = "ap")]
    average_price: String,
    #[serde(rename = "x")]
    execution_type: String,
    #[serde(rename = "X")]
    order_status: String,
    #[serde(rename = "i")]
    order_id: i64,
    #[serde(rename = "l")]
    last_filled_qty: String,
    #[serde(rename = "z")]
    cumulative_filled_qty: String,
    #[serde(rename = "L")]
    last_filled_price: String,
    #[serde(rename = "N")]
    commission_asset: Option<String>,
    #[serde(rename = "n")]
    commission: Option<String>,
    #[serde(rename = "T")]
    trade_time: u64,
    #[serde(rename = "t")]
    trade_id: i64,
    #[serde(rename = "m")]
    is_maker: bool,
    #[serde(rename = "ps")]
    position_side: String,
    #[serde(rename = "rp")]
    realized_profit: String,
}

/// Parse one websocket text frame into zero or more normalized events.
///
/// Returns [`None`] for benign control frames (subscription acks, etc.).
pub fn parse_user_data_message(message: Message) -> Result<Option<Vec<UsdmStreamEvent>>, ParseError> {
    let Message::Text(text) = message else {
        return Err(ParseError::NotText);
    };
    parse_user_data_str(&text)
}

/// Parse raw JSON bytes with the same semantics as [`parse_user_data_message`].
pub fn parse_user_data_bytes(data: &[u8]) -> Result<Option<Vec<UsdmStreamEvent>>, ParseError> {
    parse_user_data_str(&String::from_utf8_lossy(data))
}

fn parse_user_data_str(text: &str) -> Result<Option<Vec<UsdmStreamEvent>>, ParseError> {
    let data = text.as_bytes();

    if let Ok(envelope) = serde_json::from_slice::<StreamEnvelope>(data) {
        return Ok(Some(vec![convert_raw(envelope.data)?]));
    }

    if let Ok(raw) = serde_json::from_slice::<RawUserEvent>(data) {
        return Ok(Some(vec![convert_raw(raw)?]));
    }

    if let Ok(ack) = serde_json::from_slice::<NullResponse>(data) {
        if ack.result.is_none() {
            return Ok(None);
        }
    }

    Err(ParseError::Unexpected(text.to_string()))
}

fn convert_raw(raw: RawUserEvent) -> Result<UsdmStreamEvent, ParseError> {
    Ok(match raw {
        RawUserEvent::OrderTradeUpdate {
            event_time,
            transaction_time,
            order,
        } => UsdmStreamEvent::OrderTradeUpdate(OrderTradeUpdate {
            event_time,
            transaction_time,
            order: OrderDetail {
                symbol: order.symbol,
                client_order_id: order.client_order_id,
                side: order.side,
                order_type: order.order_type,
                time_in_force: order.time_in_force,
                original_qty: order.original_qty,
                original_price: order.original_price,
                average_price: order.average_price,
                execution_type: order.execution_type,
                order_status: order.order_status,
                order_id: order.order_id,
                last_filled_qty: order.last_filled_qty,
                cumulative_filled_qty: order.cumulative_filled_qty,
                last_filled_price: order.last_filled_price,
                commission_asset: order.commission_asset,
                commission: order.commission,
                trade_time: order.trade_time,
                trade_id: order.trade_id,
                is_maker: order.is_maker,
                position_side: order.position_side,
                realized_profit: order.realized_profit,
            },
        }),
        RawUserEvent::AccountUpdate {
            event_time,
            transaction_time,
            account,
        } => UsdmStreamEvent::AccountUpdate(AccountUpdate {
            event_time,
            transaction_time,
            reason: account.reason,
            balances: account
                .balances
                .into_iter()
                .map(|b| BalanceUpdate {
                    asset: b.asset,
                    wallet_balance: b.wallet_balance,
                    cross_wallet_balance: b.cross_wallet_balance,
                    balance_change: b.balance_change,
                })
                .collect(),
            positions: account
                .positions
                .into_iter()
                .map(|p| PositionUpdate {
                    symbol: p.symbol,
                    position_amount: p.position_amount,
                    entry_price: p.entry_price,
                    accumulated_realized: p.accumulated_realized,
                    unrealized_pnl: p.unrealized_pnl,
                    margin_type: p.margin_type,
                    isolated_wallet: p.isolated_wallet,
                    position_side: p.position_side,
                })
                .collect(),
        }),
        RawUserEvent::MarginCall {
            event_time,
            cross_wallet_balance,
            positions,
        } => UsdmStreamEvent::MarginCall(MarginCall {
            event_time,
            cross_wallet_balance,
            positions: positions
                .into_iter()
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
                .collect(),
        }),
        RawUserEvent::ListenKeyExpired { event_time } => {
            UsdmStreamEvent::ListenKeyExpired(ListenKeyExpired { event_time })
        }
    })
}

impl From<serde_json::Error> for ParseError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        fs::read_to_string(path).expect("fixture")
    }

    #[test]
    fn parse_order_trade_update_fixture() {
        let events = parse_user_data_str(&fixture("order_trade_update.json"))
            .unwrap()
            .expect("event");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UsdmStreamEvent::OrderTradeUpdate(o) => {
                assert_eq!(o.order.symbol, "BTCUSDT");
                assert_eq!(o.order.order_id, 8886774);
                assert_eq!(o.order.execution_type, "NEW");
                assert_eq!(o.order.position_side, "LONG");
            }
            other => panic!("expected order update, got {other:?}"),
        }
    }

    #[test]
    fn parse_account_update_fixture() {
        let events = parse_user_data_str(&fixture("account_update.json"))
            .unwrap()
            .expect("event");
        assert_eq!(events.len(), 1);
        match &events[0] {
            UsdmStreamEvent::AccountUpdate(a) => {
                assert_eq!(a.reason, "ORDER");
                assert_eq!(a.balances.len(), 2);
                assert_eq!(a.positions.len(), 3);
                assert_eq!(a.route_symbols(), vec!["BTCUSDT".to_string()]);
            }
            other => panic!("expected account update, got {other:?}"),
        }
    }

    #[test]
    fn parse_enveloped_order_trade_update() {
        let events = parse_user_data_str(&fixture("order_trade_update_envelope.json"))
            .unwrap()
            .expect("event");
        assert!(matches!(events[0], UsdmStreamEvent::OrderTradeUpdate(_)));
    }

    #[test]
    fn parse_subscription_ack_returns_none() {
        let out = parse_user_data_str(r#"{"result":null,"id":1}"#).unwrap();
        assert!(out.is_none());
    }
}
