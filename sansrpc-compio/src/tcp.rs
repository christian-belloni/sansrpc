use std::{io, marker::PhantomData};

use compio::{
    io::util::split::{ReadHalf, WriteHalf},
    net::{OwnedReadHalf, OwnedWriteHalf, TcpListener, TcpStream, ToSocketAddrsAsync},
};
use sansrpc_proto::connection_state::ConnectionConfig;

use crate::CompioSansConnection;

impl<In, Out, Encoder, Decoder>
    CompioSansConnection<
        In,
        Out,
        Encoder,
        Decoder,
        OwnedReadHalf<TcpStream>,
        OwnedWriteHalf<TcpStream>,
    >
{
    pub async fn tcp_connect(
        config: ConnectionConfig<Encoder, Decoder>,
        addr: impl ToSocketAddrsAsync,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self::new_tcp(config, stream))
    }

    pub fn new_tcp(config: ConnectionConfig<Encoder, Decoder>, stream: TcpStream) -> Self {
        let (writer, reader) = stream.into_split();

        CompioSansConnection::<In, Out, _, _, _, _>::new(config, writer, reader)
    }
}

pub struct CompioSansListener<In, Out, Encoder, Decoder> {
    config: ConnectionConfig<Encoder, Decoder>,

    _marker: PhantomData<(In, Out, Encoder, Decoder)>,
    listener: TcpListener,
}

impl<In, Out, Encoder, Decoder> CompioSansListener<In, Out, Encoder, Decoder> {
    pub async fn bind(
        config: ConnectionConfig<Encoder, Decoder>,
        addr: impl ToSocketAddrsAsync,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            config,
            listener,
            _marker: PhantomData,
        })
    }
}

impl<In, Out, Encoder, Decoder> CompioSansListener<In, Out, Encoder, Decoder>
where
    ConnectionConfig<Encoder, Decoder>: Clone,
{
    pub async fn accept(
        &mut self,
    ) -> io::Result<
        CompioSansConnection<
            In,
            Out,
            Encoder,
            Decoder,
            OwnedReadHalf<TcpStream>,
            OwnedWriteHalf<TcpStream>,
        >,
    > {
        let (stream, _addr) = self.listener.accept().await?;

        Ok(CompioSansConnection::new_tcp(self.config.clone(), stream))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZero,
        time::{Duration, Instant},
    };

    use compio::net::{TcpListener, TcpStream};
    use futures::{SinkExt, TryStreamExt};
    use sansrpc_proto::{
        bincode::BincodeSansCoder,
        connection_state::ConnectionConfig,
        message::{Message, Oneshot},
    };
    use serde::{Deserialize, Serialize};

    use crate::CompioSansConnection;

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

    #[compio::test]
    async fn test_compio_tcp() {
        const COUNT: u64 = 1_000_000;
        let listener = TcpListener::bind("[::]:3400").await.unwrap();
        let server_future = compio::runtime::spawn(async move {
            println!("waiting for connection");
            if let Ok((stream, _)) = listener.accept().await {
                println!("connected");
                let config = ConnectionConfig::builder::<MyInStruct, MyOutStruct, _>()
                    .coder(BincodeSansCoder)
                    .recv_queue_size(1.try_into().unwrap())
                    .recv_buffer_size(NonZero::new(1024 * 128).unwrap())
                    .send_buffer_size(NonZero::new(1024 * 128).unwrap())
                    .build();
                let (reader, writer) = stream.into_split();
                let mut connection =
                    CompioSansConnection::<MyOutStruct, MyInStruct, _, _, _, _>::new(
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
        let client_future = compio::runtime::spawn(async move {
            compio::time::sleep(Duration::from_millis(20)).await;
            let stream = TcpStream::connect("127.0.0.1:3400").await.unwrap();

            let config = ConnectionConfig::builder::<MyOutStruct, MyInStruct, _>()
                .coder(BincodeSansCoder)
                .build();
            let (reader, writer) = stream.into_split();
            let mut connection = CompioSansConnection::<MyInStruct, MyOutStruct, _, _, _, _>::new(
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
            compio::time::sleep(Duration::from_millis(200)).await;

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
