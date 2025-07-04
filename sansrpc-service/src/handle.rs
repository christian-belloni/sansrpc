use futures::channel::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use sansrpc_proto::message::{Oneshot, OpenStream, Request, Response, StreamMessage};

use crate::service_message::ServiceInteraction;

pub struct ServiceHandle<In, Out>(pub(crate) UnboundedSender<ServiceInteraction<In, Out>>);

impl<In, Out> ServiceHandle<In, Out> {
    pub async fn send_oneshot(&mut self, request: Oneshot<Out>) {
        _ = self.0.unbounded_send(ServiceInteraction::Oneshot(request));
    }

    pub async fn send_request(&mut self, request: Request<Out>) -> Response<In> {
        let (tx, rx) = oneshot::channel();

        _ = self
            .0
            .unbounded_send(ServiceInteraction::Request(request, tx));
        rx.await.unwrap()
    }

    pub async fn send_stream(
        &mut self,
        request: OpenStream<Out>,
    ) -> UnboundedReceiver<StreamMessage<In>> {
        let (tx, rx) = ::futures::channel::mpsc::unbounded();

        _ = self
            .0
            .unbounded_send(ServiceInteraction::OpenStream(request, tx));

        rx
    }
}
