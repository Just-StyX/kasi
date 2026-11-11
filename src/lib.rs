pub use kasi_core as kcore;
pub use kasi_macros::*;
pub use kasi_security as ksecure;

pub mod prelude {
    pub use kasi_core::{
        Application, ApplicationBuilder, BoxBody, Config, EndpointRegistry, HandlerFn, PathParams,
        RegisterEndpoint, Router,
        cookies::{Cookie, Cookies, SameSite},
        middleware::{
            CompressionMiddleware, CorsMiddleware, InternalMiddlewareLayer, LogFormat, MiddleChain,
            Module, Next, RateLimitMiddleware, RegisterRoute, TimeoutMiddleware, TracingMiddleware,
        },
        static_files::ServeDir,
        tasks::TaskScheduler,
        ws::{WebSocket, WebSocketUpgrade, WsMessage},
    };

    pub use kasi_core::metrics::{MetricsMiddleware, MetricsRegistry};
    pub use kasi_core::sse::{Sse, SseEvent};

    pub use http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode};

    pub use hyper::body::Incoming;

    pub use bytes::Bytes;
    pub use http_body_util::{BodyExt, Full};

    pub use kasi_core::openapi::{ComponentsSpec, EndpointMetadata, Schema, SchemaObject};
    pub use kasi_macros::{IntoResponse, Schema, Validate, endpoint, main};

    pub use kasi_core::error::{Error, RequestContext, Validate};
    pub use kasi_core::extract::{
        Extension, FromRequest, Json, Multipart, Path, Query, UploadedFile, ValidatedJson,
    };
    pub use kasi_core::response::IntoResponse;
    pub use kasi_security::{
        ApiKeyMiddleware, AuthContext, BearerTokenMiddleware, OAuth2, OidcConfig, RequiredRole,
    };
    pub use serde::{Deserialize, Serialize};
    pub use serde_json::{Value as JsonValue, json};

    #[cfg(feature = "templates")]
    pub use kasi_core::templates::{Html, TemplateEngine};

    #[cfg(feature = "templates")]
    pub use minijinja::context as template_context;

    #[cfg(feature = "postgres")]
    pub mod db {
        pub use sqlx::PgPool;
        // 2. FIXED: In sqlx 0.9.x, Transaction is generic. Exporting it explicitly
        // or aliasing to PgTransaction prevents generic parameter mismatches.
        pub use sea_orm::{DatabaseConnection, EntityTrait, QueryFilter};
        pub use sqlx::postgres::PgTransaction;
    }
}

#[doc(hidden)]
pub mod __private {
    pub use http::Response as HttpResponse;
    pub use hyper::body::Incoming as InternalIncoming;
    pub use kasi_core::Method as InternalMethod;
    pub use kasi_core::Router as InternalRouter;
}
