use super::handler::EventHandler;
use crate::providers::Endpoints;
use std::{collections::HashMap, marker::PhantomData};
use tokio_tungstenite::tungstenite::Message;
use tracing::error;

///Middleware to handle multiple symbols for the same Monitor.
pub(crate) struct MonitorMultiplexor<Handles, Monitorable> {
    pub writers: HashMap<String, Handles>,
    _p: PhantomData<Monitorable>,
}

impl<M, H> EventHandler<M> for MonitorMultiplexor<H, M>
where
    H: EventHandler<M> + 'static,
{
    type Error = H::Error;
    type Context = H::Context;
    type Update = H::Update;

    fn parse_update(event: Message) -> Result<Option<Self::Update>, Self::Error> {
        H::parse_update(event)
    }

    fn to_id(event: &Self::Update) -> &str {
        H::to_id(event)
    }

    fn handle_update(&mut self, update: Self::Update) -> Result<(), Self::Error> {
        let id = Self::to_id(&update);

        let Some(handle) = self.writers.get_mut(id) else {
            error!("Unknown handle {id}");
            return Ok(());
        };

        handle.handle_update(update)
    }

    async fn build<En>(
        provider: En,
        symbols: &[String],
        sender: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: Endpoints<M> + Clone + 'static,
        Self::Context: Clone + 'static,
    {
        let mut writers = HashMap::with_capacity(symbols.len() - 1);
        let mut handles = Vec::with_capacity(symbols.len() - 1);

        for s in symbols.windows(1) {
            let provider = provider.clone();
            let s = s.to_vec().clone();
            let sender = sender.clone();
            let f = async move {
                H::build(provider, s.as_slice(), sender)
                    .await
                    .unwrap_or_else(|_| panic!("Building underlying handle for {s:?} failed."))
            };

            handles.push(tokio::task::spawn_local(f));
        }

        for (i, h) in handles.into_iter().enumerate() {
            if let Ok(handle) = h.await {
                writers.insert(symbols[i].clone(), handle);
            }
        }

        Ok(Self {
            writers,
            _p: PhantomData,
        })
    }
}
