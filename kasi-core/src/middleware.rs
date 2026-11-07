use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{Full, BodyExt };
use hyper::body::Incoming;

use crate::{BoxBody, BoxFuture, Error, HandlerFn, Router, error::RequestContext, openapi::EndpointMetadata};

pub type MiddleChain = Arc<dyn Fn(Request<BoxBody>) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send>> + Send + Sync>;

pub trait Middleware: Send + Sync + 'static {
    fn handle(
        &self, 
        req: http::Request<BoxBody>, 
        next: MiddleChain
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
            allowed_methods: vec![Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS, Method::PATCH],
            allowed_headers: vec!["Content-Type".into(), "Authorization".into(), "Accept".into()],
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
            headers.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        } else if let Some(ref req_org) = request_origin {
            if let Ok(org_str) = req_org.to_str() {
                if self.allowed_origins.iter().any(|o| o == org_str) {
                    headers.insert(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, (*req_org).clone());
                }
            }
        }

        if self.allow_credentials {
            headers.insert(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
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
                let empty_body = Full::new(Bytes::new())
                    .map_err(|e| match e {})
                    .boxed();

                let mut res = Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(empty_body)
                    .unwrap();

                let headers = res.headers_mut();
                
                this.evaluate_origin(origin.as_ref(), headers);

                let methods_str = this.allowed_methods.iter()
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
            prefix: if sanitized_prefix == "/" { "".into() } else { sanitized_prefix },
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
            meta: EndpointMetadata
        ) -> Self {
            let localized_path = format!("{}/{}", self.prefix, path.trim_start_matches('/'));
            self.endpoints.push((method.clone(), localized_path.clone(), handler));
            self.metadata.insert((method, localized_path), meta);
            self
        }
    
        /// Attaches an entire child module under this routing scope group, combining endpoint definitions and OpenAPI specifications.
        pub fn nest_with_docs(mut self, child_module: Module) -> Self {
            // Merge the executable endpoint hooks (reusing your nesting wrapper above)
            self = self.nest(child_module.clone());
    
            // Merge and prefix the structural metadata tree layout components dynamically
            for ((method, child_path), meta) in child_module.metadata {
                let combined_path = format!("{}/{}", self.prefix, child_path.trim_start_matches('/'));
                self.metadata.insert((method, combined_path), meta);
            }
            self
        }

    /// Mounts an endpoint directly onto the current module group level scope.
    pub fn add_endpoint(mut self, method: http::Method, path: &str, handler: HandlerFn) -> Self {
        let localized_path = format!("{}/{}", self.prefix, path.trim_start_matches('/'));
        self.endpoints.push((method, localized_path, handler));
        self
    }

    /// Attaches a child module sub-tree under this routing scope group,
        /// dynamically prefixing paths and propagating error interception frameworks cleanly.
        pub fn nest(mut self, child_module: Module) -> Self {
            for (method, child_path, handler) in child_module.endpoints {
                // 1. Maintain clean nested path segmentation trees
                let combined_path = format!("{}/{}", self.prefix, child_path.trim_start_matches('/'));
                
                // 2. Propagate parent middleware down to child executions
                let mut layered_handler = handler;
                for parent_middleware in self.middleware.iter().rev() {
                    let current_layer = parent_middleware.run.clone();
                    let next_inner_handler = layered_handler.clone();
    
                    layered_handler = Arc::new(move |req| {
                        let inner = next_inner_handler.clone();
                        let next_node = Next {
                            chain: Box::new(move |r| {
                                // Intercept errors during deep module traversal executions
                                let inner_clone = inner.clone();
                                Box::pin(async move {
                                    inner_clone(r).await
                                })
                            }),
                        };
                        current_layer(req, next_node)
                    });
                }
    
                self.endpoints.push((method, combined_path, layered_handler));
            }
            self
        }

    pub fn wrap(mut self, layer: InternalMiddlewareLayer) -> Self {
        self.middleware.push(layer);
        self
    }
}
