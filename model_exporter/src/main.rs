use std::{collections::HashMap, str::FromStr};

use block_byte_common::content::{
    ModelAnimationData, ModelAnimationKeyframe, ModelBone, ModelCubeElement, ModelData,
    ModelItemElement,
};
use block_byte_common::{TexCoords, Vec2, Vec3};
use either::Either;
use json::JsonValue;

fn main() {
    let file_name = std::env::args().nth(1).expect("missing file name");
    let json = json::parse(
        std::fs::read_to_string(file_name)
            .expect("file not found")
            .as_str(),
    )
    .expect("malformed model file");
    let texture_resolution = &json["resolution"];
    let texture_resolution = (
        texture_resolution["width"].as_u32().unwrap(),
        texture_resolution["height"].as_u32().unwrap(),
    );
    let mut elements = HashMap::new();
    for element in json["elements"].members() {
        let name = element["name"].as_str().unwrap();
        let (id, cube) = if name.starts_with("item_") {
            let (element, id) = ItemElement::from_json(name.replacen("item_", "", 1), element);
            (id, Either::Right(element))
        } else {
            let (element, id) = cube_element_from_json(element, &texture_resolution);
            (id, Either::Left(element))
        };
        elements.insert(id, cube);
    }

    let mut root_bone = Bone::children_from_json(
        &json["outliner"],
        &mut elements,
        Vec3 {
            x: 0.,
            y: 0.,
            z: 0.,
        },
        "root".to_string(),
        uuid::Uuid::from_u128(0),
    );
    let mut animation_data = Vec::new();
    for (animation_id, animation) in json["animations"].members().enumerate() {
        let name = animation["name"].as_str().unwrap();
        let length = animation["length"].as_f32().unwrap();
        animation_data.push((name, length));
        for animator in animation["animators"].entries() {
            let uuid = uuid::Uuid::from_str(animator.0).unwrap();
            let animation_data = root_bone
                .find_sub_bone(&uuid)
                .unwrap()
                .animation_data_for_id(animation_id as u32);
            for keyframes in animator.1["keyframes"].members() {
                let channel = keyframes["channel"].as_str().unwrap();
                add_keyframe(
                    animation_data,
                    channel,
                    if channel != "rotation" {
                        Vec3Json::from_keyframe_pos(&keyframes["data_points"][0])
                    } else {
                        Vec3Json::from_keyframe_rot(&keyframes["data_points"][0])
                    },
                    keyframes["time"].as_f32().unwrap(),
                );
            }
        }
    }
    //println!("{root_bone:#?}");

    std::fs::write(
        "out.bbm",
        bitcode::serialize(&ModelData {
            root_bone: root_bone.to_content(),
            animations: animation_data
                .iter()
                .map(|data| (data.0.to_string(), data.1))
                .collect(),
        })
        .unwrap(),
    )
    .unwrap();
}
#[derive(Clone, Debug)]
struct Bone {
    uuid: uuid::Uuid,
    child_bones: Vec<Bone>,
    cube_elements: Vec<ModelCubeElement>,
    animations: HashMap<u32, ModelAnimationData>,
    origin: Vec3,
    name: String,
    item_elements: Vec<ModelItemElement>,
}
impl Bone {
    pub fn find_sub_bone(&mut self, id: &uuid::Uuid) -> Option<&mut Bone> {
        if &self.uuid == id {
            return Some(self);
        }
        for child in &mut self.child_bones {
            let sub = child.find_sub_bone(id);
            if sub.is_some() {
                return sub;
            }
        }
        None
    }
    pub fn animation_data_for_id(&mut self, id: u32) -> &mut ModelAnimationData {
        self.animations
            .entry(id)
            .or_insert_with(|| ModelAnimationData {
                position: Vec::new(),
                rotation: Vec::new(),
                scale: Vec::new(),
            })
    }
    pub fn to_content(self) -> ModelBone {
        ModelBone {
            animations: self.animations,
            origin: self.origin,
            child_bones: self
                .child_bones
                .into_iter()
                .map(|bone| bone.to_content())
                .collect(),
            cube_elements: self.cube_elements,
            item_elements: self.item_elements,
        }
    }
    pub fn children_from_json(
        json: &JsonValue,
        elements: &mut HashMap<uuid::Uuid, Either<ModelCubeElement, ModelItemElement>>,
        origin: Vec3,
        name: String,
        uuid: uuid::Uuid,
    ) -> Self {
        let mut child_bones = Vec::new();
        let mut cube_elements = Vec::new();
        let mut item_elements = Vec::new();
        for child in json.members() {
            match child {
                JsonValue::String(id) => {
                    let uuid = uuid::Uuid::from_str(id.as_str()).unwrap();
                    match elements.remove(&uuid).unwrap() {
                        Either::Left(cube) => {
                            cube_elements.push(cube);
                        }
                        Either::Right(item) => {
                            item_elements.push(item);
                        }
                    }
                }
                JsonValue::Object(bone) => {
                    child_bones.push(Bone::from_json(&JsonValue::Object(bone.clone()), elements));
                }
                _ => panic!(""),
            }
        }
        Bone {
            uuid,
            child_bones,
            cube_elements,
            origin,
            name,
            item_elements,
            animations: HashMap::new(),
        }
    }
    pub fn from_json(
        json: &JsonValue,
        elements: &mut HashMap<uuid::Uuid, Either<ModelCubeElement, ModelItemElement>>,
    ) -> Self {
        Self::children_from_json(
            &json["children"],
            elements,
            Vec3Json::from_json_pos(&json["origin"]),
            json["name"].as_str().unwrap().to_string(),
            uuid::Uuid::from_str(json["uuid"].as_str().unwrap()).unwrap(),
        )
    }
}
struct Vec3Json {}
impl Vec3Json {
    pub fn from_json_pos(json: &JsonValue) -> Vec3 {
        Vec3 {
            x: json[0].as_f32().unwrap() / 16.,
            y: json[1].as_f32().unwrap() / 16.,
            z: json[2].as_f32().unwrap() / 16.,
        }
    }
    pub fn from_json_rot(json: &JsonValue) -> Vec3 {
        Vec3 {
            x: json[0].as_f32().unwrap().to_radians(),
            y: json[1].as_f32().unwrap().to_radians(),
            z: json[2].as_f32().unwrap().to_radians(),
        }
    }
    pub fn from_keyframe_pos(json: &JsonValue) -> Vec3 {
        let x = &json["x"];
        let y = &json["y"];
        let z = &json["z"];
        let x: f32 = x
            .as_f32()
            .unwrap_or(x.as_str().unwrap_or("").parse().unwrap_or(0.));
        let y: f32 = y
            .as_f32()
            .unwrap_or(y.as_str().unwrap_or("").parse().unwrap_or(0.));
        let z: f32 = z
            .as_f32()
            .unwrap_or(z.as_str().unwrap_or("").parse().unwrap_or(0.));
        Vec3 {
            x: x / 16.,
            y: y / 16.,
            z: z / 16.,
        }
    }
    pub fn from_keyframe_rot(json: &JsonValue) -> Vec3 {
        let x = &json["x"];
        let y = &json["y"];
        let z = &json["z"];
        let x: f32 = x
            .as_f32()
            .unwrap_or(x.as_str().and_then(|v| v.parse().ok()).unwrap_or(0.));
        let y: f32 = y
            .as_f32()
            .unwrap_or(y.as_str().and_then(|v| v.parse().ok()).unwrap_or(0.));
        let z: f32 = z
            .as_f32()
            .unwrap_or(z.as_str().and_then(|v| v.parse().ok()).unwrap_or(0.));
        Vec3 {
            x: x.to_radians(),
            y: y.to_radians(),
            z: z.to_radians(),
        }
    }
}
pub fn cube_element_from_json(
    json: &JsonValue,
    resolution: &(u32, u32),
) -> (ModelCubeElement, uuid::Uuid) {
    let from = Vec3Json::from_json_pos(&json["from"]);
    let to = Vec3Json::from_json_pos(&json["to"]);
    let rotation = &json["rotation"];
    let faces = &json["faces"];
    (
        ModelCubeElement {
            scale: Vec3 {
                x: to.x - from.x,
                y: to.y - from.y,
                z: to.z - from.z,
            },
            position: from,
            rotation: if rotation.is_null() {
                Vec3 {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                }
            } else {
                Vec3Json::from_json_rot(rotation)
            },
            origin: Vec3Json::from_json_pos(&json["origin"]),
            front: CubeElementFace::from_json(&faces["north"], resolution),
            back: CubeElementFace::from_json(&faces["south"], resolution),
            left: CubeElementFace::from_json(&faces["west"], resolution),
            right: CubeElementFace::from_json(&faces["east"], resolution),
            up: CubeElementFace::from_json(&faces["up"], resolution),
            down: CubeElementFace::from_json(&faces["down"], resolution),
        },
        uuid::Uuid::from_str(json["uuid"].as_str().unwrap()).unwrap(),
    )
}
struct CubeElementFace {}
impl CubeElementFace {
    pub fn from_json(json: &JsonValue, resolution: &(u32, u32)) -> TexCoords {
        let uv = &json["uv"];
        TexCoords {
            u1: uv[0].as_f32().unwrap() / resolution.0 as f32,
            v1: uv[1].as_f32().unwrap() / resolution.1 as f32,
            u2: uv[2].as_f32().unwrap() / resolution.0 as f32,
            v2: uv[3].as_f32().unwrap() / resolution.1 as f32,
        }
    }
}
struct ItemElement {}
impl ItemElement {
    pub fn from_json(name: String, json: &JsonValue) -> (ModelItemElement, uuid::Uuid) {
        let from = Vec3Json::from_json_pos(&json["from"]);
        let to = Vec3Json::from_json_pos(&json["to"]);
        let rotation = &json["rotation"];
        (
            ModelItemElement {
                rotation: if rotation.is_null() {
                    Vec3 {
                        x: 0.,
                        y: 0.,
                        z: 0.,
                    }
                } else {
                    Vec3Json::from_json_rot(rotation)
                },
                origin: Vec3Json::from_json_pos(&json["origin"]),
                size: Vec2 {
                    x: to.x - from.x,
                    y: to.y - from.y,
                },
                position: from,
            },
            uuid::Uuid::from_str(json["uuid"].as_str().unwrap()).unwrap(),
        )
    }
}
pub fn add_keyframe(animation_data: &mut ModelAnimationData, channel: &str, data: Vec3, time: f32) {
    let keyframe = ModelAnimationKeyframe { data, time };
    match channel {
        "position" => animation_data.position.push(keyframe),
        "rotation" => animation_data.rotation.push(keyframe),
        "scale" => animation_data.scale.push(keyframe),
        _ => panic!("unknown keyframe type"),
    }
}
