use anyhow::anyhow;
use immutable_string::ImmutableString;
use std::ops::RangeInclusive;
use std::str::FromStr;
use std::sync::Weak;
use std::{
    collections::{hash_map::Keys, HashMap},
    hash::BuildHasherDefault,
    io::Write,
    sync::Arc,
};

use bbscript::eval::ExecutionEnvironment;
use bbscript::variant::{FromVariant, FunctionType, IntoVariant, Variant};
use block_byte_common::content::{
    ClientBlockData, ClientBlockRenderDataType, ClientContent, ClientEntityData, ClientItemData,
};
use block_byte_common::{BlockPosition, Face, HorizontalFace};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use strum_macros::{Display, EnumIter};
use twox_hash::XxHash64;
use zip::{write::FileOptions, DateTime, ZipWriter};

use crate::inventory::Recipe;
use crate::mods::{ClientContentData, ScriptingObject};
use crate::util::BlockLocation;
use crate::world::{Entity, PlayerData};
use crate::{
    inventory::ItemStack,
    mods::ScriptCallback,
    util::{ChunkBlockLocation, Identifier},
    world::{BlockData, Chunk, WorldBlock},
    Server,
};

pub struct StaticData {
    pub data: HashMap<String, Variant>,
}
impl StaticData {
    pub fn get(&self, id: &str) -> Option<&Variant> {
        self.data.get(id)
    }
    pub fn get_function(&self, id: &str) -> ScriptCallback {
        ScriptCallback {
            function: match self
                .data
                .get(id)
                .and_then(|variant| FunctionType::from_variant(variant))
            {
                Some(FunctionType::ScriptFunction(function)) => Some(function.clone()),
                _ => None,
            },
        }
    }
}

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
            .register(
                Identifier::new("bb", "air"),
                |default_state, id| {
                    Arc::new(Block {
                        id,
                        default_state,
                        data_container: None,
                        item_model_mapping: ItemModelMapping {
                            mapping: HashMap::new(),
                        },
                        properties: BlockStatePropertyStorage::new(),
                        networks: HashMap::new(),
                        static_data: StaticData {
                            data: HashMap::new(),
                        },
                    })
                },
                |_, _| ClientBlockData {
                    block_type: ClientBlockRenderDataType::Air,
                    dynamic: None,
                    fluid: false,
                    render_data: 0,
                    transparent: false,
                    selectable: false,
                    no_collide: true,
                },
            )
            .expect("couldn't register air");
        block_registry
    }
    pub fn register<F, T>(
        &mut self,
        id: Identifier,
        block_creator: F,
        state_creator: T,
    ) -> Result<u32, ()>
    where
        F: FnOnce(u32, Identifier) -> Arc<Block>,
        T: Fn(u32, &Block) -> ClientBlockData,
    {
        if self.blocks.get(&id).is_some() {
            return Err(());
        }
        let numeric_id = self.id_generator;
        let block = block_creator.call_once((self.id_generator, id.clone()));
        let mut block_states = Vec::new();
        for i in 0..block.properties.get_total_states() {
            let client_data = state_creator.call((i, &block));
            block_states.push(BlockState {
                parent: block.clone(),
                state_id: i,
                collidable: !(client_data.no_collide | client_data.fluid),
                client_data,
            });
        }
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
    pub fn state_by_ref(&self, block_state_ref: BlockStateRef) -> &BlockState {
        self.states.get(block_state_ref.state_id as usize).unwrap()
    }
    pub fn state_from_string(&self, state: &str) -> anyhow::Result<BlockStateRef> {
        let (block, props) = if state.contains('{') {
            let split = state.split_once('{').unwrap();
            (
                self.block_by_identifier(&Identifier::parse(split.0)?)
                    .ok_or(anyhow!("block not found"))?,
                Some(&split.1[0..split.1.len() - 1]),
            )
        } else {
            (
                self.block_by_identifier(&Identifier::parse(state)?)
                    .ok_or(anyhow!("block not found"))?,
                None,
            )
        };
        let mut state = 0;
        if let Some(props) = props {
            for prop in props.split(",") {
                let prop = prop.trim();
                if let Some((name, value)) = prop.split_once("=") {
                    state = block
                        .properties
                        .set_state_string(state, BlockStatePropertyKey::Name(name), value)
                        .unwrap_or(state);
                }
            }
        }
        Ok(block.get_state_ref(state))
    }
}

