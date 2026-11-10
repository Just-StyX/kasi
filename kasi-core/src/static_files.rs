use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;

use crate::{ApplicationBuilder, BoxBody, BoxFuture, Error, HandlerFn, error::RequestContext};

/// Serves static assets (CSS, JS, images) safely from a local directory.
#[derive(Clone)]
pub struct ServeDir {
    root_dir: Arc<PathBuf>,
    url_prefix: String,
}

impl ServeDir {
    /// Creates a static file handler mapping `url_prefix` (e.g. `"/static"`) to `dir` (e.g. `"public"`).
    pub fn new(url_prefix: &str, dir: impl AsRef<Path>) -> Self {
        let clean_prefix = format!("/{}", url_prefix.trim_matches('/'));
        Self {
            root_dir: Arc::new(dir.as_ref().to_path_buf()),
            url_prefix: if clean_prefix == "/" {
                String::new()
            } else {
                clean_prefix
            },
        }
    }

    /// Converts this `ServeDir` into a framework `HandlerFn` compatible with `Router`.
    pub fn into_handler(self) -> HandlerFn {
        Arc::new(move |req: Request<Incoming>| {
            let service = self.clone();
            Box::pin(async move { service.handle_request(req).await })
                as BoxFuture<'static, Response<BoxBody>>
        })
    }

    async fn handle_request(&self, req: Request<Incoming>) -> Response<BoxBody> {
        let context_frame = RequestContext::from_request(&req);
        let req_path = req.uri().path();

        // Strip configured prefix (e.g. "/static/css/app.css" -> "css/app.css")
        let relative_str = req_path
            .strip_prefix(&self.url_prefix)
            .unwrap_or(req_path)
            .trim_start_matches('/');

        if relative_str.is_empty() {
            return Error::NotFound("Directory listing is disabled".into())
                .into_response_with_context(&context_frame);
        }

        // Prevent path traversal attacks (e.g. `/static/../../etc/passwd`)
        let mut safe_rel_path = PathBuf::new();
        for component in Path::new(relative_str).components() {
            match component {
                Component::Normal(part) => safe_rel_path.push(part),
                _ => {
                    return Error::Forbidden("Invalid static file path traversal attempt".into())
                        .into_response_with_context(&context_frame);
                }
            }
        }

        let full_path = self.root_dir.join(&safe_rel_path);

        match tokio::fs::read(&full_path).await {
            Ok(bytes) => {
                let mime = detect_mime_type(&full_path);
                let body = Full::new(Bytes::from(bytes))
                    .map_err(|e| match e {})
                    .boxed();

                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime)
                    .header(header::CACHE_CONTROL, "public, max-age=3600")
                    .body(body)
                    .unwrap()
            }
            Err(_) => Error::NotFound(format!("Static asset not found: {}", req_path))
                .into_response_with_context(&context_frame),
        }
    }
}

fn detect_mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

impl ApplicationBuilder {
    /// Mounts a directory of static files under `url_prefix` (e.g., `.serve_dir("/static", "public")`).
    pub fn serve_dir(mut self, url_prefix: &str, dir: impl AsRef<Path>) -> Self {
        let clean_prefix = format!("/{}", url_prefix.trim_matches('/'));
        let route_pattern = if clean_prefix == "/" {
            "/{file:.+}".to_string()
        } else {
            format!("{}/{{file:.+}}", clean_prefix)
        };

        let handler = ServeDir::new(&clean_prefix, dir).into_handler();
        self.router
            .compile_and_add_route(Method::GET, &route_pattern, handler);
        self
    }
}
