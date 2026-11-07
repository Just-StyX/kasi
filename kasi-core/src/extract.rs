use http::{Request, header};
use http_body_util::BodyExt;
#[cfg(feature = "postgres")]
use sea_orm::{DatabaseConnection};
use serde::de::DeserializeOwned;
#[cfg(feature = "postgres")]
use sqlx::PgPool;

use crate::{BoxBody, BoxFuture, Error, PathParams, error::Validate};

pub trait FromRequest: Sized {
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>>;
}
pub struct Json<T>(pub T);
pub struct Query<T>(pub T);
pub struct Path<T>(pub T);

pub struct Db(pub DatabaseConnection);
pub struct DbPool(pub PgPool);

pub struct ValidatedJson<T>(pub T);

impl<T> FromRequest for ValidatedJson<T>
where
    T: serde::de::DeserializeOwned + Validate + Send + 'static, // Requires Validate trait bound!
{
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            // 1. First, reuse our existing Json extraction logic by delegating to its FromRequest lifecycle
            let (Json(payload), remaining_req) = <Json<T> as FromRequest>::extract(req).await?;

            // 2. Perform validation guards immediately after structural deserialization succeeds
            if let Err(validation_error) = payload.validate() {
                // Short-circuit the pipeline if semantic rules are violated
                return Err(validation_error);
            }

            // 3. Return the fully verified payload context safely back to the execution line
            Ok((ValidatedJson(payload), remaining_req))
        })
    }
}

impl FromRequest for Db {
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let conn = req.extensions()
                .get::<DatabaseConnection>()
                .cloned()
                .ok_or_else(|| Error::Internal(
                    "SeaORM DatabaseConnection handle missing from request extensions.".into()
                ))?;

            Ok((Db(conn), req))
        })
    }
}

impl FromRequest for DbPool {
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let pool = req.extensions()
                .get::<PgPool>()
                .cloned()
                .ok_or_else(|| Error::Internal(
                    "SQLx PgPool handle missing from request extensions.".into()
                ))?;

            Ok((DbPool(pool), req))
        })
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            if let Some(content_type) = req.headers().get(header::CONTENT_TYPE) {
                if content_type != "application/json" {
                    return Err(Error::BadRequest(format!(
                        "Invalid Content-Type. Expected application/json but got {:?}", content_type
                    )));
                }
            } else {
                return Err(Error::BadRequest("Missing Content-Type header".into()));
            }

            let (parts, body_stream) = req.into_parts();

            // Collect the stream frames natively
            let collected = body_stream.collect().await.map_err(|e| {
                Error::Internal(format!("Failed to read incoming body stream: {}", e))
            })?;

            let bytes = collected.to_bytes();
            if bytes.is_empty() {
                return Err(Error::BadRequest("Empty request body payload received".into()));
            }

            let value: T = serde_json::from_slice(&bytes)
                .map_err(|e| Error::BadRequest(format!("JSON Deserialization Failure: {}", e)))?;

            // FIX: Create an empty body stream and box it into the required framework `BoxBody` type.
            // This guarantees that the branches share identical concrete types!
            let blank_body = http_body_util::Empty::<bytes::Bytes>::new()
                .map_err(|e| match e {}) // Empty is infallible, cast it to match hyper::Error
                .boxed();

            let finalized_req = Request::from_parts(parts, blank_body);

            Ok((Json(value), finalized_req))
        })
    }
}

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let query_str = req.uri().query().unwrap_or("");
            let value: T = serde_urlencoded::from_str(query_str).map_err(|e| {
                Error::BadRequest(format!("URL Query Parameter Parsing Failure: {}", e))
            })?;

            Ok((Query(value), req))
        })
    }
}


impl<T> FromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<BoxBody>) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            // Locate the registered structural route variable map via extensions lookups
            let params = req.extensions().get::<PathParams>().ok_or_else(|| {
                Error::Internal("Router failed to inject PathParams extension framework layer".into())
            })?;
            
            let serialized_params = params.0
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<String>>()
                .join("&");

            let value: T = serde_urlencoded::from_str(&serialized_params).map_err(|e| {
                Error::BadRequest(format!("URL Path Parameter Parsing Failure: {}", e))
            })?;

            Ok((Path(value), req))
        })
    }
}
