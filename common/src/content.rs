use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientContent {
    pub blocks: Vec<ClientBlockData>,
    pub items: Vec<ClientItemData>,
    pub entities: Vec<ClientEntityData>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlockData {
    pub block_type: ClientBlockRenderDataType,
    pub dynamic: Option<ClientBlockDynamicData>,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
    pub selectable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlockDynamicData {
    pub model: String,
    pub texture: String,
    pub animations: Vec<String>,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientBlockRenderDataType {
    Air,
    Cube(ClientBlockCubeRenderData),
    Static(ClientBlockStaticRenderData),
    Foliage(ClientBlockFoliageRenderData),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlockCubeRenderData {
    pub front: String,
    pub back: String,
    pub right: String,
    pub left: String,
    pub up: String,
    pub down: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlockStaticRenderData {
    pub model: String,
    pub texture: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientBlockFoliageRenderData {
    pub texture_1: String,
    pub texture_2: String,
    pub texture_3: String,
    pub texture_4: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientItemData {
    pub name: String,
    pub model: ClientItemModel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientItemModel {
    Texture(String),
    Block(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientEntityData {
    pub model: String,
    pub texture: String,
    pub hitbox_w: f64,
    pub hitbox_h: f64,
    pub hitbox_d: f64,
    pub animations: Vec<String>,
    pub items: Vec<String>,
}
