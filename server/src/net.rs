use std::net::TcpStream;

use block_byte_common::messages::{NetworkMessageC2S, NetworkMessageS2C};
use json::JsonValue;
use tungstenite::WebSocket;

pub struct PlayerConnection {
    socket: WebSocket<TcpStream>,
    closed: bool,
}
impl PlayerConnection {
    pub fn new(mut socket: WebSocket<TcpStream>) -> Result<(Self, u8), ()> {
        let mode_message = socket.read().map_err(|_| ())?;
        match mode_message {
            tungstenite::Message::Binary(message) => {
                match bitcode::deserialize::<NetworkMessageC2S>(message.as_slice()) {
                    Ok(NetworkMessageC2S::ConnectionMode(mode)) => {
                        socket.get_ref().set_nonblocking(true).map_err(|_| ())?;
                        Ok((
                            PlayerConnection {
                                socket,
                                closed: false,
                            },
                            mode,
                        ))
                    }
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }
    pub fn send_json(&mut self, json: JsonValue) {
        self.socket
            .send(tungstenite::Message::Text(json.dump()))
            .ok();
    }
    pub fn send_binary(&mut self, data: &Vec<u8>) {
        self.socket
            .send(tungstenite::Message::Binary(data.clone()))
            .ok();
    }
    pub fn send(&mut self, message: &NetworkMessageS2C) {
        if let Err(_) = self.socket.send(tungstenite::Message::Binary(
            bitcode::serialize(message).unwrap(),
        )) {
            //panic!("socket error: {}", error);
            self.closed = true;
        }
    }
    pub fn receive_messages(&mut self) -> Vec<NetworkMessageC2S> {
        let mut messages = Vec::new();
        while let Ok(message) = self.socket.read() {
            match message {
                tungstenite::Message::Binary(message) => {
                    match bitcode::deserialize::<NetworkMessageC2S>(message.as_slice()) {
                        Ok(message) => messages.push(message),
                        Err(_) => {
                            self.closed = true;
                        }
                    }
                }
                tungstenite::Message::Close(_) => {
                    self.closed = true;
                }
                _ => {}
            }
        }
        messages
    }
    pub fn is_closed(&self) -> bool {
        self.closed | !self.socket.can_write()
    }
}
