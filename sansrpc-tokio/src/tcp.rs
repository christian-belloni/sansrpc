use std::io::ErrorKind;
use std::time::Duration;
use std::{io, pin::Pin, task::Poll};

use futures::ready;
use futures::{FutureExt, Sink, Stream};
use sansrpc_proto::connection_state::ConnectionConfig;
use sansrpc_proto::connection_state::ConnectionState;
use sansrpc_proto::encoder::{SansDecoder, SansEncoder};
use sansrpc_proto::message::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub struct TokioSansConnection<In, Out, Encoder, Decoder, Reader, Writer> {
    state: ConnectionState<In, Out, Encoder, Decoder>,
    read_state: ReadState<Reader>,
    write_state: WriteState<Writer>,
}

impl<In, Out, Encoder, Decoder, Reader, Writer>
    TokioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
{
    pub fn new(config: ConnectionConfig<Encoder, Decoder>, reader: Reader, writer: Writer) -> Self {
        Self {
            read_state: ReadState::Idle(Some((
                reader,
                vec![0; config.read_config.recv_buffer_size.get() * 2],
                0,
            ))),

            state: ConnectionState::new(config),

            write_state: WriteState::Idle(Some(writer)),
        }
    }
}

type PinBox<T> = Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>>;

type ReadInner<R> = (R, Vec<u8>, usize);

enum ReadState<R> {
    Idle(Option<ReadInner<R>>),
    Reading(PinBox<Result<ReadInner<R>, io::Error>>),
}

enum WriteState<W> {
    Idle(Option<W>),
    Writing(PinBox<Result<W, io::Error>>),
    Closing(PinBox<Result<W, io::Error>>),
}

impl<Writer> WriteState<Writer> {
    fn is_closed(&self) -> bool {
        matches!(self, Self::Closing(_))
    }
}

impl<In, Out, Encoder, Decoder, Reader, Writer> Stream
    for TokioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
where
    Decoder: SansDecoder<In>,
    Self: Unpin,
    Reader: AsyncRead + Unpin + 'static + Send + Sync,
    Decoder::Error: std::error::Error,
{
    type Item = Result<Message<In>, io::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            this.state
                .process_read()
                .map_err(|_| io::ErrorKind::InvalidInput)?;

            if let Some(item) = this.state.consume_item() {
                return Poll::Ready(Some(Ok(item)));
            }

            match &mut this.read_state {
                ReadState::Idle(idle) => {
                    let (mut reader, mut buf, read) = idle.take().unwrap();

                    if read != 0 {
                        let fed = this.state.feed_bytes(&buf[..read]);

                        if fed != buf.len() {
                            buf.copy_within(fed.., 0);

                            let new_read = read - fed;
                            std::thread::sleep(Duration::from_millis(100));

                            this.read_state = ReadState::Idle(Some((reader, buf, new_read)));
                            continue;
                        }
                    }

                    let fut = Box::pin(async move {
                        let read = reader.read(&mut buf).await?;
                        Ok((reader, buf, read))
                    });
                    this.read_state = ReadState::Reading(fut)
                }

                ReadState::Reading(pin) => {
                    let (reader, buf, read) = futures::ready!(pin.poll_unpin(cx))?;
                    if read == 0 {
                        return Poll::Ready(None);
                    }
                    this.read_state = ReadState::Idle(Some((reader, buf, read)));
                }
            }
        }
    }
}

impl<In, Out, Encoder, Decoder, Reader, Writer> Sink<Message<Out>>
    for TokioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
