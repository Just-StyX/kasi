pub use kasi_core as kcore;
pub use kasi_macros::*;
pub use kasi_security as ksecure;

pub mod prelude {
    pub use kasi_core::{
        Application, ApplicationBuilder, BoxBody, Config, HandlerFn, Router,
        middleware::{InternalMiddlewareLayer, Module, Next},
    };

    pub use kasi_core::testing::{TestRequestBuilder, TestResponse, TestServer};

    pub use http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode};

    pub use hyper::body::Incoming;

    pub use kasi_core::error::{Error, RequestContext};
    pub use kasi_core::extract::{Json, Path, Query};
    pub use kasi_core::openapi::EndpointMetadata;
    pub use kasi_core::response::IntoResponse;
    pub use kasi_macros::{IntoResponse, endpoint, main};
    pub use kasi_security::{AuthContext, OAuth2, RequiredRole};
    pub use serde::{Deserialize, Serialize};
    pub use serde_json::{Value as JsonValue, json};

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
