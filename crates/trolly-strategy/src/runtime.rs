//! Strategy runtime: ingress → state → egress.

use crate::command::Command;
use crate::egress::CommandEgress;
use crate::event::StreamEvent;
use crate::parse::{parse_stream_event, ParseError};
use crate::strategy::Strategy;
use trolly_stream::{message_ingress, MessageIngress};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::protocol::Message;

/// Single state-handling unit wired to stream ingress and command egress.
pub struct StrategyRuntime<S: Strategy> {
    strategy: S,
    egress: CommandEgress,
}

impl<S: Strategy> StrategyRuntime<S> {
    pub fn new(strategy: S, egress: CommandEgress) -> Self {
        Self { strategy, egress }
    }

    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    pub fn egress(&self) -> &CommandEgress {
        &self.egress
    }

    /// Apply one normalized event and dispatch returned commands.
    pub fn handle_event(&mut self, event: StreamEvent) {
        let commands = self.strategy.on_event(event);
        let _ = self.egress.dispatch_all(commands);
    }

    /// Parse a websocket frame and route it through the strategy.
    pub fn handle_message(&mut self, message: Message) -> Result<Vec<Command>, ParseError> {
        let event = parse_stream_event(&message)?;
        let commands = self.strategy.on_event(event);
        self.egress.dispatch_all(commands.iter().cloned()).ok();
        Ok(commands)
    }

    /// Drain synthetic or live frames from an inject channel until it closes or `should_stop`.
    pub async fn run_ingress_until(
        &mut self,
        inject_rx: &mut UnboundedReceiver<Message>,
        should_stop: impl Fn() -> bool,
    ) {
        while !should_stop() {
            let Some(message) = inject_rx.recv().await else {
                break;
            };
            let _ = self.handle_message(message);
        }
    }
}

/// Convenience bundle: ingress sender + runtime for offline integration tests.
pub struct StrategyHarness<S: Strategy> {
    ingress: Option<MessageIngress>,
    pub runtime: StrategyRuntime<S>,
    inject_rx: UnboundedReceiver<Message>,
    command_rx: UnboundedReceiver<Command>,
}

impl<S: Strategy> StrategyHarness<S> {
    pub fn new(strategy: S) -> Self {
        let (ingress, inject_rx) = message_ingress();
        let (egress, command_rx) = crate::egress::command_egress();
        let runtime = StrategyRuntime::new(strategy, egress);
        Self {
            ingress: Some(ingress),
            runtime,
            inject_rx,
            command_rx,
        }
    }

    pub fn ingress(&self) -> Option<&MessageIngress> {
        self.ingress.as_ref()
    }

    pub fn push(
        &self,
        message: Message,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<Message>> {
        self.ingress
            .as_ref()
            .expect("ingress closed")
            .push(message)
    }

    /// Drop the inject sender so [`Self::run_until`] exits after queued frames are drained.
    pub fn close_ingress(&mut self) {
        self.ingress = None;
    }

    pub fn drain_all(&mut self) {
        while let Ok(message) = self.inject_rx.try_recv() {
            let _ = self.runtime.handle_message(message);
        }
    }

    pub fn drain_commands(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(command) = self.command_rx.try_recv() {
            out.push(command);
        }
        out
    }

    pub async fn drain_one(&mut self) -> Option<Vec<Command>> {
        let message = self.inject_rx.recv().await?;
        self.runtime.handle_message(message).ok()
    }

    pub fn try_recv_command(&mut self) -> Option<Command> {
        self.command_rx.try_recv().ok()
    }

    pub async fn recv_command(&mut self) -> Option<Command> {
        self.command_rx.recv().await
    }

    pub async fn run_until<F>(&mut self, should_stop: F)
    where
        F: Fn() -> bool,
    {
        self.runtime
            .run_ingress_until(&mut self.inject_rx, should_stop)
            .await;
    }
}