#[derive(Clone, Debug)]
pub struct BlockStatePropertyStorage {
    pub properties: Vec<(BlockStateProperty, u32)>,
    pub property_names: HashMap<String, u32>,
    pub total_states: u32,
}
impl BlockStatePropertyStorage {
    pub fn new() -> Self {
        BlockStatePropertyStorage {
            properties: Vec::new(),
            property_names: HashMap::new(),
            total_states: 1,
        }
    }
    pub fn register_property(&mut self, name: String, property: BlockStateProperty) {
        self.property_names
            .insert(name, self.properties.len() as u32);
        let num_states = property.get_num_states();
        self.properties.push((property, self.total_states));
        self.total_states *= num_states;
    }
    pub fn dump_properties(&self, state: u32) -> Variant {
        let mut map: HashMap<ImmutableString, _> = HashMap::new();
        for (name, i) in &self.property_names {
            map.insert(
                name.as_str().into(),
                self.get_from_state(state, BlockStatePropertyKey::Id(*i)),
            );
        }
        Arc::new(Mutex::new(map)).into_variant()
    }
    pub fn dump_properties_to_string(&self, state: u32) -> String {
        if self.total_states == 1 {
            return String::new();
        }
        let mut output = Vec::new();
        for (name, i) in &self.property_names {
            output.push(format!(
                "{}={}",
                name.to_string(),
                self.get_string_from_state(state, BlockStatePropertyKey::Id(*i))
            ));
        }
        format!("{{{}}}", output.join(","))
    }
    pub fn get_from_state(&self, state: u32, property: BlockStatePropertyKey) -> Variant {
        let (property, before_states) = self
            .properties
            .get(match property.to_id(&self.property_names) {
                Some(id) => id,
                None => return Variant::NULL(),
            })
            .unwrap();
        let state = (state / before_states) % property.get_num_states();
        property.from_id_to_value(state)
    }
    pub fn get_string_from_state(&self, state: u32, property: BlockStatePropertyKey) -> String {
        let (property, before_states) = self
            .properties
            .get(match property.to_id(&self.property_names) {
                Some(id) => id,
                None => panic!(),
            })
            .unwrap();
        let state = (state / before_states) % property.get_num_states();
        property.from_id_to_string(state)
    }
    pub fn set_state(
        &self,
        state: u32,
        property: BlockStatePropertyKey,
        value: Variant,
    ) -> Result<u32, u32> {
        let (property, before_states) = self
            .properties
            .get(property.to_id(&self.property_names).ok_or(state)?)
            .unwrap();
        let new_state =
            state - (((state / before_states) % property.get_num_states()) * before_states);
        match property.from_value_to_id(value) {
            Some(id) => Ok(new_state + (id * before_states)),
            None => Err(state),
        }
    }
    pub fn set_state_string(
        &self,
        state: u32,
        property: BlockStatePropertyKey,
        text: &str,
    ) -> Result<u32, u32> {
        let (property, before_states) = self
            .properties
            .get(property.to_id(&self.property_names).ok_or(state)?)
            .unwrap();
        let new_state =
            state - (((state / before_states) % property.get_num_states()) * before_states);
        match property.from_string_to_id(text) {
            Some(id) => Ok(new_state + (id * before_states)),
            None => Err(state),
        }
    }
    pub fn get_total_states(&self) -> u32 {
        self.total_states
    }
}
pub enum BlockStatePropertyKey<'a> {
    Id(u32),
    Name(&'a str),
}
impl<'a> BlockStatePropertyKey<'a> {
    pub fn to_id(&self, property_names: &HashMap<String, u32>) -> Option<usize> {
        match self {
            BlockStatePropertyKey::Id(id) => Some(*id as usize),
            BlockStatePropertyKey::Name(name) => property_names.get(*name).map(|id| *id as usize),
        }
    }
}
pub struct Block {
    pub id: Identifier,
    pub default_state: u32,
    pub data_container: Option<(u32,)>,
    pub item_model_mapping: ItemModelMapping,
    pub properties: BlockStatePropertyStorage,
    pub networks: HashMap<Identifier, ScriptCallback>,
    pub static_data: StaticData,
}

