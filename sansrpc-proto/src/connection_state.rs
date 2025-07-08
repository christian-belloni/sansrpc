use crate::{
    encoder::{IntoSplitCoder, SansDecoder, SansEncoder},
    message::Message,
};
use bytes::{Bytes, BytesMut};
use std::{collections::VecDeque, num::NonZeroUsize};

pub struct ConnectionState<In, Out, Encoder, Decoder> {
    read: ConnectionReadState<In, Decoder>,
    write: ConnectionWriteState<Out, Encoder>,
}

impl<In, Out, Encoder, Decoder> ConnectionState<In, Out, Encoder, Decoder> {
    pub fn new(config: ConnectionConfig<Encoder, Decoder>) -> Self {
        let (read, write) = config.split();

        Self {
            read: ConnectionReadState::new(read),
            write: ConnectionWriteState::new(write),
        }
    }

    pub fn into_split(
        self,
    ) -> (
        ConnectionWriteState<Out, Encoder>,
        ConnectionReadState<In, Decoder>,
    ) {
        (self.write, self.read)
    }
}

pub struct ConnectionReadState<In, Decoder> {
    recv_queue: VecDeque<Message<In>>,
    recv_buffer: BytesMut,
    config: ReadConnectionConfig<Decoder>,
}

impl<In, Decoder> ConnectionReadState<In, Decoder> {
    pub fn new(config: ReadConnectionConfig<Decoder>) -> Self {
        Self {
            recv_queue: VecDeque::with_capacity(config.recv_queue_size.get()),
            recv_buffer: BytesMut::with_capacity(config.recv_buffer_size.get()),
            config,
        }
    }
}

pub struct ConnectionWriteState<Out, Encoder> {
    send_queue: VecDeque<Message<Out>>,
    send_buffer: BytesMut,
    config: WriteConnectionConfig<Encoder>,
}

