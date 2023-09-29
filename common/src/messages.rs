use crate::gui::{GUIElement, GUIElementEdit};
use crate::{BlockPosition, Face, KeyboardKey};
use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    SetBlock(i32, i32, i32, u32),
    LoadChunk(i32, i32, i32, Vec<u32>, Vec<u8>),
    UnloadChunk(i32, i32, i32),
    AddEntity(u32, u32, f32, f32, f32, f32, u32, f32),
    MoveEntity(u32, f32, f32, f32, f32),
    DeleteEntity(u32),
    GuiSetElement(String, GUIElement),
    GuiRemoveElements(String),
    GuiEditElement(String, GUIElementEdit),
    SetCursorLock(bool),
    BlockBreakTimeResponse(u32, f32),
    EntityItem(u32, u32, u32),
    BlockItem(i32, i32, i32, u32, u32),
    Knockback(f32, f32, f32, bool),
    FluidSelectable(bool),
    PlaySound(String, f32, f32, f32, f32, f32, bool),
    EntityAnimation(u32, u32),
    ChatMessage(String),
    PlayerAbilities(f32, MovementType),
    TeleportPlayer(f32, f32, f32, f32),
    BlockAnimation(i32, i32, i32, u32),
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MovementType {
    Normal = 0,
    Fly = 1,
    NoClip = 2,
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    BreakBlock(i32, i32, i32),
    RightClickBlock(i32, i32, i32, Face, bool),
    PlayerPosition(f32, f32, f32, bool, f32, bool),
    MouseScroll(i32, i32),
    Keyboard(KeyboardKey, u16, bool, bool),
    GuiClick(String, MouseButton, bool),
    RequestBlockBreakTime(u32, BlockPosition),
    LeftClickEntity(u32),
    RightClickEntity(u32),
    GuiScroll(String, i32, i32, bool),
    RightClick(bool),
    SendMessage(String),
    ConnectionMode(u8),
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}
