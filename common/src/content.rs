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
    pub no_collide: bool,
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
    pub models: Vec<(String, String, Transformation)>,
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
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Transformation {
    pub position: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub origin: Vec3,
}
impl Transformation {
    pub fn identity() -> Self {
        Transformation {
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            origin: Vec3::ZERO,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientEntityData {
    pub model: String,
    pub texture: String,
    pub hitbox_w: f64,
    pub hitbox_h: f64,
    pub hitbox_d: f64,
    pub hitbox_h_shifting: f64,
    pub animations: Vec<String>,
    pub items: Vec<String>,
    pub viewmodel: Option<(String, String, Vec<String>, Vec<String>)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelBone {
    pub child_bones: Vec<ModelBone>,
    pub cube_elements: Vec<ModelCubeElement>,
    pub animations: HashMap<u32, ModelAnimationData>,
    pub origin: Vec3,
    pub item_elements: Vec<ModelItemElement>,
}
//todo: use transformation
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
    pub name: String,
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
impl ModelAnimationData {
    pub fn get_for_time(&self, time: f32) -> (Vec3, Vec3, Vec3) {
        (
            Self::get_channel_for_time(&self.position, time, 0.),
            Self::get_channel_for_time(&self.rotation, time, 0.),
            Self::get_channel_for_time(&self.scale, time, 1.),
        )
    }
    pub fn get_default() -> (Vec3, Vec3, Vec3) {
        (Vec3::ZERO, Vec3::ZERO, Vec3::ONE)
    }
    fn get_channel_for_time(
        keyframes: &Vec<ModelAnimationKeyframe>,
        time: f32,
        default_value: f32,
    ) -> Vec3 {
        if keyframes.len() == 0 {
            Vec3 {
                x: default_value,
                y: default_value,
                z: default_value,
            }
        } else {
            let mut first = None;
            let mut second = None;
            for keyframe in keyframes {
                if keyframe.time < time {
                    first = Some(keyframe);
                } else {
                    second = Some(keyframe);
                    break;
                }
            }
            if first.is_some() && second.is_none() {
                second = first;
            }
            if second.is_some() && first.is_none() {
                first = second;
            }
            let first = first.unwrap();
            let second = second.unwrap();
            if std::ptr::eq(first, second) {
                Vec3 {
                    x: first.data.x,
                    y: first.data.y,
                    z: first.data.z,
                }
            } else {
                let lerp_val = (time - first.time) / (second.time - first.time);
                Vec3 {
                    x: (first.data.x * (1. - lerp_val)) + (second.data.x * lerp_val),
                    y: (first.data.y * (1. - lerp_val)) + (second.data.y * lerp_val),
                    z: (first.data.z * (1. - lerp_val)) + (second.data.z * lerp_val),
                }
            }
        }
    }
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
