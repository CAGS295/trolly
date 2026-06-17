use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::account::{apply_position_to_symbol, AccountBookkeeping};
use crate::parse::{parse_user_events, ParseError};
use crate::types::{SymbolBookkeeping, UsdmExec, UsdmExecUpdate};

/// Routing key for account-wide events (`ACCOUNT_UPDATE` balances, `MARGIN_CALL`, etc.).
pub const ACCOUNT_ROUTING_ID: &str = "__account__";

/// Shared context for USDM execution handlers (outbound fan-out + account bookkeeping).
#[derive(Clone)]
pub struct UsdmExecContext {
    pub outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    pub account: Arc<Mutex<AccountBookkeeping>>,
}

impl UsdmExecContext {
    pub fn new(outbound: Option<UnboundedSender<UsdmExecUpdate>>) -> Self {
        Self {
            outbound,
            account: Arc::new(Mutex::new(AccountBookkeeping::default())),
        }
    }
}

/// Per-symbol (or account) USDM execution bookkeeper implementing [`EventHandler`].
pub struct UsdmExecHandler {
    routing_id: String,
    is_account: bool,
    state: SymbolBookkeeping,
    ctx: UsdmExecContext,
    /// Test hook: record every handled update.
    recorded: Option<Arc<Mutex<Vec<UsdmExecUpdate>>>>,
}

impl UsdmExecHandler {
    pub fn new(
        routing_id: impl Into<String>,
        outbound: Option<UnboundedSender<UsdmExecUpdate>>,
    ) -> Self {
        Self::with_context(routing_id, UsdmExecContext::new(outbound))
    }

    pub fn with_context(routing_id: impl Into<String>, ctx: UsdmExecContext) -> Self {
        let routing_id = routing_id.into();
        let is_account = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            is_account,
            state: SymbolBookkeeping::default(),
            ctx,
            recorded: None,
        }
    }

    pub fn with_recorder(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
    ) -> Self {
        Self::with_recorder_and_context(
            routing_id,
            recorded,
            UsdmExecContext::new(None),
        )
    }

    pub fn with_recorder_and_context(
        routing_id: impl Into<String>,
        recorded: Arc<Mutex<Vec<UsdmExecUpdate>>>,
        ctx: UsdmExecContext,
    ) -> Self {
        let routing_id = routing_id.into();
        let is_account = routing_id == ACCOUNT_ROUTING_ID;
        Self {
            routing_id,
            is_account,
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

    pub fn context(&self) -> &UsdmExecContext {
        &self.ctx
    }

    pub fn account_bookkeeping(&self) -> Arc<Mutex<AccountBookkeeping>> {
        Arc::clone(&self.ctx.account)
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
                apply_position_to_symbol(&mut self.state, position);
                self.ctx
                    .account
                    .lock()
                    .expect("account lock poisoned")
                    .apply_position(position);
            }
            UsdmExecUpdate::BalanceChange(balance) => {
                if self.is_account {
                    self.ctx
                        .account
                        .lock()
                        .expect("account lock poisoned")
                        .apply_balance(balance);
                }
            }
            UsdmExecUpdate::ListenKeyExpired => {}
            UsdmExecUpdate::MarginCall(call) => {
                if self.is_account {
                    self.ctx
                        .account
                        .lock()
                        .expect("account lock poisoned")
                        .apply_margin_call(call);
                }
            }
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

impl std::fmt::Debug for UsdmExecHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsdmExecHandler")
            .field("routing_id", &self.routing_id)
            .field("is_account", &self.is_account)
            .field("state", &self.state)
            .field("recorded", &self.recorded.is_some())
            .finish_non_exhaustive()
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
        Ok((
            routing_id.clone(),
            Self::with_context(routing_id, ctx),
        ))
    }
}
