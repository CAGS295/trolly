use log::debug;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{Error, Result},
    MaybeTlsStream, WebSocketStream,
};
use url::Url;

type WebSocket<S> = WebSocketStream<MaybeTlsStream<S>>;

pub async fn connect(ws_uri: Url) -> Result<WebSocket<TcpStream>, Error> {
    let (socket, res): (WebSocket<_>, _) =
        connect_async_tls_with_config(ws_uri, None, None).await?;
    debug!("Connection response: {res:?}");
    Ok(socket)
}

pub async fn disconnect(ws: &mut WebSocket<TcpStream>) -> Result<(), Error> {
    ws.close(None).await
}
