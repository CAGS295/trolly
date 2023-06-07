#![cfg(feature = "grpc")]

use super::Hook;
pub use lob::limit_order_book::protos::limit_order_book_service_client;
pub(super) use lob::limit_order_book::protos::limit_order_book_service_server::LimitOrderBookServiceServer;
pub use lob::limit_order_book::protos::Pair;
pub(super) use lob::limit_order_book::protos::{
    limit_order_book_service_server::LimitOrderBookService, LimitOrderBook,
};
use tonic::{Request, Response, Status};
use tracing::trace;

#[tonic::async_trait]
impl LimitOrderBookService for Hook {
    async fn get_limit_order_book(
        &self,
        request: Request<Pair>,
    ) -> Result<Response<LimitOrderBook>, Status> {
        trace!("Got a request from {:?}", request.remote_addr());

        let pair: String = request.into_inner().pair.to_uppercase();

        let Some(native_book) = self.get_or_default(&pair).await else {
            return Err(Status::not_found(pair));
        };

        Ok(Response::new(From::from(native_book)))
    }
}
