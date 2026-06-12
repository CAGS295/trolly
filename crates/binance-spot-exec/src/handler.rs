use std::sync::{Arc, Mutex};

use trolly_stream::{EventHandler, Message, VenueEndpoints};
use tokio::sync::mpsc::UnboundedSender;

use crate::account::AccountBook;
use crate::events::{ACCOUNT_ROUTE_ID, SpotUserEvent};
use crate::parse::{ParseError, parse_user_data_message};

/// Shared context for spot execution handlers.
#[derive(Clone)]
pub struct SpotExecContext {
    pub events: UnboundedSender<SpotUserEvent>,
    pub account: Arc<Mutex<AccountBook>>,
}

/// Per-route handler for Binance spot user-data events.
pub struct SpotExecHandler {
    route_id: String,
    account_events_only: bool,
    ctx: SpotExecContext,
}

impl SpotExecHandler {
    pub fn new(route_id: impl Into<String>, ctx: SpotExecContext) -> Self {
        let route_id = route_id.into();
        let account_events_only = route_id == ACCOUNT_ROUTE_ID;
        Self {
            route_id,
            account_events_only,
            ctx,
        }
    }

    fn accepts(&self, event: &SpotUserEvent) -> bool {
        if self.account_events_only {
            matches!(
                event,
                SpotUserEvent::OutboundAccountPosition(_)
                    | SpotUserEvent::BalanceUpdate(_)
                    | SpotUserEvent::ExternalLockUpdate(_)
                    | SpotUserEvent::EventStreamTerminated(_)
            )
        } else {
            matches!(
                event,
                SpotUserEvent::ExecutionReport(_) | SpotUserEvent::ListStatus(_)
            )
        }
    }
}

impl EventHandler<()> for SpotExecHandler {
    type Error = ParseError;
    type Context = SpotExecContext;
    type Update = SpotUserEvent;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        parse_user_data_message(value)
    }

    fn to_id(event: &Self::Update) -> &str {
        event.route_id()
    }

    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
        if event.route_id() != self.route_id || !self.accepts(&event) {
            return Ok(());
        }

        self.ctx
            .account
            .lock()
            .expect("account lock poisoned")
            .apply(&event);

        let _ = self.ctx.events.send(event);
        Ok(())
    }

    async fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        ctx: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: VenueEndpoints + Clone + 'static,
    {
        let route_id = symbols
            .first()
            .expect("exactly one route id")
            .as_ref()
            .to_string();
        Ok((route_id.clone(), SpotExecHandler::new(route_id, ctx)))
    }
}
