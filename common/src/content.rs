use crate::{Face, TexCoords, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    Block(u32),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelBone {
    pub child_bones: Vec<ModelBone>,
    pub cube_elements: Vec<ModelCubeElement>,
    pub animations: HashMap<u32, ModelAnimationData>,
    pub origin: Vec3,
    pub item_elements: Vec<ModelItemElement>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelCubeElement {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub origin: Vec3,
    pub front: TexCoords,
    pub back: TexCoords,
    pub left: TexCoords,
    pub right: TexCoords,
    pub up: TexCoords,
    pub down: TexCoords,
}
impl ModelCubeElement {
    pub fn texture_by_face(&self, face: Face) -> TexCoords {
        match face {
            Face::Front => self.front,
            Face::Back => self.back,
            Face::Up => self.up,
            Face::Down => self.down,
            Face::Left => self.left,
            Face::Right => self.right,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelItemElement {
    pub position: Vec3,
    pub rotation: Vec3,
    pub origin: Vec3,
    pub size: Vec2,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelAnimationData {
    pub position: Vec<ModelAnimationKeyframe>,
    pub rotation: Vec<ModelAnimationKeyframe>,
    pub scale: Vec<ModelAnimationKeyframe>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelAnimationKeyframe {
    pub data: Vec3,
    pub time: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelData {
    pub root_bone: ModelBone,
    pub animations: Vec<(String, f32)>,
}
