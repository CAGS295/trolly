use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::parse::{parse_user_events, ParseError};
use crate::types::{is_zero_amount, AccountBookkeeping, SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
#[derive(Debug)]
pub struct UsdmExecHandler {
    routing_id: String,
    state: SymbolBookkeeping,
    /// Shared account-wide position state updated on every [`UsdmExecUpdate::PositionChange`].
    account_state: Option<Arc<Mutex<AccountBookkeeping>>>,
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
            account_state: None,
            outbound,
            recorded: None,
        }
    }

    /// Construct a handler that writes position changes into a shared [`AccountBookkeeping`].
    pub fn new_with_account_state(
        routing_id: impl Into<String>,
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
        account_state: Arc<Mutex<AccountBookkeeping>>,
    ) -> Self {
        Self {
            routing_id: routing_id.into(),
            state: SymbolBookkeeping::default(),
            account_state: Some(account_state),
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
            account_state: None,
            outbound: None,
            recorded: Some(recorded),
        }
    }

    /// Construct a recording handler that also writes positions into a shared [`AccountBookkeeping`].
    pub fn with_recorder_and_account_state(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
        account_state: Arc<Mutex<AccountBookkeeping>>,
    ) -> Self {
        Self {
            routing_id: routing_id.into(),
            state: SymbolBookkeeping::default(),
            account_state: Some(account_state),
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

    /// Access the shared account-wide bookkeeping if this handler was wired to one.
    pub fn account_state(&self) -> Option<Arc<Mutex<AccountBookkeeping>>> {
        self.account_state.clone()
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
                // Per-symbol view: keyed by position_side with zero/flatten semantics.
                if is_zero_amount(&position.position_amount) {
                    self.state.positions.remove(&position.position_side);
                } else {
                    self.state.positions.insert(position.position_side.clone(), position.clone());
                }
                // Account-wide view: keyed by (symbol, position_side).
                if let Some(account_state) = &self.account_state {
                    account_state.lock().unwrap().apply_position(position);
                }
            }
            UsdmExecUpdate::BalanceChange(_) | UsdmExecUpdate::ListenKeyExpired => {}
            UsdmExecUpdate::MarginCall(call) => {
                if let Some(account_state) = &self.account_state {
                    account_state.lock().unwrap().apply_margin_call(call);
                }
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
