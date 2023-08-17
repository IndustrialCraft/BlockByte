use std::fmt::Binary;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};

use endio::LERead;
use endio::LEWrite;
use json::JsonValue;
use tungstenite::WebSocket;

use crate::util::BlockPosition;
use crate::util::Face;

impl NetworkMessageS2C {
    pub fn to_data(&self) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        match self {
            Self::SetBlock(x, y, z, id) => {
                data.write_be(0u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*id).unwrap();
            }
            Self::LoadChunk(x, y, z, blocks) => {
                data.write_be(1u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(blocks.len() as u32).unwrap();
                for byte in blocks {
                    data.write_be(*byte).unwrap();
                }
            }
            Self::UnloadChunk(x, y, z) => {
                data.write_be(2u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
            }
            Self::AddEntity(
                entity_type,
                id,
                x,
                y,
                z,
                rotation,
                animation,
                animation_start_time,
            ) => {
                data.write_be(3u8).unwrap();
                data.write_be(*entity_type).unwrap();
                data.write_be(*id).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*rotation).unwrap();
                data.write_be(*animation).unwrap();
                data.write_be(*animation_start_time).unwrap();
            }
            Self::MoveEntity(id, x, y, z, rotation) => {
                data.write_be(4u8).unwrap();
                data.write_be(*id).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*rotation).unwrap();
            }
            Self::DeleteEntity(id) => {
                data.write_be(5u8).unwrap();
                data.write_be(*id).unwrap();
            }
            Self::GuiData(json) => {
                data.write_be(6u8).unwrap();
                write_string(&mut data, &json.dump());
            }
            Self::BlockBreakTimeResponse(id, time) => {
                data.write_be(7u8).unwrap();
                data.write_be(*id).unwrap();
                data.write_be(*time).unwrap();
            }
            Self::EntityAddItem(id, item_index, item_id) => {
                data.write_be(8u8).unwrap();
                data.write_be(*id).unwrap();
                data.write_be(*item_index).unwrap();
                data.write_be(*item_id).unwrap();
            }
            Self::BlockAddItem(x, y, z, item_index, item_id) => {
                data.write_be(9u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*item_index).unwrap();
                data.write_be(*item_id).unwrap();
            }
            Self::BlockRemoveItem(x, y, z, item_index) => {
                data.write_be(10u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*item_index).unwrap();
            }
            Self::Knockback(x, y, z, set) => {
                data.write_be(12u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*set).unwrap();
            }
            Self::FluidSelectable(selectable) => {
                data.write_be(13u8).unwrap();
                data.write_be(*selectable).unwrap();
            }
            Self::PlaySound(id, x, y, z, gain, pitch, relative) => {
                data.write_be(14u8).unwrap();
                write_string(&mut data, id);
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*gain).unwrap();
                data.write_be(*pitch).unwrap();
                data.write_be(*relative).unwrap();
            }
            Self::EntityAnimation(id, animation) => {
                data.write_be(15u8).unwrap();
                data.write_be(*id).unwrap();
                data.write_be(*animation).unwrap();
            }
            Self::ChatMessage(message) => {
                data.write_be(16u8).unwrap();
                write_string(&mut data, message);
            }
            Self::PlayerAbilities(speed, move_type) => {
                data.write_be(17u8).unwrap();
                data.write_be(*speed).unwrap();
                data.write_be(*move_type as u8).unwrap();
            }
            Self::TeleportPlayer(x, y, z) => {
                data.write_be(18u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
            }
            Self::BlockAnimation(x, y, z, animation) => {
                data.write_be(19u8).unwrap();
                data.write_be(*x).unwrap();
                data.write_be(*y).unwrap();
                data.write_be(*z).unwrap();
                data.write_be(*animation).unwrap();
            }
        };
        data
    }
}
#[repr(u8)]
pub enum NetworkMessageS2C {
    SetBlock(i32, i32, i32, u32) = 0,
    LoadChunk(i32, i32, i32, Vec<u8>) = 1,
    UnloadChunk(i32, i32, i32) = 2,
    AddEntity(u32, u32, f32, f32, f32, f32, u32, f32) = 3,
    MoveEntity(u32, f32, f32, f32, f32) = 4,
    DeleteEntity(u32) = 5,
    GuiData(json::JsonValue) = 6,
    BlockBreakTimeResponse(u32, f32) = 7,
    EntityAddItem(u32, u32, u32) = 8,
    BlockAddItem(i32, i32, i32, u32, u32) = 9,
    BlockRemoveItem(i32, i32, i32, u32) = 10,
    Knockback(f32, f32, f32, bool) = 12,
    FluidSelectable(bool) = 13,
    PlaySound(String, f32, f32, f32, f32, f32, bool) = 14,
    EntityAnimation(u32, u32) = 15,
    ChatMessage(String) = 16,
    PlayerAbilities(f32, MovementType) = 17,
    TeleportPlayer(f32, f32, f32) = 18,
    BlockAnimation(i32, i32, i32, u32) = 19,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MovementType {
    Normal = 0,
    Fly = 1,
    NoClip = 2,
}
pub fn write_string(data: &mut Vec<u8>, value: &String) {
    data.write_be(value.len() as u16).unwrap();
    for ch in value.as_bytes() {
        data.write_be(*ch).unwrap();
    }
}
pub fn read_string(data: &mut &[u8]) -> String {
    let len: u16 = data.read_be().unwrap();
    let mut str = Vec::new();
    for _ in 0..len {
        let ch: u8 = data.read_be().unwrap();
        str.push(ch);
    }
    let str = String::from_utf8(str).unwrap();
    str
}
fn read_face(data: &mut &[u8]) -> Face {
    let face: u8 = data.read_be().unwrap();
    Face::all()[face as usize]
}

pub enum NetworkMessageC2S {
    BreakBlock(i32, i32, i32),
    RightClickBlock(i32, i32, i32, Face, bool),
    PlayerPosition(f32, f32, f32, bool, f32, bool),
    MouseScroll(i32, i32),
    Keyboard(i32, bool, bool),
    GuiClick(String, MouseButton, bool),
    GuiClose,
    RequestBlockBreakTime(u32, BlockPosition),
    LeftClickEntity(u32),
    RightClickEntity(u32),
    GuiScroll(String, i32, i32, bool),
    RightClick(bool),
    SendMessage(String),
    ConnectionMode(u8),
}
impl NetworkMessageC2S {
    pub fn from_data(mut data: &[u8]) -> Option<Self> {
        let id: u8 = data.read_be().unwrap();
        match id {
            0 => Some(NetworkMessageC2S::BreakBlock(
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
            )),
            1 => Some(NetworkMessageC2S::RightClickBlock(
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                read_face(&mut data),
                data.read_be().unwrap(),
            )),
            2 => Some(NetworkMessageC2S::PlayerPosition(
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
            )),
            3 => Some(NetworkMessageC2S::MouseScroll(
                data.read_be().unwrap(),
                data.read_be().unwrap(),
            )),
            4 => Some(NetworkMessageC2S::Keyboard(
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
            )),
            5 => Some(NetworkMessageC2S::GuiClick(
                read_string(&mut data),
                MouseButton::from_data(&mut data),
                data.read_be().unwrap(),
            )),
            6 => Some(NetworkMessageC2S::GuiClose),
            7 => Some(NetworkMessageC2S::RequestBlockBreakTime(
                data.read_be().unwrap(),
                BlockPosition {
                    x: data.read_be().unwrap(),
                    y: data.read_be().unwrap(),
                    z: data.read_be().unwrap(),
                },
            )),
            8 => Some(NetworkMessageC2S::LeftClickEntity(data.read_be().unwrap())),
            9 => Some(NetworkMessageC2S::RightClickEntity(data.read_be().unwrap())),
            10 => Some(NetworkMessageC2S::GuiScroll(
                read_string(&mut data),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
                data.read_be().unwrap(),
            )),
            11 => Some(NetworkMessageC2S::RightClick(data.read_be().unwrap())),
            12 => Some(NetworkMessageC2S::SendMessage(read_string(&mut data))),
            13 => Some(NetworkMessageC2S::ConnectionMode(data.read_be().unwrap())),
            _ => None,
        }
    }
}
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum MouseButton {
    LEFT = 0,
    RIGHT = 1,
}
impl MouseButton {
    pub fn from_data(data: &mut &[u8]) -> Self {
        let id: u8 = data.read_be().unwrap();
        match id {
            0 => MouseButton::LEFT,
            1 => MouseButton::RIGHT,
            _ => panic!("unknown MouseButton"), //todo: don't crash
        }
    }
}

pub struct PlayerConnection {
    socket: WebSocket<TcpStream>,
    closed: bool,
}
impl PlayerConnection {
    pub fn new(mut socket: WebSocket<TcpStream>) -> Result<(Self, u8), ()> {
        let mode_message = socket.read_message().map_err(|_| ())?;
        match mode_message {
            tungstenite::Message::Binary(message) => {
                match NetworkMessageC2S::from_data(message.as_slice()) {
                    Some(NetworkMessageC2S::ConnectionMode(mode)) => {
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
            .write_message(tungstenite::Message::Text(json.dump()));
    }
    pub fn send_binary(&mut self, data: &Vec<u8>) {
        self.socket
            .write_message(tungstenite::Message::Binary(data.clone()));
    }
    pub fn send(&mut self, message: &NetworkMessageS2C) {
        if let Err(error) = self
            .socket
            .write_message(tungstenite::Message::Binary(message.to_data()))
        {
            println!("connection error: {}", error);
            self.closed = true;
        }
    }
    pub fn receive_messages(&mut self) -> Vec<NetworkMessageC2S> {
        let mut messages = Vec::new();
        while let Ok(message) = self.socket.read_message() {
            match message {
                tungstenite::Message::Binary(message) => {
                    match NetworkMessageC2S::from_data(message.as_slice()) {
                        Some(message) => messages.push(message),
                        None => {
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
