#![cfg(feature = "grpc")]

use axum::Router;
use hyper::server::conn::AddrIncoming;
use left_right::ReadHandleFactory;
pub use lob::limit_order_book::protos::limit_order_book_service_client;
pub use lob::limit_order_book::protos::Pair;
use lob::limit_order_book::protos::{
    limit_order_book_service_server::{LimitOrderBookService, LimitOrderBookServiceServer},
    LimitOrderBook,
};
use std::collections::HashMap;
use std::{net::Ipv6Addr, time::Duration};
use tokio::sync::mpsc::UnboundedReceiver;
use tonic::transport::NamedService;
use tonic::{Request, Response, Status};
use tracing::{error, info, trace, warn};

pub struct Hook(HashMap<String, ReadHandleFactory<lob::LimitOrderBook>>);

#[tonic::async_trait]
impl LimitOrderBookService for Hook {
    async fn get_limit_order_book(
        &self,
        request: Request<Pair>,
    ) -> Result<Response<LimitOrderBook>, Status> {
        trace!("Got a request from {:?}", request.remote_addr());

        let pair: String = request.into_inner().pair.to_uppercase();

        let Some(native_book) = self.0.get(&pair)else{
            return Err(Status::not_found(pair));
        };

        let native_book = native_book
            .handle()
            .enter()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        Ok(Response::new(From::from(native_book)))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn inner_start(
    factory: HashMap<String, ReadHandleFactory<lob::LimitOrderBook>>,
    port: u16,
) -> Result<(), ()> {
    let addr: std::net::SocketAddr = ("::1".parse::<Ipv6Addr>().unwrap(), port).into();

    info!("BookServer listening on {addr}");

    let svc = LimitOrderBookServiceServer::new(Hook(factory));
    let path = format!(
        "/{}/*rest",
        <LimitOrderBookServiceServer<Hook> as NamedService>::NAME
    );

    let app = Router::new().route_service(&path, svc);

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
