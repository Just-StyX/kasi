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

use crate::middleware::Middleware;
use crate::openapi::{EndpointMetadata, InfoSpec, OpenApiSpec};
use crate::{
    error::{Error, RequestContext},
    extract::PathParams,
    middleware::{Module, Next},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type BoxBody = combinators::BoxBody<Bytes, hyper::Error>;
pub type HandlerFn =
    Arc<dyn Fn(Request<Incoming>) -> BoxFuture<'static, Response<BoxBody>> + Send + Sync>;

pub mod error;
pub mod extract;
pub mod middleware;
pub mod openapi;
pub mod response;
pub mod testing;

#[derive(Clone)]
struct OpenApiJsonBytes(pub bytes::Bytes);

#[derive(Clone)]
struct SwaggerHtmlBytes(pub bytes::Bytes);

pub struct Router {
    routes: HashMap<(Method, String), HandlerFn>,
    pub metadata: HashMap<(Method, String), EndpointMetadata>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    pub(crate) fn add_raw_handler(&mut self, method: Method, path: &str, handler: HandlerFn) {
        self.routes.insert((method, path.to_string()), handler);
    }

    pub fn wrap_handlers() {
    
    }

    #[allow(clippy::redundant_closure, clippy::useless_conversion)]
    pub fn add_route<F, Fut>(&mut self, method: Method, path: &str, handler: F)
    where
        F: Fn(Request<BoxBody>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response<BoxBody>> + Send + 'static,
    {
        let wrapped_handler = Arc::new(move |req: Request<Incoming>| {
            let (parts, body) = req.into_parts();
            let boxed_body = body.map_err(|e| hyper::Error::from(e)).boxed();

            let transformed_req = Request::from_parts(parts, boxed_body);
            let fut = handler(transformed_req);
            Box::pin(fut) as BoxFuture<'static, Response<BoxBody>>
        });

        self.routes
            .insert((method, path.to_string()), wrapped_handler);
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

    pub async fn dispatch(&self, mut req: Request<Incoming>) -> Response<BoxBody> {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let captured_map = std::collections::HashMap::new();
        let context_frame = RequestContext::from_request(&req);
        req.extensions_mut().insert(PathParams(captured_map));
        let key = (method, path);
        if let Some(handler) = self.routes.get(&key) {
            handler(req).await
        } else {
            Error::NotFound(format!("No route found for {} {}", key.0, key.1))
                .into_response_with_context(&context_frame)
        }
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
}

impl Application {
    pub fn builder(config: Config) -> ApplicationBuilder {
        ApplicationBuilder::new(config)
    }

    pub fn into_router(self) -> Router {
        self.router
    }

    pub fn inject_extension<T>(&mut self, val: T)
    where
        T: Send + Sync + Clone + 'static,
    {
        self.extensions.insert(val);
    }

    pub fn generate_openapi_spec(&self, title: &str, version: &str) -> OpenApiSpec {
        let mut paths: HashMap<String, HashMap<String, EndpointMetadata>> = HashMap::new();

        for ((method, path), meta) in &self.router.metadata {
            let method_string = method.to_string().to_lowercase();

            paths
                .entry(path.clone())
                .or_default()
                .insert(method_string, meta.clone());
        }

        OpenApiSpec {
            openapi: "3.1.0".to_string(),
            info: InfoSpec {
                title: title.to_string(),
                version: version.to_string(),
            },
            paths,
        }
    }

    pub async fn serve(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;

        // Compile the specification document based on macro compile data
        let spec = self.generate_openapi_spec("Kasi Microservice Engine", "1.0.0");
        let spec_json_bytes = bytes::Bytes::from(serde_json::to_string_pretty(&spec).unwrap());

        // FIXED: Insert using our unique structural wrapper type
        self.extensions.insert(OpenApiJsonBytes(spec_json_bytes));

        // Compile the visual Swagger UI HTML dashboard layout
        let swagger_ui_html = r#"
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <title>Kasi Framework API Documentation</title>
            <link rel="stylesheet" href="https://cloudflare.com" />
        </head>
        <body>
            <div id="swagger-ui"></div>
            <script src="https://cloudflare.com" crossorigin></script>
            <script>
                window.onload = () => {{
                    window.ui = SwaggerUIBundle({{
                        url: '/openapi.json',
                        dom_id: '#swagger-ui',
                        deepLinking: true,
                        presets: [
                            SwaggerUIBundle.presets.apis,
                        ],
                    }});
                }};
            </script>
        </body>
        </html>
        "#
        .to_string();

        let swagger_bytes = bytes::Bytes::from(swagger_ui_html);
        // FIXED: Insert using our unique structural wrapper type
        self.extensions.insert(SwaggerHtmlBytes(swagger_bytes));

        println!("Kasi server listening natively on http://{}", addr);
        println!(
            "OpenAPI JSON documentation structure available at http://{}/openapi.json",
            addr
        );
        println!(
            "Graphical Swagger UI Dashboard active at http://{}/docs",
            addr
        );

        let shared_router = Arc::new(self.router);
        let shared_extensions = Arc::new(self.extensions);

        loop {
            let (stream, _remote_addr) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let router_clone = shared_router.clone();
            let extensions_clone = shared_extensions.clone();

            tokio::task::spawn(async move {
                let service = service_fn(move |mut req: Request<Incoming>| {
                    let router = router_clone.clone();
                    let extensions = extensions_clone.clone();
                    async move {
                        req.extensions_mut().extend(extensions.as_ref().clone());

                        // Intercept JSON Spec calls
                        if req.uri().path() == "/openapi.json" && req.method() == Method::GET {
                            // FIXED: Look up via our unique type identifier
                            if let Some(wrapped_json) = extensions.get::<OpenApiJsonBytes>() {
                                let body = http_body_util::Full::new(wrapped_json.0.clone())
                                    .map_err(|e| match e {})
                                    .boxed();
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(200)
                                        .header("Content-Type", "application/json")
                                        .body(body)
                                        .unwrap(),
                                );
                            }
                        }

                        // Intercept Graphical Swagger UI Dashboard calls
                        if req.uri().path() == "/docs" && req.method() == Method::GET {
                            // FIXED: Look up via our unique type identifier
                            if let Some(wrapped_html) = extensions.get::<SwaggerHtmlBytes>() {
                                let body = http_body_util::Full::new(wrapped_html.0.clone())
                                    .map_err(|e| match e {})
                                    .boxed();
                                return Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(200)
                                        .header("Content-Type", "text/html; charset=utf-8")
                                        .body(body)
                                        .unwrap(),
                                );
                            }
                        }

                        Ok::<_, hyper::Error>(router.dispatch(req).await)
                    }
                });
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, service)
                    .await;
            });
        }
    }

    pub fn local_addr(&self) -> String {
        format!("{}:{}", self.config.host, self.config.port)
    }
}

pub struct ApplicationBuilder {
    config: Config,
    pub(crate) router: Router,
    extensions: Extensions,
    middleware: Vec<Arc<dyn Middleware>>
}

impl ApplicationBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            router: Router::new(),
            extensions: Extensions::new(),
            middleware: Vec::new(),
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
        self.router
            .add_endpoint_with_meta(method, path, handler, meta);
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
        let middleware_chain = self.middleware;
        router.wrap_handles_with_middleware(middleware_chain);
        Ok(Application {
            router: router,
            config: self.config,
            extensions: self.extensions,
        })
    }

    pub fn route(self, registration_wrapper: fn(Self) -> Self) -> Self {
        registration_wrapper(self)
    }

    // pub fn route<R>(mut self, route_register: R) -> Self
    // where
    //     R: RegisterRoute,
    // {
    //     route_register.register_route(&mut self.router);
    //     self
    // }

    pub fn register_module(mut self, module: Module) -> Self {
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
            self.router.add_raw_handler(method, &path, active_chain);
        }
        self
    }

    pub fn middleware<M: Middleware>(mut self, middleware: M) -> Self {
        self.middleware.push(Arc::new(middleware));
        self
    }
}
