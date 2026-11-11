use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use bytes::Bytes;
pub use http::{Extensions, Method, Request, Response, StatusCode};
pub use http_body_util::combinators;
pub use http_body_util::{BodyExt, Full};
pub use hyper::body::Incoming;
pub use hyper::service::service_fn;
pub use hyper_util::rt::TokioIo;
pub use hyper_util::{rt::TokioExecutor, server::conn::auto};
pub use tokio::net::TcpListener;

use crate::middleware::{MiddleChain, Middleware};
use crate::openapi::{
    ComponentsSpec, EndpointMetadata, InfoSpec, OpenApiSpec, Schema, SchemaObject,
};
use crate::tasks::TaskScheduler;
use crate::{
    error::Error,
    middleware::{Module, Next},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Clone, Debug)]
pub struct PathParams(pub std::collections::HashMap<String, String>);
pub type BoxBody = combinators::BoxBody<Bytes, hyper::Error>;
pub type HandlerFn =
    Arc<dyn Fn(Request<Incoming>) -> BoxFuture<'static, Response<BoxBody>> + Send + Sync>;

pub mod cookies;
pub mod error;
pub mod extract;
#[cfg(feature = "graphql")]
pub mod graphql;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod response;
pub mod sse;
pub mod static_files;
pub mod tasks;
#[cfg(feature = "templates")]
pub mod templates;
pub mod ws;

#[derive(Clone)]
struct OpenApiJsonBytes(pub bytes::Bytes);

#[derive(Clone)]
struct SwaggerHtmlBytes(pub bytes::Bytes);

pub struct Router {
    pub(crate) routes: HashMap<(Method, String), HandlerFn>,
    pub metadata: HashMap<(Method, String), EndpointMetadata>,
    pub schemas: HashMap<String, SchemaObject>,
    pub(crate) regex_routes: Vec<RegexRoute>,
    pub(crate) global_middleware: Vec<std::sync::Arc<dyn Middleware>>,
}

