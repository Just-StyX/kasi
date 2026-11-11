use async_graphql::{
    Request as GqlInnerRequest, Response as GqlInnerResponse,
    http::{GraphiQLSource, receive_json},
};
use bytes::Bytes;
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use crate::{
    BoxBody,
    BoxFuture,
    Error,
    extract::FromRequest,
    response::IntoResponse,
    // templates::Html,
};

/// Extracts an `async_graphql::Request` from a `POST` JSON request body.
pub struct GraphQLRequest(pub GqlInnerRequest);

impl GraphQLRequest {
    pub fn into_inner(self) -> GqlInnerRequest {
        self.0
    }
}

impl FromRequest for GraphQLRequest {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let (parts, body_stream) = req.into_parts();

            let collected = body_stream.collect().await.map_err(|e| {
                Error::Internal(format!("Failed to read GraphQL request body: {}", e))
            })?;
            let bytes = collected.to_bytes();

            let gql_req = receive_json(bytes.as_ref())
                .await
                .map_err(|e| Error::BadRequest(format!("Invalid GraphQL JSON payload: {}", e)))?;

            let blank_body = http_body_util::Empty::<Bytes>::new()
                .map_err(|e| match e {})
                .boxed();
            let finalized_req = Request::from_parts(parts, blank_body);

            Ok((GraphQLRequest(gql_req), finalized_req))
        })
    }
}

/// Wraps an `async_graphql::Response` and implements Kasi's `IntoResponse`.
pub struct GraphQLResponse(pub GqlInnerResponse);

impl From<GqlInnerResponse> for GraphQLResponse {
    fn from(res: GqlInnerResponse) -> Self {
        Self(res)
    }
}

impl IntoResponse for GraphQLResponse {
    fn into_response(self) -> Response<BoxBody> {
        let json_string = serde_json::to_string(&self.0)
            .unwrap_or_else(|e| format!(r#"{{"errors":[{{"message":"{}"}}]}}"#, e));

        let status = if self.0.is_ok() {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };

        let body = Full::new(Bytes::from(json_string))
            .map_err(|e| match e {})
            .boxed();

        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .unwrap()
    }
}

/// Renders the interactive GraphiQL web IDE pointing at `endpoint` (e.g., `"/graphql"`).
pub fn graphiql_playground(endpoint: &str) -> Response<BoxBody> {
    let html = GraphiQLSource::build().endpoint(endpoint).finish();
    let body = Full::new(Bytes::from(html)).map_err(|e| match e {}).boxed();

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(body)
        .unwrap()
}
