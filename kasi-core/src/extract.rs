use http::{Request, header};
use http_body_util::BodyExt;
#[cfg(feature = "postgres")]
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};
use serde::de::DeserializeOwned;
#[cfg(feature = "postgres")]
use sqlx::PgPool;

use crate::{BoxBody, BoxFuture, Error, PathParams, error::Validate};

pub trait FromRequest: Sized {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>>;
}
pub struct Json<T>(pub T);
pub struct Query<T>(pub T);
pub struct Path<T>(pub T);

pub struct Db(pub DatabaseConnection);
pub struct DbPool(pub PgPool);

pub struct ValidatedJson<T>(pub T);

/// Explicitly wraps an active SeaORM transaction lifecycle boundary.
/// Automatically issues an atomic COMMIT on success, or a ROLLBACK on error.
pub struct Tx(pub DatabaseTransaction);

/// Extracts a cloned instance of `T` from the request's `Extensions` type-map.
pub struct Extension<T>(pub T);

impl<T> FromRequest for Extension<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let val = req
                .extensions()
                .get::<T>()
                .cloned()
                .ok_or_else(|| {
                    Error::Internal(format!(
                        "Missing required request extension of type '{}'. Did you register it via .extension()?",
                        std::any::type_name::<T>()
                    ))
                })?;

            Ok((Extension(val), req))
        })
    }
}

impl FromRequest for Tx {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let conn = req.extensions()
                .get::<DatabaseConnection>()
                .cloned()
                .ok_or_else(|| Error::Internal(
                    "SeaORM DatabaseConnection instance missing from request extensions context.".into()
                ))?;

            // Begin an isolated atomic transaction stream block eagerly
            let transaction = conn.begin().await.map_err(|e| {
                Error::Internal(format!(
                    "Failed to initialize database transaction block: {}",
                    e
                ))
            })?;

            Ok((Tx(transaction), req))
        })
    }
}

impl<T> FromRequest for ValidatedJson<T>
where
    T: serde::de::DeserializeOwned + Validate + Send + 'static, // Requires Validate trait bound!
{
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let (Json(payload), remaining_req) = <Json<T> as FromRequest>::extract(req).await?;

            if let Err(validation_error) = payload.validate() {
                return Err(validation_error);
            }

            Ok((ValidatedJson(payload), remaining_req))
        })
    }
}

impl FromRequest for Db {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let conn = req
                .extensions()
                .get::<DatabaseConnection>()
                .cloned()
                .ok_or_else(|| {
                    Error::Internal(
                        "SeaORM DatabaseConnection handle missing from request extensions.".into(),
                    )
                })?;

            Ok((Db(conn), req))
        })
    }
}

impl FromRequest for DbPool {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let pool = req.extensions().get::<PgPool>().cloned().ok_or_else(|| {
                Error::Internal("SQLx PgPool handle missing from request extensions.".into())
            })?;

            Ok((DbPool(pool), req))
        })
    }
}