pub struct RegexRoute {
    pub method: Method,
    pub original_template: String,
    pub compiled_regex: regex::Regex,
    pub param_names: Vec<String>,
    pub handler: HandlerFn,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            metadata: HashMap::new(),
            schemas: HashMap::new(),
            regex_routes: Vec::new(),
            global_middleware: Vec::new(),
        }
    }

    pub(crate) fn add_raw_handler(&mut self, method: Method, path: &str, handler: HandlerFn) {
        // Forward the registration down into our uniform compiling route engine!
        self.compile_and_add_route(method, path, handler);
    }

    /// Compiles a route string containing optional inline variable rules
    /// into an ordered regex routing matrix evaluation slot.
    pub fn compile_and_add_route(&mut self, method: Method, path: &str, handler: HandlerFn) {
        // Register flat key fallback state
        self.routes
            .insert((method.clone(), path.to_string()), handler.clone());

        let mut regex_pattern = String::new();
        let mut param_names = Vec::new();

        // Match expressions like: {id:\d+}
        // Segment splitting path blocks
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            regex_pattern.push('/');

            if segment.starts_with('{') && segment.ends_with('}') {
                let inner = &segment[1..segment.len() - 1];
                if let Some((param_name, rule)) = inner.split_once(':') {
                    // Turn '{id:\d+}' -> '(?P<id>\d+)'
                    regex_pattern.push_str(&format!("(?P<{}>{})", param_name, rule));
                    param_names.push(param_name.to_string());
                } else {
                    // Fallback to standard wildcard path variable matching: '{id}' -> '(?P<id>[^/]+)'
                    regex_pattern.push_str(&format!("(?P<{}>[^/]+)", inner));
                    param_names.push(inner.to_string());
                }
            } else if segment.starts_with(':') {
                // Backward-compatible colon pattern matching: ':id' -> '(?P<id>[^/]+)'
                let param_name = &segment[1..];
                regex_pattern.push_str(&format!("(?P<{}>[^/]+)", param_name));
                param_names.push(param_name.to_string());
            } else {
                // Standard literal character escape sequence wrapper matching
                regex_pattern.push_str(&regex::escape(segment));
            }
        }

        if regex_pattern.is_empty() {
            regex_pattern.push('/');
        }

        // Secure head-and-tail matches to avoid global partial URL hijacking
        let final_regex_string = format!("^{}$", regex_pattern);
        let compiled_regex = regex::Regex::new(&final_regex_string).unwrap_or_else(|_| {
            panic!(
                "Malformed inline route macro regex spec definition: {}",
                path
            )
        });

        self.regex_routes.push(RegexRoute {
            method,
            original_template: path.to_string(),
            compiled_regex,
            param_names,
            handler,
        });
    }

    pub fn wrap_handles_with_middleware(
        &mut self,
        middleware_chain: Vec<std::sync::Arc<dyn Middleware>>,
    ) {
        if middleware_chain.is_empty() {
            return;
        }

        for route in &mut self.regex_routes {
            let base_handler = route.handler.clone();
            let incoming_request_cell = std::sync::Arc::new(tokio::sync::Mutex::new(
                None::<http::Request<hyper::body::Incoming>>,
            ));

            let cell_for_chain = incoming_request_cell.clone();
            let handler_clone = base_handler.clone();

            let mut current_chain: MiddleChain = std::sync::Arc::new(move |req_box| {
                let cell = cell_for_chain.clone();
                let handler = handler_clone.clone();

                Box::pin(async move {
                    let mut guard = cell.lock().await;
                    if let Some(mut native_req) = guard.take() {
                        // Propagate extensions added by middleware (e.g., RequestContext) into the handler request
                        native_req
                            .extensions_mut()
                            .extend(req_box.extensions().clone());
                        handler(native_req).await
                    } else {
                        let error_bytes =
                            bytes::Bytes::from("Internal Engine Deserialization Error");
                        let body = http_body_util::Full::new(error_bytes)
                            .map_err(|e| match e {})
                            .boxed();

                        http::Response::builder().status(500).body(body).unwrap()
                    }
                })
            });

            for middleware_node in middleware_chain.iter().rev() {
                let active_mw = middleware_node.clone();
                let next_target = current_chain.clone();

                current_chain =
                    std::sync::Arc::new(move |req| active_mw.handle(req, next_target.clone()));
            }

            let cell_for_handler = incoming_request_cell.clone();
            let final_middleware_chain = current_chain;

            route.handler = std::sync::Arc::new(move |req_incoming| {
                let cell = cell_for_handler.clone();
                let chain = final_middleware_chain.clone();

                Box::pin(async move {
                    let (parts, body) = req_incoming.into_parts();

                    let original_incoming_req = http::Request::from_parts(parts.clone(), body);
                    {
                        let mut guard = cell.lock().await;
                        *guard = Some(original_incoming_req);
                    }

                    let blank_body = http_body_util::Empty::<bytes::Bytes>::new()
                        .map_err(|e| match e {})
                        .boxed();
                    let middleware_req = http::Request::from_parts(parts, blank_body);

                    chain(middleware_req).await
                })
            });
        }
    }

    #[allow(clippy::redundant_closure, clippy::useless_conversion)]
    pub fn add_route<F, Fut>(&mut self, method: Method, path: &str, handler: F)
    where
        F: Fn(Request<BoxBody>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response<BoxBody>> + Send + 'static,
    {
        let wrapped_handler: HandlerFn = Arc::new(move |req: Request<Incoming>| {
            let (parts, body) = req.into_parts();
            let boxed_body = body.map_err(|e| hyper::Error::from(e)).boxed();

            let transformed_req = Request::from_parts(parts, boxed_body);
            let fut = handler(transformed_req);
            Box::pin(fut) as BoxFuture<'static, Response<BoxBody>>
        });

        // Forward to `compile_and_add_route` so `self.routes` AND `self.regex_routes` are both populated
        self.compile_and_add_route(method, path, wrapped_handler);
    }

    pub fn add_endpoint_with_meta(
        &mut self,
        method: Method,
        path: &str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) {
        self.routes
            .insert((method.clone(), path.to_string()), handler);
        self.metadata.insert((method, path.to_string()), meta);
    }

    pub async fn dispatch(&self, mut req: Request<hyper::body::Incoming>) -> Response<BoxBody> {
        let method = req.method().clone();
        let req_path = req.uri().path();

        // Reuse existing RequestContext if injected upstream, otherwise create one
        let context_frame = req
            .extensions()
            .get::<error::RequestContext>()
            .cloned()
            .unwrap_or_else(|| error::RequestContext::from_request(&req));

        let mut matched_handler = None;
        let mut captured_params = std::collections::HashMap::new();

        // Iterate through compiled regex tracks in order of configuration registration
        for route in &self.regex_routes {
            if route.method != method {
                continue;
            }

            if let Some(captures) = route.compiled_regex.captures(req_path) {
                matched_handler = Some(&route.handler);

                // Extract all registered capture group variables dynamically
                for name in &route.param_names {
                    if let Some(matched_group) = captures.name(name) {
                        captured_params.insert(name.clone(), matched_group.as_str().to_string());
                    }
                }
                break;
            }
        }

        if let Some(handler) = matched_handler {
            if req.headers().contains_key(http::header::UPGRADE) {
                let on_upgrade = hyper::upgrade::on(&mut req);
                req.extensions_mut().insert(on_upgrade);
            }
            // Hydrate both RequestContext and PathParams so handlers can extract either
            req.extensions_mut().insert(context_frame);
            req.extensions_mut().insert(PathParams(captured_params));
            handler(req).await
        } else {
            let mut resp = Error::NotFound(format!(
                "No matching route rule sequence identified for {} {}",
                method, req_path
            ))
            .into_response_with_context(&context_frame);

            // Echo the trace_id on 404 responses so clients can correlate unmatched routes
            if let Ok(val) = http::HeaderValue::from_str(&context_frame.trace_id().to_string()) {
                resp.headers_mut()
                    .insert(http::header::HeaderName::from_static("x-request-id"), val);
            }
            resp
        }
    }

    pub async fn handle_with_middleware(
        self: &std::sync::Arc<Self>,
        req: Request<hyper::body::Incoming>,
    ) -> Response<BoxBody> {
        if self.global_middleware.is_empty() {
            return self.dispatch(req).await;
        }

        let incoming_cell = std::sync::Arc::new(tokio::sync::Mutex::new(Some(req)));
        let cell_for_inner = incoming_cell.clone();
        let router_clone = self.clone();

        let mut chain: MiddleChain = std::sync::Arc::new(move |req_box| {
            let cell = cell_for_inner.clone();
            let router = router_clone.clone();
            Box::pin(async move {
                let mut guard = cell.lock().await;
                if let Some(mut native_req) = guard.take() {
                    native_req
                        .extensions_mut()
                        .extend(req_box.extensions().clone());
                    router.dispatch(native_req).await
                } else {
                    crate::response::IntoResponse::into_response(Error::Internal(
                        "Request stream already consumed".into(),
                    ))
                }
            })
        });

        for mw in self.global_middleware.iter().rev() {
            let active_mw = mw.clone();
            let next_chain = chain.clone();
            chain = std::sync::Arc::new(move |r| active_mw.handle(r, next_chain.clone()));
        }

        let parts = {
            let mut guard = incoming_cell.lock().await;
            let orig = guard.take().unwrap();
            let (p, b) = orig.into_parts();
            *guard = Some(Request::from_parts(p.clone(), b));
            p
        };

        let blank_body = http_body_util::Empty::<bytes::Bytes>::new()
            .map_err(|e| match e {})
            .boxed();
        let mw_req = Request::from_parts(parts, blank_body);

        chain(mw_req).await
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Config {
    pub host: String,
    pub port: u16,
}

pub struct Application {
    router: Router,
    config: Config,
    extensions: Extensions,
    scheduler: tasks::TaskScheduler,
}

impl Application {
    /// Ergonomic surface initializer to construct an ApplicationBuilder loop.
    pub fn builder(config: Config) -> ApplicationBuilder {
        ApplicationBuilder::new(config)
    }

    /// Unwraps and returns the internal Router matrix instance.
    pub fn into_router(self) -> Router {
        self.router
    }

    /// Safely updates the shared server runtime configurations.
    pub fn local_addr(&self) -> String {
        format!("{}:{}", self.config.host, self.config.port)
    }

    /// Publicly injects a structured component layer directly into Request Extensions.
    pub fn inject_extension<T>(&mut self, val: T)
    where
        T: Send + Sync + Clone + 'static,
    {
        self.extensions.insert(val);
    }

    /// Compiles macro-generated endpoint metadata schemas dynamically into an OpenAPI 3.1.0 Tree Document.
    pub fn generate_openapi_spec(&self, title: &str, version: &str) -> OpenApiSpec {
        let mut paths: HashMap<String, HashMap<String, EndpointMetadata>> = HashMap::new();

        for ((method, path), meta) in &self.router.metadata {
            let method_string = method.to_string().to_lowercase();
            paths
                .entry(path.clone())
                .or_default()
                .insert(method_string, meta.clone());
        }

        let components = if self.router.schemas.is_empty() {
            None
        } else {
            Some(ComponentsSpec {
                schemas: self.router.schemas.clone(),
            })
        };

        OpenApiSpec {
            openapi: "3.1.0".to_string(),
            info: InfoSpec {
                title: title.to_string(),
                version: version.to_string(),
            },
            paths,
            components,
        }
    }

    /// Eagerly binds the TCP Socket listener layout to the current OS context frame.
    /// This immediately replaces a dynamic proxy configuration (port: 0) with the actual resolved port.
    pub async fn bind_listener(
        &mut self,
    ) -> Result<TcpListener, Box<dyn std::error::Error + Send + Sync>> {
        let initial_addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&initial_addr).await?;

        // Synchronize our internal server instance tracker with the real assigned port immediately
        let bound_addr = listener.local_addr()?;
        self.config.port = bound_addr.port();

        Ok(listener)
    }

    /// Serves incoming requests on a pre-bound `TcpListener` until `shutdown_signal` resolves,
    /// then drains active connections cleanly before returning.
    pub async fn serve_with_graceful_shutdown<F>(
        mut self,
        listener: TcpListener,
        shutdown_signal: F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let addr = format!("{}:{}", self.config.host, self.config.port);

        // 1. Compile OpenAPI spec & Swagger UI assets
        let spec = self.generate_openapi_spec("Kasi Microservice Engine", "1.0.0");
        let spec_json_bytes = bytes::Bytes::from(serde_json::to_string_pretty(&spec).unwrap());
        self.extensions.insert(OpenApiJsonBytes(spec_json_bytes));

        let swagger_ui_html = r#"
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <title>Kasi Framework API Documentation</title>
            <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/swagger-ui/5.29.1/swagger-ui.css" />
        </head>
        <body>
            <div id="swagger-ui"></div>
            <script src="https://cdnjs.cloudflare.com/ajax/libs/swagger-ui/5.29.1/swagger-ui.js" crossorigin></script>
            <script>
                window.onload = () => {
                    window.ui = SwaggerUIBundle({
                        url: '/openapi.json',
                        dom_id: '#swagger-ui',
                        deepLinking: true,
                        presets: [SwaggerUIBundle.presets.apis],
                    });
                };
            </script>
        </body>
        </html>
        "#
        .to_string();

        self.extensions
            .insert(SwaggerHtmlBytes(bytes::Bytes::from(swagger_ui_html)));

        println!("Kasi server listening natively on http://{}", addr);
        println!("OpenAPI JSON available at http://{}/openapi.json", addr);
        println!("Swagger UI active at http://{}/docs", addr);

        let shared_router = Arc::new(self.router);
        let shared_extensions = Arc::new(self.extensions);

        // 2. Broadcast channel to notify active connections when shutdown begins
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.scheduler.start_workers(shutdown_rx.clone());
        // Track active connection tasks so we can wait for them to finish draining
        let (active_conn_tx, mut active_conn_rx) = tokio::sync::mpsc::channel::<()>(1);

        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                // Stop accepting new sockets when the shutdown signal fires
                _ = &mut shutdown_signal => {
                    println!("\n[SHUTDOWN] Signal received. Draining active connections...");
                    let _ = shutdown_tx.send(true);
                    break;
                }

                accept_res = listener.accept() => {
                    let (stream, _remote_addr) = match accept_res {
                        Ok(conn) => conn,
                        Err(err) => {
                            eprintln!("[WARN] Failed to accept TCP connection: {}", err);
                            continue;
                        }
                    };

                    let io = TokioIo::new(stream);
                    let router_clone = shared_router.clone();
                    let extensions_clone = shared_extensions.clone();
                    let mut conn_shutdown_rx = shutdown_rx.clone();
                    let conn_guard = active_conn_tx.clone();

                    tokio::task::spawn(async move {
                        let _guard = conn_guard; // Held until this connection finishes

                        let service = service_fn(move |mut req: Request<Incoming>| {
                            let router = router_clone.clone();
                            let extensions = extensions_clone.clone();
                            async move {
                                req.extensions_mut().extend(extensions.as_ref().clone());

                                if req.uri().path() == "/openapi.json" && req.method() == Method::GET
                                    && let Some(wrapped_json) = extensions.get::<OpenApiJsonBytes>()
                                {
                                    let body = Full::new(wrapped_json.0.clone())
                                        .map_err(|e| match e {})
                                        .boxed();
                                    return Ok::<_, hyper::Error>(
                                        Response::builder()
                                            .status(StatusCode::OK)
                                            .header("Content-Type", "application/json")
                                            .body(body)
                                            .unwrap(),
                                    );
                                }

                                if req.uri().path() == "/docs" && req.method() == Method::GET
                                    && let Some(wrapped_html) = extensions.get::<SwaggerHtmlBytes>()
                                {
                                    let body = Full::new(wrapped_html.0.clone())
                                        .map_err(|e| match e {})
                                        .boxed();
                                    return Ok::<_, hyper::Error>(
                                        Response::builder()
                                            .status(StatusCode::OK)
                                            .header("Content-Type", "text/html; charset=utf-8")
                                            .body(body)
                                            .unwrap(),
                                    );
                                }

                                Ok::<_, hyper::Error>(router.handle_with_middleware(req).await)
                            }
                        });

                        let builder = auto::Builder::new(TokioExecutor::new());
                        let conn = builder.serve_connection_with_upgrades(io, service);
                        tokio::pin!(conn);

                        tokio::select! {
                            res = conn.as_mut() => {
                                if let Err(err) = res {
                                    eprintln!("[ERROR] Connection error: {}", err);
                                }
                            }
                            _ = conn_shutdown_rx.changed() => {
                                // Tell Hyper to finish the current in-flight request and close HTTP/1.1 keep-alive
                                conn.as_mut().graceful_shutdown();
                                let _ = conn.await;
                            }
                        }
                    });
                }
            }
        }

        // Drop our local sender and wait for all spawned connection guards to drop
        drop(active_conn_tx);
        let _ = active_conn_rx.recv().await;
        println!("[SHUTDOWN] All connections drained. Server stopped cleanly.");

        Ok(())
    }

    /// Keeps existing `serve_on_listener` compatible for tests (runs indefinitely until task abort).
    pub async fn serve_on_listener(
        self,
        listener: TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.serve_with_graceful_shutdown(listener, std::future::pending())
            .await
    }

    /// Production entrypoint that listens for `Ctrl+C` to trigger a graceful shutdown.
    pub async fn serve(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = self.bind_listener().await?;
        self.serve_with_graceful_shutdown(listener, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    }
}

