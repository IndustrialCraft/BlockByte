use crate::gui::{GUIElement, GUIElementEdit};
use crate::{BlockPosition, ChunkPosition, Direction, Face, KeyboardKey, Position};
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumIter};

#[repr(u8)]
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    SetBlock(BlockPosition, u32),
    LoadChunk(ChunkPosition, Vec<u32>, Vec<u8>),
    UnloadChunk(ChunkPosition),
    AddEntity(u32, u32, Position, Direction, u32, f32),
    MoveEntity(u32, Position, Direction),
    DeleteEntity(u32),
    GuiSetElement(String, GUIElement),
    GuiRemoveElements(String),
    GuiEditElement(String, GUIElementEdit),
    SetCursorLock(bool),
    BlockBreakTimeResponse(u32, f32),
    Knockback(f32, f32, f32, bool),
    FluidSelectable(bool),
    PlaySound(String, Position, f32, f32, bool),
    ChatMessage(String),
    PlayerAbilities(f32, MovementType),
    TeleportPlayer(Position, Direction),
    ModelItem(ClientModelTarget, u32, Option<u32>),
    ModelAnimation(ClientModelTarget, u32),
    ControllingEntity(u32),
}
#[derive(Serialize, Deserialize)]
pub enum ClientModelTarget {
    Block(BlockPosition),
    Entity(u32),
    ViewModel,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumIter)]
pub enum MovementType {
    Normal = 0,
    Fly = 1,
    NoClip = 2,
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    BreakBlock(BlockPosition),
    RightClickBlock(BlockPosition, Face, bool),
    PlayerPosition(Position, bool, Direction, bool),
    MouseScroll(i32, i32),
    Keyboard(KeyboardKey, u8, bool, bool),
    GuiClick(String, MouseButton, bool),
    RequestBlockBreakTime(u32, BlockPosition),
    LeftClickEntity(u32),
    RightClickEntity(u32),
    GuiScroll(String, i32, i32, bool),
    RightClick(bool),
    SendMessage(String),
    ConnectionMode(u8),
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumIter, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}
