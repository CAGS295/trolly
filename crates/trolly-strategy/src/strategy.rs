//! Strategy trait and test doubles.

use crate::command::Command;
use crate::event::{StreamEvent, StreamEventKind};

/// Core strategy interface: consume normalized stream events, emit outbound commands.
pub trait Strategy {
    fn on_event(&mut self, event: StreamEvent) -> Vec<Command>;
}

/// Test double that records every consumed event and returns configured commands.
#[derive(Debug, Default, Clone)]
pub struct RecordingStrategy {
    pub events: Vec<StreamEvent>,
    responses: Vec<Command>,
}

impl RecordingStrategy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_responses(mut self, responses: impl IntoIterator<Item = Command>) -> Self {
        self.responses = responses.into_iter().collect();
        self
    }

    pub fn push_response(&mut self, command: Command) {
        self.responses.push(command);
    }
}

impl Strategy for RecordingStrategy {
    fn on_event(&mut self, event: StreamEvent) -> Vec<Command> {
        self.events.push(event);
        if self.responses.is_empty() {
            Vec::new()
        } else {
            vec![self.responses.remove(0)]
        }
    }
}

/// Echo strategy: emits one `SendMessage` per event for integration smoke tests.
#[derive(Debug, Default, Clone)]
pub struct EchoStrategy;

impl Strategy for EchoStrategy {
    fn on_event(&mut self, event: StreamEvent) -> Vec<Command> {
        vec![Command::send_message(format!(
            "echo:{}:{}:{}",
            event.kind, event.symbol, event.payload
        ))]
    }
}

/// Example stateful strategy used in tests: tracks last depth per symbol.
#[derive(Debug, Default, Clone)]
pub struct LastDepthStrategy {
    pub last_depth: Vec<(String, String)>,
}

impl Strategy for LastDepthStrategy {
    fn on_event(&mut self, event: StreamEvent) -> Vec<Command> {
        if event.kind != StreamEventKind::Depth {
            return Vec::new();
        }
        self.last_depth
            .push((event.symbol.clone(), event.payload.clone()));
        vec![Command::subscribe(
            event.symbol,
            format!("depth@{}", event.payload),
        )]
    }
}
