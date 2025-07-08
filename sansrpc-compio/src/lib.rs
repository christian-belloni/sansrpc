pub mod tcp;

use compio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use futures::{FutureExt, Sink, Stream, ready};
use sansrpc_proto::{
    connection_state::{
        ConnectionConfig, ConnectionReadState, ConnectionState, ConnectionWriteState,
    },
    encoder::{SansDecoder, SansEncoder},
    message::Message,
};
use std::{
    io::{self, ErrorKind},
    pin::Pin,
    task::Poll,
};

pub struct CompioSansConnection<In, Out, Encoder, Decoder, Reader, Writer> {
    connection_state: ConnectionState<In, Out, Encoder, Decoder>,

    read_state: ReadState<Reader>,
    write_state: WriteState<Writer>,
}

impl<In, Out, Encoder, Decoder, Reader, Writer>
    CompioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
{
    pub fn new(config: ConnectionConfig<Encoder, Decoder>, reader: Reader, writer: Writer) -> Self {
        CompioSansConnection {
            read_state: ReadState::Idle(Some((
                reader,
                Vec::with_capacity(config.read_config.recv_buffer_size.get()),
            ))),

            connection_state: ConnectionState::new(config),

            write_state: WriteState::Idle(Some(writer)),
        }
    }

    pub fn into_split(
        self,
    ) -> (
        CompioSansWriteConnection<Out, Encoder, Writer>,
        CompioSansReadConnection<In, Decoder, Reader>,
    ) {
        let (w_conf, r_conf) = self.connection_state.into_split();

        (
            CompioSansWriteConnection {
                connection_state: w_conf,
                write_state: self.write_state,
            },
            CompioSansReadConnection {
                connection_state: r_conf,
                read_state: self.read_state,
            },
        )
    }
}

pub struct CompioSansReadConnection<In, Decoder, Reader> {
    connection_state: ConnectionReadState<In, Decoder>,

    read_state: ReadState<Reader>,
}

type PinBox<T> = Pin<Box<dyn Future<Output = T>>>;

enum ReadState<Reader> {
    Idle(Option<(Reader, Vec<u8>)>),
    Reading(PinBox<Result<(Reader, Vec<u8>), io::Error>>),
}

// Safe as long WriteState is not sent after being polled
unsafe impl<Reader: Send> Send for ReadState<Reader> {}

impl<In, Decoder, Reader> Stream for CompioSansReadConnection<In, Decoder, Reader>
where
    Decoder: SansDecoder<In>,
    Self: Unpin,
    Reader: AsyncRead + 'static,
    Decoder::Error: std::error::Error,
{
    type Item = Result<Message<In>, io::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            this.connection_state
                .process()
                .map_err(|_| io::ErrorKind::InvalidInput)?;

            if let Some(item) = this.connection_state.consume_item() {
                return Poll::Ready(Some(Ok(item)));
            }

            match &mut this.read_state {
                ReadState::Idle(idle) => {
                    let (mut reader, mut buf) = idle.take().unwrap();

                    if !buf.is_empty() {
                        let fed = this.connection_state.feed_bytes(&buf);

                        if fed != buf.len() {
                            let remaining = buf[..fed].to_vec();
                            buf.clear();
                            buf.extend_from_slice(&remaining);
                            this.read_state = ReadState::Idle(Some((reader, buf)));
                            continue;
                        }
                        buf.clear();
                    }

                    let fut = Box::pin(async move {
                        let res = reader.read(buf).await;
                        match res.0 {
                            Ok(_) => Ok((reader, res.1)),
                            Err(error) => Err(error),
                        }
                    });
                    this.read_state = ReadState::Reading(fut)
                }

                ReadState::Reading(pin) => {
                    let (reader, buf) = ready!(pin.poll_unpin(cx))?;
                    let is_empty = buf.is_empty();
                    this.read_state = ReadState::Idle(Some((reader, buf)));
                    if is_empty {
                        if let Some(item) = this.connection_state.consume_item() {
                            return Poll::Ready(Some(Ok(item)));
                        } else {
                            return Poll::Ready(None);
                        }
                    }
                }
            }
        }
    }
}

impl<In, Out, Encoder, Decoder, Reader, Writer> Stream
    for CompioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
