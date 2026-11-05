use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request, Response};
use tokio::io::duplex;
use hyper_util::rt::TokioIo;
use hyper::{body::Incoming, server::conn::http1};
use hyper::service::service_fn;
use http_body_util::{Empty, BodyExt};

use crate::{Application, BoxBody, Router};

pub struct TestServer {
    router: Arc<Router>,
}

pub struct TestRequestBuilder;
pub struct TestResponse;

impl TestServer {
    pub fn new(app: Application) -> Self {
        Self {
            router: Arc::new(app.into_router())
        }
    }

    pub async fn simulate_request(&self, method: Method, path: &str) -> Response<BoxBody> {
        let empty_body = Empty::<Bytes>::new()
            .map_err(|e| match e {})
            .boxed();
    
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", "localhost")
            .body(empty_body)
            .unwrap();
    
        let (client_stream, server_stream) = duplex(1024);
    
        let (tx, rx) = tokio::sync::oneshot::channel::<Request<Incoming>>();
        let tx_mutex = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    
        tokio::spawn(async move {
            let io = TokioIo::new(server_stream);
            let service = service_fn(move |req: Request<Incoming>| {
                let tx_mutex = std::sync::Arc::clone(&tx_mutex);
                async move {
                    // Safely lock the mutex and take ownership of the sender if it exists
                    if let Ok(mut guard) = tx_mutex.lock() {
                        if let Some(sender) = guard.take() {
                            let _ = sender.send(req);
                        }
                    }
                    Ok::<_, hyper::Error>(Response::new(Empty::<Bytes>::new().boxed()))
                }
            });
    
            let _ = http1::Builder::new().serve_connection(io, service).await;
        });

        let client_io = TokioIo::new(client_stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(client_io).await.unwrap();
    
        tokio::spawn(async move {
            let _ = conn.await;
        });
    
        let _ = sender.send_request(request).await;
    
        let genuine_incoming_request = rx.await.unwrap();
    
        self.router.dispatch(genuine_incoming_request).await
    }

}