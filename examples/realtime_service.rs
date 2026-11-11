use futures_util::stream;
use kasi_jb::prelude::*;
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize, Validate, Schema)]
struct BroadcastInput {
    #[validate(min_len = 1)]
    message: String,
}

#[derive(Debug, Serialize, IntoResponse, Schema)]
struct BroadcastReceipt {
    delivered: bool,
    echo: String,
}

#[endpoint(POST, "/api/broadcast")]
#[response(status = 200, description = "Broadcasts a message", json = BroadcastReceipt)]
async fn broadcast_message(
    ValidatedJson(input): ValidatedJson<BroadcastInput>,
) -> BroadcastReceipt {
    BroadcastReceipt {
        delivered: true,
        echo: input.message,
    }
}

#[endpoint(GET, "/events/ticks")]
async fn live_ticks() -> Sse {
    let tick_stream = stream::unfold(1u64, |count| async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let event = SseEvent::data(format!("tick #{}", count))
            .event("heartbeat")
            .id(count.to_string());
        Some((Ok(event), count + 1))
    });

    Sse::new(tick_stream)
}

#[endpoint(GET, "/ws/chat")]
async fn chat_ws(ws: WebSocketUpgrade) -> Response<BoxBody> {
    ws.on_upgrade(|mut socket| async move {
        let _ = socket.send_text("Connected to Kasi realtime chat!").await;
        while let Some(Ok(WsMessage::Text(msg))) = socket.recv().await {
            let _ = socket.send_text(format!("[server]: {}", msg)).await;
        }
        let _ = socket.close().await;
    })
}

#[main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 8080,
    })
    .with_observability() // Mounts GET /healthz and GET /metrics
    .middleware(TracingMiddleware::new())
    .middleware(CompressionMiddleware::new())
    .middleware(RateLimitMiddleware::new(120, Duration::from_secs(60)))
    .schedule_task("periodic-cleanup", Duration::from_secs(30), || async {
        println!("[SCHEDULER] Running background cleanup task...");
    })
    .register_schema::<BroadcastInput>()
    .register_schema::<BroadcastReceipt>()
    .route(broadcast_message)
    .route(live_ticks)
    .route(chat_ws)
    .build()
    .await?;

    println!("Try running: cargo run --example realtime_service");
    app.serve().await?;
    Ok(())
}
