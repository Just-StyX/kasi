use std::{pin::Pin, sync::Arc, future::Future};

use http::{Method, Request, Response};
use hyper::body::Incoming;

use crate::{BoxBody, BoxFuture, Error, HandlerFn, Router, error::RequestContext};

pub type MiddleChain = Arc<dyn Fn(Request<BoxBody>) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send>> + Send + Sync>;

pub trait Middleware: Send + Sync + 'static {
    fn handle<'a>(&'a self, req: Request<BoxBody>, next: MiddleChain) -> Pin<Box<dyn Future<Output = Response<BoxBody>> + Send + 'a>>;
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

#[allow(clippy::type_complexity)]
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
pub struct Module {
    name: String,
    pub(crate) endpoints: Vec<(Method, String, HandlerFn)>,
    pub(crate) middleware: Vec<InternalMiddlewareLayer>,
}

impl Module {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            endpoints: Vec::new(),
            middleware: Vec::new(),
        }
    }

    pub fn add_endpoint(mut self, method: Method, path: &str, handler: HandlerFn) -> Self {
        self.endpoints.push((method, path.to_string(), handler));
        self
    }

    pub fn wrap(mut self, layer: InternalMiddlewareLayer) -> Self {
        self.middleware.push(layer);
        self
    }
}
