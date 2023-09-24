use block_byte_common::messages::{NetworkMessageC2S, NetworkMessageS2C};
use std::net::TcpStream;
use tungstenite::{Message, WebSocket};
use url::Url;

pub struct SocketConnection {
    socket: WebSocket<TcpStream>,
}
impl SocketConnection {
    pub fn new(address: &str) -> Self {
        let tcp_stream = std::net::TcpStream::connect(address).unwrap();
        let (mut socket, _response) = tungstenite::client::client_with_config(
            Url::parse("ws://aaa123").unwrap(),
            tcp_stream,
            None,
        )
        .unwrap();
        let mut connection = SocketConnection { socket };
        connection.send_message(&NetworkMessageC2S::ConnectionMode(0));
        connection.socket.get_mut().set_nonblocking(true).unwrap();
        connection
    }
    pub fn send_message(&mut self, message: &NetworkMessageC2S) {
        self.socket
            .send(Message::Binary(bitcode::serialize(message).unwrap()))
            .unwrap();
    }
    pub fn read_messages(&mut self) -> Vec<NetworkMessageS2C> {
        let mut messages = Vec::new();
        while let Ok(message) = self.socket.read() {
            match message {
                Message::Binary(data) => messages
                    .push(bitcode::deserialize::<NetworkMessageS2C>(data.as_slice()).unwrap()),
                Message::Close(_) => panic!("close"),
                _ => {}
            }
        }
        messages
    }
}
