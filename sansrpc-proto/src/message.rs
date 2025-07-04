use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Message<T> {
    Oneshot(Oneshot<T>),
    Request(Request<T>),
    Response(Response<T>),
    OpenStream(OpenStream<T>),
    StreamMessage(StreamMessage<T>),
    CloseStream { stream_id: u64 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Oneshot<T> {
    pub message: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Request<T> {
    pub request_id: u64,
    pub data: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Response<T> {
    pub request_id: u64,
    pub data: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenStream<T> {
    pub stream_id: u64,
    pub data: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StreamMessage<T> {
    pub stream_id: u64,
    pub data: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OpenBiDirectionalStream<T> {
    pub stream_id: u64,
    pub data: T,
}
