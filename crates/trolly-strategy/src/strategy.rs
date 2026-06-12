//! Strategy trait and test doubles.

use crate::egress::OutboundMessage;
use crate::event::StreamEvent;

/// Core state-handling unit: consume stream events and emit outbound commands.
pub trait Strategy {
    fn on_event(&mut self, event: &StreamEvent) -> Vec<OutboundMessage>;
}

/// Records every consumed event; optionally returns canned outbound commands.
#[derive(Debug, Default)]
pub struct RecordingStrategy {
    pub consumed: Vec<StreamEvent>,
    pub responses: Vec<OutboundMessage>,
}

impl RecordingStrategy {
    pub fn with_responses(responses: Vec<OutboundMessage>) -> Self {
        Self {
            consumed: Vec::new(),
            responses,
        }
    }
}

impl Strategy for RecordingStrategy {
    fn on_event(&mut self, event: &StreamEvent) -> Vec<OutboundMessage> {
        self.consumed.push(event.clone());
        self.responses.clone()
    }
}