impl Block {
    pub fn get_state_ref(&self, state_id: u32) -> BlockStateRef {
        if state_id >= self.properties.get_total_states() {
            panic!();
        }
        BlockStateRef {
            state_id: self.default_state + state_id,
        }
    }
}
#[derive(Clone, Debug)]
pub enum BlockStateProperty {
    Face,
    HorizontalFace,
    Number(RangeInclusive<i32>),
    String(Vec<String>),
    Bool,
}
impl BlockStateProperty {
    pub fn get_num_states(&self) -> u32 {
        match self {
            BlockStateProperty::Face => 6,
            BlockStateProperty::HorizontalFace => 4,
            BlockStateProperty::Number(range) => (range.end() - range.start() + 1) as u32,
            BlockStateProperty::String(list) => list.len() as u32,
            BlockStateProperty::Bool => 2,
        }
    }
    pub fn from_value_to_id(&self, value: Variant) -> Option<u32> {
        match self {
            BlockStateProperty::Face => {
                let face = Face::from_variant(&value).cloned();
                face.map(|face| Face::all().iter().position(|f| *f == face).unwrap() as u32)
            }
            BlockStateProperty::HorizontalFace => {
                let face = HorizontalFace::from_variant(&value).cloned();
                face.map(|face| {
                    HorizontalFace::all()
                        .iter()
                        .position(|f| *f == face)
                        .unwrap() as u32
                })
            }
            BlockStateProperty::Number(range) => i64::from_variant(&value).and_then(|number| {
                let number = *number as i32;
                if range.contains(&number) {
                    Some((number - range.start()) as u32)
                } else {
                    None
                }
            }),
            BlockStateProperty::String(list) => {
                ImmutableString::from_variant(&value).and_then(|text| {
                    list.iter()
                        .position(|t| t.as_str() == text.as_ref())
                        .map(|pos| pos as u32)
                })
            }
            BlockStateProperty::Bool => {
                bool::from_variant(&value).map(|value| if *value { 1 } else { 0 })
            }
        }
    }
    pub fn from_id_to_string(&self, id: u32) -> String {
        match self {
            BlockStateProperty::Face => match id {
                0 => "front",
                1 => "back",
                2 => "up",
                3 => "down",
                4 => "left",
                5 => "right",
                _ => unreachable!(),
            }
            .to_string(),
            BlockStateProperty::HorizontalFace => match id {
                0 => "front",
                1 => "back",
                3 => "left",
                4 => "right",
                _ => unreachable!(),
            }
            .to_string(),
            BlockStateProperty::Number(range) => (*range.start() + id as i32).to_string(),
            BlockStateProperty::String(list) => list.get(id as usize).unwrap().clone(),
            BlockStateProperty::Bool => match id {
                0 => "false",
                1 => "true",
                _ => panic!(),
            }
            .to_string(),
        }
    }
    pub fn from_string_to_id(&self, text: &str) -> Option<u32> {
        match self {
            BlockStateProperty::Face => {
                let face = match text {
                    "front" => Face::Front,
                    "back" => Face::Back,
                    "left" => Face::Left,
                    "right" => Face::Right,
                    "up" => Face::Up,
                    "down" => Face::Down,
                    _ => return None,
                };
                Face::all()
                    .iter()
                    .position(|f| *f == face)
                    .map(|id| id as u32)
            }
            BlockStateProperty::HorizontalFace => {
                let face = match text {
                    "front" => HorizontalFace::Front,
                    "back" => HorizontalFace::Back,
                    "left" => HorizontalFace::Left,
                    "right" => HorizontalFace::Right,
                    _ => return None,
                };
                HorizontalFace::all()
                    .iter()
                    .position(|f| *f == face)
                    .map(|id| id as u32)
            }
            BlockStateProperty::Number(range) => text.parse().ok().and_then(|number: u32| {
                let number = number as i32;
                if range.contains(&number) {
                    Some((number - range.start()) as u32)
                } else {
                    None
                }
            }),
            BlockStateProperty::String(list) => list
                .iter()
                .position(|t| t.as_str() == text)
                .map(|pos| pos as u32),
            BlockStateProperty::Bool => match text {
                "true" => Some(1),
                "false" => Some(0),
                _ => None,
            },
        }
    }
    pub fn from_id_to_value(&self, id: u32) -> Variant {
        match self {
            BlockStateProperty::Face => Face::all()[id as usize].into_variant(),
            BlockStateProperty::HorizontalFace => HorizontalFace::all()[id as usize].into_variant(),
            BlockStateProperty::Number(range) => {
                ((id as i32 + range.start()) as i64).into_variant()
            }
            BlockStateProperty::String(list) => {
                Variant::from_str(list.get(id as usize).unwrap().as_str())
            }
            BlockStateProperty::Bool => (id == 1).into_variant(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BlockStateRef {
    state_id: u32,
}

impl BlockStateRef {
    pub const AIR: BlockStateRef = BlockStateRef { state_id: 0 };
    pub fn create_block_data(&self, chunk: &Chunk, position: BlockPosition) -> BlockData {
        chunk
            .world
            .server
            .block_registry
            .state_by_ref(*self)
            .to_block_data(chunk, position)
    }
    pub fn from_state_id(state_id: u32) -> Self {
        Self { state_id }
    }
    pub fn get_client_id(&self) -> u32 {
        self.state_id
    }
    pub fn get_id(&self) -> u32 {
        self.state_id
    }
    pub fn is_air(&self) -> bool {
        self.state_id == 0
    }
}

pub struct BlockState {
    pub state_id: u32,
    pub client_data: ClientBlockData,
    pub parent: Arc<Block>,
    pub collidable: bool,
}

impl BlockState {
    pub fn to_block_data(&self, chunk: &Chunk, position: BlockPosition) -> BlockData {
        if self.parent.data_container.is_some() {
            BlockData::Data(WorldBlock::new(
                ChunkBlockLocation::new(position, chunk.ptr()).unwrap(),
                self.get_ref(),
            ))
        } else {
            BlockData::Simple(self.get_full_id())
        }
    }
    pub fn get_full_id(&self) -> u32 {
        self.state_id + self.parent.default_state
    }
    pub fn get_ref(&self) -> BlockStateRef {
        BlockStateRef {
            state_id: self.get_full_id(),
        }
    }
    pub fn on_block_update(&self, location: ChunkBlockLocation) {
        let _ = self
            .parent
            .static_data
            .get_function("on_neighbor_update")
            .call_function(
                &location.chunk.world.server.script_environment,
                Some(Into::<BlockLocation>::into(&location).into_variant()),
                vec![],
            );
    }
    pub fn with_property(&self, property: &str, value: Variant) -> Result<BlockStateRef, ()> {
        self.parent
            .properties
            .set_state(self.state_id, BlockStatePropertyKey::Name(property), value)
            .map_err(|_| ())
            .map(|state| self.parent.get_state_ref(state))
    }
    pub fn get_property(&self, property: &str) -> Variant {
        self.parent
            .properties
            .get_from_state(self.state_id, BlockStatePropertyKey::Name(property))
    }
}
impl ToString for BlockState {
    fn to_string(&self) -> String {
        format!(
            "{}{}",
            self.parent.id.to_string(),
            self.parent
                .properties
                .dump_properties_to_string(self.state_id)
        )
    }
}
impl ScriptingObject for BlockState {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<BlockStateRef, _>("BlockState");
        {
            let server = server.clone();
            env.register_function("BlockState", move |state: &ImmutableString| {
                Ok(Variant::from_option(
                    server
                        .upgrade()
                        .unwrap()
                        .block_registry
                        .state_from_string(state)
                        .ok(),
                ))
            });
        }
        {
            let server = server.clone();
            env.register_method(
                "with_property",
                move |state: &BlockStateRef, property: &ImmutableString, value: &Variant| {
                    let server = server.upgrade().unwrap();
                    let block_state = server.block_registry.state_by_ref(*state);
                    Ok(Variant::from_option(
                        block_state.with_property(property, value.clone()).ok(),
                    ))
                },
            );
        }
        {
            let server = server.clone();
            env.register_method(
                "get_property",
                move |state: &BlockStateRef, property: &ImmutableString| {
                    let server = server.upgrade().unwrap();
                    let block_state = server.block_registry.state_by_ref(*state);
                    Ok(block_state.get_property(property))
                },
            );
        }
        {
            let server = server.clone();
            env.register_method("to_string", move |state: &BlockStateRef| {
                Ok(Variant::from_str(
                    server
                        .upgrade()
                        .unwrap()
                        .block_registry
                        .state_by_ref(*state)
                        .to_string()
                        .as_str(),
                ))
            });
        }
        env.register_method(
            "operator==",
            |first: &BlockStateRef, second: &BlockStateRef| {
                Ok((first.state_id == second.state_id).into_variant())
            },
        );
    }
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
    pub client_data: ClientItemData,
    pub client_id: u32,
    pub stack_size: u32,
    pub static_data: StaticData,
}

impl Item {
    pub fn on_right_click_block(
        &self,
        item: &mut ItemStack,
        player: Arc<PlayerData>,
        block_position: BlockPosition,
        block_face: Face,
    ) -> InteractionResult {
        let mut new_item = item.clone().into_variant();
        let result = *InteractionResult::from_variant(
            &self
                .static_data
                .get_function("on_right_click_block")
                .call_function(
                    &player.server.clone().script_environment,
                    Some(new_item),
                    vec![
                        player.into_variant(),
                        block_position.into_variant(),
                        block_face.into_variant(),
                    ],
                )
                .unwrap(),
        )
        .unwrap();
        //*item = new_item.cast();
        result
    }
    pub fn on_right_click(
        &self,
        item: &mut ItemStack,
        player: Arc<PlayerData>,
        entity: Option<Arc<Entity>>,
    ) -> InteractionResult {
        let mut new_item = item.clone().into_variant();
        let result = InteractionResult::from_variant(
            &self
                .static_data
                .get_function("on_right_click")
                .call_function(
                    &player.server.clone().script_environment,
                    Some(new_item),
                    vec![player.into_variant(), Variant::from_option(entity)],
                )
                .unwrap(),
        )
        .unwrap()
        .clone();
        result
    }
}

#[derive(Copy, PartialEq, Eq, Clone, Debug, Display, EnumIter)]
pub enum InteractionResult {
    Consumed,
    Ignored,
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
    pub id: Identifier,
    pub client_id: u32,
    pub client_data: ClientEntityData,
    pub item_model_mapping: ItemModelMapping,
    pub static_data: StaticData,
}

pub struct ClientContentGenerator {}

impl ClientContentGenerator {
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
                .write_all(include_bytes!("assets/font.ttf"))
                .unwrap();
        }
        zip_writer.finish().unwrap().into_inner()
    }
    pub fn generate_content_json(
        block_registry: &BlockRegistry,
        item_registry: &ItemRegistry,
        entity_registry: &EntityRegistry,
    ) -> String {
        serde_json::to_string(&ClientContent {
            blocks: block_registry
                .states
                .iter()
                .map(|state| state.client_data.clone())
                .collect(),
            items: {
                let mut items: Vec<_> = item_registry
                    .items
                    .iter()
                    .map(|item| (item.1.client_id, item.1.client_data.clone()))
                    .collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                items.iter().map(|item| item.1.clone()).collect()
            },
            entities: {
                let mut entities: Vec<_> = entity_registry
                    .entities
                    .iter()
                    .map(|entity| (entity.1.client_id, entity.1.client_data.clone()))
                    .collect();
                entities.sort_by(|a, b| a.0.cmp(&b.0));
                entities.iter().map(|item| item.1.clone()).collect()
            },
        })
        .unwrap()
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
