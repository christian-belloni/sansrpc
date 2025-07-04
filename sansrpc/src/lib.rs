pub use sansrpc_proto as proto;
pub use sansrpc_service::*;

#[cfg(feature = "compio")]
pub use sansrpc_compio as compio;

#[cfg(feature = "tokio")]
pub use sansrpc_tokio as tokio;

#[cfg(feature = "macros")]
pub use sansrpc_macros::service;

pub trait Runtime: Clone {
    fn spawn<Fn, Fut, R>(&self, f: Fn)
    where
        Fn: FnOnce() -> Fut + 'static,
        Fn: Send,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static;
}

#[cfg(feature = "compio")]
mod compio_runtime {
    use std::sync::Arc;

    use compio::dispatcher::Dispatcher;

    #[derive(Debug, Clone, Copy)]
    pub struct CompioRuntime;

    impl super::Runtime for CompioRuntime {
        fn spawn<Fn, Fut, R>(&self, f: Fn)
        where
            Fn: FnOnce() -> Fut + 'static,
            Fn: Send,
            Fut: Future<Output = R> + 'static,
            R: Send + 'static,
        {
            _ = ::compio::runtime::spawn(f());
        }
    }

    impl super::Runtime for Arc<Dispatcher> {
        fn spawn<Fn, Fut, R>(&self, f: Fn)
        where
            Fn: FnOnce() -> Fut + 'static,
            Fn: Send,
            Fut: Future<Output = R> + 'static,
            R: Send + 'static,
        {
            _ = self.dispatch(f);
        }
    }
}

#[cfg(feature = "compio")]
pub use compio_runtime::*;

#[cfg(test)]
pub mod tests {
    use std::{io, net::ToSocketAddrs, sync::Arc};

    use async_trait::async_trait;
    use compio::{
        dispatcher,
        net::{TcpStream, ToSocketAddrsAsync},
    };
    use futures::{
        Sink, SinkExt, Stream, StreamExt,
        channel::mpsc::UnboundedReceiver,
        stream::{SplitSink, SplitStream},
    };
    use sansrpc_compio::tcp::CompioSansConnection;
    use sansrpc_macros::{oneshot, request, streaming};
    use sansrpc_proto::{
        bincode::BincodeSansCoder, connection_state::ConnectionConfig, message::Message,
    };
    use sansrpc_service::{handle::ServiceHandle, service::SansService, service_message};
    use serde::{Deserialize, Serialize};

    use crate::{Runtime, compio_runtime::CompioRuntime};

    #[async_trait]
    pub trait MyService {
        type StreamStringsStream: Stream<Item = String> + Send + 'static;

        async fn request_string(&self, my_string: String) -> String;

        async fn stream_strings(&self, values: u64) -> Self::StreamStringsStream;

