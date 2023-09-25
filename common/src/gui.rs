use crate::{Color, Position, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum PositionAnchor {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
    Cursor,
}
impl PositionAnchor {
    pub fn get_center(&self, mouse: Vec2) -> Vec2 {
        match self {
            PositionAnchor::Top => Vec2 { x: 0., y: 1. },
            PositionAnchor::Bottom => Vec2 { x: 0., y: -1. },
            PositionAnchor::Left => Vec2 { x: -1., y: 0. },
            PositionAnchor::Right => Vec2 { x: 1., y: 0. },
            PositionAnchor::TopLeft => Vec2 { x: -1., y: 1. },
            PositionAnchor::TopRight => Vec2 { x: 1., y: 1. },
            PositionAnchor::BottomLeft => Vec2 { x: -1., y: -1. },
            PositionAnchor::BottomRight => Vec2 { x: 1., y: -1. },
            PositionAnchor::Center => Vec2 { x: 0., y: 0. },
            PositionAnchor::Cursor => mouse,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct GUIElement {
    pub component_type: GUIComponent,
    pub position: Position,
    pub anchor: PositionAnchor,
    pub base_color: Color,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum GUIComponent {
    ImageComponent {
        texture: String,
        size: Vec2,
    },
    TextComponent {
        font_size: f32,
        text: String,
    },
    SlotComponent {
        item_id: u32,
        background: String,
        size: Vec2,
    },
}
