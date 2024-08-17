pub mod grpc;
pub mod scale;

use axum::{extract::Request, Router};
use futures_util::TryFutureExt;
#[cfg(feature = "grpc")]
pub use grpc::{limit_order_book_service_client, Pair};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use left_right::ReadHandleFactory;
use std::collections::HashMap;
use std::net::Ipv6Addr;
use tokio::sync::mpsc::UnboundedReceiver;
use tower::Service;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct Hook(HashMap<String, ReadHandleFactory<lob::LimitOrderBook>>);

impl Hook {
    async fn get_or_default(&self, pair: &String) -> Option<lob::LimitOrderBook> {
        let native_book = self.0.get(&pair.to_uppercase())?;

        let native_book = native_book
            .handle()
            .enter()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        Some(native_book)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn inner_start(
    factory: HashMap<String, ReadHandleFactory<lob::LimitOrderBook>>,
    port: u16,
) -> Result<(), ()> {
    let addr: std::net::SocketAddr = ("::1".parse::<Ipv6Addr>().unwrap(), port).into();

    info!("BookServer listening on {addr}");

    let router = Router::new();

    #[cfg(feature = "grpc")]
    let router = {
        use tonic::server::NamedService;

        let svc = grpc::LimitOrderBookServiceServer::new(Hook(factory.clone()));
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
                .with_state(Hook(factory))
                .layer(CompressionLayer::new()),
        )
    };

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
            //builder.keep_alive_interval(Some(Duration::from_millis(500)));
            //builder.timer(TokioTimer::new());

            let x = builder
                .serve_connection(stream, hyper_service)
                .map_err(|e| {
                    error!("{e}");
                });
            x.await.unwrap();
        });
    }
}

pub fn start(
    mut receiver: UnboundedReceiver<(String, ReadHandleFactory<lob::LimitOrderBook>)>,
    port: u16,
    n: usize,
) -> Result<(), ()> {
    let mut readers = HashMap::new();
    for _ in 0..n {
        if let Some((pair, factory)) = receiver.blocking_recv() {
            readers.insert(pair, factory);
        } else {
            warn!("Channel closed while waiting on RPC handlers.");
            break;
        }
    }
    inner_start(readers, port)?;
    Ok(())
}
