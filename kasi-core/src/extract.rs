use http::{Request, header};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};
use serde::de::DeserializeOwned;

use crate::{BoxFuture, Error};

pub trait FromRequest: Sized {
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>>;
}

pub struct Json<T>(pub T);
pub struct Query<T>(pub T);
pub struct Path<T>(pub T);

#[derive(Clone, Debug)]
pub struct PathParams(pub std::collections::HashMap<String, String>);

pub struct Db(pub DatabaseConnection);
pub struct DbPool(pub sqlx::PgPool);

impl Db {
    pub async fn transaction<F, Fut, T, E>(&self, closure: F) -> Result<T, E>
    where
        F: FnOnce(&DatabaseTransaction) -> Fut + Send,
        Fut: Future<Output = Result<T, E>> + Send,
        E: From<sea_orm::DbErr> + Send,
    {
        let tx_conn = self.0.begin().await?;
        match closure(&tx_conn).await {
            Ok(result) => {
                tx_conn.commit().await?;
                Ok(result)
            }
            Err(error) => {
                let _ = tx_conn.rollback().await;
                Err(error)
            }
        }
    }
}

impl FromRequest for Db {
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            let conn = req.extensions()
                .get::<sea_orm::DatabaseConnection>()
                .cloned()
                .ok_or_else(|| Error::Internal(
                    "SeaORM DatabaseConnection handle missing from request extensions. Did you register it via .extension()?".into()
                ))?;

            Ok(Db(conn))
        })
    }
}

impl FromRequest for DbPool {
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            let pool = req.extensions()
                .get::<sqlx::PgPool>()
                .cloned() // PgPool wraps an internal Arc pool tracker, making clones incredibly cheap
                .ok_or_else(|| Error::Internal(
                    "SQLx PgPool handle missing from request extensions. Did you register it via .extension()?".into()
                ))?;

            Ok(DbPool(pool))
        })
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            if let Some(content_type) = req.headers().get(header::CONTENT_TYPE) {
                if content_type != "application/json" {
                    let msg = format!(
                        "Invalid Content-Type. Expected application/json but got {:?}",
                        content_type
                    );
                    return Err(Error::BadRequest(msg));
                }
            } else {
                return Err(Error::BadRequest("Missing Content-Type header".into()));
            }
            let (_parts, body_stream) = req.into_parts();

            let collected = body_stream.collect().await.map_err(|e| {
                Error::Internal(format!("Failed to read incoming body stream: {}", e))
            })?;

            let bytes = collected.to_bytes();
            if bytes.is_empty() {
                return Err(Error::BadRequest(
                    "Empty request body payload received".into(),
                ));
            }

            let value: T = serde_json::from_slice(&bytes)
                .map_err(|e| Error::BadRequest(format!("JSON Deserialization Failure: {}", e)))?;

            Ok(Json(value))
        })
    }
}

impl<T> FromRequest for Query<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            let query_str = req.uri().query().unwrap_or("");
            let value: T = serde_urlencoded::from_str(query_str).map_err(|e| {
                Error::BadRequest(format!("URL Query Parameter Parsing Failure: {}", e))
            })?;

            Ok(Query(value))
        })
    }
}

impl<T> FromRequest for Path<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            let params = req.extensions().get::<PathParams>().ok_or_else(|| {
                Error::Internal(
                    "Router failed to inject PathParams extension framework layer".into(),
                )
            })?;

            // Convert the key-value map back into an encoded string format that serde_urlencoded can parse
            // (e.g. {"id": "123", "name": "alice"} becomes "id=123&name=alice")
            let serialized_params = params
                .0
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<String>>()
                .join("&");

            let value: T = serde_urlencoded::from_str(&serialized_params).map_err(|e| {
                Error::BadRequest(format!("URL Path Parameter Parsing Failure: {}", e))
            })?;

            Ok(Path(value))
        })
    }
}
