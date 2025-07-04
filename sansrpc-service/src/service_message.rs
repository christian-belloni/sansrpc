use futures::channel::{mpsc::UnboundedSender, oneshot};
use sansrpc_proto::message::{Oneshot, OpenStream, Request, Response, StreamMessage};

#[derive(Debug, Clone, Copy)]
pub enum ServiceMessage<T> {
    OpenStream(OpenStream<T>),
    Request(Request<T>),
    Oneshot(Oneshot<T>),
}

pub(crate) enum ServiceInteraction<In, Out> {
    OpenStream(OpenStream<Out>, UnboundedSender<StreamMessage<In>>),
    Request(Request<Out>, oneshot::Sender<Response<In>>),
    Oneshot(Oneshot<Out>),
}
