use crate::sans_service::SansService;
use futures::{Sink, SinkExt, Stream, StreamExt};
use sansrpc_proto::message::Message;
use std::io;

pub trait Service<In, Out>: Clone + Send + 'static {
    type Request: From<In> + Send;
    type Oneshot: From<In> + Send;
    type Stream: From<In> + Send;

    type Response: Into<Out> + Send;
    type StreamMessage: Into<Out> + Send;
    type StreamResponse: Stream<Item = Result<Self::StreamMessage, Self::Error>> + Unpin + Send;

    type Error: From<io::Error>
        + From<futures::channel::mpsc::SendError>
        + std::error::Error
        + Send
        + Sync
        + 'static;

    fn spawn<K>(&self, future: impl Future<Output = K> + Send);

    fn handle_request(
        &self,
        request: Self::Request,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send;

    fn handle_stream(
        &self,
        stream: Self::Stream,
    ) -> impl Future<Output = Result<Self::StreamResponse, Self::Error>> + Send;

    fn handle_oneshot(&self, oneshot: Self::Oneshot) -> impl Future<Output = ()> + Send;

    fn shutdown(&self) -> impl Future<Output = ()>;
}

pub struct ServiceInner<S, In, Out, Reader, Writer> {
    sans_service: SansService<In, Out, Reader, Writer>,
    service: S,
}

impl<S, In, Out, Reader, Writer> ServiceInner<S, In, Out, Reader, Writer>
where
    S: Service<In, Out>,
    Out: 'static,
    In: 'static,
    Reader: Stream<Item = io::Result<Message<In>>> + Unpin,
    Writer: Sink<Message<Out>, Error = io::Error> + Unpin,
{
    pub async fn run(&mut self) -> Result<(), S::Error> {
        while let Some(next) = self.sans_service.next_message().await? {
            match next {
                crate::service_message::ServiceMessage::OpenStream(open_stream) => {
                    let stream_id = open_stream.stream_id;
                    let stream_request: S::Stream = open_stream.data.into();
                    let sink = self
                        .sans_service
                        .recv_stream(stream_id, |data: S::StreamMessage| data.into())
                        .await;
                    let service = self.service.clone();

                    self.service.spawn(async move {
                        let stream = service.handle_stream(stream_request).await?;
                        stream.forward(sink.sink_err_into()).await
                    });
                }
                crate::service_message::ServiceMessage::Request(request) => {
                    let request_id = request.request_id;
                    let request: S::Request = request.data.into();

                    let response_sender = self
                        .sans_service
                        .recv_request(request_id, |data: S::Response| data.into())
                        .await;

                    let service = self.service.clone();

                    self.service.spawn(async move {
                        let response = service.handle_request(request).await?;
                        _ = response_sender.send(response);
                        Ok::<_, S::Error>(())
                    });
                }
                crate::service_message::ServiceMessage::Oneshot(oneshot) => {
                    let oneshot: S::Oneshot = oneshot.message.into();

                    let service = self.service.clone();

                    self.service
                        .spawn(async move { service.handle_oneshot(oneshot).await });
                }
            }
        }
        Ok(())
    }
}
