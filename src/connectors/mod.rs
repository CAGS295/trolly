use crate::net::streaming::EventHandler;
use async_trait::async_trait;
use std::{collections::HashMap, marker::PhantomData};
use tokio_tungstenite::tungstenite::Message;
use tracing::error;

///Middleware to handle multiple symbols for the same Monitor.
pub(crate) struct MonitorMultiplexor<'a, Handles, Monitorable> {
    pub writers: HashMap<&'a str, Handles>,
    _p: PhantomData<Monitorable>,
}

#[async_trait(?Send)]
impl<'a, M, H> EventHandler<'a, M> for MonitorMultiplexor<'a, H, M>
where
    M: 'a,
    H: EventHandler<'a, M> + 'a,
{
    type Error = H::Error;
    type Context = H::Context;
    type Update = H::Update;

    fn parse_update(event: Message) -> Result<Option<Self::Update>, ()> {
        H::parse_update(event)
    }

    fn to_id<'b>(event: &'b Self::Update) -> &'b str {
        H::to_id(event)
    }

    fn handle_update(&mut self, update: Self::Update) -> Result<(), ()> {
        let id = Self::to_id(&update);

        let Some(handle) = self.writers.get_mut(id)else{
            error!("Unknown handle {id}");
            return Ok(());
        };

        handle.handle_update(update)
    }

    async fn build<En>(
        provider: &'a En,
        symbols: &'a [String],
        sender: &'a Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: crate::providers::Endpoints<M> + Sync,
        Self::Context: Sync,
    {
        let mut writers: HashMap<&str, H> = HashMap::new();

        for s in symbols.windows(1) {
            let handle = H::build(provider, s, sender)
                .await
                .expect("Building underlying handle");
            writers.insert(&s[0], handle);
        }
        Ok(Self {
            writers,
            _p: PhantomData,
        })
    }
}