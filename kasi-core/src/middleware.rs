use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;

use flate2::{Compression, write::GzEncoder};
use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crate::{
    BoxBody, BoxFuture, EndpointRegistry, Error, HandlerFn, RegisterEndpoint, Router,
    error::RequestContext, openapi::EndpointMetadata,
};

pub type MiddleChain = Arc<
    dyn Fn(Request<BoxBody>) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send>>
        + Send
        + Sync,
>;

pub trait Middleware: Send + Sync + 'static {
    fn handle(
        &self,
        req: http::Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn std::future::Future<Output = http::Response<BoxBody>> + Send + 'static>>;
}

pub trait RegisterRoute {
    fn register_route(self, router: &mut Router);
}

pub struct Next {
    pub(crate) chain:
        Box<dyn FnOnce(Request<Incoming>) -> BoxFuture<'static, Response<BoxBody>> + Send>,
}

impl Next {
    pub async fn run(self, req: Request<Incoming>) -> Response<BoxBody> {
        (self.chain)(req).await
    }
}

#[derive(Clone)]
pub struct CorsMiddleware {
    allowed_origins: Vec<String>,
    allowed_methods: Vec<Method>,
    allowed_headers: Vec<String>,
    allow_all_origins: bool,
    allow_credentials: bool,
    max_age: Option<u64>,
}

