pub mod grpc;
pub mod scale;

use axum::routing::get;
use axum::Router;
use grpc::LimitOrderBookServiceServer;
pub use grpc::{limit_order_book_service_client, Pair};
use hyper::server::conn::AddrIncoming;
use left_right::ReadHandleFactory;
use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tonic::transport::NamedService;
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

    let svc = LimitOrderBookServiceServer::new(Hook(factory.clone()));
    let path = format!(
        "/{}/*rest",
        <LimitOrderBookServiceServer<Hook> as NamedService>::NAME
    );

    let app = Router::new().route_service(&path, svc).route(
        &"/scale/depth/:symbol",
        get(scale::serve_book).with_state(Hook(factory)),
    );

    let incoming = AddrIncoming::bind(&addr).unwrap();
    axum::Server::builder(incoming)
        .tcp_keepalive_interval(Some(Duration::from_millis(500)))
        .tcp_nodelay(true)
        .http2_only(true)
        .serve(app.into_make_service())
        .await
        .map_err(|e| {
            error!("{e}");
        })?;

    Ok(())
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
