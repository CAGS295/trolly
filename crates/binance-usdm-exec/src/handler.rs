use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::parse::{parse_user_events, ParseError};
use crate::types::{
    apply_position_change, AccountBookkeeping, SymbolBookkeeping, UsdmExec, UsdmExecUpdate,
};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
#[derive(Debug)]
pub struct UsdmExecHandler {
    routing_id: String,
    is_account: bool,
    symbol_state: SymbolBookkeeping,
    account_state: AccountBookkeeping,
    outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    /// Test hook: record every handled update.
    recorded: Option<Arc<Mutex<Vec<UsdmExecUpdate>>>>,
}

impl UsdmExecHandler {
    pub fn new(
        routing_id: impl Into<String>,
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    ) -> Self {
        let routing_id = routing_id.into();
        let is_account = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            is_account,
            symbol_state: SymbolBookkeeping::default(),
            account_state: AccountBookkeeping::default(),
            outbound,
            recorded: None,
        }
    }

    pub fn with_recorder(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
    ) -> Self {
        let routing_id = routing_id.into();
        let is_account = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            is_account,
            symbol_state: SymbolBookkeeping::default(),
            account_state: AccountBookkeeping::default(),
            outbound: None,
            recorded: Some(recorded),
        }
    }

    pub fn routing_id(&self) -> &str {
        &self.routing_id
    }

    pub fn is_account(&self) -> bool {
        self.is_account
    }

    pub fn symbol_state(&self) -> &SymbolBookkeeping {
        &self.symbol_state
    }

    pub fn account_state(&self) -> &AccountBookkeeping {
        &self.account_state
    }

    /// Apply balance/position bookkeeping without forwarding or recording.
    pub fn apply_bookkeeping(&mut self, update: &UsdmExecUpdate) {
        match update {
            UsdmExecUpdate::OrderTrade(order) => {
                if self.is_account {
                    return;
                }
                if order.order_status == "NEW" || order.order_status == "PARTIALLY_FILLED" {
                    self.symbol_state
                        .open_orders
                        .insert(order.order_id, order.clone());
                } else {
                    self.symbol_state.open_orders.remove(&order.order_id);
                }
            }
            UsdmExecUpdate::BalanceChange(balance) => {
                if !self.is_account {
                    return;
                }
                self.account_state
                    .balances
                    .insert(balance.asset.clone(), balance.clone());
            }
            UsdmExecUpdate::PositionChange(position) => {
                if self.is_account {
                    apply_position_change(&mut self.account_state.positions, position);
                } else {
                    apply_position_change(&mut self.symbol_state.positions, position);
                }
            }
            UsdmExecUpdate::ListenKeyExpired | UsdmExecUpdate::MarginCall(_) => {}
        }
    }

    fn apply(&mut self, update: UsdmExecUpdate) -> Result<(), ParseError> {
        self.apply_bookkeeping(&update);

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
