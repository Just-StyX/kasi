use crate::{BoxBody, Error, error::RequestContext};
use bytes::Bytes;
use http::{Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

pub trait IntoResponse {
    fn into_response(self) -> Response<BoxBody>;
}

// 1. Allow users to return fully constructed Responses directly
impl IntoResponse for Response<BoxBody> {
    fn into_response(self) -> Response<BoxBody> {
        self
    }
}

// 2. Allow users to return simple text strings
impl IntoResponse for String {
    fn into_response(self) -> Response<BoxBody> {
        let body = Full::new(Bytes::from(self)).map_err(|e| match e {}).boxed();

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(body)
            .unwrap()
    }
}

// 3. Allow users to return standard static string slices
impl IntoResponse for &'static str {
    fn into_response(self) -> Response<BoxBody> {
        self.to_string().into_response()
    }
}

// 4. Map your internal framework Error to a response automatically
impl IntoResponse for Error {
    fn into_response(self) -> Response<BoxBody> {
        let temporary_context: RequestContext =
            ("/unknown", "unknown", uuid::Uuid::new_v4()).into();
        self.into_response_with_context(&temporary_context)
    }
}

// 5. Provide automatic support for the standard Result type
impl<R, E> IntoResponse for Result<R, E>
where
    R: IntoResponse,
    E: IntoResponse,
{
    fn into_response(self) -> Response<BoxBody> {
        match self {
            Ok(res) => res.into_response(),
            Err(err) => err.into_response(),
        }
    }
}
