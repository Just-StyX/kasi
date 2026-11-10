use minijinja::{Environment, path_loader};
use std::path::Path;
use std::sync::Arc;

use crate::{BoxBody, BoxFuture, Error, extract::FromRequest, response::IntoResponse};
use bytes::Bytes;
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

/// Thread-safe wrapper around MiniJinja's template environment.
#[derive(Clone)]
pub struct TemplateEngine {
    env: Arc<Environment<'static>>,
}

impl TemplateEngine {
    /// Loads templates dynamically from a local directory (e.g., `"templates"`).
    pub fn from_dir(dir: impl AsRef<Path>) -> Self {
        let mut env = Environment::new();
        env.set_loader(path_loader(dir));
        Self { env: Arc::new(env) }
    }

    /// Creates an engine from a pre-configured `minijinja::Environment` (useful with `minijinja-embed`).
    pub fn from_env(env: Environment<'static>) -> Self {
        Self { env: Arc::new(env) }
    }

    /// Renders a named template with any serializable context (such as `minijinja::context!`).
    pub fn render<S: Into<minijinja::Value>>(&self, name: &str, ctx: S) -> Result<Html, Error> {
        let tmpl = self
            .env
            .get_template(name)
            .map_err(|e| Error::Internal(format!("Template '{}' not found: {}", name, e)))?;

        let rendered = tmpl
            .render(ctx)
            .map_err(|e| Error::Internal(format!("Failed to render template '{}': {}", name, e)))?;

        Ok(Html(rendered))
    }
}

/// Allow endpoints to extract `TemplateEngine` directly in their function parameters.
impl FromRequest for TemplateEngine {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let engine = req
                .extensions()
                .get::<TemplateEngine>()
                .cloned()
                .ok_or_else(|| {
                    Error::Internal(
                        "TemplateEngine missing from extensions. Register it via .extension()."
                            .into(),
                    )
                })?;

            Ok((engine, req))
        })
    }
}

/// Response wrapper that sets `Content-Type: text/html; charset=utf-8`.
pub struct Html(pub String);

impl IntoResponse for Html {
    fn into_response(self) -> Response<BoxBody> {
        let body = Full::new(Bytes::from(self.0))
            .map_err(|e| match e {})
            .boxed();

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(body)
            .unwrap()
    }
}
