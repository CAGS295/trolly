//! Strategy runtime: consume multi-symbol stream events and dispatch outbound messages.
//!
//! Wires `trolly-stream` [`MessageIngress`] into a single state-handling unit that applies
//! [`Strategy::on_event`] transitions and dispatches [`Command`] values through [`CommandEgress`].

mod command;
mod egress;
mod event;
mod parse;
mod runtime;
mod strategy;

pub use command::Command;
pub use egress::{command_egress, CommandEgress};
pub use event::{StreamEvent, StreamEventKind};
pub use parse::{parse_stream_event, ParseError};
pub use runtime::{StrategyHarness, StrategyRuntime};
pub use strategy::{EchoStrategy, LastDepthStrategy, RecordingStrategy, Strategy};

pub use trolly_stream;

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_stream::Message;

    #[test]
    fn crate_name() {
        assert_eq!(CRATE, "trolly-strategy");
    }

    #[test]
    fn parse_stream_event_variants() {
        let depth = parse_stream_event(&Message::Text("depth:BTCUSDT:100".into())).unwrap();
        assert_eq!(depth.kind, StreamEventKind::Depth);
        assert_eq!(depth.symbol, "BTCUSDT");
        assert_eq!(depth.payload, "100");

        let execution =
            parse_stream_event(&Message::Text("execution:ETHUSDT:fill".into())).unwrap();
        assert_eq!(execution.kind, StreamEventKind::Execution);

        let account = parse_stream_event(&Message::Text("account:BTCUSDT:bal".into())).unwrap();
        assert_eq!(account.kind, StreamEventKind::Account);
    }

    #[test]
    fn recording_strategy_records_events_and_commands() {
        let mut strategy = RecordingStrategy::new().with_responses(vec![
            Command::send_message("a"),
            Command::subscribe("BTCUSDT", "depth"),
        ]);

        let cmds = strategy.on_event(StreamEvent::new("BTCUSDT", StreamEventKind::Depth, "1"));
        assert_eq!(cmds, vec![Command::send_message("a")]);
        assert_eq!(strategy.events.len(), 1);

        let cmds = strategy.on_event(StreamEvent::new(
            "ETHUSDT",
            StreamEventKind::Execution,
            "x",
        ));
        assert_eq!(
            cmds,
            vec![Command::subscribe("BTCUSDT", "depth")]
        );
        assert_eq!(strategy.events.len(), 2);
    }

    #[test]
    fn runtime_dispatches_commands_to_egress() {
        let (egress, mut rx) = command_egress();
        let mut runtime = StrategyRuntime::new(EchoStrategy, egress);

        runtime.handle_event(StreamEvent::new("BTCUSDT", StreamEventKind::Depth, "42"));
        let cmd = rx.try_recv().unwrap();
        assert_eq!(
            cmd,
            Command::send_message("echo:depth:BTCUSDT:42")
        );
    }

    #[test]
    fn integration_synthetic_multi_symbol_events() {
        let mut harness = StrategyHarness::new(RecordingStrategy::new().with_responses(vec![
            Command::send_message("depth-ack"),
            Command::send_message("exec-ack"),
            Command::subscribe("BTCUSDT", "account"),
        ]));

        harness
            .push(Message::Text("depth:BTCUSDT:1".into()))
            .unwrap();
        harness
            .push(Message::Text("execution:ETHUSDT:fill".into()))
            .unwrap();
        harness
            .push(Message::Text("account:BTCUSDT:updated".into()))
            .unwrap();

        harness.drain_all();

        let strategy = harness.runtime.strategy();
        assert_eq!(strategy.events.len(), 3);
        assert_eq!(strategy.events[0].kind, StreamEventKind::Depth);
        assert_eq!(strategy.events[0].symbol, "BTCUSDT");
        assert_eq!(strategy.events[1].kind, StreamEventKind::Execution);
        assert_eq!(strategy.events[1].symbol, "ETHUSDT");
        assert_eq!(strategy.events[2].kind, StreamEventKind::Account);

        assert_eq!(
            harness.drain_commands(),
            vec![
                Command::send_message("depth-ack"),
                Command::send_message("exec-ack"),
                Command::subscribe("BTCUSDT", "account"),
            ]
        );
    }

    #[test]
    fn integration_stateful_last_depth_strategy() {
        let mut harness = StrategyHarness::new(LastDepthStrategy::default());

        harness
            .push(Message::Text("depth:BTCUSDT:100".into()))
            .unwrap();
        harness
            .push(Message::Text("execution:BTCUSDT:ignored".into()))
            .unwrap();
        harness
            .push(Message::Text("depth:ETHUSDT:200".into()))
            .unwrap();

        harness.drain_all();

        let strategy = harness.runtime.strategy();
        assert_eq!(
            strategy.last_depth,
            vec![
                ("BTCUSDT".to_string(), "100".to_string()),
                ("ETHUSDT".to_string(), "200".to_string()),
            ]
        );

        assert_eq!(
            harness.drain_commands(),
            vec![
                Command::subscribe("BTCUSDT", "depth@100"),
                Command::subscribe("ETHUSDT", "depth@200"),
            ]
        );
    }

    #[tokio::test]
    async fn integration_async_ingress_loop() {
        let mut harness = StrategyHarness::new(EchoStrategy);

        harness
            .push(Message::Text("depth:BTCUSDT:9".into()))
            .unwrap();
        harness.close_ingress();

        harness.run_until(|| false).await;

        assert_eq!(
            harness.drain_commands(),
            vec![Command::send_message("echo:depth:BTCUSDT:9")]
        );
    }
}
