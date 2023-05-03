#![cfg(feature = "grpc")]

use axum::Router;
use hyper::server::conn::AddrIncoming;
use left_right::ReadHandleFactory;
pub use lob::limit_order_book::protos::Pair;
use lob::limit_order_book::protos::{
    limit_order_book_service_server::{LimitOrderBookService, LimitOrderBookServiceServer},
    LimitOrderBook,
};
use std::{net::Ipv6Addr, time::Duration};
use tonic::transport::NamedService;
use tonic::{Request, Response, Status};
use tracing::{error, info, trace};

pub struct Hook(ReadHandleFactory<lob::LimitOrderBook>);

#[tonic::async_trait]
impl LimitOrderBookService for Hook {
    async fn get_limit_order_book(
        &self,
        request: Request<Pair>,
    ) -> Result<Response<LimitOrderBook>, Status> {
        trace!("Got a request from {:?}", request.remote_addr());

        let native_book = self
            .0
            .handle()
            .enter()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        Ok(Response::new(From::from(native_book)))
    }
}

#[tokio::main(flavor = "current_thread")]
pub async fn start(factory: ReadHandleFactory<lob::LimitOrderBook>, port: u16) -> Result<(), ()> {
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