        async fn string_oneshot(&self, value: String);
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub enum ClientMessage {
        Request(ClientRequest),
        Stream(ClientStreamRequest),
        Oneshot(ClientOneshot),
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub enum ClientRequest {
        RequestString { my_string: String },
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub enum ClientStreamRequest {
        StreamStrings { values: u64 },
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub enum ClientOneshot {
        StringOneshot { value: String },
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub enum ServerMessage {
        RequestString(String),
        StreamStringMessage(String),
    }

    pub struct MyServiceImpl<Impl, RT, Reader, Writer> {
        inner: SansService<ClientMessage, ServerMessage, Reader, Writer>,
        service: Impl,
        runtime: RT,
    }

    type CompioTcpConnection = CompioSansConnection<
        ClientMessage,
        ServerMessage,
        BincodeSansCoder,
        BincodeSansCoder,
        compio::net::OwnedReadHalf<compio::net::TcpStream>,
        compio::net::OwnedWriteHalf<compio::net::TcpStream>,
    >;

    impl<Impl, RT>
        MyServiceImpl<
            Impl,
            RT,
            SplitStream<CompioTcpConnection>,
            SplitSink<CompioTcpConnection, ServerMessage>,
        >
    where
        Impl: MyService + Send + 'static,
        RT: Runtime + Send + 'static,
    {
        pub async fn serve(
            service: Impl,
            addr: impl ToSocketAddrsAsync,
            runtime: RT,
        ) -> io::Result<()>
        where
            Impl: Clone,
        {
            let listener = ::compio::net::TcpListener::bind(addr).await?;

            while let Ok((stream, _addr)) = listener.accept().await {
                println!("new connection");

                let service = service.clone();
                let _runtime = runtime.clone();
                runtime.spawn(async move || {
                    let (reader, writer) = stream.into_split();
                    let connection = ::sansrpc_compio::tcp::CompioSansConnection::new(
                        ConnectionConfig::builder::<ClientMessage, ServerMessage, _>()
                            .coder(BincodeSansCoder)
                            .build(),
                        reader,
                        writer,
                    );
                    let (writer, reader) = connection.split();
                    MyServiceImpl::new(service, _runtime, reader, writer)
                        .0
                        .run()
                        .await
                        .unwrap();
                });
            }
            Ok(())
        }
    }

    impl<Impl, RT, Reader, Writer> MyServiceImpl<Impl, RT, Reader, Writer>
    where
        Reader: Stream<Item = io::Result<Message<ClientMessage>>> + Unpin,
        Writer: Sink<Message<ServerMessage>, Error = io::Error> + Unpin,
        Impl: MyService + 'static,
        RT: Runtime + 'static,
    {
        pub fn new(
            service: Impl,
            runtime: RT,
            reader: Reader,
            writer: Writer,
        ) -> (Self, ServiceHandle<ClientMessage, ServerMessage>) {
            let (inner, handle) = SansService::new(reader, writer);
            (
                Self {
                    inner,
                    service,
                    runtime,
                },
                handle,
            )
        }

        pub async fn run(&mut self) -> io::Result<()> {
            while let Some(next) = self.inner.next_message().await? {
                match next {
                    service_message::ServiceMessage::OpenStream(open_stream) => {
                        match open_stream.data {
                            ClientMessage::Stream(stream) => match stream {
                                ClientStreamRequest::StreamStrings { values } => {
                                    let stream = self.service.stream_strings(values).await;
                                    let sender = self
                                        .inner
                                        .recv_stream(open_stream.stream_id, |str: String| {
                                            ServerMessage::StreamStringMessage(str)
                                        })
                                        .await;

                                    let stream = Box::pin(stream);

                                    self.runtime.spawn(async move || {
                                        _ = stream.map(Ok).forward(sender).await;
                                    });
                                }
                            },
                            _ => unreachable!(),
                        }
                    }
                    service_message::ServiceMessage::Request(request) => match request.data {
                        ClientMessage::Request(client_request) => match client_request {
                            ClientRequest::RequestString { my_string } => {
                                let sender = self
                                    .inner
                                    .recv_request(request.request_id, |str| {
                                        ServerMessage::RequestString(str)
                                    })
                                    .await;
                                let response = self.service.request_string(my_string).await;
                                _ = sender.send(response);
                            }
                        },
                        _ => unreachable!(),
                    },
                    service_message::ServiceMessage::Oneshot(oneshot) => match oneshot.message {
                        ClientMessage::Oneshot(client_oneshot) => match client_oneshot {
                            ClientOneshot::StringOneshot { value } => {
                                self.service.string_oneshot(value).await;
                            }
                        },
                        _ => unreachable!(),
                    },
                }
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct ExampleMyService;

    #[async_trait]
    impl MyService for ExampleMyService {
        type StreamStringsStream = UnboundedReceiver<String>;
        async fn request_string(&self, my_string: String) -> String {
            println!("[server] {my_string}");
            my_string
        }

        async fn stream_strings(&self, values: u64) -> Self::StreamStringsStream {
            let (mut tx, rx) = futures::channel::mpsc::unbounded();

            for i in 0..5 {
                _ = tx.send(format!("[server] my string {i}")).await;
            }

            rx
        }

        async fn string_oneshot(&self, value: String) {
            println!("[server] {value}");
        }
    }

    #[::compio::test]
    async fn test_my_service() {
        let (client_tx, client_rx) = futures::channel::mpsc::unbounded();
        let (mut server_tx, server_rx) = futures::channel::mpsc::unbounded();

        let (mut service, handle) = MyServiceImpl::new(
            ExampleMyService,
            Arc::new(compio::dispatcher::Dispatcher::new().unwrap()),
            server_rx,
            client_tx.sink_map_err(io::Error::other),
        );

        let fut = ::compio::runtime::spawn(async move {
            service.run().await.unwrap();
        });

        server_tx
            .send(Ok(Message::Request(sansrpc_proto::message::Request {
                request_id: 1,
                data: ClientMessage::Request(ClientRequest::RequestString {
                    my_string: "hello".to_string(),
                }),
            })))
            .await
            .unwrap();
        fut.await.unwrap();
    }

    #[::compio::test]
    async fn test_my_service_tcp() {
        let dispatcher = Arc::new(compio::dispatcher::Dispatcher::new().unwrap());
        let server_fut = {
            let dispatcher = dispatcher.clone();

            dispatcher
                .clone()
                .dispatch(|| MyServiceImpl::serve(ExampleMyService, "[::]:3400", dispatcher))
        };

        let stream = TcpStream::connect("0.0.0.0:3400").await.unwrap();

        let (reader, writer) = stream.into_split();
        let mut connection =
            ::sansrpc_compio::tcp::CompioSansConnection::<ClientMessage, _, _, _, _, _>::new(
                ConnectionConfig::builder::<ClientMessage, ServerMessage, _>()
                    .coder(BincodeSansCoder)
                    .build(),
                reader,
                writer,
            );

        connection
            .send(Message::Request(sansrpc_proto::message::Request {
                request_id: 1,
                data: ClientMessage::Request(ClientRequest::RequestString {
                    my_string: "hello".to_string(),
                }),
            }))
            .await
            .unwrap();

        server_fut.unwrap().await.unwrap().unwrap();
    }
}
