use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::parse::{parse_user_events, ParseError};
use crate::types::{SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Shared context for USDM execution handlers (account book + outbound fan-out).
#[derive(Clone, Debug)]
pub struct UsdmExecContext {
    pub account: Arc<Mutex<SymbolBookkeeping>>,
    pub outbound: Option<UnboundedSender<UsdmExecUpdate>>,
}

impl UsdmExecContext {
    pub fn new(outbound: Option<UnboundedSender<UsdmExecUpdate>>) -> Self {
        Self {
            account: Arc::new(Mutex::new(SymbolBookkeeping::default())),
            outbound,
        }
    }

    pub fn with_account(
        account: Arc<Mutex<SymbolBookkeeping>>,
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    ) -> Self {
        Self { account, outbound }
    }

    /// Read-only snapshot of the account-wide book (balances + positions).
    pub fn account_bookkeeping(&self) -> SymbolBookkeeping {
        self.account
            .lock()
            .expect("account bookkeeping lock poisoned")
            .clone()
    }

    /// Query a single open position leg.
    pub fn position(&self, symbol: &str, position_side: &str) -> Option<crate::types::PositionChange> {
        self.account
            .lock()
            .expect("account bookkeeping lock poisoned")
            .position(symbol, position_side)
            .cloned()
    }
}

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
#[derive(Debug)]
pub struct UsdmExecHandler {
    routing_id: String,
    account_events_only: bool,
    symbol_state: SymbolBookkeeping,
    ctx: UsdmExecContext,
    /// Test hook: record every handled update.
    recorded: Option<Arc<Mutex<Vec<UsdmExecUpdate>>>>,
}

impl UsdmExecHandler {
    pub fn new(routing_id: impl Into<String>, ctx: UsdmExecContext) -> Self {
        let routing_id = routing_id.into();
        let account_events_only = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            account_events_only,
            symbol_state: SymbolBookkeeping::default(),
            ctx,
            recorded: None,
        }
    }

    pub fn with_recorder(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
        ctx: UsdmExecContext,
    ) -> Self {
        let routing_id = routing_id.into();
        let account_events_only = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            account_events_only,
            symbol_state: SymbolBookkeeping::default(),
            ctx,
            recorded: Some(recorded),
        }
    }

    pub fn routing_id(&self) -> &str {
        &self.routing_id
    }

    pub fn context(&self) -> &UsdmExecContext {
        &self.ctx
    }

    /// Per-symbol open-order state (empty for the account handler).
    pub fn symbol_state(&self) -> &SymbolBookkeeping {
        &self.symbol_state
    }

    /// Shared account-wide balances and positions.
    pub fn account_state(&self) -> SymbolBookkeeping {
        self.ctx.account_bookkeeping()
    }

    fn apply(&mut self, update: UsdmExecUpdate) -> Result<(), ParseError> {
        match &update {
            UsdmExecUpdate::OrderTrade(order) => {
                if self.account_events_only {
                    return Ok(());
                }
                if order.order_status == "NEW" || order.order_status == "PARTIALLY_FILLED" {
                    self.symbol_state
                        .open_orders
                        .insert(order.order_id, order.clone());
                } else {
                    self.symbol_state.open_orders.remove(&order.order_id);
                }
            }
            UsdmExecUpdate::PositionChange(position) => {
                if self.account_events_only {
                    return Ok(());
                }
                self.ctx
                    .account
                    .lock()
                    .expect("account bookkeeping lock poisoned")
                    .apply_position(position.clone());
            }
            UsdmExecUpdate::BalanceChange(balance) => {
                if !self.account_events_only {
                    return Ok(());
                }
                self.ctx
                    .account
                    .lock()
                    .expect("account bookkeeping lock poisoned")
                    .apply_balance(balance.clone());
            }
            UsdmExecUpdate::ListenKeyExpired | UsdmExecUpdate::MarginCall(_) => {}
        }

        if let Some(tx) = &self.ctx.outbound {
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
    type Context = UsdmExecContext;
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
        ctx: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: VenueEndpoints + Clone + 'static,
    {
        let routing_id = symbols
            .first()
            .map(|s| s.as_ref().to_string())
            .unwrap_or_else(|| ACCOUNT_ROUTING_ID.to_string());
        Ok((routing_id.clone(), Self::new(routing_id, ctx)))
    }
}
