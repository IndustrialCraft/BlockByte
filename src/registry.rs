use std::{
    collections::HashMap,
    fs::File,
    hash::BuildHasherDefault,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
};

use json::{array, object, JsonValue};
use twox_hash::XxHash64;
use zip::{write::FileOptions, DateTime, ZipWriter};

use crate::{
    inventory::ItemStack,
    mods::ClientContentData,
    util::{BlockPosition, Face, Identifier},
    world::{BlockData, Entity},
};

pub struct BlockRegistry {
    blocks: HashMap<Identifier, Arc<Block>, BuildHasherDefault<XxHash64>>,
    states: Vec<BlockState>,
    id_generator: u32,
}
impl BlockRegistry {
    pub fn new() -> Self {
        let mut block_registry = BlockRegistry {
            blocks: Default::default(),
            states: Vec::new(),
            id_generator: 0,
        };
        block_registry
            .register(Identifier::new("bb", "air"), |id| {
                let block = Arc::new(Block { default_state: id });
                let state = vec![BlockState {
                    state_id: id,
                    client_data: ClientBlockRenderData {
                        block_type: ClientBlockRenderDataType::Air,
                        dynamic: None,
                        fluid: false,
                        render_data: 0,
                        transparent: false,
                    },
                    parent: block.clone(),
                }];
                (block, state)
            })
            .expect("couldn't register air");
        block_registry
    }
    pub fn register<F>(&mut self, id: Identifier, creator: F) -> Result<u32, ()>
    where
        F: FnOnce(u32) -> (Arc<Block>, Vec<BlockState>),
    {
        if self.blocks.get(&id).is_some() {
            return Err(());
        }
        let numeric_id = self.id_generator;
        let (block, mut block_states) = creator.call_once((self.id_generator,));
        self.blocks.insert(id, block);
        self.id_generator += block_states.len() as u32;
        self.states.append(&mut block_states);
        Ok(numeric_id)
    }
    pub fn block_by_identifier(&self, id: &Identifier) -> Option<&Arc<Block>> {
        self.blocks.get(id)
    }
}

pub struct Block {
    pub default_state: u32,
}

pub struct BlockState {
    pub state_id: u32,
    pub client_data: ClientBlockRenderData,
    pub parent: Arc<Block>,
}
impl BlockState {
    pub fn get_full_id(&self) -> u32 {
        self.state_id
    }
}
#[derive(Clone, Debug)]
pub struct ClientBlockRenderData {
    pub block_type: ClientBlockRenderDataType,
    pub dynamic: Option<ClientBlockDynamicData>,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
}
#[derive(Clone, Debug)]
pub struct ClientBlockDynamicData {
    pub model: String,
    pub texture: String,
    pub animations: Vec<String>,
    pub items: Vec<String>,
}
#[derive(Clone, Debug)]
pub enum ClientBlockRenderDataType {
    Air,
    Cube(ClientBlockCubeRenderData),
}
#[derive(Clone, Debug)]
pub struct ClientBlockCubeRenderData {
    pub front: String,
    pub back: String,
    pub right: String,
    pub left: String,
    pub up: String,
    pub down: String,
}

pub struct ItemRegistry {
    items: HashMap<Identifier, Arc<Item>, BuildHasherDefault<XxHash64>>,
    id_generator: u32,
}
impl ItemRegistry {
    pub fn new() -> Self {
        ItemRegistry {
            items: Default::default(),
            id_generator: 0,
        }
    }
    pub fn register<F>(&mut self, id: Identifier, creator: F) -> Result<Arc<Item>, ()>
    where
        F: FnOnce(u32) -> Arc<Item>,
    {
        if self.items.get(&id).is_some() {
            return Err(());
        }
        let item = creator.call_once((self.id_generator,));
        self.items.insert(id, item.clone());
        self.id_generator += 1;
        Ok(item)
    }
    pub fn item_by_identifier(&self, id: &Identifier) -> Option<&Arc<Item>> {
        self.items.get(id)
    }
}
pub struct Item {
    pub client_data: ClientItemRenderData,
    pub id: u32,
    pub place_block: Option<Arc<Block>>,
}
impl Item {
    pub fn on_right_click_block(
        &self,
        item: &mut ItemStack,
        player: Arc<Entity>,
        block_position: BlockPosition,
        block_face: Face,
    ) -> InteractionResult {
        if let Some(place) = &self.place_block {
            let block_position = block_position.offset_by_face(block_face);
            let world = player.get_location().chunk.world.clone();
            world.replace_block(block_position, |block| match block {
                BlockData::Simple(0) => {
                    item.add_count(-1);
                    Some(BlockData::Simple(place.default_state))
                }
                _ => None,
            });
        }
        InteractionResult::Ignored
    }
}
pub enum InteractionResult {
    Consumed,
    Ignored,
}
#[derive(Clone)]
pub struct ClientItemRenderData {
    pub name: String,
    pub model: ClientItemModel,
}
#[derive(Clone)]
pub enum ClientItemModel {
    Texture(String),
    Block(Identifier),
}
pub struct EntityRegistry {
    entities: HashMap<Identifier, Arc<EntityData>, BuildHasherDefault<XxHash64>>,
    id_generator: u32,
}
impl EntityRegistry {
    pub fn new() -> Self {
        EntityRegistry {
            entities: Default::default(),
            id_generator: 0,
        }
    }
    pub fn register<F>(&mut self, id: Identifier, creator: F) -> Result<Arc<EntityData>, ()>
    where
        F: FnOnce(u32) -> Arc<EntityData>,
    {
        if self.entities.get(&id).is_some() {
            return Err(());
        }
        let entity = creator.call_once((self.id_generator,));
        self.entities.insert(id, entity.clone());
        self.id_generator += 1;
        Ok(entity)
    }
    pub fn entity_by_identifier(&self, id: &Identifier) -> Option<&Arc<EntityData>> {
        self.entities.get(id)
    }
}
pub struct EntityData {
    pub id: u32,
    pub client_data: ClientEntityData,
    pub ticker: Mutex<Option<rhai::FnPtr>>,
}
#[derive(Clone)]
pub struct ClientEntityData {
    pub model: String,
    pub texture: String,
    pub hitbox_w: f32,
    pub hitbox_h: f32,
    pub hitbox_d: f32,
    pub animations: Vec<String>,
    pub items: Vec<String>,
}