pub struct ApplicationBuilder {
    config: Config,
    pub(crate) router: Router,
    extensions: Extensions,
    middleware: Vec<Arc<dyn Middleware>>,
    scheduler: tasks::TaskScheduler,
}

impl ApplicationBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            router: Router::new(),
            extensions: Extensions::new(),
            middleware: Vec::new(),
            scheduler: TaskScheduler::new(),
        }
    }

    pub fn extension<T>(mut self, val: T) -> Self
    where
        T: Send + Sync + Clone + 'static,
    {
        self.extensions.insert(val);
        self
    }

    pub fn mount_endpoint(
        mut self,
        method: Method,
        path: &'static str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) -> Self {
        // 1. Store metadata definitions matching the path template exactly for OpenAPI specs
        self.router
            .metadata
            .insert((method.clone(), path.to_string()), meta);

        // 2. Compile route expression layout rules and cache evaluation bindings natively
        self.router.compile_and_add_route(method, path, handler);

        self
    }

    pub fn add_raw_route<F, Fut>(mut self, method: Method, path: &str, handler: F) -> Self
    where
        F: Fn(Request<BoxBody>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response<BoxBody>> + Send + 'static,
    {
        self.router.add_route(method, path, handler);
        self
    }

    pub async fn build(self) -> Result<Application, Box<dyn std::error::Error + Send + Sync>> {
        let mut router = self.router;
        let mut extensions = self.extensions;

        // Allow handlers to extract `Extension<TaskScheduler>` to call `scheduler.spawn(...)`
        extensions.insert(self.scheduler.clone());
        router.global_middleware = self.middleware;

        Ok(Application {
            router,
            config: self.config,
            extensions,
            scheduler: self.scheduler,
        })
    }

    pub fn schedule_task<F, Fut>(
        mut self,
        name: impl Into<String>,
        interval: std::time::Duration,
        task: F,
    ) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.scheduler = self.scheduler.every(name, interval, task);
        self
    }

    /// Registers a struct implementing `#[derive(Schema)]` into the OpenAPI components dictionary.
    pub fn register_schema<T: Schema>(mut self) -> Self {
        self.router
            .schemas
            .insert(T::schema_name().to_string(), T::schema_object());
        self
    }

    // pub fn route(self, registration_wrapper: fn(Self) -> Self) -> Self {
    //     registration_wrapper(self)
    // }

    pub fn register_module(mut self, module: Module) -> Self {
        // 1. First, copy the metadata into the router before consuming module fields
        for ((method, path), meta) in module.metadata {
            self.router.metadata.insert((method, path), meta);
        }

        // 2. Loop through endpoints, wrap them with middleware, and mount them once
        for (method, path, base_handler) in module.endpoints {
            let mut active_chain: HandlerFn = base_handler;

            for middleware_layer in module.middleware.iter().rev() {
                let current_layer = middleware_layer.run.clone();
                let next_inner_handler = active_chain.clone();

                active_chain = Arc::new(move |req| {
                    let inner = next_inner_handler.clone();
                    let next_node = Next {
                        chain: Box::new(move |r| inner(r)),
                    };
                    current_layer(req, next_node)
                });
            }

            // This adds the fully wrapped middleware execution chain into the routing matrix!
            self.router.add_raw_handler(method, &path, active_chain);
        }

        self
    }

    pub fn middleware<M: Middleware>(mut self, middleware: M) -> Self {
        self.middleware.push(Arc::new(middleware));
        self
    }

    /// Registers an `#[endpoint]` directly by its function name (e.g., `.route(create_item)`).
    pub fn route<E: RegisterEndpoint>(self, endpoint: E) -> Self {
        endpoint.register(self)
    }
}

pub trait RegisterEndpoint {
    fn register<R: EndpointRegistry>(self, registry: R) -> R;
}

/// Shared target implemented by `ApplicationBuilder` and `Module` for mounting endpoints.
pub trait EndpointRegistry {
    fn mount_endpoint(
        self,
        method: Method,
        path: &'static str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) -> Self;
}

impl EndpointRegistry for ApplicationBuilder {
    fn mount_endpoint(
        mut self,
        method: Method,
        path: &'static str,
        handler: HandlerFn,
        meta: EndpointMetadata,
    ) -> Self {
        self.router
            .metadata
            .insert((method.clone(), path.to_string()), meta);
        self.router.compile_and_add_route(method, path, handler);
        self
    }
}
