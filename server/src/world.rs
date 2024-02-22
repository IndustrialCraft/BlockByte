use std::any::{Any, TypeId};
use std::fmt::Formatter;
use std::hash::Hasher;
use std::ops::{Add, Range};
use std::sync::atomic::Ordering;
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8},
        Arc, Weak,
    },
};

use array_init::array_init;
use atomic_counter::{AtomicCounter, RelaxedCounter};
use bbscript::eval::ExecutionEnvironment;
use bbscript::lex::FilePosition;
use bbscript::variant::{
    FromVariant, FunctionType, FunctionVariant, IntoVariant, Primitive, Variant,
};
use bitcode::__private::Serialize;
use block_byte_common::gui::{GUIComponent, GUIElement, GUIElementEdit, PositionAnchor};
use block_byte_common::messages::{
    ClientModelTarget, MovementType, NetworkMessageC2S, NetworkMessageS2C,
};
use block_byte_common::{
    BlockPosition, ChunkPosition, Color, Face, KeyboardKey, KeyboardModifier, Position, Vec2, AABB,
};
use flate2::Compression;
use fxhash::{FxHashMap, FxHashSet};
use immutable_string::ImmutableString;
use json::{object, JsonValue};
use parking_lot::Mutex;
use pathfinding::prelude::astar;
use rand::{thread_rng, Rng};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serializer};
use uuid::Uuid;

use crate::inventory::{
    GUILayout, GuiInventoryData, GuiInventoryViewer, InventorySaveData, InventoryView,
    LootTableGenerationParameters, ModGuiViewer,
};
use crate::mods::{ScriptCallback, ScriptingObject, UserDataWrapper};
use crate::registry::{Block, BlockState};
use crate::util::BlockLocation;
use crate::{
    inventory::{Inventory, InventoryWrapper, ItemStack, WeakInventoryWrapper},
    net::PlayerConnection,
    registry::{BlockRegistry, BlockStateRef, EntityType, InteractionResult},
    util::{ChunkBlockLocation, ChunkLocation, Identifier, Location},
    worldgen::WorldGenerator,
    Server,
};

pub struct World {
    pub server: Arc<Server>,
    this: Weak<Self>,
    chunks: Mutex<FxHashMap<ChunkPosition, Arc<Chunk>>>,
    unload_timer: RelaxedCounter,
    world_generator: Box<dyn WorldGenerator + Send + Sync>,
    unloaded_structure_placements:
        Mutex<HashMap<ChunkPosition, Vec<(BlockPosition, Arc<Structure>)>>>,
    pub id: Identifier,
    temporary: bool,
    pub user_data: Mutex<UserData>,
}

impl World {
    const UNLOAD_TIME: usize = 1000;
    pub fn new(
        server: Arc<Server>,
        world_generator: Box<dyn WorldGenerator + Send + Sync>,
        id: Identifier,
    ) -> Arc<Self> {
        let world = Arc::new_cyclic(|this| World {
            this: this.clone(),
            chunks: Mutex::new(FxHashMap::default()),
            server,
            unload_timer: RelaxedCounter::new(0),
            world_generator,
            unloaded_structure_placements: Mutex::new(HashMap::new()),
            id,
            temporary: false,
            user_data: Mutex::new(UserData::new()),
        });
        std::fs::create_dir_all(world.get_world_path()).unwrap();
        world
    }
    pub fn drop_item_on_ground(
        &self,
        position: Position,
        item: ItemStack,
        rotation: Option<f32>,
        velocity: Option<(f64, f64, f64)>,
    ) {
        let item_entity = Entity::new(
            &Location {
                world: self.ptr(),
                position,
            },
            self.server
                .entity_registry
                .entity_by_identifier(&Identifier::new("bb", "item"))
                .unwrap(),
        );
        item_entity
            .inventory
            .get_full_view()
            .set_item(0, Some(item))
            .unwrap();

        if let Some(velocity) = velocity {
            item_entity.apply_knockback(velocity.0, velocity.1, velocity.2);
        }
        if let Some(rotation) = rotation {
            *item_entity.rotation_shifting.lock() = (rotation, false);
        }
    }
    pub fn scatter_items(&self, position: Position, items: Vec<ItemStack>, one_by_one: bool) {
        for item in items {
            for _ in 0..item.get_count() {
                let rotation: f32 = thread_rng().gen_range((0.)..(360.));
                let rotation_radians = rotation.to_radians();
                let vertical_strength = 0.4;
                let horizontal_strength = 0.2;
                self.drop_item_on_ground(
                    position,
                    if one_by_one {
                        item.copy(1)
                    } else {
                        item.clone()
                    },
                    Some(rotation),
                    Some((
                        rotation_radians.sin() as f64 * horizontal_strength,
                        vertical_strength,
                        rotation_radians.cos() as f64 * horizontal_strength,
                    )),
                );
                if !one_by_one {
                    break;
                }
            }
        }
    }
    pub fn get_world_path(&self) -> PathBuf {
        let mut path = self.server.save_directory.clone();
        path.push("worlds");
        path.push(self.id.to_string());
        path
    }
    pub fn place_structure(
        &self,
        position: BlockPosition,
        structure: &Arc<Structure>,
        load_chunks: bool,
    ) {
        let chunks = structure.get_chunks(position);
        for chunk_position in chunks {
            let chunk = if load_chunks {
                Some(self.load_chunk(chunk_position))
            } else {
                self.get_chunk(chunk_position)
            };
            let loaded = chunk
                .as_ref()
                .map(|chunk| {
                    chunk
                        .loading_stage
                        .load(std::sync::atomic::Ordering::SeqCst)
                        > 0
                })
                .unwrap_or(false);
            {
                if loaded {
                    chunk.unwrap().place_structure(position, structure.clone());
                } else {
                    let mut unloaded_structure_placements =
                        self.unloaded_structure_placements.lock();

                    if !unloaded_structure_placements.contains_key(&chunk_position) {
                        unloaded_structure_placements.insert(chunk_position, Vec::new());
                    }
                    let placement_list = unloaded_structure_placements
                        .get_mut(&chunk_position)
                        .unwrap();
                    placement_list.push((position, structure.clone()));
                }
            }
        }
    }
    pub fn get_chunks_with_center_radius(
        &self,
        position: ChunkPosition,
        radius: u32,
    ) -> Vec<Arc<Chunk>> {
        let radius = radius as i32;
        let mut chunks = Vec::new();
        for x in (-radius)..=radius {
            for y in (-radius)..=radius {
                for z in (-radius)..=radius {
                    let chunk = self.get_chunk(ChunkPosition {
                        x: position.x + x,
                        y: position.y + y,
                        z: position.z + z,
                    });
                    if let Some(chunk) = chunk {
                        chunks.push(chunk);
                    }
                }
            }
        }
        chunks
    }
    pub fn collides_entity_with_block(&self, position: BlockPosition) -> bool {
        let chunks = self.get_chunks_with_center_radius(position.to_chunk_pos(), 1);
        for chunk in chunks {
            for entity in &*chunk.entities.lock() {
                if entity
                    .get_collider()
                    .iter_blocks()
                    .find(|block_position| block_position == &position)
                    .is_some()
                {
                    return true;
                }
            }
        }
        false
    }
    pub fn set_block(
        &self,
        position: BlockPosition,
        block: BlockStateRef,
        update_neighbors: bool,
        player: Option<Arc<PlayerData>>,
    ) {
        let chunk_offset = position.chunk_offset();
        self.load_chunk(position.to_chunk_pos()).set_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
            block,
            update_neighbors,
            player,
        );
    }
    pub fn get_block_load(&self, position: BlockPosition) -> BlockData {
        let chunk_offset = position.chunk_offset();
        self.load_chunk(position.to_chunk_pos()).get_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
        )
    }
    pub fn get_block(&self, position: &BlockPosition) -> Option<BlockData> {
        let chunk_offset = position.chunk_offset();
        self.get_chunk(position.to_chunk_pos())
            .map(|chunk| chunk.get_block(chunk_offset.0, chunk_offset.1, chunk_offset.2))
    }

    pub fn replace_block<F>(
        &self,
        position: BlockPosition,
        replacer: F,
        update_neighbors: bool,
        player: Option<Arc<PlayerData>>,
    ) where
        F: FnOnce(BlockData) -> Option<BlockStateRef>,
    {
        let chunk_offset = position.chunk_offset();
        let chunk = self.load_chunk(position.to_chunk_pos());
        let new_block =
            replacer.call_once((chunk.get_block(chunk_offset.0, chunk_offset.1, chunk_offset.2),));
        if let Some(new_block) = new_block {
            chunk.set_block(
                chunk_offset.0,
                chunk_offset.1,
                chunk_offset.2,
                new_block,
                update_neighbors,
                player,
            );
        }
    }
    pub fn load_chunk(&self, position: ChunkPosition) -> Arc<Chunk> {
        {
            let chunks = self.chunks.lock();
            if let Some(chunk) = chunks.get(&position) {
                return chunk.clone();
            }
        }
        let mut chunks = self.chunks.lock();
        let chunk = Chunk::new(position, self.this.upgrade().unwrap());
        chunks.insert(position, chunk.clone());
        chunk
    }
    pub fn get_chunk(&self, position: ChunkPosition) -> Option<Arc<Chunk>> {
        let chunks = self.chunks.lock();
        chunks.get(&position).map(|c| c.clone())
    }
    pub fn tick(&self) {
        let mut chunks = self.chunks.lock();
        chunks
            .extract_if(|_, chunk| {
                let should_unload = chunk.tick();
                if should_unload {
                    chunk.destroy();
                }
                should_unload
            })
            .count();
        if chunks.len() > 0 {
            self.unload_timer.reset();
        } else {
            self.unload_timer.inc();
        }
    }
    pub fn ptr(&self) -> Arc<World> {
        self.this.upgrade().unwrap()
    }
    pub fn should_unload(&self) -> bool {
        self.unload_timer.get() >= World::UNLOAD_TIME
    }
    pub fn destroy(&self) {
        for chunk in self.chunks.lock().drain() {
            chunk.1.destroy();
        }
    }
}
impl ScriptingObject for World {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<World>, _>("World");
        {
            let server = server.clone();
            env.register_function("load_world", move |id: &ImmutableString| {
                Ok(server
                    .upgrade()
                    .unwrap()
                    .get_or_create_world(Identifier::parse(id.as_ref()).unwrap()))
            });
        }
        env.register_member("user_data", |world: &Arc<World>| {
            Some(UserDataWrapper::World(world.ptr()).into_variant())
        });
        /*engine.register_fn(
            "place_structure",
            |world: &mut Arc<World>, structure: Arc<Structure>, position: BlockPosition| {
                world.place_structure(position, &structure, true);
            },
        );
        engine.register_fn(
            "get_structure",
            |world: &mut Arc<World>,
             first: BlockPosition,
             second: BlockPosition,
             origin: BlockPosition| {
                Structure::from_world(&world, first, second, origin)
            },
        );*/
    }
}
impl Eq for World {}
impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.this, &other.this)
    }
}

