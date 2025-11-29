use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use url::Url;
use serde_json::json;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Centrifugo WebSocket URL
    #[arg(long, default_value = "ws://localhost:8000/connection/websocket")]
    url: String,

    /// Channel to publish to
    #[arg(long, default_value = "news")]
    channel: String,

    /// Message to send
    #[arg(long, default_value = "Hello from Rust")]
    message: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let url = Url::parse(&args.url)?;
    println!("Connecting to {}", url);

    let (ws_stream, _) = connect_async(url).await.expect("Failed to connect");
    println!("Connected to Centrifugo");

    let (mut write, mut read) = ws_stream.split();

    // 1. Send Connect command (minimal)
    // Note: In a real app, you might need a token if the server is protected.
    let connect_msg = json!({
        "id": 1,
        "connect": {}
    });
    write.send(Message::Text(connect_msg.to_string())).await?;
    println!("Sent connect message");

    // Wait for connect reply
    if let Some(msg) = read.next().await {
        let msg = msg?;
        if msg.is_text() || msg.is_binary() {
             println!("Received connect reply: {}", msg);
        }
    }

    // 2. Send Publish command
    let publish_msg = json!({
        "id": 2,
        "publish": {
            "channel": args.channel,
            "data": {
                "content": args.message
            }
        }
    });
    write.send(Message::Text(publish_msg.to_string())).await?;
    println!("Sent publish message to channel '{}'", args.channel);

    // Wait for publish reply
    if let Some(msg) = read.next().await {
        let msg = msg?;
         if msg.is_text() || msg.is_binary() {
            println!("Received publish reply: {}", msg);
        }
    }

    Ok(())
}