impl<T> FromRequest for Json<T>
where
    T: DeserializeOwned + Send + 'static,
{
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            if let Some(content_type) = req.headers().get(header::CONTENT_TYPE) {
                if content_type != "application/json" {
                    return Err(Error::BadRequest(format!(
                        "Invalid Content-Type. Expected application/json but got {:?}",
                        content_type
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
                return Err(Error::BadRequest(
                    "Empty request body payload received".into(),
                ));
            }

            let value: T = serde_json::from_slice(&bytes)
                .map_err(|e| Error::BadRequest(format!("JSON Deserialization Failure: {}", e)))?;

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
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
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
    T: serde::de::DeserializeOwned + Send + 'static,
{
    fn extract(
        req: Request<BoxBody>,
    ) -> crate::BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let params = req.extensions().get::<PathParams>().ok_or_else(|| {
                Error::Internal(
                    "Router failed to inject PathParams extension framework layer".into(),
                )
            })?;

            let mut json_map = serde_json::Map::new();
            for (k, v) in &params.0 {
                json_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            let json_object = serde_json::Value::Object(json_map);

            if let Ok(value) = serde_json::from_value::<T>(json_object) {
                return Ok((Path(value), req));
            }

            if params.0.len() == 1 {
                if let Some(raw_string_value) = params.0.values().next() {
                    let primitive_json_value = serde_json::Value::String(raw_string_value.clone());
                    if let Ok(value) = serde_json::from_value::<T>(primitive_json_value) {
                        return Ok((Path(value), req));
                    }
                }
            }

            Err(Error::BadRequest(
                "URL Path Parameter Parsing Failure: Mismatched path type signature layout rules."
                    .into(),
            ))
        })
    }
}

use std::collections::HashMap;

/// Represents a single file uploaded via `multipart/form-data`.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub field_name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub bytes: bytes::Bytes,
}

/// Extracts parsed text fields and binary files from a `multipart/form-data` request.
#[derive(Debug, Clone)]
pub struct Multipart {
    pub fields: HashMap<String, String>,
    pub files: Vec<UploadedFile>,
}

impl FromRequest for Multipart {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let content_type = req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::BadRequest("Missing Content-Type header".into()))?
                .to_string();

            if !content_type.starts_with("multipart/form-data") {
                return Err(Error::BadRequest(
                    "Invalid Content-Type. Expected multipart/form-data".into(),
                ));
            }

            let boundary = content_type
                .split(';')
                .find_map(|part| {
                    let trimmed = part.trim();
                    trimmed.strip_prefix("boundary=")
                })
                .map(|b| b.trim_matches('"').to_string())
                .ok_or_else(|| Error::BadRequest("Missing multipart boundary parameter".into()))?;

            let (parts, body_stream) = req.into_parts();
            let collected = body_stream
                .collect()
                .await
                .map_err(|e| Error::Internal(format!("Failed to read multipart stream: {}", e)))?;
            let raw_bytes = collected.to_bytes();

            let parsed = parse_multipart_body(&raw_bytes, &boundary)?;

            let blank_body = http_body_util::Empty::<bytes::Bytes>::new()
                .map_err(|e| match e {})
                .boxed();
            let finalized_req = Request::from_parts(parts, blank_body);

            Ok((parsed, finalized_req))
        })
    }
}

fn parse_multipart_body(body: &[u8], boundary: &str) -> Result<Multipart, Error> {
    let delimiter = format!("--{}", boundary).into_bytes();
    let mut fields = HashMap::new();
    let mut files = Vec::new();

    // Split raw bytes by `--<boundary>`
    let mut positions = Vec::new();
    let mut i = 0;
    while i + delimiter.len() <= body.len() {
        if &body[i..i + delimiter.len()] == delimiter.as_slice() {
            positions.push(i);
            i += delimiter.len();
        } else {
            i += 1;
        }
    }

    for window in positions.windows(2) {
        let mut part = &body[window[0] + delimiter.len()..window[1]];
        if part.starts_with(b"--") {
            break;
        }
        if let Some(stripped) = part.strip_prefix(b"\r\n") {
            part = stripped;
        }
        if let Some(stripped) = part.strip_suffix(b"\r\n") {
            part = stripped;
        }

        // Separate part headers from part payload at `\r\n\r\n`
        let header_end = part
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| Error::BadRequest("Malformed multipart part headers".into()))?;

        let headers_str = std::str::from_utf8(&part[..header_end])
            .map_err(|_| Error::BadRequest("Invalid UTF-8 in multipart headers".into()))?;
        let payload = &part[header_end + 4..];

        let mut field_name = None;
        let mut file_name = None;
        let mut part_content_type = None;

        for line in headers_str.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("content-disposition:") {
                for segment in line.split(';') {
                    let s = segment.trim();
                    if let Some(val) = s.strip_prefix("name=") {
                        field_name = Some(val.trim_matches('"').to_string());
                    } else if let Some(val) = s.strip_prefix("filename=") {
                        file_name = Some(val.trim_matches('"').to_string());
                    }
                }
            } else if lower.starts_with("content-type:") {
                part_content_type = Some(line.split_once(':').unwrap().1.trim().to_string());
            }
        }

        let name = field_name
            .ok_or_else(|| Error::BadRequest("Multipart part missing field name".into()))?;

        if file_name.is_some() {
            files.push(UploadedFile {
                field_name: name,
                file_name,
                content_type: part_content_type,
                bytes: bytes::Bytes::copy_from_slice(payload),
            });
        } else {
            let text_val = String::from_utf8_lossy(payload).to_string();
            fields.insert(name, text_val);
        }
    }

    Ok(Multipart { fields, files })
}
