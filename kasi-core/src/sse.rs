use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use http::{Response, StatusCode, header};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;

use crate::{BoxBody, response::IntoResponse};

/// Represents a single Server-Sent Event message frame.
#[derive(Debug, Clone, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
    pub retry_ms: Option<u64>,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            ..Default::default()
        }
    }

    pub fn json<T: serde::Serialize>(val: &T) -> Result<Self, serde_json::Error> {
        Ok(Self::data(serde_json::to_string(val)?))
    }

    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn retry_ms(mut self, ms: u64) -> Self {
        self.retry_ms = Some(ms);
        self
    }

    pub fn to_bytes(&self) -> Bytes {
        let mut out = String::new();
        if let Some(ref id) = self.id {
            out.push_str(&format!("id: {}\n", id));
        }
        if let Some(ref ev) = self.event {
            out.push_str(&format!("event: {}\n", ev));
        }
        if let Some(retry) = self.retry_ms {
            out.push_str(&format!("retry: {}\n", retry));
        }
        for line in self.data.lines() {
            out.push_str(&format!("data: {}\n", line));
        }
        if self.data.is_empty() {
            out.push_str("data: \n");
        }
        out.push('\n');
        Bytes::from(out)
    }
}

pub type BoxEventStream = Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>>;

struct SseFrameStream {
    inner: Mutex<BoxEventStream>,
}

impl Stream for SseFrameStream {
    type Item = Result<Frame<Bytes>, hyper::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut guard = self.inner.lock().unwrap();
        match guard.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => Poll::Ready(Some(Ok(Frame::data(event.to_bytes())))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Response wrapper that streams `text/event-stream` events to connected clients.
pub struct Sse {
    stream: BoxEventStream,
}

impl Sse {
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<SseEvent, Infallible>> + Send + 'static,
    {
        Self {
            stream: Box::pin(stream),
        }
    }
}

impl IntoResponse for Sse {
    fn into_response(self) -> Response<BoxBody> {
        let frame_stream = SseFrameStream {
            inner: Mutex::new(self.stream),
        };
        let body = StreamBody::new(frame_stream).boxed();

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .unwrap()
    }
}
