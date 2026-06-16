use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::parse::{parse_user_events, ParseError};
use crate::types::{position_is_flat, PositionKey, SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
#[derive(Debug)]
pub struct UsdmExecHandler {
    routing_id: String,
    state: SymbolBookkeeping,
    outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    /// Test hook: record every handled update.
    recorded: Option<Arc<Mutex<Vec<UsdmExecUpdate>>>>,
}

impl UsdmExecHandler {
    pub fn new(
        routing_id: impl Into<String>,
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    ) -> Self {
        Self {
            routing_id: routing_id.into(),
            state: SymbolBookkeeping::default(),
            outbound,
            recorded: None,
        }
    }

    pub fn with_recorder(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
    ) -> Self {
        Self {
            routing_id: routing_id.into(),
            state: SymbolBookkeeping::default(),
            outbound: None,
            recorded: Some(recorded),
        }
    }

    pub fn routing_id(&self) -> &str {
        &self.routing_id
    }

    pub fn state(&self) -> &SymbolBookkeeping {
        &self.state
    }

    fn apply(&mut self, update: UsdmExecUpdate) -> Result<(), ParseError> {
        match &update {
            UsdmExecUpdate::OrderTrade(order) => {
                if order.order_status == "NEW" || order.order_status == "PARTIALLY_FILLED" {
                    self.state
                        .open_orders
                        .insert(order.order_id, order.clone());
                } else {
                    self.state.open_orders.remove(&order.order_id);
                }
            }
            UsdmExecUpdate::PositionChange(position) => {
                let key = PositionKey::from_change(position);
                if position_is_flat(&position.position_amount) {
                    self.state.positions.remove(&key);
                } else {
                    self.state.positions.insert(key, position.clone());
                }
            }
            UsdmExecUpdate::BalanceChange(_)
            | UsdmExecUpdate::ListenKeyExpired => {}
            UsdmExecUpdate::MarginCall(call) => {
                self.state.apply_margin_call(call);
            }
        }

        if let Some(tx) = &self.outbound {
            let _ = tx.send(update.clone());
        }
        if let Some(recorded) = &self.recorded {
            recorded.lock().unwrap().push(update);
        }

        Ok(())
    }
}

impl EventHandler<UsdmExec> for UsdmExecHandler {
    type Error = ParseError;
    type Context = Option<UnboundedSender<UsdmExecUpdate>>;
    type Update = UsdmExecUpdate;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        let events = parse_user_events(value)?;
        Ok(events.into_iter().next())
    }

    fn to_id(event: &Self::Update) -> &str {
        event.routing_id()
    }

    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
        self.apply(event)
    }

    async fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        outbound: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: VenueEndpoints + Clone + 'static,
    {
        let routing_id = symbols
            .first()
            .map(|s| s.as_ref().to_string())
            .unwrap_or_else(|| ACCOUNT_ROUTING_ID.to_string());
        Ok((
            routing_id.clone(),
            Self::new(routing_id, outbound),
        ))
    }
}
