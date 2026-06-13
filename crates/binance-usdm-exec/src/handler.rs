use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::account::UsdmAccountBookkeeping;
use crate::parse::{parse_user_events, ParseError};
use crate::types::{SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Shared context for USDM execution handlers.
#[derive(Clone)]
pub struct UsdmExecContext {
    pub outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    pub account: Arc<Mutex<UsdmAccountBookkeeping>>,
}

impl UsdmExecContext {
    pub fn new(outbound: Option<UnboundedSender<UsdmExecUpdate>>) -> Self {
        Self {
            outbound,
            account: Arc::new(Mutex::new(UsdmAccountBookkeeping::default())),
        }
    }

    pub fn with_account(
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
        account: Arc<Mutex<UsdmAccountBookkeeping>>,
    ) -> Self {
        Self { outbound, account }
    }
}

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
pub struct UsdmExecHandler {
    routing_id: String,
    account_events_only: bool,
    state: SymbolBookkeeping,
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
            state: SymbolBookkeeping::default(),
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
            state: SymbolBookkeeping::default(),
            ctx,
            recorded: Some(recorded),
        }
    }

    pub fn routing_id(&self) -> &str {
        &self.routing_id
    }

    pub fn state(&self) -> &SymbolBookkeeping {
        &self.state
    }

    pub fn account(&self) -> Arc<Mutex<UsdmAccountBookkeeping>> {
        self.ctx.account.clone()
    }

    fn apply(&mut self, update: UsdmExecUpdate) -> Result<(), ParseError> {
        match &update {
            UsdmExecUpdate::OrderTrade(order) => {
                if self.account_events_only {
                    return Ok(());
                }
                if order.order_status == "NEW" || order.order_status == "PARTIALLY_FILLED" {
                    self.state
                        .open_orders
                        .insert(order.order_id, order.clone());
                } else {
                    self.state.open_orders.remove(&order.order_id);
                }
            }
            UsdmExecUpdate::PositionChange(position) => {
                if self.account_events_only {
                    return Ok(());
                }
                self.state.apply_position(position);
                self.ctx
                    .account
                    .lock()
                    .expect("account lock poisoned")
                    .apply_position(position);
            }
            UsdmExecUpdate::BalanceChange(balance) => {
                if !self.account_events_only {
                    return Ok(());
                }
                self.ctx
                    .account
                    .lock()
                    .expect("account lock poisoned")
                    .apply_balance(balance);
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