impl CorsMiddleware {
    /// Creates an empty CORS configuration that blocks everything by default until explicitly permitted.
    pub fn new() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: Vec::new(),
            allowed_headers: Vec::new(),
            allow_all_origins: false,
            allow_credentials: false,
            max_age: None,
        }
    }

    /// Creates a default permissive CORS profile for rapid development environments.
    pub fn permissive() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: vec![
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
                Method::PATCH,
            ],
            allowed_headers: vec![
                "Content-Type".into(),
                "Authorization".into(),
                "Accept".into(),
            ],
            allow_all_origins: true,
            allow_credentials: false,
            max_age: None,
        }
    }

    /// Permits requests from any origin (*).
    pub fn allow_any_origin(mut self) -> Self {
        self.allow_all_origins = true;
        self.allowed_origins.clear();
        self
    }

    /// Restricts CORS access to a single designated origin.
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.allow_all_origins = false;
        self.allowed_origins.push(origin.into());
        self
    }

    /// appends an authorized HTTP routing verb to the verification profile.
    pub fn allow_method(mut self, method: Method) -> Self {
        self.allowed_methods.push(method);
        self
    }

    /// appends an authorized HTTP header name to the validation profile.
    pub fn allow_header(mut self, header: impl Into<String>) -> Self {
        self.allowed_headers.push(header.into());
        self
    }

    /// Explicitly allows cross-origin requests to include user cookies or Authorization contexts.
    pub fn allow_credentials(mut self) -> Self {
        self.allow_credentials = true;
        self
    }

    /// Configures the browser pre-flight cache age (Access-Control-Max-Age) in seconds.
    pub fn max_age(mut self, seconds: u64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    // Shared evaluation helper to inject origin checks on options and standard responses
    fn evaluate_origin(&self, request_origin: Option<&HeaderValue>, headers: &mut http::HeaderMap) {
        if self.allow_all_origins {
            headers.insert(
                http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        } else if let Some(ref req_org) = request_origin {
            if let Ok(org_str) = req_org.to_str() {
                if self.allowed_origins.iter().any(|o| o == org_str) {
                    headers.insert(
                        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                        (*req_org).clone(),
                    );
                }
            }
        }

        if self.allow_credentials {
            headers.insert(
                http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
    }
}

impl Middleware for CorsMiddleware {
    fn handle(
        &self,
        req: Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'static>> {
        let this = self.clone();

        Box::pin(async move {
            let origin = req.headers().get(http::header::ORIGIN).cloned();

            if req.method() == Method::OPTIONS {
                let empty_body = Full::new(Bytes::new()).map_err(|e| match e {}).boxed();

                let mut res = Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(empty_body)
                    .unwrap();

                let headers = res.headers_mut();

                this.evaluate_origin(origin.as_ref(), headers);

                let methods_str = this
                    .allowed_methods
                    .iter()
                    .map(|m| m.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Ok(val) = HeaderValue::from_str(&methods_str) {
                    headers.insert(http::header::ACCESS_CONTROL_ALLOW_METHODS, val);
                }

                let headers_str = this.allowed_headers.join(", ");
                if let Ok(val) = HeaderValue::from_str(&headers_str) {
                    headers.insert(http::header::ACCESS_CONTROL_ALLOW_HEADERS, val);
                }

                if let Some(age) = this.max_age {
                    headers.insert(http::header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from(age));
                }

                return res;
            }

            let mut response = next(req).await;
            this.evaluate_origin(origin.as_ref(), response.headers_mut());

            response
        })
    }
}

#[allow(clippy::type_complexity)]
#[derive(Clone)]
pub struct InternalMiddlewareLayer {
    pub(crate) run:
        Arc<dyn Fn(Request<Incoming>, Next) -> BoxFuture<'static, Response<BoxBody>> + Send + Sync>,
}

pub fn from_fn<F, Fut>(f: F) -> InternalMiddlewareLayer
where
    F: Fn(Request<Incoming>, Next) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response<BoxBody>, Error>> + Send + 'static,
{
    InternalMiddlewareLayer {
        run: Arc::new(move |req, next| {
            let f = f.clone();
            let context_frame = RequestContext::from_request(&req);
            Box::pin(async move {
                match f(req, next).await {
                    Ok(res) => res,
                    Err(err) => err.into_response_with_context(&context_frame),
                }
            })
        }),
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Module {
    name: String,
    prefix: String,
    pub(crate) endpoints: Vec<(http::Method, String, crate::HandlerFn)>,
    pub(crate) metadata: std::collections::HashMap<(http::Method, String), EndpointMetadata>, // New field!
    pub(crate) middleware: Vec<InternalMiddlewareLayer>,
}

impl Module {
    /// Initialized a fresh routing scope group bound to a specific URL base token prefix.
    pub fn new(prefix: &str) -> Self {
        // Sanitize prefix to guarantee clean leading slashes and strip trailing configurations
        let sanitized_prefix = format!("/{}", prefix.trim_matches('/'));
        Self {
            name: prefix.to_string(),
            prefix: if sanitized_prefix == "/" {
                "".into()
            } else {
                sanitized_prefix
            },
            endpoints: Vec::new(),
            metadata: HashMap::new(),
            middleware: Vec::new(),
        }
    }

    /// Mounts an endpoint directly onto the current module group level scope along with OpenAPI documentation metadata.
    pub fn add_endpoint_with_meta(
        mut self,
        method: http::Method,
        path: &str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) -> Self {
        let localized_path = format!("{}/{}", self.prefix, path.trim_start_matches('/'));
        self.endpoints
            .push((method.clone(), localized_path.clone(), handler));
        self.metadata.insert((method, localized_path), meta);
        self
    }

    /// Mounts an endpoint directly onto the current module group level scope.
    pub fn add_endpoint(mut self, method: http::Method, path: &str, handler: HandlerFn) -> Self {
        let localized_path = format!("{}/{}", self.prefix, path.trim_start_matches('/'));
        self.endpoints.push((method, localized_path, handler));
        self
    }

    pub fn wrap(mut self, layer: InternalMiddlewareLayer) -> Self {
        self.middleware.push(layer);
        self
    }

    /// Registers an `#[endpoint]` directly inside this module scope (e.g., `module.route(restock)`).
    pub fn route<E: RegisterEndpoint>(self, endpoint: E) -> Self {
        endpoint.register(self)
    }

    /// Attaches a child module under this routing scope, combining paths, parent middleware, and OpenAPI metadata.
    pub fn nest_with_docs(mut self, child_module: Module) -> Self {
        // 1. Merge and prefix OpenAPI metadata
        for ((method, child_path), meta) in &child_module.metadata {
            let combined_path = format!("{}/{}", self.prefix, child_path.trim_start_matches('/'));
            self.metadata
                .insert((method.clone(), combined_path), meta.clone());
        }

        // 2. Delegate endpoint & middleware wrapping to `nest`
        self.nest(child_module)
    }

    /// Attaches a child module sub-tree under this routing scope group,
    /// prefixing paths and wrapping child handlers with both child and parent middleware.
    pub fn nest(mut self, child_module: Module) -> Self {
        for (method, child_path, handler) in child_module.endpoints {
            let combined_path = format!("{}/{}", self.prefix, child_path.trim_start_matches('/'));

            // First apply the child module's own middleware
            let mut layered_handler = handler;
            for child_mw in child_module.middleware.iter().rev() {
                let current_layer = child_mw.run.clone();
                let next_inner = layered_handler.clone();
                layered_handler = Arc::new(move |req| {
                    let inner = next_inner.clone();
                    let next_node = Next {
                        chain: Box::new(move |r| inner(r)),
                    };
                    current_layer(req, next_node)
                });
            }

            // Next wrap with this parent module's middleware
            for parent_mw in self.middleware.iter().rev() {
                let current_layer = parent_mw.run.clone();
                let next_inner = layered_handler.clone();
                layered_handler = Arc::new(move |req| {
                    let inner = next_inner.clone();
                    let next_node = Next {
                        chain: Box::new(move |r| inner(r)),
                    };
                    current_layer(req, next_node)
                });
            }

            self.endpoints
                .push((method, combined_path, layered_handler));
        }
        self
    }
}

impl EndpointRegistry for Module {
    fn mount_endpoint(
        self,
        method: http::Method,
        path: &'static str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) -> Self {
        self.add_endpoint_with_meta(method, path, handler, meta)
    }
}

// Add to kasi-core/src/middleware.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Clone)]
pub struct TracingMiddleware {
    format: LogFormat,
}

impl TracingMiddleware {
    /// Creates a new tracing middleware outputting human-readable structured lines.
    pub fn new() -> Self {
        Self {
            format: LogFormat::Pretty,
        }
    }

    /// Configures the middleware to emit newline-delimited JSON objects.
    pub fn json() -> Self {
        Self {
            format: LogFormat::Json,
        }
    }
}

impl Default for TracingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for TracingMiddleware {
    fn handle(
        &self,
        mut req: Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'static>> {
        let format = self.format;

        Box::pin(async move {
            let start = Instant::now();

            // 1. Reuse existing RequestContext if injected upstream, otherwise build one
            let ctx = req
                .extensions()
                .get::<RequestContext>()
                .cloned()
                .unwrap_or_else(|| RequestContext::from_request(&req));

            let trace_id_str = ctx.trace_id().to_string();
            let method = ctx.method().to_string();
            let path = ctx.uri_path().to_string();

            // 2. Store RequestContext in extensions so handlers and error formatters share the same trace_id
            req.extensions_mut().insert(ctx);

            // 3. Execute downstream middleware and route handler
            let mut response = next(req).await;

            // 4. Attach X-Request-Id to the outgoing response headers
            if let Ok(header_val) = HeaderValue::from_str(&trace_id_str) {
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("x-request-id"),
                    header_val,
                );
            }

            // 5. Compute duration and emit structured log entry
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
            let status = response.status().as_u16();

            match format {
                LogFormat::Pretty => {
                    println!(
                        "[TRACE] trace_id={} method={} path={} status={} latency_ms={:.2}",
                        trace_id_str, method, path, status, duration_ms
                    );
                }
                LogFormat::Json => {
                    let log_entry = serde_json::json!({
                        "level": if status >= 500 { "ERROR" } else if status >= 400 { "WARN" } else { "INFO" },
                        "trace_id": trace_id_str,
                        "method": method,
                        "path": path,
                        "status": status,
                        "latency_ms": (duration_ms * 100.0).round() / 100.0,
                    });
                    println!("{}", log_entry);
                }
            }

            response
        })
    }
}

#[derive(Clone)]
struct BucketState {
    tokens: u32,
    window_start: Instant,
}

/// In-memory fixed-window rate limiter implementing `Middleware`.
#[derive(Clone)]
pub struct RateLimitMiddleware {
    max_requests: u32,
    window: Duration,
    buckets: Arc<Mutex<HashMap<String, BucketState>>>,
}

impl RateLimitMiddleware {
    /// Allows `max_requests` per `window` for each client key.
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn extract_client_key(req: &Request<BoxBody>) -> String {
        req.headers()
            .get("x-forwarded-for")
            .or_else(|| req.headers().get("x-real-ip"))
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "global-client".to_string())
    }
}

impl Middleware for RateLimitMiddleware {
    fn handle(
        &self,
        req: Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'static>> {
        let max_requests = self.max_requests;
        let window = self.window;
        let buckets = self.buckets.clone();

        Box::pin(async move {
            let client_key = Self::extract_client_key(&req);
            let now = Instant::now();

            let (allowed, remaining, reset_secs) = {
                let mut map = buckets.lock().unwrap();
                let entry = map.entry(client_key).or_insert(BucketState {
                    tokens: max_requests,
                    window_start: now,
                });

                if now.duration_since(entry.window_start) >= window {
                    entry.tokens = max_requests;
                    entry.window_start = now;
                }

                let elapsed = now.duration_since(entry.window_start);
                let remaining_window = window.saturating_sub(elapsed).as_secs().max(1);

                if entry.tokens > 0 {
                    entry.tokens -= 1;
                    (true, entry.tokens, remaining_window)
                } else {
                    (false, 0, remaining_window)
                }
            };

            if !allowed {
                let json_body = serde_json::json!({
                    "type": "https://kasi.local",
                    "title": "Too Many Requests",
                    "status": 429,
                    "detail": format!("Rate limit exceeded. Try again in {} seconds.", reset_secs),
                });

                let body = Full::new(Bytes::from(json_body.to_string()))
                    .map_err(|e| match e {})
                    .boxed();

                return Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .header("Content-Type", "application/problem+json")
                    .header("X-RateLimit-Limit", max_requests.to_string())
                    .header("X-RateLimit-Remaining", "0")
                    .header("Retry-After", reset_secs.to_string())
                    .body(body)
                    .unwrap();
            }

            let mut response = next(req).await;
            let headers = response.headers_mut();
            if let Ok(limit_val) = HeaderValue::from_str(&max_requests.to_string()) {
                headers.insert("x-ratelimit-limit", limit_val);
            }
            if let Ok(rem_val) = HeaderValue::from_str(&remaining.to_string()) {
                headers.insert("x-ratelimit-remaining", rem_val);
            }

            response
        })
    }
}

/// Compresses HTTP response bodies using Gzip when `Accept-Encoding: gzip` is present.
#[derive(Clone)]
pub struct CompressionMiddleware {
    min_size_bytes: usize,
}

impl CompressionMiddleware {
    /// Compresses responses larger than `min_size_bytes` (defaults to 256 bytes).
    pub fn new() -> Self {
        Self {
            min_size_bytes: 256,
        }
    }

    pub fn min_size(mut self, bytes: usize) -> Self {
        self.min_size_bytes = bytes;
        self
    }
}

impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for CompressionMiddleware {
    fn handle(
        &self,
        req: Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'static>> {
        let min_size = self.min_size_bytes;
        let accepts_gzip = req
            .headers()
            .get(http::header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("gzip"))
            .unwrap_or(false);

        Box::pin(async move {
            let response = next(req).await;

            // Skip compression if client doesn't support gzip, response is already encoded, or it's SSE/WebSocket
            let is_sse = response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.starts_with("text/event-stream"))
                .unwrap_or(false);

            if !accepts_gzip
                || is_sse
                || response.status() == StatusCode::SWITCHING_PROTOCOLS
                || response
                    .headers()
                    .contains_key(http::header::CONTENT_ENCODING)
            {
                return response;
            }

            let (mut parts, body) = response.into_parts();
            let collected = match body.collect().await {
                Ok(c) => c.to_bytes(),
                Err(_) => {
                    return Response::from_parts(
                        parts,
                        Full::new(Bytes::new()).map_err(|e| match e {}).boxed(),
                    );
                }
            };

            if collected.len() < min_size {
                return Response::from_parts(
                    parts,
                    Full::new(collected).map_err(|e| match e {}).boxed(),
                );
            }

            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            if encoder.write_all(&collected).is_ok()
                && let Ok(compressed) = encoder.finish()
            {
                parts.headers.remove(http::header::CONTENT_LENGTH);
                parts.headers.insert(
                    http::header::CONTENT_ENCODING,
                    HeaderValue::from_static("gzip"),
                );
                return Response::from_parts(
                    parts,
                    Full::new(Bytes::from(compressed))
                        .map_err(|e| match e {})
                        .boxed(),
                );
            }

            Response::from_parts(parts, Full::new(collected).map_err(|e| match e {}).boxed())
        })
    }
}

/// Aborts requests that exceed `duration` and returns an RFC 7807 `408 Request Timeout` response.
#[derive(Clone)]
pub struct TimeoutMiddleware {
    duration: Duration,
}

impl TimeoutMiddleware {
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl Middleware for TimeoutMiddleware {
    fn handle(
        &self,
        req: Request<BoxBody>,
        next: MiddleChain,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'static>> {
        let timeout_dur = self.duration;
        let uri_path = req.uri().path().to_string();

        Box::pin(async move {
            match tokio::time::timeout(timeout_dur, next(req)).await {
                Ok(response) => response,
                Err(_) => {
                    let json_body = serde_json::json!({
                        "type": "https://kasi.local",
                        "title": "Request Timeout",
                        "status": 408,
                        "detail": format!("Request exceeded maximum execution duration of {:?}", timeout_dur),
                        "instance": uri_path,
                    });

                    let body = Full::new(Bytes::from(json_body.to_string()))
                        .map_err(|e| match e {})
                        .boxed();

                    Response::builder()
                        .status(StatusCode::REQUEST_TIMEOUT)
                        .header("Content-Type", "application/problem+json")
                        .body(body)
                        .unwrap()
                }
            }
        })
    }
}
