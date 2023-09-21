use std::{
    collections::{hash_map::Keys, HashMap},
    hash::BuildHasherDefault,
    io::Write,
    sync::Arc,
};

use block_byte_common::messages::NetworkMessageS2C;
use block_byte_common::{BlockPosition, Face};
use json::{array, object, JsonValue};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rhai::Dynamic;
use twox_hash::XxHash64;
use zip::{write::FileOptions, DateTime, ZipWriter};

use crate::inventory::Recipe;
use crate::{
    inventory::ItemStack,
    mods::{ClientContentData, ScriptCallback},
    util::{ChunkBlockLocation, Identifier},
    world::{BlockData, Chunk, Entity, WorldBlock},
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
                let block = Arc::new(Block {
                    id: Identifier::new("bb", "air"),
                    default_state: id,
                    data_container: false,
                });
                let state = vec![BlockState {
                    state_id: id,
                    client_data: ClientBlockRenderData {
                        block_type: ClientBlockRenderDataType::Air,
                        dynamic: None,
                        fluid: false,
                        render_data: 0,
                        transparent: false,
                        selectable: false,
                    },
                    parent: block.clone(),
                    breaking_data: (0., None),
                    loottable: None,
                    collidable: false,
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
    pub fn list_blocks(&self) -> std::collections::hash_map::Iter<Identifier, Arc<Block>> {
        self.blocks.iter()
    }
    pub fn list_block_states(&self) -> &Vec<BlockState> {
        &self.states
    }
    pub fn block_by_identifier(&self, id: &Identifier) -> Option<&Arc<Block>> {
        self.blocks.get(id)
    }
    pub fn state_by_ref(&self, block_state_ref: &BlockStateRef) -> &BlockState {
        self.states.get(block_state_ref.state_id as usize).unwrap()
    }
}

pub struct Block {
    pub id: Identifier,
    pub default_state: u32,
    pub data_container: bool,
}

impl Block {
    pub fn get_default_state_ref(&self) -> BlockStateRef {
        BlockStateRef {
            state_id: self.default_state,
        }
    }
}

#[derive(Clone, Copy)]
pub struct BlockStateRef {
    state_id: u32,
}

impl BlockStateRef {
    pub fn create_block_data(&self, chunk: &Chunk, position: BlockPosition) -> BlockData {
        chunk
            .world
            .server
            .block_registry
            .state_by_ref(self)
            .to_block_data(ChunkBlockLocation::new(position, chunk.ptr()).unwrap())
    }
    pub fn from_state_id(state_id: u32) -> Self {
        Self { state_id }
    }
    pub fn get_client_id(&self) -> u32 {
        self.state_id
    }
    pub fn is_air(&self) -> bool {
        self.state_id == 0
    }
}

pub struct BlockState {
    pub state_id: u32,
    pub client_data: ClientBlockRenderData,
    pub breaking_data: (f32, Option<(ToolType, f32)>),
    pub loottable: Option<Identifier>,
    pub parent: Arc<Block>,
    pub collidable: bool,
}

impl BlockState {
    pub fn to_block_data(&self, chunk_block_location: ChunkBlockLocation) -> BlockData {
        if self.parent.data_container {
            BlockData::Data(WorldBlock::new(chunk_block_location, self.get_ref()))
        } else {
            BlockData::Simple(self.state_id)
        }
    }
    pub fn get_full_id(&self) -> u32 {
        self.state_id
    }
    pub fn get_ref(&self) -> BlockStateRef {
        BlockStateRef {
            state_id: self.get_full_id(),
        }
    }
    pub fn on_break(&self, position: ChunkBlockLocation, player: &Entity) {
        if let Some(loottable) = &self.loottable {
            let loottable = player.server.loot_tables.get(loottable).unwrap();
            loottable.generate_items(|item| {
                player.inventory.get_full_view().add_item(&item);
            });
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientBlockRenderData {
    pub block_type: ClientBlockRenderDataType,
    pub dynamic: Option<ClientBlockDynamicData>,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
    pub selectable: bool,
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
    Static(ClientBlockStaticRenderData),
    Foliage(ClientBlockFoliageRenderData),
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

#[derive(Clone, Debug)]
pub struct ClientBlockStaticRenderData {
    pub model: String,
    pub texture: String,
}
#[derive(Clone, Debug)]
pub struct ClientBlockFoliageRenderData {
    pub texture_1: String,
    pub texture_2: String,
    pub texture_3: String,
    pub texture_4: String,
}

pub struct ItemRegistry {
    items: HashMap<Identifier, Arc<Item>, BuildHasherDefault<XxHash64>>,
    id_generator: u32,
}

impl ItemRegistry {
    pub fn new() -> Self {
        ItemRegistry {
            items: Default::default(),
            id_generator: 1,
        }
    }
    pub fn list(&self) -> Keys<Identifier, Arc<Item>> {
        self.items.keys()
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
    pub id: Identifier,
    pub client_data: ClientItemRenderData,
    pub client_id: u32,
    pub place_block: Option<Arc<Block>>,
    pub on_right_click: Option<ScriptCallback>,
    pub stack_size: u32,
    pub tool_data: Option<ToolData>,
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
                    if !world.collides_entity_with_block(block_position) {
                        if !player.entity_data.lock().creative {
                            item.add_count(-1);
                        }
                        Some(place.get_default_state_ref())
                    } else {
                        None
                    }
                }
                _ => None,
            });
            let target_chunk = world.get_chunk(block_position.to_chunk_pos()).unwrap();
            target_chunk.announce_to_viewers(NetworkMessageS2C::BlockItem(
                block_position.x,
                block_position.y,
                block_position.z,
                0,
                world
                    .server
                    .item_registry
                    .item_by_identifier(&Identifier::new("example", "log_block"))
                    .unwrap()
                    .client_id,
            ));
            /*target_chunk.announce_to_viewers(crate::net::NetworkMessageS2C::BlockAnimation(
                block_position.x,
                block_position.y,
                block_position.z,
                0,
            ));*/
            return InteractionResult::Consumed;
        }
        if let Some(right_click) = &self.on_right_click {
            right_click.call(&player.server.clone().engine, (player, block_position));
            return InteractionResult::Consumed;
        }
        InteractionResult::Ignored
    }
    pub fn on_right_click(&self, item: &mut ItemStack, player: Arc<Entity>) -> InteractionResult {
        if let Some(right_click) = &self.on_right_click {
            right_click.call(&player.server.clone().engine, (player, Dynamic::UNIT));
            return InteractionResult::Consumed;
        }
        InteractionResult::Ignored
    }
}

#[derive(PartialEq, Eq)]
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
    entities: HashMap<Identifier, Arc<EntityType>, BuildHasherDefault<XxHash64>>,
    id_generator: u32,
}

impl EntityRegistry {
    pub fn new() -> Self {
        EntityRegistry {
            entities: Default::default(),
            id_generator: 0,
        }
    }
    pub fn register<F>(&mut self, id: Identifier, creator: F) -> Result<Arc<EntityType>, ()>
    where
        F: FnOnce(u32) -> Arc<EntityType>,
    {
        if self.entities.get(&id).is_some() {
            return Err(());
        }
        let entity = creator.call_once((self.id_generator,));
        self.entities.insert(id, entity.clone());
        self.id_generator += 1;
        Ok(entity)
    }
    pub fn entity_by_identifier(&self, id: &Identifier) -> Option<&Arc<EntityType>> {
        self.entities.get(id)
    }
}

pub struct ItemModelMapping {
    pub mapping: HashMap<u32, u32>,
}

pub struct EntityType {
    pub id: u32,
    pub client_data: ClientEntityData,
    pub ticker: Mutex<Option<ScriptCallback>>,
    pub item_model_mapping: ItemModelMapping,
}

#[derive(Clone)]
pub struct ClientEntityData {
    pub model: String,
    pub texture: String,
    pub hitbox_w: f64,
    pub hitbox_h: f64,
    pub hitbox_d: f64,
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
                render_data: client_data.render_data,
                selectable: client_data.selectable,
                no_collide: !block.1.collidable
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
                ClientBlockRenderDataType::Static(static_data) => {
                    model_json.insert("type", "static").unwrap();
                    model_json
                        .insert("model", static_data.model.clone())
                        .unwrap();
                    model_json
                        .insert("texture", static_data.texture.clone())
                        .unwrap();
                }
                ClientBlockRenderDataType::Foliage(foliage_data) => {
                    model_json.insert("type", "foliage").unwrap();
                    model_json
                        .insert("texture1", foliage_data.texture_1.clone())
                        .unwrap();
                    model_json
                        .insert("texture2", foliage_data.texture_2.clone())
                        .unwrap();
                    model_json
                        .insert("texture3", foliage_data.texture_3.clone())
                        .unwrap();
                    model_json
                        .insert("texture4", foliage_data.texture_4.clone())
                        .unwrap();
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
                    id: item.1.client_id,
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

pub struct RecipeManager {
    recipes: HashMap<Identifier, Arc<Recipe>>,
    by_type: HashMap<Identifier, Vec<Arc<Recipe>>>,
}
impl RecipeManager {
    pub fn new(recipes: HashMap<Identifier, Arc<Recipe>>) -> Self {
        let mut by_type = HashMap::new();
        for (_, recipe) in &recipes {
            by_type
                .entry(recipe.get_type().clone())
                .or_insert_with(|| Vec::new())
                .push(recipe.clone());
        }
        RecipeManager { recipes, by_type }
    }
    pub fn by_id(&self, id: &Identifier) -> Option<Arc<Recipe>> {
        self.recipes.get(id).cloned()
    }
    pub fn by_type(&self, id: &Identifier) -> &Vec<Arc<Recipe>> {
        self.by_type.get(id).unwrap_or(&EMPTY_RECIPE_LIST)
    }
}
static EMPTY_RECIPE_LIST: Lazy<&'static mut Vec<Arc<Recipe>>> =
    Lazy::new(|| Box::leak(Box::new(Vec::new())));

#[derive(Clone)]
pub struct ToolData {
    pub durability: u32,
    pub speed: f32,
    pub hardness: f32,
    pub type_bitmap: u8,
}

impl ToolData {
    pub fn new(durability: u32, speed: f32, hardness: f32, types: Vec<ToolType>) -> Self {
        let mut type_bitmap = 0;
        for tool_type in types {
            type_bitmap |= tool_type as u8;
        }
        Self {
            durability,
            speed,
            hardness,
            type_bitmap,
        }
    }
    pub fn add_type(&mut self, tool_type: ToolType) {
        self.type_bitmap |= tool_type as u8;
    }
    pub fn breaks_type(&self, tool_type: ToolType) -> bool {
        (tool_type as u8) & self.type_bitmap > 0
    }
}

#[repr(u8)]
#[derive(Clone, Debug, Copy)]
pub enum ToolType {
    Axe = 1,
    Shovel = 2,
    Pickaxe = 4,
    Wrench = 8,
}
