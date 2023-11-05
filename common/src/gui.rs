use crate::{Color, Position, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Eq, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
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
impl GUIElement {
    pub fn edit(&mut self, edit: GUIElementEdit) {
        if let Some(position) = edit.position {
            self.position = position;
        }
        if let Some(anchor) = edit.anchor {
            self.anchor = anchor;
        }
        if let Some(base_color) = edit.base_color {
            self.base_color = base_color;
        }
        self.component_type.edit(edit.component_type);
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub enum GUIComponent {
    ImageComponent {
        texture: String,
        size: Vec2,
        slice: Option<(Vec2, Vec2)>,
    },
    TextComponent {
        font_size: f32,
        text: String,
    },
    SlotComponent {
        item_id: Option<(u32, u32)>,
        background: String,
        size: Vec2,
    },
}
impl GUIComponent {
    pub fn edit(&mut self, edit: GUIComponentEdit) {
        match (self, edit) {
            (
                GUIComponent::ImageComponent {
                    texture,
                    size,
                    slice,
                },
                GUIComponentEdit::ImageComponent {
                    texture: texture_edit,
                    size: size_edit,
                    slice: slice_edit,
                },
            ) => {
                if let Some(texture_edit) = texture_edit {
                    *texture = texture_edit;
                }
                if let Some(size_edit) = size_edit {
                    *size = size_edit;
                }
                if let Some(slice_edit) = slice_edit {
                    *slice = slice_edit;
                }
            }
            (
                GUIComponent::SlotComponent {
                    background,
                    size,
                    item_id,
                },
                GUIComponentEdit::SlotComponent {
                    background: background_edit,
                    size: size_edit,
                    item_id: item_id_edit,
                },
            ) => {
                if let Some(background_edit) = background_edit {
                    *background = background_edit;
                }
                if let Some(size_edit) = size_edit {
                    *size = size_edit;
                }
                if let Some(item_id_edit) = item_id_edit {
                    *item_id = item_id_edit;
                }
            }
            (
                GUIComponent::TextComponent { text, font_size },
                GUIComponentEdit::TextComponent {
                    text: text_edit,
                    font_size: font_size_edit,
                },
            ) => {
                if let Some(text_edit) = text_edit {
                    *text = text_edit;
                }
                if let Some(font_size_edit) = font_size_edit {
                    *font_size = font_size_edit;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct GUIElementEdit {
    pub component_type: GUIComponentEdit,
    pub position: Option<Position>,
    pub anchor: Option<PositionAnchor>,
    pub base_color: Option<Color>,
}
#[derive(Clone, Serialize, Deserialize, Default)]
pub enum GUIComponentEdit {
    #[default]
    None,
    ImageComponent {
        texture: Option<String>,
        size: Option<Vec2>,
        slice: Option<Option<(Vec2, Vec2)>>,
    },
    TextComponent {
        font_size: Option<f32>,
        text: Option<String>,
    },
    SlotComponent {
        item_id: Option<Option<(u32, u32)>>,
        background: Option<String>,
        size: Option<Vec2>,
    },
}
