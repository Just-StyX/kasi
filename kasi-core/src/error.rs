use std::fmt::Display;

pub use bytes::Bytes;
use http::Request;
pub use http::{Response, StatusCode};
pub use http_body_util::{BodyExt, Full};
// use hyper::body::Incoming;

pub use crate::BoxBody;
use crate::{BoxFuture, extract::FromRequest};

#[derive(Debug, Clone)]
pub enum Error {
    NotFound(String),
    Internal(String),
    Unauthorized(String),
    BadRequest(String),
    Forbidden(String),
}

pub trait Validate {
    /// Evaluates structural or logical constraints on a type.
    /// Returns a structured ValidationError matrix or Ok(()) if completely compliant.
    fn validate(&self) -> Result<(), Error>;
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub(crate) uri_path: String,
    pub(crate) method: String,
    pub(crate) trace_id: uuid::Uuid,
}

impl From<(&str, &str, uuid::Uuid)> for RequestContext {
    fn from(value: (&str, &str, uuid::Uuid)) -> Self {
        Self {
            uri_path: value.0.to_string(),
            method: value.1.to_string().to_uppercase(),
            trace_id: value.2,
        }
    }
}

impl RequestContext {
    pub fn from_request<B>(req: &Request<B>) -> Self {
        // Reuse incoming X-Request-Id header if valid UUID, otherwise mint a fresh v4 UUID
        let trace_id = req
            .headers()
            .get("x-request-id")
            .and_then(|val| val.to_str().ok())
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .unwrap_or_else(uuid::Uuid::new_v4);

        Self {
            uri_path: req.uri().path().to_string(),
            method: req.method().to_string(),
            trace_id,
        }
    }

    pub fn trace_id(&self) -> uuid::Uuid {
        self.trace_id
    }

    pub fn uri_path(&self) -> &str {
        &self.uri_path
    }

    pub fn method(&self) -> &str {
        &self.method
    }
}

impl FromRequest for RequestContext {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let ctx = req
                .extensions()
                .get::<RequestContext>()
                .cloned()
                .unwrap_or_else(|| RequestContext::from_request(&req));

            Ok((ctx, req))
        })
    }
}

impl Error {
    pub fn into_response_with_context(self, context: &RequestContext) -> Response<BoxBody> {
        let (status, title, detail) = match self {
            Error::NotFound(m) => (StatusCode::NOT_FOUND, "Not Found", m),
            Error::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error",
                m,
            ),
            Error::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "Unauthorized Access Sequence", m),
            Error::BadRequest(m) => (StatusCode::BAD_REQUEST, "Bad Request Structural Pattern", m),
            Error::Forbidden(m) => (StatusCode::FORBIDDEN, "Forbidden Payload Constraint", m),
        };

        let json_body = serde_json::json!({
            "type": format!("https://kasi.local{}", title.to_lowercase().replace(" ", "-")),
            "title": title,
            "status": &status.as_u16(),
            "detail": detail,
            "instance": &context.uri_path(),
            "trace_id": &&context.trace_id().to_string()
        });

        let body = Full::new(Bytes::from(json_body.to_string()))
            .map_err(|e| match e {})
            .boxed();

        Response::builder()
            .status(status)
            .header("Content-Type", "application/problem+json")
            .body(body)
            .unwrap()
    }
}

impl std::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(msg) => write!(f, "Not Found: {}", msg),
            Error::Internal(msg) => write!(f, "Internal Server Error: {}", msg),
            Error::Unauthorized(msg) => write!(f, "Unauthorized Access Sequence: {}", msg),
            Error::BadRequest(msg) => write!(f, "Bad Request Structural Pattern: {}", msg),
            Error::Forbidden(msg) => write!(f, "Forbidden Payload Constraint: {}", msg),
        }
    }
}