pub struct ClientContent {}
impl ClientContent {
    pub fn generate_zip(
        block_registry: &BlockRegistry,
        item_registry: &ItemRegistry,
        entity_registry: &EntityRegistry,
        client_content: ClientContentData,
    ) -> Vec<u8> {
        let mut zip_writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o444)
            .last_modified_time(DateTime::from_msdos(0, 0));
        zip_writer.start_file("content.json", options).unwrap();
        zip_writer
            .write_all(
                Self::generate_content_json(block_registry, item_registry, entity_registry)
                    .dump()
                    .as_str()
                    .as_bytes(),
            )
            .unwrap();
        for image in client_content.images {
            let mut file_name = image.0.to_string();
            file_name.push_str(".png");
            zip_writer.start_file(file_name, options).unwrap();
            zip_writer.write_all(image.1.as_slice()).unwrap();
        }
        for sound in client_content.sounds {
            let mut file_name = sound.0.to_string();
            file_name.push_str(".wav");
            zip_writer.start_file(file_name, options).unwrap();
            zip_writer.write_all(sound.1.as_slice()).unwrap();
        }
        for model in client_content.models {
            let mut file_name = model.0.to_string();
            file_name.push_str(".bbm");
            zip_writer.start_file(file_name, options).unwrap();
            zip_writer.write_all(model.1.as_slice()).unwrap();
        }
        {
            zip_writer.start_file("font.ttf", options).unwrap();
            zip_writer
                .write_all(std::fs::read("font.ttf").unwrap().as_slice())
                .unwrap();
        }
        zip_writer.finish().unwrap().into_inner()
    }
    pub fn generate_content_json(
        block_registry: &BlockRegistry,
        item_registry: &ItemRegistry,
        entity_registry: &EntityRegistry,
    ) -> JsonValue {
        let mut blocks = array![];
        for block in block_registry.states.iter().skip(1).enumerate() {
            let client_data = &block.1.client_data;
            let mut model_json = object! {
                transparent: client_data.transparent,
                fluid: client_data.fluid,
                render_data: client_data.render_data
            };
            if let Some(dynamic) = &client_data.dynamic {
                model_json["dynamic"] = object! {
                    model: dynamic.model.clone(),
                    texture: dynamic.texture.clone(),
                    animations: dynamic.animations.clone(),
                    items: dynamic.items.clone()
                };
            }
            match &client_data.block_type {
                ClientBlockRenderDataType::Air => {
                    model_json.insert("type", "air").unwrap();
                }
                ClientBlockRenderDataType::Cube(cube_data) => {
                    model_json.insert("type", "cube").unwrap();
                    model_json.insert("north", cube_data.front.clone()).unwrap();
                    model_json.insert("south", cube_data.back.clone()).unwrap();
                    model_json.insert("right", cube_data.right.clone()).unwrap();
                    model_json.insert("left", cube_data.left.clone()).unwrap();
                    model_json.insert("up", cube_data.up.clone()).unwrap();
                    model_json.insert("down", cube_data.down.clone()).unwrap();
                }
            }
            blocks
                .push(object! {id: block.1.state_id,
                    model: model_json
                })
                .unwrap();
        }
        let mut items = array![];
        for item in item_registry.items.values().into_iter().enumerate() {
            let model = match &item.1.client_data.model {
                ClientItemModel::Texture(texture) => {
                    ("texture", JsonValue::String(texture.clone()))
                }
                ClientItemModel::Block(block) => (
                    "block",
                    JsonValue::from(
                        block_registry
                            .block_by_identifier(block)
                            .unwrap()
                            .default_state,
                    ),
                ),
            };
            items
                .push(object! {
                    id: item.1.id,
                    name: item.1.client_data.name.clone(),
                    modelType: model.0,
                    modelValue: model.1
                })
                .unwrap();
        }
        let mut entities = array![];
        for entity in entity_registry.entities.values().into_iter().enumerate() {
            entities.push(object! {id: entity.1.id,model:entity.1.client_data.model.clone(),texture:entity.1.client_data.texture.clone(),hitboxW:entity.1.client_data.hitbox_w,hitboxH:entity.1.client_data.hitbox_h,hitboxD:entity.1.client_data.hitbox_d,animations:entity.1.client_data.animations.clone(),items:entity.1.client_data.items.clone()}).unwrap();
        }
        object! {
            blocks: blocks,
            items: items,
            entities: entities,
        }
    }
}
