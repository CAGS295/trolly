#![cfg(any(feature = "codec", feature = "grpc"))]

pub mod grpc;
pub mod scale;

#[cfg(feature = "prometheus")]
pub mod metrics;

use axum::{extract::Request, response::IntoResponse, Router};
use futures_util::TryFutureExt;
#[cfg(feature = "grpc")]
pub use grpc::{limit_order_book_service_client, Pair};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use left_right::ReadHandleFactory;
use lob::LimitOrderBook;
use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedReceiver;
use tower::Service;
use tracing::{error, info, warn};

pub type BookRegistry = Arc<Mutex<HashMap<String, ReadHandleFactory<LimitOrderBook>>>>;

pub fn new_registry() -> BookRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

#[derive(Clone)]
pub struct Hook(BookRegistry);

impl Hook {
    pub fn new(registry: BookRegistry) -> Self {
        Self(registry)
    }

    pub fn register(&self, key: impl Into<String>, factory: ReadHandleFactory<LimitOrderBook>) {
        self.0
            .lock()
            .expect("book registry lock")
            .insert(key.into().to_uppercase(), factory);
    }

    async fn get(&self, pair: &str) -> Option<LimitOrderBook> {
        let factory = {
            let guard = self.0.lock().expect("book registry lock");
            guard.get(&pair.to_uppercase())?.clone()
        };
        factory.handle().enter().map(|guard| guard.clone())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn inner_start(hook: Hook, port: u16) -> Result<(), ()> {
    let addr: std::net::SocketAddr = ("::1".parse::<Ipv6Addr>().unwrap(), port).into();

    info!("BookServer listening on {addr}");

    let router = Router::new();

    #[cfg(feature = "grpc")]
    let router = {
        use tonic::server::NamedService;

        let svc = grpc::LimitOrderBookServiceServer::new(hook.clone());
        let path = format!(
            "/{}/*rest",
            <grpc::LimitOrderBookServiceServer<Hook> as NamedService>::NAME
        );
        router.route_service(&path, svc)
    };

    #[cfg(feature = "codec")]
    let router = {
        use tower_http::compression::CompressionLayer;

        router.route(
            "/scale/depth/:symbol",
            axum::routing::get(scale::serve_book)
                .with_state(hook.clone())
                .layer(CompressionLayer::new()),
        )
    };

    #[cfg(feature = "prometheus")]
    let router = router.route(
        "/metrics",
        axum::routing::get(|| async {
            (
                [(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                metrics::encode(),
            )
                .into_response()
        }),
    );

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("{e}");
                continue;
            }
        };

        info!("Connection {addr}, accepted");

        stream.set_nodelay(true).unwrap();

        let tower_service = router.clone();
        tokio::spawn(async move {
            let stream = TokioIo::new(stream);
            let hyper_service = hyper::service::service_fn(move |req: Request<Incoming>| {
                tower_service.clone().call(req)
            });

            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

            let x = builder
                .serve_connection(stream, hyper_service)
                .map_err(|e| {
                    error!("{e}");
                });
            x.await.unwrap();
        });
    }
}

/// Start the API server after `n` `(symbol, factory)` pairs arrive on `receiver`.
pub fn start(
    mut receiver: UnboundedReceiver<(String, ReadHandleFactory<LimitOrderBook>)>,
    port: u16,
    n: usize,
) -> Result<(), ()> {
    let hook = Hook::new(new_registry());
    for _ in 0..n {
        if let Some((pair, factory)) = receiver.blocking_recv() {
            hook.register(pair, factory);
        } else {
            warn!("Channel closed while waiting on RPC handlers.");
            break;
        }
    }
    inner_start(hook, port)?;
    Ok(())
}

/// Start the API server immediately; callers register books on `registry` as they come online.
pub fn start_background(registry: BookRegistry, port: u16) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        inner_start(Hook::new(registry), port).expect("book server");
    })
}

#[cfg(test)]
mod test {
    use super::Hook;
    use crate::monitor::order_book::Operations;
    use left_right::WriteHandle;
    use lob::LimitOrderBook;

    struct TestBooks {
        hook: Hook,
        _writers: Vec<WriteHandle<LimitOrderBook, Operations>>,
    }

    fn hook_with_books(keys: &[&str]) -> TestBooks {
        let registry = super::new_registry();
        let mut writers = Vec::new();
        for key in keys {
            let (mut w, r) = left_right::new::<LimitOrderBook, Operations>();
            w.publish();
            registry
                .lock()
                .expect("lock")
                .insert(key.to_string(), r.factory());
            writers.push(w);
        }
        TestBooks {
            hook: Hook::new(registry),
            _writers: writers,
        }
    }

    #[tokio::test]
    async fn lookup_standard_symbol() {
        let tb = hook_with_books(&["BTCUSDT"]);

        assert!(tb.hook.get("BTCUSDT").await.is_some());
        assert!(tb.hook.get("btcusdt").await.is_some());
        assert!(tb.hook.get("RPI:BTCUSDT").await.is_none());
    }

    #[tokio::test]
    async fn lookup_rpi_symbol() {
        let tb = hook_with_books(&["RPI:BTCUSDT"]);

        assert!(tb.hook.get("RPI:BTCUSDT").await.is_some());
        assert!(tb.hook.get("rpi:btcusdt").await.is_some());
        assert!(tb.hook.get("BTCUSDT").await.is_none());
    }

    #[tokio::test]
    async fn lookup_both_standard_and_rpi() {
        let tb = hook_with_books(&["BTCUSDT", "RPI:BTCUSDT"]);

        assert!(tb.hook.get("btcusdt").await.is_some());
        assert!(tb.hook.get("rpi:btcusdt").await.is_some());
    }

    #[tokio::test]
    async fn lookup_missing_symbol_returns_none() {
        let tb = hook_with_books(&["BTCUSDT"]);

        assert!(tb.hook.get("ETHUSDT").await.is_none());
        assert!(tb.hook.get("RPI:ETHUSDT").await.is_none());
    }

    #[tokio::test]
    async fn dynamic_register_after_start() {
        let registry = super::new_registry();
        let hook = Hook::new(registry.clone());
        assert!(hook.get("BTCUSDT").await.is_none());

        let (mut w, r) = left_right::new::<LimitOrderBook, Operations>();
        w.publish();
        hook.register("BTCUSDT", r.factory());

        assert!(hook.get("BTCUSDT").await.is_some());
    }
}
