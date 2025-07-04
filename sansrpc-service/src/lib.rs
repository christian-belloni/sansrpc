pub mod handle;
pub mod service;
pub mod service_message;
pub mod service_sink;

#[cfg(test)]
#[allow(unused)]
mod tests {

    #[derive(Debug)]
    struct ServerMessage {}
    #[derive(Debug)]
    struct ClientMessage {
        pub message: String,
    }

    use std::{io, time::Duration};

    use futures::{SinkExt, StreamExt};
    use sansrpc_proto::message::{Message, Oneshot, OpenStream, Request};

    use crate::{service::SansService, service_message::ServiceMessage};

    #[compio::test]
    async fn in_memory_test() {
        let (mut server_tx, server_rx) =
            futures::channel::mpsc::unbounded::<Result<Message<ServerMessage>, io::Error>>();
        let (client_tx, mut client_rx) =
            futures::channel::mpsc::unbounded::<Message<ClientMessage>>();

        let (mut service, mut handle) =
            SansService::new(server_rx, client_tx.sink_map_err(io::Error::other));

        let recv_fut = compio::runtime::spawn(async move {
            while let Ok(Some(next)) = service.next_message().await {
                println!("server recvd {next:?}");
                match next {
                    ServiceMessage::OpenStream(stream_request) => {
                        let mut stream = service
                            .recv_stream(stream_request.stream_id, move |req: String| {
                                ClientMessage { message: req }
                            })
                            .await;

                        compio::runtime::spawn(async move {
                            for i in 0..3 {
                                stream.send(format!("{i}")).await.unwrap();
                            }
                        })
                        .await
                        .unwrap();
                    }
                    ServiceMessage::Request(request) => {
                        service
                            .recv_request(request.request_id, |str| ClientMessage { message: str })
                            .await
                            .send("response".to_string())
                            .unwrap();
                    }
                    ServiceMessage::Oneshot(oneshot) => {}
                }
            }
        });

        let client_fut = compio::runtime::spawn(async move {
            while let Some(next) = client_rx.next().await {
                println!("client recvd {next:?}");
            }
        });

        handle
            .send_oneshot(Oneshot {
                message: ClientMessage {
                    message: "my message".to_string(),
                },
            })
            .await;

        server_tx
            .unbounded_send(Ok(Message::Request(Request {
                request_id: 2,
                data: ServerMessage {},
            })))
            .unwrap();

        server_tx
            .send(Ok(Message::OpenStream(OpenStream {
                stream_id: 1,
                data: ServerMessage {},
            })))
            .await
            .unwrap();

        compio::time::sleep(Duration::from_millis(400)).await;

        drop(server_tx);
        drop(handle);

        futures::try_join!(recv_fut, client_fut).unwrap();
    }
}