where
    Decoder: SansDecoder<In>,
    Self: Unpin,
    Reader: AsyncRead + 'static,
    Decoder::Error: std::error::Error,
{
    type Item = Result<Message<In>, io::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            this.connection_state
                .process_read()
                .map_err(|_| io::ErrorKind::InvalidInput)?;

            if let Some(item) = this.connection_state.consume_item() {
                return Poll::Ready(Some(Ok(item)));
            }

            match &mut this.read_state {
                ReadState::Idle(idle) => {
                    let (mut reader, mut buf) = idle.take().unwrap();

                    if !buf.is_empty() {
                        let fed = this.connection_state.feed_bytes(&buf);

                        if fed != buf.len() {
                            let remaining = buf[..fed].to_vec();
                            buf.clear();
                            buf.extend_from_slice(&remaining);
                            this.read_state = ReadState::Idle(Some((reader, buf)));
                            continue;
                        }
                        buf.clear();
                    }

                    let fut = Box::pin(async move {
                        let res = reader.read(buf).await;
                        match res.0 {
                            Ok(_) => Ok((reader, res.1)),
                            Err(error) => Err(error),
                        }
                    });
                    this.read_state = ReadState::Reading(fut)
                }

                ReadState::Reading(pin) => {
                    let (reader, buf) = ready!(pin.poll_unpin(cx))?;
                    let is_empty = buf.is_empty();
                    this.read_state = ReadState::Idle(Some((reader, buf)));
                    if is_empty {
                        if let Some(item) = this.connection_state.consume_item() {
                            return Poll::Ready(Some(Ok(item)));
                        } else {
                            return Poll::Ready(None);
                        }
                    }
                }
            }
        }
    }
}

pub struct CompioSansWriteConnection<Out, Encoder, Writer> {
    connection_state: ConnectionWriteState<Out, Encoder>,

    write_state: WriteState<Writer>,
}

enum WriteState<Writer> {
    Idle(Option<Writer>),
    Writing(PinBox<Result<Writer, io::Error>>),
    Closing(PinBox<Result<Writer, io::Error>>),
}

// Safe as long WriteState is not sent after being polled
unsafe impl<Writer: Send> Send for WriteState<Writer> {}

impl<Writer> WriteState<Writer> {
    fn is_closed(&self) -> bool {
        matches!(self, Self::Closing(_))
    }
}

impl<Out, Encoder, Writer> Sink<Message<Out>> for CompioSansWriteConnection<Out, Encoder, Writer>
where
    Encoder: SansEncoder<Out>,
    Self: Unpin,
    Writer: AsyncWrite + 'static,
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
            _ => Poll::Ready(Ok(())),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<Out>) -> Result<(), Self::Error> {
        if self.write_state.is_closed() {
            return Err(ErrorKind::ConnectionReset.into());
        }

        let this = self.get_mut();
        if this.connection_state.feed_item(item).is_some() {
            return Err(ErrorKind::WouldBlock.into());
        }
        this.connection_state
            .process()
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
                    if !this.connection_state.write_queue_empty() {
                        this.connection_state
                            .process()
                            .map_err(|_| Into::<io::Error>::into(ErrorKind::InvalidData))?;
                    }
                    let Some(bytes) = this.connection_state.consume_all() else {
                        this.write_state = WriteState::Idle(Some(writer));
                        return Poll::Ready(Ok(()));
                    };

                    let fut = Box::pin(async move {
                        let res = writer.write_all(bytes).await.0;

                        res.map(|_| writer)
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
                    let Some(bytes) = this.connection_state.consume_all() else {
                        let fut = Box::pin(async move {
                            writer.flush().await?;
                            writer.shutdown().await?;
                            Ok(writer)
                        });
                        this.write_state = WriteState::Closing(fut);

                        continue;
                    };

                    let fut = Box::pin(async move {
                        let res = writer.write_all(bytes).await.0;

                        res.map(|_| writer)
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

impl<In, Out, Encoder, Decoder, Reader, Writer> Sink<Message<Out>>
    for CompioSansConnection<In, Out, Encoder, Decoder, Reader, Writer>
where
    Encoder: SansEncoder<Out>,
    Self: Unpin,
    Writer: AsyncWrite + 'static,
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
            _ => Poll::Ready(Ok(())),
        }
    }

    fn start_send(self: Pin<&mut Self>, item: Message<Out>) -> Result<(), Self::Error> {
        if self.write_state.is_closed() {
            return Err(ErrorKind::ConnectionReset.into());
        }

        let this = self.get_mut();
        if this.connection_state.feed_item(item).is_some() {
            return Err(ErrorKind::WouldBlock.into());
        }
        this.connection_state
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
                    if !this.connection_state.write_queue_empty() {
                        this.connection_state
                            .process_write()
                            .map_err(|_| Into::<io::Error>::into(ErrorKind::InvalidData))?;
                    }
                    let Some(bytes) = this.connection_state.consume_all() else {
                        this.write_state = WriteState::Idle(Some(writer));
                        return Poll::Ready(Ok(()));
                    };

                    let fut = Box::pin(async move {
                        let res = writer.write_all(bytes).await.0;

                        res.map(|_| writer)
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
                    let Some(bytes) = this.connection_state.consume_all() else {
                        let fut = Box::pin(async move {
                            writer.flush().await?;
                            writer.shutdown().await?;
                            Ok(writer)
                        });
                        this.write_state = WriteState::Closing(fut);

                        continue;
                    };

                    let fut = Box::pin(async move {
                        let res = writer.write_all(bytes).await.0;

                        res.map(|_| writer)
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
