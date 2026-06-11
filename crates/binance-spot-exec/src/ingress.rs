use crate::parse::{parse_user_data_message, ParseError};
use crate::types::SpotUserDataEvent;
use trolly_stream::{Message, MessageIngress};
use tokio::sync::mpsc::error::SendError;

/// Push raw user-data JSON into a multiplexor ingress channel.
pub fn push_raw_user_data(
    ingress: &MessageIngress,
    text: impl Into<String>,
) -> Result<(), SendError<Message>> {
    ingress.push(Message::Text(text.into().into()))
}

/// Parse a websocket text payload and push the original frame into ingress when it is a user-data event.
pub fn parse_and_push(
    ingress: &MessageIngress,
    text: &str,
) -> Result<Option<SpotUserDataEvent>, IngressError> {
    let event = parse_user_data_message(text)?;
    if event.is_some() {
        push_raw_user_data(ingress, text).map_err(IngressError::Send)?;
    }
    Ok(event)
}

#[derive(Debug, thiserror::Error)]
pub enum IngressError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("ingress send failed")]
    Send(#[from] SendError<Message>),
}
