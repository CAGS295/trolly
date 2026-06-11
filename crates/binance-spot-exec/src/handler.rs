use crate::parse::{parse_user_data_message, ParseError};
use crate::types::{SpotExec, SpotUserDataEvent};
use std::cell::RefCell;
use std::rc::Rc;
use trolly_stream::{Endpoints, EventHandler, Message};

/// Shared event sink for tests and wiring; clone the `Rc` across symbol handlers.
pub type SpotExecContext = Rc<RefCell<Vec<SpotUserDataEvent>>>;

/// Per-symbol (or account) handler that records parsed user-data events.
#[derive(Clone, Debug)]
pub struct SpotExecHandler {
    route_id: String,
    events: SpotExecContext,
}

impl SpotExecHandler {
    pub fn route_id(&self) -> &str {
        &self.route_id
    }

    pub fn recorded_events(&self) -> Vec<SpotUserDataEvent> {
        self.events.borrow().clone()
    }

    /// Build a handler for tests and custom wiring without async multiplexor setup.
    pub fn for_route(route_id: impl Into<String>, events: SpotExecContext) -> Self {
        let route_id = route_id.into();
        Self {
            route_id: route_id.clone(),
            events,
        }
    }
}

impl EventHandler<SpotExec> for SpotExecHandler {
    type Error = ParseError;
    type Context = SpotExecContext;
    type Update = SpotUserDataEvent;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        let Message::Text(text) = value else {
            return Err(ParseError::NotText);
        };
        parse_user_data_message(&text)
    }

    fn to_id(event: &Self::Update) -> &str {
        event.route_id()
    }

    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
        self.events.borrow_mut().push(event);
        Ok(())
    }

    async fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        events: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: Endpoints<SpotExec> + Clone + 'static,
    {
        let route_id = symbols
            .first()
            .expect("one route id per handler")
            .as_ref()
            .to_string();

        Ok((
            route_id.clone(),
            SpotExecHandler { route_id, events },
        ))
    }
}