pub struct BlockBreakParameters {
    pub player: Option<Arc<Entity>>,
    pub item: Option<ItemStack>,
}
impl BlockBreakParameters {
    pub fn from_entity(entity: &Entity) -> Self {
        BlockBreakParameters {
            player: Some(entity.ptr()),
            item: entity.get_hand_item(),
        }
    }
}

#[derive(Clone)]
pub enum BlockData {
    Simple(u32),
    Data(Arc<WorldBlock>),
}

impl BlockData {
    pub fn get_client_id(&self) -> u32 {
        match self {
            Self::Simple(id) => *id,
            Self::Data(block) => block.state.get_client_id(),
        }
    }
    pub fn get_block_state(&self) -> BlockStateRef {
        match self {
            Self::Simple(id) => BlockStateRef::from_state_id(*id),
            Self::Data(block) => block.state,
        }
    }
    pub fn is_air(&self) -> bool {
        match self {
            BlockData::Simple(id) => *id == 0,
            BlockData::Data(_) => false,
        }
    }
    pub fn is_collidable(&self, block_registry: &BlockRegistry) -> bool {
        let state = match self {
            BlockData::Simple(id) => BlockStateRef::from_state_id(*id),
            BlockData::Data(data) => data.state,
        };
        let state = block_registry.state_by_ref(state);
        state.collidable
    }
}

pub struct Chunk {
    pub position: ChunkPosition,
    pub world: Arc<World>,
    blocks: Mutex<[[[BlockData; 16]; 16]; 16]>,
    entities: Mutex<Vec<Arc<Entity>>>,
    viewers: Mutex<FxHashSet<ChunkViewer>>,
    loading_stage: AtomicU8,
    ticking_blocks: Mutex<HashSet<(u8, u8, u8)>>,
    scheduled_updates: Mutex<HashSet<(u8, u8, u8)>>,
    this: Weak<Chunk>,
}

impl Chunk {
    const UNLOAD_TIME: u8 = 200;
    pub fn new(position: ChunkPosition, world: Arc<World>) -> Arc<Self> {
        let chunk = Arc::new_cyclic(|this| Chunk {
            position,
            blocks: Mutex::new(array_init(|_| {
                array_init(|_| array_init(|_| BlockData::Simple(0)))
            })),
            world: world.clone(),
            entities: Mutex::new(Vec::new()),
            viewers: Mutex::new(FxHashSet::default()),
            loading_stage: AtomicU8::new(0),
            ticking_blocks: Mutex::new(HashSet::new()),
            scheduled_updates: Mutex::new(HashSet::new()),
            this: this.clone(),
        });
        let gen_chunk = chunk.clone();
        world.clone().server.thread_pool.execute(Box::new(move || {
            {
                let save_path = gen_chunk.get_chunk_path();
                *gen_chunk.blocks.lock() = match gen_chunk.load_from_save(save_path) {
                    Ok((blocks, entities)) => {
                        if entities.len() > 0 {}
                        for entity_data in entities {
                            let entity = Entity::new(
                                ChunkLocation {
                                    position: entity_data.position,
                                    chunk: gen_chunk.clone(),
                                },
                                gen_chunk
                                    .world
                                    .server
                                    .entity_registry
                                    .entity_by_identifier(&entity_data.entity_type)
                                    .unwrap(),
                            );
                            *entity.user_data.lock() = entity_data.user_data;
                            *entity.velocity.lock() = entity_data.velocity;
                            entity.rotation_shifting.lock().0 = entity_data.rotation;
                            entity.inventory.deserialize(
                                entity_data.inventory,
                                &gen_chunk.world.server.item_registry,
                            );
                        }
                        blocks
                    }
                    Err(()) => {
                        gen_chunk.ticking_blocks.lock().clear();
                        gen_chunk.world.world_generator.generate(&gen_chunk)
                    }
                };
                for x in 0..16 {
                    for y in 0..16 {
                        for z in 0..16 {
                            match gen_chunk.get_block(x, y, z) {
                                BlockData::Simple(_) => {}
                                BlockData::Data(data) => data.on_place(),
                            }
                        }
                    }
                }
            }

            gen_chunk
                .loading_stage
                .store(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(placement_list) = {
                gen_chunk
                    .world
                    .unloaded_structure_placements
                    .lock()
                    .remove(&position)
            } {
                for (position, structure) in placement_list {
                    gen_chunk.place_structure(position, structure);
                }
            }
            gen_chunk
                .loading_stage
                .store(2, std::sync::atomic::Ordering::SeqCst);
        }));
        chunk
    }
    pub fn schedule_update(&self, block: (u8, u8, u8)) {
        self.scheduled_updates.lock().insert(block);
    }
    pub fn set_ticking_enabled(&self, block: (u8, u8, u8), enabled: bool) {
        if enabled {
            self.ticking_blocks.lock().insert(block);
        } else {
            self.ticking_blocks.lock().remove(&block);
        }
    }
    pub fn load_from_save(
        &self,
        save_path: PathBuf,
    ) -> Result<([[[BlockData; 16]; 16]; 16], Vec<EntitySaveData>), ()> {
        let mut chunk_save_data = bitcode::deserialize::<ChunkSaveData>(
            std::fs::read(save_path).map_err(|_| ())?.as_slice(),
        )
        .map_err(|_| ())?;
        let block_registry = &self.world.server.block_registry;
        let block_palette: Vec<_> = chunk_save_data
            .palette
            .iter()
            .map(|id| (block_registry.block_by_identifier(&id.0), id.1))
            .collect();
        let blocks = array_init(|x| {
            array_init(|y| {
                array_init(|z| {
                    let block_id = chunk_save_data.blocks[x][y][z];
                    let block = block_palette.get(block_id as usize).unwrap();
                    let offset = (x as u8, y as u8, z as u8);
                    let block_state_ref = match block.0 {
                        Some(block_id) => block_id.get_state_ref(block.1),
                        None => BlockStateRef::AIR,
                    };
                    let block_data = block_state_ref.create_block_data(
                        self,
                        BlockPosition {
                            x: (self.position.x * 16) + x as i32,
                            y: (self.position.y * 16) + y as i32,
                            z: (self.position.z * 16) + z as i32,
                        },
                    );

                    if let BlockData::Data(block) = &block_data {
                        if let Some(data) = chunk_save_data.block_data.remove(&offset) {
                            block.deserialize(data);
                        }
                    }
                    block_data
                })
            })
        });
        Ok((blocks, chunk_save_data.entities))
    }
    pub fn ptr(&self) -> Arc<Chunk> {
        self.this.upgrade().unwrap()
    }
    pub fn place_structure(&self, position: BlockPosition, structure: Arc<Structure>) {
        structure.place(
            |block_position, block| {
                if block_position.to_chunk_pos() == self.position {
                    let offset = block_position.chunk_offset();
                    self.set_block(offset.0, offset.1, offset.2, block, false, None);
                }
            },
            position,
        );
    }
    pub fn set_block(
        &self,
        offset_x: u8,
        offset_y: u8,
        offset_z: u8,
        block: BlockStateRef,
        update_neighbors: bool,
        player: Option<Arc<PlayerData>>,
    ) {
        match self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize] {
            BlockData::Simple(id) => {
                if block.get_id() == id {
                    return;
                }
            }
            BlockData::Data(_) => {}
        }
        let block_position = BlockPosition {
            x: self.position.x * 16 + offset_x as i32,
            y: self.position.y * 16 + offset_y as i32,
            z: self.position.z * 16 + offset_z as i32,
        };
        let block_location = BlockLocation {
            world: self.world.clone(),
            position: block_position,
        };
        let previous_block =
            self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize].clone();
        match &previous_block {
            BlockData::Simple(_) => {}
            BlockData::Data(data) => {
                data.on_destroy();
            }
        }
        let previous_block = &self
            .world
            .server
            .block_registry
            .state_by_ref(previous_block.get_block_state())
            .parent;
        if let Some(player) = &player {
            if let Some(loottable) = &self.world.server.loot_tables.get(&previous_block.id) {
                self.world.scatter_items(
                    Position {
                        x: block_position.x as f64 + 0.5,
                        y: block_position.y as f64 + 0.5,
                        z: block_position.z as f64 + 0.5,
                    },
                    loottable.generate_items(LootTableGenerationParameters {
                        item: player.get_entity().get_hand_item().as_ref(),
                    }),
                    true,
                );
            }
        }
        let _ = previous_block
            .static_data
            .get_function("on_destroy")
            .call_function(
                &self.world.server.script_environment,
                Some(block_location.clone().into_variant()),
                vec![player.clone().into_variant()],
            );
        let new_block = &self.world.server.block_registry.state_by_ref(block).parent;
        let block = block.create_block_data(&self.this.upgrade().unwrap(), block_position);
        if self.loading_stage.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            self.announce_to_viewers(&NetworkMessageS2C::SetBlock(
                block_position,
                block.get_client_id(),
            ));
        }
        let offset = (offset_x, offset_y, offset_z);
        self.ticking_blocks.lock().remove(&offset);
        let new_block_data = match &block {
            BlockData::Simple(_) => None,
            BlockData::Data(data) => Some(data.clone()),
        };
        self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize] = block;
        let _ = new_block.static_data.get_function("on_set").call_function(
            &self.world.server.script_environment,
            Some(block_location.into_variant()),
            vec![player.into_variant()],
        );
        if let Some(new_block_data) = new_block_data {
            new_block_data.on_place();
            new_block_data.update_to_clients();
        }
        if update_neighbors {
            self.schedule_update((offset_x, offset_y, offset_z));
            for neighbor_face in Face::all() {
                let neighbor_position = block_position.offset_by_face(*neighbor_face);
                if let Some(chunk) = self.world.get_chunk(neighbor_position.to_chunk_pos()) {
                    chunk.schedule_update(neighbor_position.chunk_offset());
                }
            }
        }
    }
    pub fn get_block(&self, offset_x: u8, offset_y: u8, offset_z: u8) -> BlockData {
        self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize].clone()
    }
    fn add_entity(&self, entity: Arc<Entity>) {
        self.entities.lock().push(entity);
    }
    fn add_viewer(&self, viewer: Arc<PlayerData>) {
        self.viewers.lock().insert(ChunkViewer {
            player: viewer.clone(),
        });
        viewer.chunk_loading_manager.load(self.ptr());
        for entity in self.entities.lock().iter() {
            if entity.id == viewer.get_entity().id {
                continue;
            }
            viewer.send_messages(&entity.create_add_messages(entity.get_location().position));
        }
    }
    fn remove_viewer(&self, viewer: &PlayerData, unload_entities: bool) {
        viewer.chunk_loading_manager.unload(self.ptr());
        if unload_entities {
            for entity in self.entities.lock().iter() {
                if entity.id == viewer.get_entity().id {
                    continue;
                }
                viewer.send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id));
            }
        }
        self.viewers.lock().remove(&ChunkViewer {
            player: viewer.ptr(),
        });
    }
    pub fn announce_to_viewers_except(&self, message: NetworkMessageS2C, player: &Entity) {
        for viewer in self.viewers.lock().iter() {
            if viewer.player.get_entity().id != player.id {
                viewer.player.send_message(&message);
            }
        }
    }
    pub fn announce_to_viewers(&self, message: &NetworkMessageS2C) {
        for viewer in self.viewers.lock().iter() {
            viewer.player.send_message(message);
        }
    }
    pub fn tick(&self) -> bool {
        let mut entities = self.entities.lock();
        entities
            .extract_if(|entity| {
                let new_location = entity.get_location();
                let not_same_chunk = new_location.chunk.position != self.position;
                if not_same_chunk {
                    for viewer in self
                        .viewers
                        .lock()
                        .difference(&new_location.chunk.viewers.lock())
                    {
                        viewer
                            .player
                            .send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id));
                    }
                }
                let removed = entity.is_removed();
                if removed && !not_same_chunk {
                    for viewer in self.viewers.lock().iter() {
                        viewer
                            .player
                            .send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id));
                    }
                    entity.post_remove();
                }
                removed || not_same_chunk
            })
            .count();
        let entities: Vec<_> = entities.iter().map(|e| e.clone()).collect();
        let blocks: Vec<_> = {
            let blocks = self.blocks.lock();
            self.ticking_blocks
                .lock()
                .iter()
                .map(|e| {
                    (
                        self.world
                            .server
                            .block_registry
                            .state_by_ref(
                                blocks[e.0 as usize][e.1 as usize][e.2 as usize].get_block_state(),
                            )
                            .parent
                            .clone(),
                        BlockLocation {
                            world: self.world.clone(),
                            position: BlockPosition {
                                x: self.position.x * 16 + e.0 as i32,
                                y: self.position.y * 16 + e.1 as i32,
                                z: self.position.z * 16 + e.2 as i32,
                            },
                        },
                    )
                })
                .collect()
        };
        let block_updates: Vec<_> = { self.scheduled_updates.lock().drain().collect() };
        if entities.len() > 0 || blocks.len() > 0 || block_updates.len() > 0 {
            let chunk = self.ptr();
            self.world.server.thread_pool.execute(Box::new(move || {
                for entity in entities {
                    entity.tick();
                }
                for block in blocks {
                    let _ = block.0.static_data.get_function("on_tick").call_function(
                        &chunk.world.server.script_environment,
                        Some(block.1.into_variant()),
                        vec![],
                    );
                }
                for block_update in block_updates {
                    let state = chunk.world.server.block_registry.state_by_ref(
                        chunk
                            .get_block(block_update.0, block_update.1, block_update.2)
                            .get_block_state(),
                    );
                    state.on_block_update(ChunkBlockLocation {
                        chunk: chunk.clone(),
                        position: BlockPosition {
                            x: chunk.position.x * 16 + block_update.0 as i32,
                            y: chunk.position.y * 16 + block_update.1 as i32,
                            z: chunk.position.z * 16 + block_update.2 as i32,
                        },
                    })
                }
            }));
        }
        self.viewers.lock().len() == 0
    }
    pub fn destroy(&self) {
        let chunk = self.this.upgrade().unwrap();
        if !self.world.temporary {
            self.world.server.thread_pool.execute(Box::new(move || {
                let mut blocks_save = [[[0u16; 16]; 16]; 16];
                let mut block_map = FxHashMap::default();
                let blocks = chunk.blocks.lock();
                let block_registry = &chunk.world.server.block_registry;
                let mut block_data = HashMap::new();
                for x in 0..16 {
                    for y in 0..16 {
                        for z in 0..16 {
                            let block = &blocks[x][y][z];
                            let (block_state_ref, serialized_block) = match block {
                                BlockData::Simple(id) => (BlockStateRef::from_state_id(*id), None),
                                BlockData::Data(block) => (block.state, Some(block.serialize())),
                            };
                            let block = block_registry.state_by_ref(block_state_ref);
                            let block_map_len = block_map.len();
                            let numeric_id = *block_map
                                .entry((&block.parent.id, block.state_id))
                                .or_insert(block_map_len);
                            blocks_save[x][y][z] = numeric_id as u16;
                            if let Some(serialized_block) = serialized_block {
                                block_data.insert((x as u8, y as u8, z as u8), serialized_block);
                            }
                        }
                    }
                }
                let mut entities = Vec::new();
                for entity in chunk.entities.lock().iter() {
                    let position = entity.get_location().position;
                    if position.to_chunk_pos() != chunk.position
                        || entity.is_removed()
                        || entity.get_player().is_some()
                    {
                        continue;
                    }
                    entities.push(EntitySaveData {
                        entity_type: entity.entity_type.id.clone(),
                        velocity: entity.velocity.lock().clone(),
                        rotation: entity.get_rotation(),
                        position,
                        inventory: entity.inventory.serialize(),
                        user_data: entity.user_data.lock().clone(),
                    });
                }
                let chunk_save_data = ChunkSaveData {
                    blocks: blocks_save,
                    palette: {
                        let mut block_map: Vec<_> = block_map.iter().collect();
                        block_map.sort_by(|first, second| first.1.cmp(second.1));
                        block_map.iter().map(|e| (e.0 .0.clone(), e.0 .1)).collect()
                    },
                    block_data,
                    entities,
                };
                std::fs::write(
                    chunk.get_chunk_path(),
                    bitcode::serialize(&chunk_save_data).unwrap(),
                )
                .unwrap();
                chunk.entities.lock().clear();
            }));
        }
        self.viewers.lock().clear();
    }
    pub fn get_chunk_path(&self) -> PathBuf {
        let mut path = self.world.get_world_path();
        path.push(format!(
            "chunk{},{},{}.bws",
            self.position.x, self.position.y, self.position.z
        ));
        path
    }
}
impl Eq for Chunk {}
impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position && (self.world == other.world)
    }
}
impl Hash for Chunk {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.position.hash(state);
    }
}
#[derive(Serialize, Deserialize)]
pub struct ChunkSaveData {
    palette: Vec<(Identifier, u32)>,
    blocks: [[[u16; 16]; 16]; 16],
    block_data: HashMap<(u8, u8, u8), BlockSaveData>,
    entities: Vec<EntitySaveData>,
}
#[derive(Serialize, Deserialize)]
pub struct BlockSaveData {
    inventory: InventorySaveData,
}
#[derive(Serialize, Deserialize)]
pub struct EntitySaveData {
    position: Position,
    rotation: f32,
    entity_type: Identifier,
    inventory: InventorySaveData,
    velocity: (f64, f64, f64),
    user_data: UserData,
}