where
    Encoder: SansEncoder<Out>,
    Self: Unpin,
    Writer: AsyncWrite + 'static + Unpin + Send + Sync,
    Encoder::Error: std::error::Error,
{
    type Error = io::Error;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        match &mut this.write_state {
            WriteState::Idle(_) => Poll::Ready(Ok(())),
            WriteState::Writing(pin) => {
                let result = ready!(pin.poll_unpin(cx))?;
                this.write_state = WriteState::Idle(Some(result));
                Poll::Ready(Ok(()))
            }
            _ => Poll::Pending,
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<Out>) -> Result<(), Self::Error> {
        if self.write_state.is_closed() {
            return Err(ErrorKind::ConnectionReset.into());
        }

        let this = self.get_mut();
        this.state.feed_item(item);
        this.state
            .process_write()
            .map_err(|_| io::ErrorKind::InvalidInput.into())
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();

        loop {
            match &mut this.write_state {
                WriteState::Idle(idle) => {
                    let mut writer = idle.take().unwrap();
                    let Some(bytes) = this.state.consume_all() else {
                        this.write_state = WriteState::Idle(Some(writer));
                        return Poll::Ready(Ok(()));
                    };

                    let fut = Box::pin(async move {
                        writer.write_all(&bytes).await?;

                        Ok(writer)
                    });

                    this.write_state = WriteState::Writing(fut);
                }
                WriteState::Writing(pin) => {
                    let writer = ready!(pin.poll_unpin(cx))?;
                    this.write_state = WriteState::Idle(Some(writer));
                }

                _ => return Poll::Ready(Err(ErrorKind::ConnectionReset.into())),
            }
        }
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();

        loop {
            match &mut this.write_state {
                WriteState::Idle(idle) => {
                    let mut writer = idle.take().unwrap();
                    let Some(bytes) = this.state.consume_all() else {
                        let fut = Box::pin(async move {
                            writer.flush().await?;
                            writer.shutdown().await?;
                            Ok(writer)
                        });
                        this.write_state = WriteState::Closing(fut);

                        continue;
                    };

                    let fut = Box::pin(async move {
                        writer.write_all(&bytes).await?;

                        Ok(writer)
                    });

                    this.write_state = WriteState::Writing(fut);
                }
                WriteState::Writing(pin) => {
                    let writer = ready!(pin.poll_unpin(cx))?;
                    this.write_state = WriteState::Idle(Some(writer));
                }

                WriteState::Closing(pin) => {
                    ready!(pin.poll_unpin(cx))?;
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZero,
        time::{Duration, Instant},
    };

    use futures::{SinkExt, TryStreamExt};
    use sansrpc_proto::{
        bincode::BincodeSansCoder,
        connection_state::ConnectionConfig,
        message::{Message, Oneshot},
    };
    use serde::{Deserialize, Serialize};
    use tokio::net::{TcpListener, TcpStream};

    #[derive(Debug, Deserialize, Serialize)]
    struct MyInStruct {
        pub name: String,
        pub number: u64,
        pub description: String,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct MyOutStruct {
        pub name: String,
        pub number: u64,
    }

    use super::*;
    #[tokio::test]
    async fn test_tokio_tcp() {
        const COUNT: u64 = 1_000_000;
        let listener = TcpListener::bind("[::]:3400").await.unwrap();
        let server_future = tokio::spawn(async move {
            println!("waiting for connection");
            if let Ok((stream, _)) = listener.accept().await {
                println!("connected");
                let config = ConnectionConfig::builder::<MyOutStruct, MyInStruct, _>()
                    .coder(BincodeSansCoder)
                    .recv_buffer_size(NonZero::new(1024 * 128).unwrap())
                    .build();
                let (reader, writer) = stream.into_split();
                let mut connection =
                    TokioSansConnection::<MyOutStruct, MyInStruct, _, _, _, _>::new(
                        config, reader, writer,
                    );

                let mut count = 0;
                while connection.try_next().await.unwrap().is_some() {
                    count += 1;
                }
                assert_eq!(count, COUNT);
            }
        });

        let now = Instant::now();

        let client_future = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let stream = TcpStream::connect("127.0.0.1:3400").await.unwrap();

            let config = ConnectionConfig::builder::<MyInStruct, MyOutStruct, _>()
                .coder(BincodeSansCoder)
                .build();
            let (reader, writer) = stream.into_split();
            let mut connection = TokioSansConnection::<MyInStruct, MyOutStruct, _, _, _, _>::new(
                config, reader, writer,
            );

            for i in 0..COUNT {
                connection
                    .feed(Message::Oneshot(Oneshot {
                        message: MyOutStruct {
                            name: "name".to_string(),
                            number: i,
                        },
                    }))
                    .await
                    .unwrap();
            }

            connection.flush().await.unwrap();
            connection.close().await.unwrap();
        });

        futures::try_join!(server_future, client_future).unwrap();

        let elapsed = now.elapsed();
        let msg_per_sec = COUNT / elapsed.as_millis() as u64;
        let elapsed = elapsed.as_millis();

        println!(
            "sent {} msgs in {} ms {} msg/ms",
            COUNT, elapsed, msg_per_sec
        );
    }
}
