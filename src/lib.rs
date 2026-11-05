pub use kasi_core as kcore; 
pub use kasi_security as ksecure;
pub use kasi_macros::*;

pub mod prelude {
    pub use kasi_core::{
        Application, ApplicationBuilder, Config, Router, middleware::{Next, InternalMiddlewareLayer, Module}, BoxBody
    };

    pub use kasi_core::testing::{
        TestServer, TestRequestBuilder, TestResponse
    };

    pub use http::{
        Method, StatusCode, HeaderValue, HeaderName, Request, Response
    };

    pub use hyper::body::Incoming; 
    
    pub use serde::{ Serialize, Deserialize };
    pub use serde_json::{ json, Value as JsonValue };
    pub use kasi_macros::{ endpoint, main, IntoResponse};
    pub use kasi_core::extract::{ Json, Query, Path };
    pub use kasi_core::error::{ Error, RequestContext };
    pub use kasi_core::response::IntoResponse;
    pub use kasi_core::openapi::EndpointMetadata;
    pub use kasi_security::{ AuthContext, RequiredRole, OAuth2 };

    #[cfg(feature = "postgres")]
    pub mod db {
        pub use sqlx::PgPool;
        // 2. FIXED: In sqlx 0.9.x, Transaction is generic. Exporting it explicitly 
        // or aliasing to PgTransaction prevents generic parameter mismatches.
        pub use sqlx::postgres::PgTransaction; 
        pub use sea_orm::{ DatabaseConnection, EntityTrait, QueryFilter };
    }
}

#[doc(hidden)]
pub mod __private {
    pub use kasi_core::Router as InternalRouter;
    pub use kasi_core::Method as InternalMethod;
    pub use http::Response as HttpResponse;
    pub use hyper::body::Incoming as InternalIncoming;
}