struct ChunkViewer {
    pub player: Arc<PlayerData>,
}

impl Hash for ChunkViewer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.player.get_entity().id.hash(state)
    }
}

impl PartialEq for ChunkViewer {
    fn eq(&self, other: &Self) -> bool {
        self.player.get_entity().id == other.player.get_entity().id
    }
}

impl Eq for ChunkViewer {}

#[derive(Clone)]
pub struct UserData(pub HashMap<Identifier, Variant>);
impl UserData {
    pub fn new() -> Self {
        UserData(HashMap::new())
    }
}

impl Serialize for UserData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_unit()
    }
}
impl<'de> Deserialize<'de> for UserData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_unit(UserDataVisitor)?;
        Ok(UserData::new())
    }
}
struct UserDataVisitor;
impl<'de> serde::de::Visitor<'de> for UserDataVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("unit")
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(())
    }
}

pub struct PlayerData {
    entity: Mutex<Arc<Entity>>,
    pub connection: Mutex<PlayerConnection>,
    pub open_inventory: Mutex<Option<(InventoryWrapper, Uuid)>>,
    pub chunk_loading_manager: ChunkLoadingManager,
    pub speed: Mutex<f32>,
    pub move_type: Mutex<MovementType>,
    pub hand_item: Mutex<Option<ItemStack>>,
    pub user_data: Mutex<UserData>,
    pub server: Arc<Server>,
    pub overlays: Mutex<HashMap<Identifier, Uuid>>,
    this: Weak<PlayerData>,
}
impl PlayerData {
    pub fn new(
        connection: PlayerConnection,
        server: Arc<Server>,
        entity: Arc<Entity>,
    ) -> Arc<Self> {
        let player = Arc::new_cyclic(|this| PlayerData {
            connection: Mutex::new(connection),
            open_inventory: Mutex::new(None),
            chunk_loading_manager: ChunkLoadingManager::new(
                this.clone(),
                server.clone(),
                (&entity.get_location()).into(),
            ),
            entity: Mutex::new(entity.clone()),
            speed: Mutex::new(1.),
            move_type: Mutex::new(MovementType::Normal),
            hand_item: Mutex::new(None),
            user_data: Mutex::new(UserData::new()),
            overlays: Mutex::new(HashMap::new()),
            server,
            this: this.clone(),
        });
        player.chunk_loading_manager.load_initial_chunks();
        Inventory::set_cursor(&player, &None);
        player.resync_abilities();
        entity.set_player(player.clone());
        player
    }
    pub fn get_open_inventory_id(&self) -> Option<Uuid> {
        self.open_inventory.lock().as_ref().map(|inv| inv.1)
    }
    pub fn destroy(&self) {
        self.chunk_loading_manager.unload_chunks();

        if let Some(inventory) = self.open_inventory.lock().as_ref() {
            inventory
                .0
                .get_inventory()
                .remove_viewer(self.get_entity().get_id());
        }
    }
    pub fn modify_inventory_hand<F>(&self, function: F)
    where
        F: FnOnce(&mut Option<ItemStack>),
    {
        let mut hand_item = self.hand_item.lock();
        function.call_once((&mut *hand_item,));
        let set_as_empty = match &*hand_item {
            Some(item) => item.get_count() == 0,
            None => true,
        };
        if set_as_empty {
            *hand_item = None;
        }
        Inventory::set_cursor(self, &hand_item);
    }
    pub fn set_inventory_hand(&self, item: Option<ItemStack>) {
        let mut hand_item = self.hand_item.lock();
        *hand_item = match item {
            Some(item) => {
                if item.get_count() == 0 {
                    None
                } else {
                    Some(item)
                }
            }
            None => None,
        };
        Inventory::set_cursor(self, &hand_item);
    }
    pub fn resync_abilities(&self) {
        self.send_message(&NetworkMessageS2C::PlayerAbilities(
            *self.speed.lock(),
            *self.move_type.lock(),
        ));
    }
    pub fn tick(&self) {
        self.chunk_loading_manager.tick();
    }
    pub fn get_entity(&self) -> Arc<Entity> {
        self.entity.lock().clone()
    }
    pub fn send_message(&self, message: &NetworkMessageS2C) {
        self.connection.lock().send(message);
    }
    pub fn send_messages(&self, messages: &Vec<NetworkMessageS2C>) {
        let mut connection = self.connection.lock();
        for message in messages {
            connection.send(message);
        }
    }
    pub fn set_open_inventory(&self, new_inventory: Option<(InventoryWrapper, GuiInventoryData)>) {
        let mut current_inventory = self.open_inventory.lock();
        if let Some(current_inventory) = &*current_inventory {
            current_inventory
                .0
                .get_inventory()
                .remove_viewer(&current_inventory.1);
        }
        let (inventory, data) = new_inventory
            .map(|inv| (Some(inv.0), Some(inv.1)))
            .unwrap_or((None, None));
        let view_id = if let Some(data) = data {
            let viewer = data.into_viewer(self.ptr());
            let id = viewer.id.clone();
            inventory
                .as_ref()
                .unwrap()
                .get_inventory()
                .add_viewer(viewer);
            Some(id)
        } else {
            if let Some(inventory_hand) = &*self.hand_item.lock() {
                self.get_entity().throw_item(inventory_hand.clone());
            }
            self.set_inventory_hand(None);
            Inventory::set_cursor(self, &None);
            None
        };
        self.send_message(&NetworkMessageS2C::SetCursorLock(inventory.is_none()));
        self.send_message(&NetworkMessageS2C::GuiRemoveElements("cursor".to_string()));
        if inventory.is_none() {
            self.send_message(&NetworkMessageS2C::GuiSetElement(
                "cursor".to_string(),
                GUIElement {
                    position: Position {
                        x: 0.,
                        y: 0.,
                        z: 0.,
                    },
                    anchor: PositionAnchor::Center,
                    base_color: Color::WHITE,
                    component_type: GUIComponent::ImageComponent {
                        size: Vec2 { x: 50., y: 50. },
                        texture: "bb:cursor".to_string(),
                        slice: None,
                    },
                },
            ));
        }
        *current_inventory = inventory.map(|inventory| (inventory, view_id.unwrap()));
    }
    pub fn send_chat_message(&self, text: String) {
        self.send_message(&NetworkMessageS2C::ChatMessage(text));
    }
    pub fn open_overlay(&self, id: Identifier, layout: Arc<GUILayout>) -> Uuid {
        let mut overlays = self.overlays.lock();
        if let Some(id) = overlays.get(&id) {
            self.send_message(&NetworkMessageS2C::GuiRemoveElements(id.to_string()));
        }
        let uuid = Uuid::new_v4();
        overlays.insert(id, uuid);
        layout.send_to_player(self, uuid.to_string().as_str());
        uuid
    }
    pub fn get_overlay(&self, id: &Identifier) -> Option<Uuid> {
        self.overlays.lock().get(id).map(|id| *id)
    }
    pub fn close_overlay(&self, id: &Identifier) {
        let mut overlays = self.overlays.lock();
        if let Some(id) = overlays.remove(id) {
            self.send_message(&NetworkMessageS2C::GuiRemoveElements(id.to_string()));
        }
    }
    pub fn ptr(&self) -> Arc<PlayerData> {
        self.this.upgrade().unwrap()
    }
}
impl ScriptingObject for PlayerData {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<PlayerData>, _>("Player");
        env.register_method("get_entity", |player: &Arc<PlayerData>| {
            Ok(player.get_entity().into_variant())
        });
        env.register_method(
            "send_chat_message",
            |player: &Arc<PlayerData>, message: &ImmutableString| {
                player.send_chat_message(message.to_string());
                Ok(())
            },
        );
        env.register_method("speed", |player: &Arc<PlayerData>, speed: &f64| {
            *player.speed.lock() = *speed as f32;
            player.resync_abilities();
            Ok(())
        });
        env.register_method(
            "movement_type",
            |player: &Arc<PlayerData>, movement_type: &MovementType| {
                *player.move_type.lock() = *movement_type;
                player.resync_abilities();
                Ok(())
            },
        );
        env.register_member("user_data", |player: &Arc<PlayerData>| {
            Some(UserDataWrapper::Player(player.ptr()).into_variant())
        });
        env.register_method("close_inventory", |player: &Arc<PlayerData>| {
            player.set_open_inventory(None);
            Ok(())
        });
        {
            let server = server.clone();
            env.register_method(
                "open_inventory",
                move |player: &Arc<PlayerData>,
                      inventory: &InventoryWrapper,
                      range: &Range<i64>,
                      layout: &ImmutableString,
                      on_click: &FunctionVariant,
                      on_scroll: &FunctionVariant| {
                    player.set_open_inventory(Some((
                        inventory.clone(),
                        GuiInventoryData {
                            slot_range: Range::<u32> {
                                start: range.start as u32,
                                end: range.end as u32,
                            },
                            layout: server
                                .upgrade()
                                .unwrap()
                                .gui_layouts
                                .get(&Identifier::parse(layout.as_ref()).unwrap())
                                .unwrap()
                                .clone(),
                            on_click: ScriptCallback::from_function_variant(on_click),
                            on_scroll: ScriptCallback::from_function_variant(on_scroll),
                        },
                    )));
                    Ok(())
                },
            );
        }
        env.register_method("get_open_inventory", |player: &Arc<PlayerData>| {
            Ok(Variant::from_option(
                player
                    .open_inventory
                    .lock()
                    .as_ref()
                    .map(|inv| inv.0.clone()),
            ))
        });
        env.register_method(
            "set_hand_item",
            |player: &Arc<PlayerData>, item: &Variant| {
                player.set_inventory_hand(
                    Variant::into_option(item, &FilePosition::INVALID)?.cloned(),
                );
                Ok(())
            },
        );
        env.register_method("get_hand_item", |player: &Arc<PlayerData>| {
            Ok(Variant::from_option(
                player.hand_item.lock().as_ref().cloned(),
            ))
        });
        {
            let server = server.clone();
            env.register_method(
                "open_overlay",
                move |player: &Arc<PlayerData>, id: &ImmutableString, layout: &ImmutableString| {
                    Ok(ModGuiViewer {
                        id: player.open_overlay(
                            Identifier::parse(id.as_ref()).unwrap(),
                            server
                                .upgrade()
                                .unwrap()
                                .gui_layouts
                                .get(&Identifier::parse(layout.as_ref()).unwrap())
                                .unwrap()
                                .clone(),
                        ),
                        viewer: player.clone(),
                    })
                },
            );
        }
        env.register_method(
            "get_overlay",
            |player: &Arc<PlayerData>, id: &ImmutableString| {
                Ok(Variant::from_option(
                    player
                        .get_overlay(&Identifier::parse(id.clone()).unwrap())
                        .map(|id| ModGuiViewer {
                            id,
                            viewer: player.clone(),
                        }),
                ))
            },
        );
        env.register_method(
            "close_overlay",
            |player: &Arc<PlayerData>, id: &ImmutableString| {
                player.close_overlay(&Identifier::parse(id.clone()).unwrap());
                Ok(())
            },
        );
    }
}

pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    pub rotation_shifting: Mutex<(f32, bool)>,
    teleport: Mutex<Option<ChunkLocation>>,
    pub entity_type: Arc<EntityType>,
    removed: AtomicBool,
    pub client_id: u32,
    id: Uuid,
    animation_controller: Mutex<AnimationController<Entity>>,
    pub inventory: Inventory,
    pub server: Arc<Server>,
    velocity: Mutex<(f64, f64, f64)>,
    pub user_data: Mutex<UserData>,
    pub slot: Mutex<u32>,
    pub player: Mutex<Option<Weak<PlayerData>>>,
    pathfinder: Mutex<Pathfinder>,
}

static ENTITY_CLIENT_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);

impl Entity {
    pub fn new<T: Into<ChunkLocation>>(location: T, entity_type: &Arc<EntityType>) -> Arc<Entity> {
        let location: ChunkLocation = location.into();
        let chunk = location.chunk.clone();
        let server = location.chunk.world.server.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            server: server.clone(),
            entity_type: entity_type.clone(),
            removed: AtomicBool::new(false),
            this: weak.clone(),
            client_id: ENTITY_CLIENT_ID_GENERATOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            id: Uuid::new_v4(),
            teleport: Mutex::new(None),
            rotation_shifting: Mutex::new((0., false)),
            animation_controller: Mutex::new(AnimationController::new(weak.clone(), 1)),
            inventory: Inventory::new(WeakInventoryWrapper::Entity(weak.clone()), 18, None),
            velocity: Mutex::new((0., 0., 0.)),
            user_data: Mutex::new(UserData::new()),
            slot: Mutex::new(0),
            player: Mutex::new(None),
            pathfinder: Mutex::new(Pathfinder::new((&location).into())),
            location: Mutex::new(location.clone()),
        });
        chunk.add_entity(entity.clone());
        let add_message = entity.create_add_messages(entity.get_location().position);
        for viewer in chunk.viewers.lock().iter() {
            if viewer.player.get_entity().id != entity.id {
                viewer.player.send_messages(&add_message);
            }
        }
        entity
    }
    pub fn set_player(&self, player: Arc<PlayerData>) {
        player.send_message(&NetworkMessageS2C::TeleportPlayer(
            self.get_location().position,
            0.,
        ));
        player.send_message(&NetworkMessageS2C::ControllingEntity(
            self.entity_type.client_id,
        ));
        self.inventory.add_viewer(GuiInventoryViewer {
            slot_range: 0..9,
            layout: self
                .server
                .gui_layouts
                .get(&Identifier::new("core", "layout_hotbar"))
                .unwrap()
                .clone(),
            id: self.get_id().clone(),
            viewer: player.clone(),
            on_click: ScriptCallback::empty(),
            on_scroll: ScriptCallback::empty(),
        });

        *self.player.lock() = Some(Arc::downgrade(&player));

        self.set_hand_slot(0);
    }
    pub fn set_hand_slot(&self, slot: u32) {
        let slot = if slot == u32::MAX {
            self.inventory.get_size() - 1
        } else {
            slot.rem_euclid(self.inventory.get_size())
        };
        let old_slot = *self.slot.lock();
        *self.slot.lock() = slot;
        if let Some(player) = self.get_player() {
            player.send_message(&NetworkMessageS2C::GuiEditElement(
                self.inventory.get_slot_id_entity(self, old_slot),
                GUIElementEdit {
                    base_color: Some(Color::WHITE),
                    ..Default::default()
                },
            ));
            player.send_message(&NetworkMessageS2C::GuiEditElement(
                self.inventory.get_slot_id_entity(self, slot),
                GUIElementEdit {
                    base_color: Some(Color {
                        r: 100,
                        g: 100,
                        b: 100,
                        a: 255,
                    }),
                    ..Default::default()
                },
            ));
            self.sync_main_hand_viewmodel(self.get_hand_item().as_ref());
        }
    }
    pub fn get_collider(&self) -> AABB {
        let position = self.get_location().position;
        AABB {
            x: position.x,
            y: position.y,
            z: position.z,
            w: self.entity_type.client_data.hitbox_w, //todo: move from client data
            h: self.entity_type.client_data.hitbox_h,
            d: self.entity_type.client_data.hitbox_d,
        }
    }
    pub fn get_rotation(&self) -> f32 {
        self.rotation_shifting.lock().0
    }
    pub fn is_shifting(&self) -> bool {
        self.rotation_shifting.lock().1
    }
    pub fn get_player(&self) -> Option<Arc<PlayerData>> {
        match &*self.player.lock() {
            Some(player) => player.upgrade(),
            None => None,
        }
    }
    pub fn get_id(&self) -> &Uuid {
        &self.id
    }
    pub fn create_add_messages(&self, position: Position) -> Vec<NetworkMessageS2C> {
        let animation_controller = self.animation_controller.lock();
        let mut messages = Vec::new();
        messages.push(NetworkMessageS2C::AddEntity(
            self.entity_type.client_id,
            self.client_id,
            position,
            self.rotation_shifting.lock().0,
            animation_controller.animation,
            animation_controller.animation_start_time,
        ));
        for (inventory_index, model_index) in &self.entity_type.item_model_mapping.mapping {
            messages.push(NetworkMessageS2C::ModelItem(
                ClientModelTarget::Entity(self.client_id),
                *model_index,
                self.inventory
                    .get_full_view()
                    .get_item(*inventory_index)
                    .unwrap()
                    .as_ref()
                    .map(|item| item.item_type.client_id),
            ));
        }
        messages
    }
    pub fn teleport<T: Into<ChunkLocation>>(
        &self,
        location: T,
        rotation_shifting: Option<(f32, bool)>,
    ) {
        let location: ChunkLocation = location.into();
        let position = location.position.clone();
        self.move_to(location, rotation_shifting);
        if let Some(player) = self.get_player() {
            player.send_message(&NetworkMessageS2C::TeleportPlayer(
                position,
                rotation_shifting
                    .map(|rotation_shifting| rotation_shifting.0)
                    .unwrap_or(f32::NAN),
            ));
        }
    }
    pub fn move_to<T: Into<ChunkLocation>>(
        &self,
        location: T,
        rotation_shifting: Option<(f32, bool)>,
    ) {
        {
            *self.teleport.lock() = Some(location.into());
        }
        if let Some(rotation_shifting) = rotation_shifting {
            *self.rotation_shifting.lock() = rotation_shifting;
        }
    }
    pub fn get_location(&self) -> ChunkLocation {
        let location = self.location.lock();
        location.clone()
    }
    pub fn apply_knockback(&self, x: f64, y: f64, z: f64) {
        let mut velocity = self.velocity.lock();
        velocity.0 += x;
        velocity.1 += y;
        velocity.2 += z;
    }
    pub fn tick(&self) {
        let mut teleport_location = { self.teleport.lock().as_ref().map(|loc| loc.clone()) };
        if self.get_player().is_none() {
            let mut velocity = self.velocity.lock();
            velocity.0 *= 0.8;
            velocity.1 *= 0.8;
            velocity.2 *= 0.8;
            velocity.1 -= 2. / 20.;

            let mut physics_aabb = self.get_collider();
            let world = if let Some(teleport_location) = &teleport_location {
                physics_aabb.set_position(teleport_location.position);
                teleport_location.chunk.world.clone()
            } else {
                self.get_location().chunk.world.clone()
            };
            let is_on_ground = physics_aabb
                .move_by(0., -0.1, 0.)
                .has_block(&world, |block| block.collidable);
            if let Some(face) = self.pathfinder.lock().get_required_face() {
                let offset = face.get_offset();
                velocity.0 = offset.x as f64 * 0.2;
                if is_on_ground && offset.y > 0 {
                    velocity.1 = offset.y as f64 * 0.6;
                }
                velocity.2 = offset.z as f64 * 0.2;
            }
            {
                let x_moved_physics_aabb = physics_aabb.move_by(velocity.0, 0., 0.);
                if !x_moved_physics_aabb.has_block(&world, |state| state.collidable) {
                    physics_aabb = x_moved_physics_aabb;
                } else {
                    velocity.0 = 0.;
                }
            }
            {
                let y_moved_physics_aabb = physics_aabb.move_by(0., velocity.1, 0.);
                if !y_moved_physics_aabb.has_block(&world, |state| state.collidable) {
                    physics_aabb = y_moved_physics_aabb;
                } else {
                    velocity.1 = 0.;
                }
            }
            {
                let z_moved_physics_aabb = physics_aabb.move_by(0., 0., velocity.2);
                if !z_moved_physics_aabb.has_block(&world, |state| state.collidable) {
                    physics_aabb = z_moved_physics_aabb;
                } else {
                    velocity.2 = 0.;
                }
            }
            teleport_location = Some(ChunkLocation::from(&Location {
                world,
                position: physics_aabb.get_position(),
            }))
        }
        if let Some(teleport_location) = teleport_location {
            self.pathfinder
                .lock()
                .set_current_location((&teleport_location).into());
            let old_location = { self.location.lock().clone() };
            let new_location: ChunkLocation = teleport_location.clone();
            {
                *self.location.lock() = new_location.clone();
            }
            if !Arc::ptr_eq(&old_location.chunk, &new_location.chunk) {
                new_location.chunk.add_entity(self.this.upgrade().unwrap());

                {
                    let old_viewers = old_location.chunk.viewers.lock();
                    let new_viewers = new_location.chunk.viewers.lock();
                    let add_message = self.create_add_messages(new_location.position);
                    let delete_message = NetworkMessageS2C::DeleteEntity(self.client_id);
                    for viewer in old_viewers.difference(&new_viewers) {
                        viewer.player.send_message(&delete_message);
                    }
                    for viewer in new_viewers.difference(&old_viewers) {
                        if self.id != viewer.player.get_entity().id {
                            viewer.player.send_messages(&add_message);
                        }
                    }
                }
                if let Some(player) = self.get_player() {
                    if Arc::ptr_eq(&old_location.chunk.world, &new_location.chunk.world) {
                        player
                            .chunk_loading_manager
                            .transfer_position(new_location.position.to_chunk_pos());
                    } else {
                        player.chunk_loading_manager.transfer_world(
                            new_location.chunk.world.clone(),
                            new_location.position.to_chunk_pos(),
                        );
                    }
                }
            }
            new_location.chunk.announce_to_viewers_except(
                NetworkMessageS2C::MoveEntity(
                    self.client_id,
                    new_location.position,
                    self.rotation_shifting.lock().0,
                ),
                self,
            );
        }
        {
            *self.teleport.lock() = None;
        }
        let _ = self
            .entity_type
            .static_data
            .get_function("on_tick")
            .call_function(
                &self.server.script_environment,
                None,
                vec![self.this.upgrade().unwrap().into_variant()],
            );

        if let Some(player) = self.get_player() {
            let messages = player.connection.lock().receive_messages();
            for message in messages {
                match message {
                    NetworkMessageC2S::Keyboard(key, key_mod, pressed, _repeat) => {
                        let mut keyboard_event: HashMap<ImmutableString, Variant> = HashMap::new();
                        keyboard_event.insert("key".into(), key.into_variant());
                        keyboard_event.insert("pressed".into(), pressed.into_variant());
                        keyboard_event.insert("player".into(), player.ptr().into_variant());
                        let _ = self.server.call_event(
                            Identifier::new("bb", "keyboard"),
                            Arc::new(Mutex::new(keyboard_event)).into_variant(),
                        );
                        match key {
                            KeyboardKey::Q => {
                                if pressed {
                                    let slot = { *self.slot.lock() };
                                    self.inventory
                                        .get_full_view()
                                        .modify_item(slot, |item| {
                                            let item = item.as_mut();
                                            if let Some(item) = item {
                                                let count = if key_mod & KeyboardModifier::CTRL != 0
                                                {
                                                    item.get_count()
                                                } else {
                                                    1
                                                };
                                                let item_stack = item.copy(count);

                                                item.add_count(-(count as i32));

                                                self.throw_item(item_stack);
                                            }
                                        })
                                        .unwrap();
                                }
                            }
                            KeyboardKey::Escape => {
                                player.set_open_inventory(None);
                            }
                            _ => {}
                        }
                        if let Some(slot) = key.get_slot() {
                            if pressed {
                                self.set_hand_slot(slot as u32);
                            }
                        }
                    }
                    NetworkMessageC2S::GuiClick(element, button, shifting) => {
                        {
                            let id = self.inventory.resolve_id(self.get_id(), element.as_str());
                            if let Some(id) = id {
                                self.inventory
                                    .on_click(self.id, &player, &id, button, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *player.open_inventory.lock() {
                                let inventory = open_inventory.0.get_inventory();
                                let id = inventory.resolve_id(&open_inventory.1, element.as_str());
                                if let Some(id) = id {
                                    inventory.on_click(
                                        open_inventory.1,
                                        &player,
                                        &id,
                                        button,
                                        shifting,
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                    NetworkMessageC2S::GuiScroll(element, x, y, shifting) => {
                        {
                            let id = self.inventory.resolve_id(self.get_id(), element.as_str());
                            if let Some(id) = id {
                                self.inventory
                                    .on_scroll(self.id, &player, &id, x, y, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *player.open_inventory.lock() {
                                let inventory = open_inventory.0.get_inventory();
                                let id = inventory.resolve_id(&open_inventory.1, element.as_str());
                                if let Some(id) = id {
                                    inventory.on_scroll(
                                        open_inventory.1,
                                        &player,
                                        &id,
                                        x,
                                        y,
                                        shifting,
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                    NetworkMessageC2S::PlayerPosition(position, shift, rotation, moved) => {
                        let world = { self.location.lock().chunk.world.clone() };
                        self.move_to(&Location { position, world }, Some((rotation, shift)));
                        self.animation_controller
                            .lock()
                            .set_animation(Some(if moved { 2 } else { 1 }));
                    }
                    NetworkMessageC2S::RequestBlockBreakTime(id, position) => {
                        let world = { self.location.lock().chunk.world.clone() };
                        let block_break_time = (*f64::from_variant(
                            &world
                                .server
                                .block_registry
                                .state_by_ref(world.get_block_load(position).get_block_state())
                                .parent
                                .static_data
                                .get_function("on_left_click")
                                .call_function(
                                    &world.server.script_environment,
                                    Some(
                                        BlockLocation {
                                            world: world.clone(),
                                            position,
                                        }
                                        .into_variant(),
                                    ),
                                    vec![self.get_player().unwrap().into_variant()],
                                )
                                .unwrap(),
                        )
                        .unwrap_or(&-1.));
                        if block_break_time >= 0. {
                            player.send_message(&NetworkMessageS2C::BlockBreakTimeResponse(
                                id,
                                block_break_time as f32,
                            ));
                        }
                    }
                    NetworkMessageC2S::BreakBlock(block_position) => {
                        let world = &self.get_location().chunk.world;
                        world.set_block(
                            block_position,
                            BlockStateRef::AIR,
                            true,
                            self.get_player(),
                        );
                    }
                    NetworkMessageC2S::RightClickBlock(block_position, face, shifting) => {
                        let hand_slot = *self.slot.lock();
                        let block = self
                            .get_location()
                            .chunk
                            .world
                            .get_block_load(block_position);
                        let mut right_click_result = InteractionResult::Ignored;
                        if !shifting {
                            let block = &self
                                .server
                                .block_registry
                                .state_by_ref(block.get_block_state())
                                .parent;
                            right_click_result = block
                                .static_data
                                .get_function("on_right_click")
                                .call_action(
                                    &self.server.script_environment,
                                    Some(
                                        BlockLocation {
                                            world: self.get_location().chunk.world.clone(),
                                            position: block_position,
                                        }
                                        .into_variant(),
                                    ),
                                    vec![player.ptr().into_variant()],
                                )
                                .unwrap();
                        }
                        if right_click_result == InteractionResult::Consumed {
                            continue;
                        }
                        self.inventory
                            .get_full_view()
                            .modify_item(hand_slot, |stack| {
                                if let Some(stack) = stack {
                                    right_click_result =
                                        stack.item_type.clone().on_right_click_block(
                                            stack,
                                            player.clone(),
                                            BlockLocation {
                                                position: block_position,
                                                world: player
                                                    .get_entity()
                                                    .get_location()
                                                    .chunk
                                                    .world
                                                    .clone(),
                                            },
                                            face,
                                        );
                                }
                            })
                            .unwrap();
                    }
                    NetworkMessageC2S::RightClick(_shifting) => {
                        let hand_slot = *self.slot.lock();
                        let mut right_click_result = InteractionResult::Ignored;
                        self.inventory
                            .get_full_view()
                            .modify_item(hand_slot, |stack| {
                                if let Some(stack) = stack {
                                    //todo: send shifting state
                                    right_click_result = stack.item_type.clone().on_right_click(
                                        stack,
                                        player.ptr(),
                                        None,
                                    );
                                }
                            })
                            .unwrap();
                    }
                    NetworkMessageC2S::LeftClickEntity(client_id) => {
                        let location = self.get_location();
                        for chunk in location
                            .chunk
                            .world
                            .get_chunks_with_center_radius(location.chunk.position, 1)
                        {
                            if let Some(entity) = chunk
                                .entities
                                .lock()
                                .iter()
                                .find(|entity| entity.client_id == client_id)
                            {
                                entity.on_attack(self);
                                break;
                            }
                        }
                    }
                    NetworkMessageC2S::RightClickEntity(client_id) => {
                        let location = self.get_location();
                        for chunk in location
                            .chunk
                            .world
                            .get_chunks_with_center_radius(location.chunk.position, 1)
                        {
                            if let Some(entity) = chunk
                                .entities
                                .lock()
                                .iter()
                                .find(|entity| entity.client_id == client_id)
                            {
                                entity.on_right_click(self);
                                break;
                            }
                        }
                    }
                    NetworkMessageC2S::MouseScroll(_scroll_x, scroll_y) => {
                        let new_slot = (*self.slot.lock() as i32 - scroll_y).rem_euclid(9);
                        self.set_hand_slot(new_slot as u32);
                    }
                    NetworkMessageC2S::SendMessage(message) => {
                        if message.starts_with("/") {
                            /*let message = &message[1..].trim_end();
                            let parts: rhai::Array = message
                                .split(" ")
                                .map(|str| Dynamic::from_str(str).unwrap())
                                .collect();
                            let mut event_data = rhai::Map::new();
                            event_data.insert("player".into(), Dynamic::from(player.clone()));
                            event_data.insert("command".into(), parts.into());
                            let _ = self.server.call_event(
                                Identifier::new("bb", "command"),
                                Dynamic::from(event_data),
                            );*/
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    pub fn throw_item(&self, item: ItemStack) {
        let mut location = self.get_location();
        location.position.y += 1.7;
        let rotation = { *self.rotation_shifting.lock() };
        let rotation_radians = rotation.0.to_radians();
        location.chunk.world.drop_item_on_ground(
            location.position,
            item,
            Some(rotation.0),
            Some((
                rotation_radians.sin() as f64,
                0.,
                rotation_radians.cos() as f64,
            )),
        );
    }
    pub fn on_attack(&self, _player: &Entity) {}
    pub fn on_right_click(&self, player: &Entity) {
        if self.entity_type.client_data.model == "bb:item" {
            let inventory_view = self.inventory.get_full_view();
            let item_stack = inventory_view.get_item(0).unwrap();
            let overflow = match item_stack {
                Some(item_stack) => player.inventory.get_full_view().add_item(&item_stack),
                None => None,
            };
            if overflow.is_none() {
                self.remove();
            }
            inventory_view.set_item(0, overflow).unwrap();
        }
    }
    pub fn get_hand_item(&self) -> Option<ItemStack> {
        let inventory = self.inventory.get_full_view();
        inventory.get_item(*self.slot.lock()).unwrap()
    }
    pub fn remove(&self) {
        self.removed
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
    pub fn is_removed(&self) -> bool {
        self.get_player()
            .map(|player| player.connection.lock().is_closed())
            .unwrap_or(self.removed.load(std::sync::atomic::Ordering::Relaxed))
    }
    pub fn post_remove(&self) {}
    pub fn sync_main_hand_viewmodel(&self, item: Option<&ItemStack>) {
        if let Some(player) = self.get_player() {
            player.send_message(&NetworkMessageS2C::ModelItem(
                ClientModelTarget::ViewModel,
                0,
                item.map(|item| item.item_type.client_id),
            ));
            if item.is_some() {
                player.send_message(&NetworkMessageS2C::ModelAnimation(
                    ClientModelTarget::ViewModel,
                    0,
                ));
                player.send_message(&NetworkMessageS2C::PlaySound(
                    "core:equip".to_string(),
                    self.get_location().position,
                    1.,
                    1.,
                    false,
                ));
            }
        }
    }
    pub fn ptr(&self) -> Arc<Entity> {
        self.this.upgrade().unwrap()
    }
}
impl ScriptingObject for Entity {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<Entity>, _>("Entity");
        {
            let server = server.clone();
            env.register_function(
                "Entity",
                move |id: &ImmutableString, location: &Location| {
                    Ok(Entity::new(
                        location,
                        server
                            .upgrade()
                            .unwrap()
                            .entity_registry
                            .entity_by_identifier(&Identifier::parse(id.as_ref()).unwrap())
                            .unwrap(),
                    ))
                },
            );
        }
        /*
        engine.register_fn("is_shifting", |entity: &mut Arc<Entity>| {
            entity.is_shifting()
        });
        engine.register_fn("get_location", |entity: &mut Arc<Entity>| {
            Into::<Location>::into(&entity.get_location())
        });
        engine.register_fn(
            "teleport",
            |entity: &mut Arc<Entity>, location: Location| {
                entity.teleport(&location, None);
            },
        );
        engine.register_get("user_data", |entity: &mut Arc<Entity>| {
            UserDataWrapper::Entity(entity.ptr())
        });
        engine.register_get("inventory", |entity: &mut Arc<Entity>| {
            InventoryWrapper::Entity(entity.ptr())
        });
        engine.register_fn("get_hand_item", |entity: &mut Arc<Entity>| {
            entity
                .get_hand_item()
                .map(|item| Dynamic::from(item))
                .unwrap_or(Dynamic::UNIT)
        });*/
    }
}
impl Animatable for Entity {
    fn send_animation_to_viewers(&self, animation: u32) {
        self.get_location()
            .chunk
            .announce_to_viewers(&NetworkMessageS2C::ModelAnimation(
                ClientModelTarget::Entity(self.client_id),
                animation,
            ));
    }
    fn send_animation_to(&self, viewer: &PlayerData, animation: u32) {
        viewer.send_message(&NetworkMessageS2C::ModelAnimation(
            ClientModelTarget::Entity(self.client_id),
            animation,
        ));
    }
}
pub struct Pathfinder {
    current_location: BlockLocation,
    target_location: Option<BlockLocation>,
    path: Vec<(BlockPosition, Face)>,
}
impl Pathfinder {
    pub fn new(location: BlockLocation) -> Self {
        Pathfinder {
            current_location: location,
            target_location: None,
            path: Vec::new(),
        }
    }
    pub fn set_current_location(&mut self, location: BlockLocation) {
        //todo: multiple worlds
        if self.current_location == location {
            return;
        }
        self.current_location = location.clone();
        if self
            .path
            .iter()
            .find(|item| item.0 == location.position)
            .is_some()
        {
            while self.path.remove(0).0 != location.position {}
            return;
        }
        self.recalculate_path();
    }
    pub fn set_target(&mut self, target: Option<BlockLocation>) {
        if self.target_location == target {
            return;
        }
        self.target_location = target;
        self.recalculate_path();
    }
    pub fn recalculate_path(&mut self) {
        if let Some(target_location) = &self.target_location {
            if !Arc::ptr_eq(&target_location.world, &self.current_location.world) {
                return;
            }
            let path = astar(
                &(self.current_location.position, Face::Down),
                |(position, _)| {
                    let mut vec = Vec::with_capacity(6);
                    for face in Face::all() {
                        let position = position.offset_by_face(*face);
                        if position.distance(&self.current_location.position) > 20.
                        /* todo: config option*/
                        {
                            continue;
                        }
                        let block_registry = &self.current_location.world.server.block_registry;
                        if self
                            .current_location
                            .world
                            .get_block(&position)
                            .map(|block| !block.is_collidable(block_registry))
                            .unwrap_or(false)
                        {
                            if (self
                                .current_location
                                .world
                                .get_block(&position.offset_by_face(Face::Down))
                                .map(|block| !block.is_collidable(block_registry))
                                .unwrap_or(true)
                                && face.get_offset().y == 0)
                                || (face.get_offset().y != 0
                                    && self
                                        .current_location
                                        .world
                                        .get_block(
                                            &position
                                                .offset_by_face(Face::Down)
                                                .offset_by_face(Face::Down),
                                        )
                                        .map(|block| !block.is_collidable(block_registry))
                                        .unwrap_or(true))
                            {
                                continue;
                            }
                            vec.push(((position, *face), 1));
                        }
                    }
                    vec
                },
                |(position, _)| {
                    position
                        .to_position()
                        .distance(&target_location.position.to_position())
                        as u32
                },
                |(position, _)| *position == target_location.position,
            )
            .map(|path| path.0);
            self.path = path.unwrap_or(Vec::new());
            if self.path.len() > 0 {
                self.path.remove(0);
            }
        }
    }
    pub fn get_required_face(&self) -> Option<Face> {
        self.path.get(0).map(|item| item.1)
    }
}
pub struct ChunkLoadingManager {
    server: Arc<Server>,
    player: Weak<PlayerData>,
    to_load: Mutex<HashSet<Arc<Chunk>>>,
    old_position: Mutex<ChunkPosition>,
    old_world: Mutex<Arc<World>>,
}
impl ChunkLoadingManager {
    pub fn new(player: Weak<PlayerData>, server: Arc<Server>, location: Location) -> Self {
        ChunkLoadingManager {
            player,
            server,
            to_load: Mutex::new(HashSet::new()),
            old_position: Mutex::new(location.position.to_chunk_pos()),
            old_world: Mutex::new(location.world),
        }
    }
    pub fn load(&self, chunk: Arc<Chunk>) {
        self.to_load.lock().insert(chunk);
    }
    pub fn unload(&self, chunk: Arc<Chunk>) {
        self.player
            .upgrade()
            .unwrap()
            .send_message(&NetworkMessageS2C::UnloadChunk(chunk.position));
    }
    pub fn tick(&self) {
        for chunk in self
            .to_load
            .lock()
            .extract_if(|chunk| chunk.loading_stage.load(Ordering::Relaxed) >= 2)
            .take(
                self.server
                    .settings
                    .get_i64("server.max_chunks_sent_per_tick", 200) as usize,
            )
        {
            let entity = self.player.upgrade().unwrap();
            self.server.thread_pool.execute(Box::new(move || {
                let mut palette = Vec::new();
                let mut block_data = [[[0; 16]; 16]; 16];
                {
                    let blocks = chunk.blocks.lock();
                    for x in 0..16 {
                        for y in 0..16 {
                            for z in 0..16 {
                                let block_id = blocks[x][y][z].get_client_id();
                                let palette_entry =
                                    match palette.iter().position(|block| *block == block_id) {
                                        Some(entry) => entry,
                                        None => {
                                            palette.push(block_id);
                                            palette.len() - 1
                                        }
                                    };
                                block_data[x][y][z] = palette_entry as u16;
                            }
                        }
                    }
                }
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
                std::io::copy(
                    &mut bitcode::serialize(&block_data).unwrap().as_slice(),
                    &mut encoder,
                )
                .unwrap();
                let load_message = NetworkMessageS2C::LoadChunk(
                    chunk.position,
                    palette,
                    encoder.finish().unwrap(),
                );
                entity.send_message(&load_message);
                {
                    let blocks = chunk.blocks.lock();
                    for x in 0..16 {
                        for y in 0..16 {
                            for z in 0..16 {
                                let block = &blocks[x][y][z];
                                match &block {
                                    BlockData::Simple(_) => {}
                                    BlockData::Data(block) => block.on_sent_to_client(&entity),
                                }
                            }
                        }
                    }
                }
            }));
        }
    }
    pub fn unload_chunks(&self) {
        let world = self.old_world.lock();
        for pos in Self::get_chunks_to_load_at(self, *self.old_position.lock()) {
            world
                .load_chunk(pos)
                .remove_viewer(&self.player.upgrade().unwrap(), false);
        }
    }
    pub fn load_initial_chunks(&self) {
        let world = self.old_world.lock();
        for pos in Self::get_chunks_to_load_at(self, *self.old_position.lock()) {
            world
                .load_chunk(pos)
                .add_viewer(self.player.upgrade().unwrap());
        }
    }
    pub fn transfer_world(&self, new_world: Arc<World>, new_position: ChunkPosition) {
        let mut old_position = self.old_position.lock();
        let old_loaded = Self::get_chunks_to_load_at(self, *old_position);
        let new_loaded = Self::get_chunks_to_load_at(self, new_position);
        let mut old_world = self.old_world.lock();
        for pos in old_loaded {
            old_world
                .load_chunk(pos)
                .remove_viewer(&self.player.upgrade().unwrap(), true);
        }
        for pos in new_loaded {
            new_world
                .load_chunk(pos)
                .add_viewer(self.player.upgrade().unwrap());
        }
        *old_world = new_world;
        *old_position = new_position;
    }
    pub fn transfer_position(&self, new_position: ChunkPosition) {
        let mut old_position = self.old_position.lock();
        let old_loaded = Self::get_chunks_to_load_at(self, *old_position);
        let new_loaded = Self::get_chunks_to_load_at(self, new_position);
        let world = self.old_world.lock();
        for pos in old_loaded.difference(&new_loaded) {
            world
                .load_chunk(*pos)
                .remove_viewer(&self.player.upgrade().unwrap(), true);
        }
        for pos in new_loaded.difference(&old_loaded) {
            world
                .load_chunk(*pos)
                .add_viewer(self.player.upgrade().unwrap());
        }

        *old_position = new_position;
    }
    pub fn get_chunks_to_load_at(&self, position: ChunkPosition) -> FxHashSet<ChunkPosition> {
        let vertical_view_distance =
            self.server
                .settings
                .get_i64("server.view_distance.vertical", 16) as i32;
        let horizontal_view_distance =
            self.server
                .settings
                .get_i64("server.view_distance.horizontal", 8) as i32;
        let mut positions = FxHashSet::default();
        for x in (-vertical_view_distance)..=vertical_view_distance {
            for y in (-horizontal_view_distance)..=horizontal_view_distance {
                for z in (-vertical_view_distance)..=vertical_view_distance {
                    positions.insert(ChunkPosition {
                        x: position.x + x,
                        y: position.y + y,
                        z: position.z + z,
                    });
                }
            }
        }
        positions
    }
}

impl PartialEq for Entity {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Into<WeakInventoryWrapper> for &Entity {
    fn into(self) -> WeakInventoryWrapper {
        WeakInventoryWrapper::Entity(self.this.clone())
    }
}

impl Hash for Entity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}
#[extend::ext]
impl AABB {
    fn has_block<F>(&self, world: &World, predicate: F) -> bool
    where
        F: Fn(&BlockState) -> bool,
    {
        self.iter_blocks()
            .find(|position| {
                predicate.call((world
                    .server
                    .block_registry
                    .state_by_ref(world.get_block_load(*position).get_block_state()),))
            })
            .is_some()
    }
}
pub struct AnimationController<T> {
    viewable: Weak<T>,
    animation: u32,
    animation_start_time: f32,
    default_animation: u32,
}

impl<T: Animatable> AnimationController<T> {
    pub fn new(entity: Weak<T>, default_animation: u32) -> Self {
        AnimationController {
            viewable: entity,
            animation: default_animation,
            animation_start_time: 0., //todo
            default_animation,
        }
    }
    pub fn set_animation(&mut self, animation: Option<u32>) {
        let new_animation = animation.unwrap_or(self.default_animation);
        if self.animation != new_animation {
            self.animation = new_animation;
            self.animation_start_time = 0.; //todo
            self.viewable
                .upgrade()
                .unwrap()
                .send_animation_to_viewers(self.animation);
        }
    }
    pub fn sync_to(&self, viewer: &PlayerData) {
        self.viewable
            .upgrade()
            .unwrap()
            .send_animation_to(viewer, self.animation);
    }
    pub fn resync(&self) {
        self.viewable
            .upgrade()
            .unwrap()
            .send_animation_to_viewers(self.animation);
    }
}
pub trait Animatable {
    fn send_animation_to_viewers(&self, animation: u32);
    fn send_animation_to(&self, viewer: &PlayerData, animation: u32);
}
#[derive(Clone)]
pub struct Structure {
    blocks: Vec<(BlockPosition, (BlockStateRef, f32))>,
}

impl Structure {
    pub fn from_json(json: JsonValue, block_registry: &BlockRegistry) -> Self {
        let mut blocks = Vec::new();
        for block in json["blocks"].members() {
            blocks.push((
                BlockPosition {
                    x: block["x"].as_i32().unwrap(),
                    y: block["y"].as_i32().unwrap(),
                    z: block["z"].as_i32().unwrap(),
                },
                {
                    (
                        block_registry
                            .state_from_string(block["id"].as_str().unwrap())
                            .unwrap(),
                        block["chance"].as_f32().unwrap_or(1.),
                    )
                },
            ));
        }
        Structure { blocks }
    }
    pub fn from_world(
        world: &World,
        first: BlockPosition,
        second: BlockPosition,
        origin: BlockPosition,
    ) -> Self {
        let fixed_first = BlockPosition {
            x: first.x.min(second.x),
            y: first.y.min(second.y),
            z: first.z.min(second.z),
        };
        let fixed_second = BlockPosition {
            x: first.x.max(second.x),
            y: first.y.max(second.y),
            z: first.z.max(second.z),
        };
        let mut blocks = Vec::new();
        for x in fixed_first.x..=fixed_second.x {
            for y in fixed_first.y..=fixed_second.y {
                for z in fixed_first.z..=fixed_second.z {
                    let block_position = BlockPosition { x, y, z };
                    if let Some(block) = world.get_block(&block_position) {
                        if !block.get_block_state().is_air() {
                            blocks
                                .push((block_position.add(-origin), (block.get_block_state(), 1.)));
                        }
                    }
                }
            }
        }
        Structure { blocks }
    }
    pub fn export(&self, block_registry: &BlockRegistry) -> JsonValue {
        let mut blocks = Vec::new();
        for (position, block) in &self.blocks {
            let state = block_registry.state_by_ref(block.0);
            blocks.push(object! {
                x:position.x,
                y:position.y,
                z:position.z,
                id:state.to_string(),
            });
        }
        object! {
            blocks:JsonValue::Array(blocks)
        }
    }
    pub fn place<F>(&self, mut placer: F, position: BlockPosition)
    where
        F: FnMut(BlockPosition, BlockStateRef),
    {
        for (block_position, block) in &self.blocks {
            if rand::thread_rng().gen_bool(block.1 as f64) {
                placer.call_mut((block_position.clone() + position, block.0.clone()));
            }
        }
    }
    pub fn get_chunks(&self, position: BlockPosition) -> HashSet<ChunkPosition> {
        let mut chunks = HashSet::new();
        for (block_position, _) in &self.blocks {
            chunks.insert((block_position.clone() + position).to_chunk_pos());
        }
        chunks
    }
}
impl ScriptingObject for Structure {
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<Arc<Structure>, _>("Structure");
        /*{
        let server = server.clone();
        engine.register_fn(
            "export_structure",
            move |structure: &mut Structure, name: &str| {
                let json = structure.export(&server.upgrade().unwrap().block_registry);
                server
                    .upgrade()
                    .unwrap()
                    .export_file(name.to_string(), json.to_string().as_bytes().to_vec());
            },
        );
        }
         */
    }
}
pub struct BlockNetwork {
    this: Weak<Self>,
    id: Identifier,
    pub user_data: Mutex<UserData>,
    members: Mutex<HashSet<BlockLocation>>,
}
impl BlockNetwork {
    pub fn new(id: Identifier) -> Arc<Self> {
        Arc::new_cyclic(|this| BlockNetwork {
            id,
            this: this.clone(),
            user_data: Mutex::new(UserData::new()),
            members: Mutex::new(HashSet::new()),
        })
    }
    pub fn merge(&self, other: Arc<BlockNetwork>) {
        assert_eq!(self.id, other.id);
        if Arc::ptr_eq(&self.ptr(), &other) {
            return;
        }
        let members = other.members.lock().drain().collect::<Vec<_>>();
        for member in members {
            member.get_data().unwrap().set_network(self.ptr());
        }
    }
    pub fn ptr(&self) -> Arc<BlockNetwork> {
        self.this.upgrade().unwrap()
    }
}
impl ScriptingObject for BlockNetwork {
    fn engine_register(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        /*engine.register_fn("list_members", |network: &mut Arc<BlockNetwork>| {
            network
                .members
                .lock()
                .iter()
                .map(|location| Dynamic::from(location.clone()))
                .collect::<Array>()
        });
        engine.register_get("user_data", |network: &mut Arc<BlockNetwork>| {
            UserDataWrapper::BlockNetwork(network.ptr())
        });*/
    }
}
pub struct NetworkController {
    networks: HashMap<Identifier, Arc<BlockNetwork>>,
}
impl NetworkController {
    pub fn new() -> Self {
        NetworkController {
            networks: HashMap::new(),
        }
    }
}

pub struct WorldBlock {
    this: Weak<WorldBlock>,
    pub chunk: Weak<Chunk>,
    pub position: BlockPosition,
    pub state: BlockStateRef,
    pub block: Arc<Block>,
    pub inventory: Inventory,
    pub user_data: Mutex<UserData>,
    animation_controller: AnimationController<WorldBlock>,
    pub network_controller: Mutex<NetworkController>,
}

impl WorldBlock {
    pub fn new(location: ChunkBlockLocation, state: BlockStateRef) -> Arc<WorldBlock> {
        let block = location
            .chunk
            .world
            .server
            .block_registry
            .state_by_ref(state)
            .parent
            .clone();
        Arc::new_cyclic(|this| WorldBlock {
            chunk: Arc::downgrade(&location.chunk),
            position: location.position,
            state,
            inventory: Inventory::new(
                WeakInventoryWrapper::Block(this.clone()),
                block.data_container.as_ref().unwrap().0,
                None,
            ),
            animation_controller: AnimationController::new(this.clone(), 0),
            block,
            user_data: Mutex::new(UserData::new()),
            network_controller: Mutex::new(NetworkController::new()),
            this: this.clone(),
        })
    }
    pub fn on_place(&self) {
        for (id, connector) in &self.block.networks {
            let connections: Vec<BlockLocation> = Vec::new(); /*connector
                                                              .call_function(
                                                                  &self.chunk().world.server.engine,
                                                                  Some(
                                                                      &BlockLocation {
                                                                          world: self.chunk().world.clone(),
                                                                          position: self.position,
                                                                      }
                                                                      .into_variant(),
                                                                  ),
                                                                  vec![],
                                                              )
                                                              .cast::<Array>()
                                                              .into_iter()
                                                              .map(|position| position.cast::<BlockLocation>())
                                                              .collect();*/
            let network = BlockNetwork::new(id.clone());
            self.set_network(network.clone());
            for connection in connections {
                if let Some(block_data) = connection.get_data() {
                    if let Some(other) = block_data.get_network(id) {
                        network.merge(other);
                    }
                }
            }
        }
    }
    pub fn get_location(&self) -> BlockLocation {
        BlockLocation {
            position: self.position,
            world: self.chunk().world.clone(),
        }
    }
    pub fn get_network(&self, id: &Identifier) -> Option<Arc<BlockNetwork>> {
        self.network_controller.lock().networks.get(id).cloned()
    }
    pub fn set_network(&self, network: Arc<BlockNetwork>) {
        if let Some(network) = self.network_controller.lock().networks.get(&network.id) {
            network.members.lock().remove(&self.get_location());
        }
        network.members.lock().insert(self.get_location());
        self.network_controller
            .lock()
            .networks
            .insert(network.id.clone(), network);
    }
    pub fn on_destroy(&self) {
        //todo: remove network connections
    }
    pub fn on_sent_to_client(&self, player: &PlayerData) {
        self.animation_controller.sync_to(player);
        for (inventory_index, model_index) in &self.block.item_model_mapping.mapping {
            player.send_message(&NetworkMessageS2C::ModelItem(
                ClientModelTarget::Block(self.position),
                *model_index,
                self.inventory
                    .get_full_view()
                    .get_item(*inventory_index)
                    .unwrap()
                    .as_ref()
                    .map(|item| item.item_type.client_id),
            ));
        }
    }
    pub fn get_inputs_view_for_side(&self, _side: Face) -> InventoryView {
        self.inventory.get_full_view()
    }
    pub fn serialize(&self) -> BlockSaveData {
        BlockSaveData {
            inventory: self.inventory.serialize(),
        }
    }
    pub fn deserialize(&self, data: BlockSaveData) {
        self.inventory.deserialize(
            data.inventory,
            &self.chunk.upgrade().unwrap().world.server.item_registry,
        );
    }
    pub fn update_to_clients(&self) {
        self.animation_controller.resync();
        let chunk = self.chunk.upgrade().unwrap();
        for (inventory_index, model_index) in &self.block.item_model_mapping.mapping {
            chunk.announce_to_viewers(&NetworkMessageS2C::ModelItem(
                ClientModelTarget::Block(self.position),
                *model_index,
                self.inventory
                    .get_full_view()
                    .get_item(*inventory_index)
                    .unwrap()
                    .as_ref()
                    .map(|item| item.item_type.client_id),
            ));
        }
    }
    pub fn ptr(&self) -> Arc<WorldBlock> {
        self.this.upgrade().unwrap()
    }
    pub fn chunk(&self) -> Arc<Chunk> {
        self.chunk.upgrade().unwrap()
    }
}
impl ScriptingObject for WorldBlock {
    fn engine_register(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<Arc<WorldBlock>, _>("WorldBlock");
        /*engine.register_get("user_data", |block: &mut Arc<WorldBlock>| {
            UserDataWrapper::Block(block.ptr())
        });
        engine.register_get("inventory", |block: &mut Arc<WorldBlock>| {
            InventoryWrapper::Block(block.ptr())
        });
        engine.register_get("location", |block: &mut Arc<WorldBlock>| BlockLocation {
            position: block.position,
            world: block.chunk().world.clone(),
        });
        engine.register_fn("network", |block: &mut Arc<WorldBlock>, id: &str| {
            block
                .get_network(&Identifier::parse(id).unwrap())
                .map(|network| Dynamic::from(network))
                .unwrap_or(Dynamic::UNIT)
        });*/
    }
}
impl Animatable for WorldBlock {
    fn send_animation_to_viewers(&self, animation: u32) {
        self.chunk
            .upgrade()
            .unwrap()
            .announce_to_viewers(&NetworkMessageS2C::ModelAnimation(
                ClientModelTarget::Block(self.position),
                animation,
            ));
    }
    fn send_animation_to(&self, viewer: &PlayerData, animation: u32) {
        viewer.send_message(&NetworkMessageS2C::ModelAnimation(
            ClientModelTarget::Block(self.position),
            animation,
        ));
    }
}

impl Into<WeakInventoryWrapper> for &WorldBlock {
    fn into(self) -> WeakInventoryWrapper {
        WeakInventoryWrapper::Block(self.this.clone())
    }
}