impl<Out, Encoder> ConnectionWriteState<Out, Encoder> {
    pub fn new(config: WriteConnectionConfig<Encoder>) -> Self {
        Self {
            send_queue: VecDeque::with_capacity(config.send_queue_size.get()),
            send_buffer: BytesMut::with_capacity(config.send_buffer_size.get()),
            config,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ConnectionConfig<Encoder, Decoder> {
    pub read_config: ReadConnectionConfig<Decoder>,
    pub write_config: WriteConnectionConfig<Encoder>,
}

#[bon::bon]
impl<Encoder, Decoder> ConnectionConfig<Encoder, Decoder> {
    #[builder]
    pub fn new<In, Out>(
        #[builder(default = 10.try_into().unwrap())] recv_queue_size: NonZeroUsize,
        #[builder(default = 4096.try_into().unwrap())] recv_buffer_size: NonZeroUsize,
        #[builder(default = 10.try_into().unwrap())] send_queue_size: NonZeroUsize,
        #[builder(default = 4096.try_into().unwrap())] send_buffer_size: NonZeroUsize,
        coder: impl IntoSplitCoder<In, Out, Encoder = Encoder, Decoder = Decoder>,
    ) -> ConnectionConfig<Encoder, Decoder> {
        let (read, write) = coder.into_split();

        ConnectionConfig {
            read_config: ReadConnectionConfig {
                recv_queue_size,
                recv_buffer_size,
                coder: read,
            },
            write_config: WriteConnectionConfig {
                send_queue_size,
                send_buffer_size,
                coder: write,
            },
        }
    }

    pub fn split(
        self,
    ) -> (
        ReadConnectionConfig<Decoder>,
        WriteConnectionConfig<Encoder>,
    ) {
        let ConnectionConfig {
            read_config,
            write_config,
        } = self;
        (read_config, write_config)
    }
}

#[derive(Clone, Copy)]
pub struct ReadConnectionConfig<Decoder> {
    pub recv_queue_size: NonZeroUsize,
    pub recv_buffer_size: NonZeroUsize,
    coder: Decoder,
}

#[derive(Clone, Copy)]
pub struct WriteConnectionConfig<Encoder> {
    pub send_queue_size: NonZeroUsize,
    pub send_buffer_size: NonZeroUsize,
    coder: Encoder,
}

impl<In, Out, Encoder, Decoder> ConnectionState<In, Out, Encoder, Decoder>
where
    Encoder: SansEncoder<Out>,
{
    /// Available sender space counted in items
    pub fn available_space(&self) -> usize {
        self.write.available_space()
    }

    /// Consume up to [`len`] bytes from the send buffer
    pub fn consume_bytes(&mut self, len: usize) -> Option<Bytes> {
        self.write.consume_bytes(len)
    }

    /// Consume up the bytes from the send buffer
    pub fn consume_all(&mut self) -> Option<Bytes> {
        self.write.consume_all()
    }

    pub fn write_queue_empty(&self) -> bool {
        self.write.write_queue_empty()
    }

    /// Called by the writer to send outgoing items
    pub fn feed_item(&mut self, item: Message<Out>) -> Option<Message<Out>> {
        self.write.feed_item(item)
    }

    pub fn process_write(&mut self) -> Result<(), Encoder::Error> {
        self.write.process()
    }
}

impl<In, Out, Encoder, Decoder> ConnectionState<In, Out, Encoder, Decoder>
where
    Decoder: SansDecoder<In>,
{
    /// Pop one item from the recv queue
    pub fn consume_item(&mut self) -> Option<Message<In>> {
        self.read.consume_item()
    }

    /// Called by the reader to process incoming items
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> usize {
        self.read.feed_bytes(bytes)
    }

    /// Process function to advance the current state
    pub fn process_read(&mut self) -> Result<(), Decoder::Error> {
        self.read.process()
    }
}

impl<In, Decoder> ConnectionReadState<In, Decoder>
where
    Decoder: SansDecoder<In>,
{
    /// Pop one item from the recv queue
    pub fn consume_item(&mut self) -> Option<Message<In>> {
        self.recv_queue.pop_front()
    }

    /// Called by the reader to process incoming items
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> usize {
        if self.recv_buffer.len() + bytes.len() >= self.config.recv_buffer_size.get() {
            if self.recv_buffer.len() >= self.config.recv_buffer_size.get() {
                0
            } else {
                self.recv_buffer.extend_from_slice(bytes);
                bytes.len()
            }
        } else {
            self.recv_buffer.extend_from_slice(bytes);
            bytes.len()
        }
    }

    /// Process function to advance the current state
    pub fn process(&mut self) -> Result<(), Decoder::Error> {
        loop {
            if self.recv_queue.len() >= self.config.recv_queue_size.get() {
                return Ok(());
            }

            if self.recv_buffer.len() < 8 {
                return Ok(());
            }

            let next_len = u64::from_le_bytes(self.recv_buffer[0..8].try_into().unwrap()) as usize;

            if self.recv_buffer.len() < 8 + next_len {
                return Ok(());
            }

            let items_buf = &self.recv_buffer[8..(8 + next_len)];

            let next_item = self.config.coder.decode(items_buf)?;

            self.recv_queue.push_back(next_item);

            _ = self.recv_buffer.split_to(8 + next_len);
        }
    }
}

impl<Out, Encoder> ConnectionWriteState<Out, Encoder>
where
    Encoder: SansEncoder<Out>,
{
    /// Available sender space counted in items
    pub fn available_space(&self) -> usize {
        self.config.send_queue_size.get()
            - self.send_queue.len().max(self.config.send_queue_size.get())
    }

    /// Consume up to [`len`] bytes from the send buffer
    pub fn consume_bytes(&mut self, len: usize) -> Option<Bytes> {
        if self.send_buffer.is_empty() {
            return None;
        }

        if self.send_buffer.len() < len {
            let to_send = std::mem::replace(
                &mut self.send_buffer,
                BytesMut::with_capacity(self.config.send_buffer_size.get()),
            );
            Some(to_send.freeze())
        } else {
            let to_send = self.send_buffer.split_to(len);
            Some(to_send.freeze())
        }
    }

    /// Consume all the bytes from the send buffer
    pub fn consume_all(&mut self) -> Option<Bytes> {
        if self.send_buffer.is_empty() {
            return None;
        }

        let to_send = std::mem::replace(
            &mut self.send_buffer,
            BytesMut::with_capacity(self.config.send_buffer_size.get()),
        );
        Some(to_send.freeze())
    }

    /// Called by the writer to send outgoing items
    pub fn feed_item(&mut self, item: Message<Out>) -> Option<Message<Out>> {
        if self.send_queue.len() >= self.config.send_queue_size.get() {
            Some(item)
        } else {
            self.send_queue.push_back(item);
            None
        }
    }

    pub fn write_queue_empty(&self) -> bool {
        self.send_queue.is_empty()
    }

    /// Process function to advance the current state
    pub fn process(&mut self) -> Result<(), Encoder::Error> {
        // static SENT: AtomicUsize = AtomicUsize::new(0);

        while let Some(to_send) = self.send_queue.pop_front() {
            let (bytes, len) = self.config.coder.encode(&to_send)?;

            self.send_buffer.extend_from_slice(&len.to_le_bytes());
            self.send_buffer.extend_from_slice(&bytes);
        }

        Ok(())
    }
}
