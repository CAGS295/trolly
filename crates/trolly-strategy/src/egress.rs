//! Egress API for outbound strategy commands.

use crate::command::Command;
use tokio::sync::mpsc::{self, error::SendError, UnboundedReceiver, UnboundedSender};

/// Injectable sink for strategy outbound commands.
#[derive(Clone, Debug)]
pub struct CommandEgress {
    tx: UnboundedSender<Command>,
}

impl CommandEgress {
    pub fn dispatch(&self, command: Command) -> Result<(), SendError<Command>> {
        self.tx.send(command)
    }

    pub fn dispatch_all(
        &self,
        commands: impl IntoIterator<Item = Command>,
    ) -> Result<(), SendError<Command>> {
        for command in commands {
            self.dispatch(command)?;
        }
        Ok(())
    }
}

/// Pair of egress handle and receiver consumed by tests or downstream wiring.
pub fn command_egress() -> (CommandEgress, UnboundedReceiver<Command>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (CommandEgress { tx }, rx)
}
