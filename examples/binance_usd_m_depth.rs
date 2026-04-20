use futures_util::{SinkExt, StreamExt};
use http::Uri;
use tokio_tungstenite::tungstenite::Message;
use trolly::providers::{BinanceUsdM, Endpoints};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = BinanceUsdM;
    let url: Uri = provider.websocket_url().parse()?;

    let (mut stream, _) =
        tokio_tungstenite::connect_async_tls_with_config(url, None, false, None).await?;

    let symbols = ["btcusdt"];
    let sub = provider.ws_subscriptions(symbols.iter());
    stream.send(Message::Text(sub.into())).await?;

    let mut n = 0;
    while let Some(Ok(msg)) = stream.next().await {
        if let Message::Text(text) = msg {
            println!("{text}");
            n += 1;
            if n >= 5 {
                break;
            }
        }
    }

    stream.close(None).await.ok();
    Ok(())
}
