use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use crate::{
    ApplicationBuilder, BoxBody,
    middleware::{MiddleChain, Middleware},
};

/// Thread-safe Prometheus metrics collector for Kasi services.
#[derive(Clone, Default)]
pub struct MetricsRegistry {
    inner: Arc<MetricsInner>,
}

#[derive(Default)]
struct MetricsInner {
    requests_total: AtomicU64,
    requests_2xx: AtomicU64,
    requests_4xx: AtomicU64,
    requests_5xx: AtomicU64,
    in_flight: AtomicU64,
    latency_micros_sum: AtomicU64,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders metrics in standard Prometheus text exposition format (`0.0.4`).
    pub fn render_prometheus(&self) -> String {
        let total = self.inner.requests_total.load(Ordering::Relaxed);
        let r2xx = self.inner.requests_2xx.load(Ordering::Relaxed);
        let r4xx = self.inner.requests_4xx.load(Ordering::Relaxed);
        let r5xx = self.inner.requests_5xx.load(Ordering::Relaxed);
        let in_flight = self.inner.in_flight.load(Ordering::Relaxed);
        let latency_secs =
            self.inner.latency_micros_sum.load(Ordering::Relaxed) as f64 / 1_000_000.0;

        format!(
            "# HELP kasi_http_requests_total Total number of HTTP requests processed.\n\
             # TYPE kasi_http_requests_total counter\n\
             kasi_http_requests_total {}\n\
             # HELP kasi_http_responses_by_class_total HTTP responses grouped by status class.\n\
             # TYPE kasi_http_responses_by_class_total counter\n\
             kasi_http_responses_by_class_total{{class=\"2xx\"}} {}\n\
             kasi_http_responses_by_class_total{{class=\"4xx\"}} {}\n\
             kasi_http_responses_by_class_total{{class=\"5xx\"}} {}\n\
             # HELP kasi_http_in_flight_requests Current number of active HTTP requests.\n\
             # TYPE kasi_http_in_flight_requests gauge\n\
             kasi_http_in_flight_requests {}\n\
             # HELP kasi_http_request_duration_seconds_sum Total time spent handling HTTP requests.\n\
             # TYPE kasi_http_request_duration_seconds_sum counter\n\
             kasi_http_request_duration_seconds_sum {:.6}\n",
            total, r2xx, r4xx, r5xx, in_flight, latency_secs
        )
    }
}

/// Middleware that records request counts, status classes, and latency into `MetricsRegistry`.
#[derive(Clone)]
pub struct MetricsMiddleware {
    registry: MetricsRegistry,
}

impl MetricsMiddleware {
    pub fn new(registry: MetricsRegistry) -> Self {
        Self { registry }
    }
}

impl Middleware for MetricsMiddleware {
    fn handle(
        &self,
        req: Request<BoxBody>,
        next: MiddleChain,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response<BoxBody>> + Send + 'static>>
    {
        let reg = self.registry.clone();
        Box::pin(async move {
            let start = Instant::now();
            reg.inner.requests_total.fetch_add(1, Ordering::Relaxed);
            reg.inner.in_flight.fetch_add(1, Ordering::Relaxed);

            let response = next(req).await;

            reg.inner.in_flight.fetch_sub(1, Ordering::Relaxed);
            let elapsed_micros = start.elapsed().as_micros() as u64;
            reg.inner
                .latency_micros_sum
                .fetch_add(elapsed_micros, Ordering::Relaxed);

            match response.status().as_u16() {
                200..=299 => {
                    reg.inner.requests_2xx.fetch_add(1, Ordering::Relaxed);
                }
                400..=499 => {
                    reg.inner.requests_4xx.fetch_add(1, Ordering::Relaxed);
                }
                500..=599 => {
                    reg.inner.requests_5xx.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }

            response
        })
    }
}

impl ApplicationBuilder {
    /// Automatically mounts `GET /healthz` (JSON liveness check) and `GET /metrics` (Prometheus metrics),
    /// and attaches `MetricsMiddleware` to track all requests.
    pub fn with_observability(self) -> Self {
        let registry = MetricsRegistry::new();
        let metrics_reg_clone = registry.clone();

        self.extension(registry.clone())
            .middleware(MetricsMiddleware::new(registry))
            .add_raw_route(Method::GET, "/healthz", |_req| async move {
                let payload = serde_json::json!({ "status": "healthy" }).to_string();
                let body = Full::new(Bytes::from(payload))
                    .map_err(|e| match e {})
                    .boxed();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap()
            })
            .add_raw_route(Method::GET, "/metrics", move |_req| {
                let reg = metrics_reg_clone.clone();
                async move {
                    let output = reg.render_prometheus();
                    let body = Full::new(Bytes::from(output))
                        .map_err(|e| match e {})
                        .boxed();
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(
                            header::CONTENT_TYPE,
                            "text/plain; version=0.0.4; charset=utf-8",
                        )
                        .body(body)
                        .unwrap()
                }
            })
    }
}
