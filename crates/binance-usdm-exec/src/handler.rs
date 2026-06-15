use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::parse::{parse_user_events, ParseError};
use crate::types::{
    position_amount_is_zero, MarginCall, PositionChange, PositionKey, SymbolBookkeeping, UsdmExec,
    UsdmExecUpdate,
};

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

    /// Apply a position row to local bookkeeping without outbound side effects.
    pub fn apply_position_bookkeeping(&mut self, position: &PositionChange) {
        Self::upsert_position(&mut self.state, position);
    }

    /// Apply a margin-call payload to local bookkeeping without outbound side effects.
    pub fn apply_margin_call_bookkeeping(&mut self, call: &MarginCall) {
        Self::apply_margin_call(&mut self.state, call);
    }

    fn upsert_position(state: &mut SymbolBookkeeping, position: &PositionChange) {
        let key = PositionKey::from(position);
        if position_amount_is_zero(&position.position_amount) {
            state.positions.remove(&key);
        } else {
            state.positions.insert(key, position.clone());
        }
    }

    fn apply_margin_call(state: &mut SymbolBookkeeping, call: &MarginCall) {
        if state
            .latest_margin_call
            .as_ref()
            .is_some_and(|existing| call.event_time <= existing.event_time)
        {
            return;
        }

        if !call.cross_wallet_balance.is_empty() {
            state.cross_wallet_balance = call.cross_wallet_balance.clone();
        }
        state.latest_margin_call = Some(call.clone());
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
                Self::upsert_position(&mut self.state, position);
            }
            UsdmExecUpdate::BalanceChange(_) | UsdmExecUpdate::ListenKeyExpired => {}
            UsdmExecUpdate::MarginCall(call) => Self::apply_margin_call(&mut self.state, call),
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
