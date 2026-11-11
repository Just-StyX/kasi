use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use sha1::{Digest, Sha1};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

use crate::{BoxBody, BoxFuture, Error, extract::FromRequest};

pub type WsMessage = Message;

/// Extractor for upgrading an HTTP GET request into a bidirectional WebSocket connection.
pub struct WebSocketUpgrade {
    sec_key: String,
    on_upgrade: hyper::upgrade::OnUpgrade,
}

impl FromRequest for WebSocketUpgrade {
    fn extract(
        mut req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let is_upgrade = req
                .headers()
                .get(header::UPGRADE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.eq_ignore_ascii_case("websocket"))
                .unwrap_or(false);

            if !is_upgrade {
                return Err(Error::BadRequest(
                    "Expected 'Upgrade: websocket' header".into(),
                ));
            }

            let sec_key = req
                .headers()
                .get("sec-websocket-key")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::BadRequest("Missing Sec-WebSocket-Key header".into()))?
                .to_string();

            // Pull the OnUpgrade handle captured while `req` was still `Request<Incoming>`
            let on_upgrade = req
                .extensions_mut()
                .remove::<hyper::upgrade::OnUpgrade>()
                .ok_or_else(|| {
                    Error::Internal("Hyper OnUpgrade extension missing from request".into())
                })?;

            Ok((
                Self {
                    sec_key,
                    on_upgrade,
                },
                req,
            ))
        })
    }
}

impl WebSocketUpgrade {
    /// Completes the HTTP 101 Switching Protocols handshake and runs `callback` on the `WebSocket`.
    pub fn on_upgrade<F, Fut>(self, callback: F) -> Response<BoxBody>
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let accept_key = compute_accept_key(&self.sec_key);
        let on_upgrade = self.on_upgrade;

        tokio::task::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    let io = TokioIo::new(upgraded);
                    let ws_stream = WebSocketStream::from_raw_socket(io, Role::Server, None).await;
                    callback(WebSocket { inner: ws_stream }).await;
                }
                Err(err) => {
                    eprintln!("[WS] WebSocket upgrade failed: {}", err);
                }
            }
        });

        let empty_body = Full::new(Bytes::new()).map_err(|e| match e {}).boxed();

        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header("Sec-WebSocket-Accept", accept_key)
            .body(empty_body)
            .unwrap()
    }
}

/// Ergonomic wrapper around an active server WebSocket stream.
pub struct WebSocket {
    inner: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
}

impl WebSocket {
    /// Receives the next WebSocket frame (`Text`, `Binary`, `Ping`, `Pong`, `Close`).
    pub async fn recv(
        &mut self,
    ) -> Option<Result<WsMessage, tokio_tungstenite::tungstenite::Error>> {
        self.inner.next().await
    }

    /// Sends a UTF-8 text message to the connected client.
    pub async fn send_text(
        &mut self,
        text: impl Into<String>,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.inner.send(Message::Text(text.into().into())).await
    }

    /// Sends a binary message frame to the connected client.
    pub async fn send_binary(
        &mut self,
        data: impl Into<Bytes>,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.inner.send(Message::Binary(data.into())).await
    }

    /// Gracefully closes the WebSocket connection.
    pub async fn close(mut self) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.inner.close(None).await
    }
}

fn compute_accept_key(sec_key: &str) -> String {
    const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(sec_key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}
