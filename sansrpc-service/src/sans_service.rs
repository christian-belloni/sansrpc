use futures::{
    FutureExt, Sink, SinkExt, Stream, StreamExt, TryStreamExt,
    channel::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use sansrpc_proto::message::{Message, OpenStream, Request, Response, StreamMessage};
use std::{collections::HashMap, io, pin::Pin};

use crate::{
    handle::ServiceHandle,
    service_message::{ServiceInteraction, ServiceMessage},
};

pub struct SansService<In, Out, Reader, Writer> {
    reader: Reader,
    writer: Writer,

    outgoing_stream: Pin<Box<dyn Stream<Item = StreamMessage<Out>>>>,
    outgoing_responses: Pin<Box<dyn Stream<Item = Response<Out>>>>,

    incoming_streams: HashMap<u64, UnboundedSender<StreamMessage<In>>>,
    incoming_responses: HashMap<u64, oneshot::Sender<Response<In>>>,

    interaction: UnboundedReceiver<ServiceInteraction<In, Out>>,
}

impl<In, Out, Reader, Writer> SansService<In, Out, Reader, Writer>
where
    Out: 'static,
    In: 'static,
    Reader: Stream<Item = io::Result<Message<In>>> + Unpin,
    Writer: Sink<Message<Out>, Error = io::Error> + Unpin,
{
    pub fn new(reader: Reader, writer: Writer) -> (Self, ServiceHandle<In, Out>) {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let this = Self {
            reader,
            writer,
            outgoing_stream: Box::pin(futures::stream::pending()),
            outgoing_responses: Box::pin(futures::stream::pending()),
            incoming_streams: Default::default(),
            incoming_responses: Default::default(),
            interaction: rx,
        };
        (this, ServiceHandle(tx))
    }

    pub async fn next_message(&mut self) -> io::Result<Option<ServiceMessage<In>>> {
        loop {
            let mut next_out_stream = self.outgoing_stream.next().fuse();
            let mut next_out_response = self.outgoing_responses.next().fuse();
            let mut next_message = self.reader.try_next().fuse();

            let mut next_interaction = self.interaction.next().fuse();

            futures::select! {
                next = next_message => {
                    let Some(next) = next? else { return Ok(None) };
                    match next {
                        Message::Oneshot(oneshot) => return Ok(Some(ServiceMessage::Oneshot(oneshot))),
                        Message::Request(request) => return Ok(Some(ServiceMessage::Request(request))),
                        Message::Response(response) => {
                            let Some(tx) = self.incoming_responses.remove(&response.request_id) else { continue };
                            _ = tx.send(response);
                        },
                        Message::OpenStream(stream_request) => return Ok(Some(ServiceMessage::OpenStream(stream_request))),
                        Message::StreamMessage(stream_message) => {
                            let Some(tx) = self.incoming_streams.get(&stream_message.stream_id) else { continue };
                            tx.unbounded_send(stream_message).unwrap();
                        },
                        Message::CloseStream { stream_id } => {
                            self.incoming_streams.remove(&stream_id).unwrap();
                        },
                    }
                },
                out = next_out_stream => {
                    if let Some(next) = out {
                        self.writer.send(Message::StreamMessage(next)).await?;
                    }
                }
                out = next_out_response => {
                    if let Some(next) = out {
                        self.writer.send(Message::Response(next)).await?;
                    }
                }
                interaction = next_interaction => {
                    let Some(interaction) = interaction else { continue };

                    match interaction {
                        ServiceInteraction::OpenStream(stream_request, sender) => {
                            self.incoming_streams.insert(stream_request.stream_id, sender);
                            self.writer.send(Message::OpenStream(stream_request)).await.unwrap();
                        },
                        ServiceInteraction::Request(request, sender) => {
                            self.incoming_responses.insert(request.request_id, sender);
                            self.writer.send(Message::Request(request)).await.unwrap();
                        },
                        ServiceInteraction::Oneshot(oneshot) => {
                            self.writer.send(Message::Oneshot(oneshot)).await.unwrap();
                        },
                    }
                },
            }
        }
    }

    pub async fn recv_stream<K: 'static, F: Fn(K) -> Out + 'static>(
        &mut self,
        stream_id: u64,
        map: F,
    ) -> futures::channel::mpsc::UnboundedSender<K> {
        let (tx, rx) = futures::channel::mpsc::unbounded::<K>();

        let rx = rx.map(map).map(move |value| StreamMessage {
            stream_id,
            data: value,
        });

        let stream = std::mem::replace(
            &mut self.outgoing_stream,
            Box::pin(futures::stream::pending()),
        );

        self.outgoing_stream = Box::pin(futures::stream::select(stream, rx));

        tx
    }

    pub async fn recv_request<K: 'static>(
        &mut self,
        request_id: u64,
        map: impl Fn(K) -> Out + 'static,
    ) -> futures::channel::oneshot::Sender<K> {
        let (tx, rx) = futures::channel::oneshot::channel::<K>();

        let rx = rx.map(|a| a.unwrap()).map(map).map(move |value| Response {
            request_id,
            data: value,
        });

        let rx = futures::stream::once(rx);

        let stream = std::mem::replace(
            &mut self.outgoing_responses,
            Box::pin(::futures::stream::pending()),
        );

        self.outgoing_responses = Box::pin(futures::stream::select(stream, rx));
        tx
    }
}
