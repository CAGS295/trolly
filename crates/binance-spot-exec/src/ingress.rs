use std::collections::HashMap;

use tracing::error;
use trolly_stream::{EventHandler, Message, MonitorMultiplexor};

use crate::handler::{SpotExecContext, SpotExecHandler};
use crate::parse::parse_user_data_message;

/// Fan one raw user-data websocket payload through multiplexor routing semantics.
pub fn ingest_user_data(
    hub: &mut MonitorMultiplexor<SpotExecHandler, ()>,
    msg: Message,
) {
    let event = match parse_user_data_message(msg) {
        Ok(Some(event)) => event,
        Ok(None) => return,
        Err(e) => {
            error!("spot user-data parse error: {e}");
            return;
        }
    };

    let id = SpotExecHandler::to_id(&event);
    let Some(handler) = hub.writers.get_mut(id) else {
        error!("missing spot handler id {id}");
        return;
    };

    handler
        .handle_update(event)
        .inspect_err(|e| error!("spot handler error: {e}"))
        .ok();
}

/// Build a multiplexor with per-symbol handlers plus the account route.
pub fn build_multiplexor(
    symbols: &[&str],
    ctx: SpotExecContext,
) -> MonitorMultiplexor<SpotExecHandler, ()> {
    let routes = crate::exec_subscription_symbols(symbols);
    let mut writers = HashMap::with_capacity(routes.len());
    for route in routes {
        writers.insert(route.clone(), SpotExecHandler::new(route, ctx.clone()));
    }
    MonitorMultiplexor::from_writers(writers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;

    use crate::events::SpotUserEvent;

    #[test]
    fn ingest_routes_execution_and_account_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = SpotExecContext {
            events: tx,
            account: Arc::new(Mutex::new(crate::AccountBook::default())),
        };
        let mut hub = build_multiplexor(&["ETHBTC"], ctx);

        ingest_user_data(
            &mut hub,
            Message::Text(include_str!("../tests/fixtures/execution_report.json").into()),
        );
        ingest_user_data(
            &mut hub,
            Message::Text(
                include_str!("../tests/fixtures/outbound_account_position.json").into(),
            ),
        );
        ingest_user_data(
            &mut hub,
            Message::Text(include_str!("../tests/fixtures/balance_update.json").into()),
        );

        let first = rx.try_recv().unwrap();
        assert!(matches!(first, SpotUserEvent::ExecutionReport(_)));

        let second = rx.try_recv().unwrap();
        assert!(matches!(
            second,
            SpotUserEvent::OutboundAccountPosition(_)
        ));

        let third = rx.try_recv().unwrap();
        assert!(matches!(third, SpotUserEvent::BalanceUpdate(_)));
    }
}
