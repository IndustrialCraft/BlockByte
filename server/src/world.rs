use std::hash::Hasher;
use std::ops::Add;
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
use bitcode::__private::Serialize;
use block_byte_common::gui::{GUIComponent, GUIElement, GUIElementEdit, PositionAnchor};
use block_byte_common::messages::{
    MouseButton, MovementType, NetworkMessageC2S, NetworkMessageS2C,
};
use block_byte_common::{
    BlockPosition, ChunkPosition, Color, Face, KeyboardKey, KeyboardModifier, Position, Vec2, AABB,
};
use flate2::Compression;
use fxhash::{FxHashMap, FxHashSet};
use json::{object, JsonValue};
use parking_lot::Mutex;
use rand::Rng;
use rhai::{Array, Dynamic};
use serde::Deserialize;
use uuid::Uuid;

use crate::inventory::{GuiInventoryData, GuiInventoryViewer, InventorySaveData};
use crate::registry::{Block, BlockState};
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
    id: Identifier,
    temporary: bool,
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
        });
        std::fs::create_dir_all(world.get_world_path()).unwrap();
        world
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
    pub fn set_block(&self, position: BlockPosition, block: BlockStateRef, update_neighbors: bool) {
        let chunk_offset = position.chunk_offset();
        self.load_chunk(position.to_chunk_pos()).set_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
            block,
            update_neighbors,
        );
    }
    pub fn break_block(&self, position: BlockPosition, params: BlockBreakParameters) {
        let chunk_offset = position.chunk_offset();
        let chunk = self.load_chunk(position.to_chunk_pos());
        let block_state = self.server.block_registry.state_by_ref(
            &chunk
                .get_block(chunk_offset.0, chunk_offset.1, chunk_offset.2)
                .get_block_state(),
        );
        block_state.on_break(
            ChunkBlockLocation {
                position,
                chunk: chunk.clone(),
            },
            params,
        );
        chunk.set_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
            BlockStateRef::from_state_id(0),
            true,
        );
    }
    pub fn get_block_load(&self, position: &BlockPosition) -> BlockData {
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

    pub fn replace_block<F>(&self, position: BlockPosition, replacer: F, update_neighbors: bool)
    where
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
        let chunks: Vec<_> = self.chunks.lock().values().cloned().collect();
        for chunk in chunks {
            chunk.tick();
        }
        let non_empty = {
            let mut chunks = self.chunks.lock();
            chunks
                .extract_if(|_, chunk| {
                    let should_unload = chunk.should_unload();
                    if should_unload {
                        chunk.destroy();
                    }
                    should_unload
                })
                .count();
            chunks.len() > 0
        };
        if non_empty {
            self.unload_timer.reset();
        } else {
            self.unload_timer.inc();
        }
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
impl Eq for World {}
impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.this, &other.this)
    }
}

pub struct BlockBreakParameters {
    pub(crate) player: Option<Arc<Entity>>,
    pub(crate) item: Option<ItemStack>,
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
}

pub struct Chunk {
    pub position: ChunkPosition,
    pub world: Arc<World>,
    blocks: Mutex<[[[BlockData; 16]; 16]; 16]>,
    entities: Mutex<Vec<Arc<Entity>>>,
    viewers: Mutex<FxHashSet<ChunkViewer>>,
    unload_timer: AtomicU8,
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
            unload_timer: AtomicU8::new(0),
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
                                None,
                            );
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
            .map(|id| (block_registry.block_by_identifier(&id.0).unwrap(), id.1))
            .collect();
        let blocks = array_init(|x| {
            array_init(|y| {
                array_init(|z| {
                    let block_id = chunk_save_data.blocks[x][y][z];
                    let block = block_palette.get(block_id as usize).unwrap();
                    let block_data = block.0.get_state_ref(block.1).create_block_data(
                        self,
                        BlockPosition {
                            x: (self.position.x * 16) + x as i32,
                            y: (self.position.y * 16) + y as i32,
                            z: (self.position.z * 16) + z as i32,
                        },
                    );
                    let offset = (x as u8, y as u8, z as u8);
                    if let BlockData::Data(block) = &block_data {
                        if let Some(data) = chunk_save_data.block_data.remove(&offset) {
                            block.deserialize(data);
                            if block.needs_ticking() {
                                self.ticking_blocks.lock().insert(offset);
                            }
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
                    self.set_block(offset.0, offset.1, offset.2, block, false);
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
    ) {
        let block_position = BlockPosition {
            x: self.position.x * 16 + offset_x as i32,
            y: self.position.y * 16 + offset_y as i32,
            z: self.position.z * 16 + offset_z as i32,
        };
        let previous_ticking =
            match &self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize] {
                BlockData::Simple(_) => false,
                BlockData::Data(block) => block.needs_ticking(),
            };
        let block = block.create_block_data(&self.this.upgrade().unwrap(), block_position);
        if self.loading_stage.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            self.announce_to_viewers(&NetworkMessageS2C::SetBlock(
                block_position,
                block.get_client_id(),
            ));
        }
        let new_ticking = match &block {
            BlockData::Simple(_) => false,
            BlockData::Data(block) => {
                block.update_to_clients();
                block.needs_ticking()
            }
        };
        let offset = (offset_x, offset_y, offset_z);
        match (previous_ticking, new_ticking) {
            (true, false) => {
                self.ticking_blocks.lock().remove(&offset);
            }
            (false, true) => {
                self.ticking_blocks.lock().insert(offset);
            }
            _ => {}
        }
        self.blocks.lock()[offset_x as usize][offset_y as usize][offset_z as usize] = block;
        if update_neighbors {
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
    fn add_viewer(&self, viewer: Arc<Entity>) {
        self.viewers.lock().insert(ChunkViewer {
            player: viewer.clone(),
        });
        viewer.chunk_loading_manager.lock().load(self.ptr());
        for entity in self.entities.lock().iter() {
            if Arc::ptr_eq(entity, &viewer) {
                continue;
            }
            viewer
                .try_send_messages(&entity.create_add_messages(entity.get_location().position))
                .unwrap();
        }
    }
    fn remove_viewer(&self, viewer: &Entity, unload_entities: bool) {
        viewer.chunk_loading_manager.lock().unload(self.ptr());
        if unload_entities {
            for entity in self.entities.lock().iter() {
                if entity.as_ref() == viewer {
                    continue;
                }
                viewer
                    .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                    .unwrap();
            }
        }
        self.viewers.lock().remove(&ChunkViewer {
            player: viewer.ptr(),
        });
    }
    pub fn announce_to_viewers_except(&self, message: NetworkMessageS2C, player: &Entity) {
        for viewer in self.viewers.lock().iter() {
            if viewer.player.id != player.id {
                viewer.player.try_send_message(&message).unwrap();
            }
        }
    }
    pub fn announce_to_viewers(&self, message: &NetworkMessageS2C) {
        for viewer in self.viewers.lock().iter() {
            viewer.player.try_send_message(message).unwrap();
        }
    }
    pub fn tick(&self) {
        self.unload_timer
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.entities
            .lock()
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
                            .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                            .unwrap();
                    }
                }
                let removed = entity.is_removed();
                if removed && !not_same_chunk {
                    for viewer in self.viewers.lock().iter() {
                        viewer
                            .player
                            .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                            .unwrap();
                    }
                    entity.post_remove();
                }
                removed || not_same_chunk
            })
            .count();
        let entities: Vec<_> = self.entities.lock().iter().map(|e| e.clone()).collect();
        let blocks: Vec<_> = {
            let blocks = self.blocks.lock();
            self.ticking_blocks
                .lock()
                .iter()
                .map(
                    |e| match &blocks[e.0 as usize][e.1 as usize][e.2 as usize] {
                        BlockData::Simple(_) => panic!("simple block was marked as tickable"),
                        BlockData::Data(block) => block.clone(),
                    },
                )
                .collect()
        };
        let block_updates: Vec<_> = { self.scheduled_updates.lock().drain().collect() };
        if self.viewers.lock().len() > 0 {
            self.unload_timer
                .store(0, std::sync::atomic::Ordering::Relaxed);
        }
        if entities.len() > 0 || blocks.len() > 0 || block_updates.len() > 0 {
            let chunk = self.ptr();
            self.world.server.thread_pool.execute(Box::new(move || {
                for entity in entities {
                    entity.tick();
                }
                for block in blocks {
                    block.tick();
                }
                for block_update in block_updates {
                    let state = chunk.world.server.block_registry.state_by_ref(
                        &chunk
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
    }
    pub fn should_unload(&self) -> bool {
        self.unload_timer.load(std::sync::atomic::Ordering::Relaxed) >= Chunk::UNLOAD_TIME
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
                            let block = block_registry.state_by_ref(&block_state_ref);
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
                        || entity.is_player()
                    {
                        continue;
                    }
                    entities.push(EntitySaveData {
                        entity_type: entity.entity_type.id.clone(),
                        velocity: entity.velocity.lock().clone(),
                        rotation: entity.get_rotation(),
                        position,
                        inventory: entity.inventory.serialize(),
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
    //todo: user data
}

struct ChunkViewer {
    pub player: Arc<Entity>,
}

impl Hash for ChunkViewer {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.player.id.hash(state)
    }
}

impl PartialEq for ChunkViewer {
    fn eq(&self, other: &Self) -> bool {
        self.player.id == other.player.id
    }
}

impl Eq for ChunkViewer {}

#[derive(Clone)]
pub struct UserData {
    data: HashMap<Identifier, Dynamic>,
}
impl UserData {
    pub fn new() -> Self {
        UserData {
            data: HashMap::new(),
        }
    }
    pub fn put_data_point(&mut self, id: &Identifier, data: Dynamic) {
        if data.is_unit() {
            self.data.remove(id);
        } else {
            self.data.insert(id.clone(), data);
        }
    }
    pub fn take_data_point(&mut self, id: &Identifier) -> Option<Dynamic> {
        self.data.remove(id)
    }
    pub fn get_data_point_ref(&mut self, id: &Identifier) -> Option<&mut Dynamic> {
        self.data.get_mut(id)
    }
}

pub struct EntityData {
    pub player: Weak<Entity>,
    slot: u32,
    speed: f32,
    move_type: MovementType,
    pub creative: bool,
    hand_item: Option<ItemStack>,
}

impl EntityData {
    pub fn new(player: Weak<Entity>) -> Self {
        EntityData {
            player,
            slot: u32::MAX,
            speed: 1.,
            move_type: MovementType::Normal,
            creative: false,
            hand_item: None,
        }
    }

    pub fn modify_inventory_hand<F>(&mut self, function: F)
    where
        F: FnOnce(&mut Option<ItemStack>),
    {
        function.call_once((&mut self.hand_item,));
        let set_as_empty = match &self.hand_item {
            Some(item) => item.get_count() == 0,
            None => true,
        };
        if set_as_empty {
            self.hand_item = None;
        }
        Inventory::set_cursor(self);
    }
    pub fn set_inventory_hand(&mut self, item: Option<ItemStack>) {
        self.hand_item = match item {
            Some(item) => {
                if item.get_count() == 0 {
                    None
                } else {
                    Some(item)
                }
            }
            None => None,
        };
        Inventory::set_cursor(self);
    }
    pub fn get_inventory_hand(&self) -> &Option<ItemStack> {
        &self.hand_item
    }
    fn send_abilities(&mut self) {
        self.player
            .upgrade()
            .unwrap()
            .try_send_message(&NetworkMessageS2C::PlayerAbilities(
                self.speed,
                self.move_type,
            ))
            .ok();
    }
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.send_abilities();
    }
    pub fn set_move_type(&mut self, move_type: MovementType) {
        self.move_type = move_type;
        self.send_abilities();
    }
    pub fn set_hand_slot(&mut self, slot: u32) {
        let player = self.player.upgrade().unwrap();
        let slot = if slot == u32::MAX {
            player.inventory.get_size() - 1
        } else {
            slot % player.inventory.get_size()
        };
        player
            .try_send_message(&NetworkMessageS2C::GuiEditElement(
                player.inventory.get_slot_id_entity(&player, self.slot),
                GUIElementEdit {
                    base_color: Some(Color::WHITE),
                    ..Default::default()
                },
            ))
            .ok();
        self.slot = slot;
        player
            .try_send_message(&NetworkMessageS2C::GuiEditElement(
                player.inventory.get_slot_id_entity(&player, self.slot),
                GUIElementEdit {
                    base_color: Some(Color {
                        r: 100,
                        g: 100,
                        b: 100,
                        a: 255,
                    }),
                    ..Default::default()
                },
            ))
            .ok();
    }
    pub fn get_hand_slot(&self) -> u32 {
        self.slot
    }
}

pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    pub(crate) rotation_shifting: Mutex<(f32, bool)>,
    teleport: Mutex<Option<ChunkLocation>>,
    pub entity_type: Arc<EntityType>,
    pub entity_data: Mutex<EntityData>,
    removed: AtomicBool,
    pub client_id: u32,
    id: Uuid,
    animation_controller: Mutex<AnimationController<Entity>>,
    pub inventory: Inventory,
    pub server: Arc<Server>,
    open_inventory: Mutex<Option<(InventoryWrapper, Uuid)>>,
    pub connection: Mutex<Option<PlayerConnection>>,
    velocity: Mutex<(f64, f64, f64)>,
    pub user_data: Mutex<UserData>,
    pub chunk_loading_manager: Mutex<ChunkLoadingManager>,
}

static ENTITY_CLIENT_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);

impl Entity {
    pub fn new<T: Into<ChunkLocation>>(
        location: T,
        entity_type: &Arc<EntityType>,
        connection: Option<PlayerConnection>,
    ) -> Arc<Entity> {
        let location: ChunkLocation = location.into();
        let position = location.position;
        let chunk = location.chunk.clone();
        let server = location.chunk.world.server.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            server: server.clone(),
            location: Mutex::new(location),
            entity_type: entity_type.clone(),
            removed: AtomicBool::new(false),
            this: weak.clone(),
            client_id: ENTITY_CLIENT_ID_GENERATOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            id: Uuid::new_v4(),
            teleport: Mutex::new(None),
            entity_data: Mutex::new(EntityData::new(weak.clone())),
            rotation_shifting: Mutex::new((0., false)),
            animation_controller: Mutex::new(AnimationController::new(weak.clone(), 1)),
            inventory: Inventory::new(
                WeakInventoryWrapper::Entity(weak.clone()),
                18,
                None,
                None,
                None,
            ),
            open_inventory: Mutex::new(None),
            connection: Mutex::new(connection),
            velocity: Mutex::new((0., 0., 0.)),
            user_data: Mutex::new(UserData::new()),
            chunk_loading_manager: Mutex::new(ChunkLoadingManager::new(weak.clone(), server)),
        });
        entity
            .try_send_message(&NetworkMessageS2C::TeleportPlayer(position, 0.))
            .ok();

        if entity.is_player() {
            entity
                .try_send_message(&NetworkMessageS2C::ControllingEntity(
                    entity.entity_type.client_id,
                ))
                .unwrap();
            entity.inventory.add_viewer(GuiInventoryViewer {
                slot_range: 0..9,
                slots: {
                    let mut slots = Vec::with_capacity(9);
                    for i in 0..9 {
                        slots.push((PositionAnchor::Bottom, ((i - 4) as f32 * 130.), 100.));
                    }
                    slots
                },
                id: entity.get_id().clone(),
                viewer: entity.clone(),
            });
            for chunk_position in Entity::get_chunks_to_load_at(&chunk.world.server, &position) {
                chunk
                    .world
                    .load_chunk(chunk_position)
                    .add_viewer(entity.clone());
            }
        }

        if entity.is_player() {
            let mut entity_data = entity.entity_data.lock();
            entity_data.set_hand_slot(0);
            Inventory::set_cursor(&mut *entity_data);
        }
        chunk.add_entity(entity.clone());
        let add_message = entity.create_add_messages(position);
        for viewer in chunk.viewers.lock().iter() {
            if viewer.player.id != entity.id {
                viewer.player.try_send_messages(&add_message).unwrap();
            }
        }
        if entity.is_player() {
            entity.set_open_inventory(None);
        }
        /*entity.try_send_message(&NetworkMessageS2C::PlayerAbilities(
            1.,
            crate::net::MovementType::NoClip,
        ));*/
        entity
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
            let viewer = data.into_viewer(self.this.upgrade().unwrap());
            let id = viewer.id.clone();
            inventory
                .as_ref()
                .unwrap()
                .get_inventory()
                .add_viewer(viewer);
            Some(id)
        } else {
            let mut player_data = self.entity_data.lock();
            //todo: drop hand item
            player_data.set_inventory_hand(None);
            Inventory::set_cursor(&mut *player_data);
            None
        };
        self.try_send_message(&NetworkMessageS2C::SetCursorLock(inventory.is_none()))
            .unwrap();
        self.try_send_message(&NetworkMessageS2C::GuiRemoveElements("cursor".to_string()))
            .unwrap();
        if inventory.is_none() {
            self.try_send_message(&NetworkMessageS2C::GuiSetElement(
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
                    },
                },
            ))
            .unwrap();
        }
        *current_inventory = inventory.map(|inventory| (inventory, view_id.unwrap()));
    }
    pub fn get_id(&self) -> &Uuid {
        &self.id
    }
    pub fn try_send_message(&self, message: &NetworkMessageS2C) -> Result<(), ()> {
        if let Some(connection) = &mut *self.connection.lock() {
            connection.send(message);
            Ok(())
        } else {
            Err(())
        }
    }
    pub fn try_send_messages(&self, messages: &Vec<NetworkMessageS2C>) -> Result<(), ()> {
        if let Some(connection) = &mut *self.connection.lock() {
            for message in messages {
                connection.send(message);
            }
            Ok(())
        } else {
            Err(())
        }
    }
    pub fn send_chat_message(&self, text: String) {
        self.try_send_message(&NetworkMessageS2C::ChatMessage(text))
            .ok();
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
            messages.push(NetworkMessageS2C::EntityItem(
                self.client_id,
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
        self.try_send_message(&NetworkMessageS2C::TeleportPlayer(
            position,
            rotation_shifting
                .map(|rotation_shifting| rotation_shifting.0)
                .unwrap_or(f32::NAN),
        ))
        .ok();
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
    pub fn get_chunks_to_load_at(server: &Server, position: &Position) -> FxHashSet<ChunkPosition> {
        let chunk_pos = position.to_chunk_pos();
        let vertical_view_distance =
            server.settings.get_i64("server.view_distance.vertical", 16) as i32;
        let horizontal_view_distance = server
            .settings
            .get_i64("server.view_distance.horizontal", 8)
            as i32;
        let mut positions = FxHashSet::default();
        for x in (-vertical_view_distance)..=vertical_view_distance {
            for y in (-horizontal_view_distance)..=horizontal_view_distance {
                for z in (-vertical_view_distance)..=vertical_view_distance {
                    positions.insert(ChunkPosition {
                        x: chunk_pos.x + x,
                        y: chunk_pos.y + y,
                        z: chunk_pos.z + z,
                    });
                }
            }
        }
        positions
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
        self.chunk_loading_manager.lock().tick();
        let mut teleport_location = { self.teleport.lock().as_ref().map(|loc| loc.clone()) };
        if !self.is_player() {
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
                        viewer.player.try_send_message(&delete_message).unwrap();
                    }
                    for viewer in new_viewers.difference(&old_viewers) {
                        if self.id != viewer.player.id {
                            viewer.player.try_send_messages(&add_message).unwrap();
                        }
                    }
                }
                let is_player = self.is_player();
                if is_player {
                    let old_loaded =
                        Entity::get_chunks_to_load_at(&self.server, &old_location.position);
                    let new_loaded =
                        Entity::get_chunks_to_load_at(&self.server, &new_location.position);

                    if !Arc::ptr_eq(&old_location.chunk.world, &new_location.chunk.world) {
                        for pos in old_loaded {
                            old_location
                                .chunk
                                .world
                                .load_chunk(pos)
                                .remove_viewer(self, true);
                        }
                        for pos in new_loaded {
                            new_location
                                .chunk
                                .world
                                .load_chunk(pos)
                                .add_viewer(self.this.upgrade().unwrap());
                        }
                    } else {
                        let world = old_location.chunk.world.clone(); //old or new doesn't matter
                        for pos in old_loaded.difference(&new_loaded) {
                            world.load_chunk(pos.clone()).remove_viewer(self, true);
                        }
                        for pos in new_loaded.difference(&old_loaded) {
                            world
                                .load_chunk(pos.clone())
                                .add_viewer(self.this.upgrade().unwrap());
                        }
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
        if let Some(ticker) = &*self.entity_type.ticker.lock() {
            let _ = ticker.call(&self.server.engine, (self.this.upgrade().unwrap(),));
        }
        if self.is_player() {
            let messages = self.connection.lock().as_mut().unwrap().receive_messages();
            for message in messages {
                match message {
                    NetworkMessageC2S::Keyboard(key, key_mod, pressed, _repeat) => {
                        match key {
                            KeyboardKey::Q => {
                                if pressed {
                                    let slot = { self.entity_data.lock().slot };
                                    self.inventory
                                        .get_full_view()
                                        .modify_item(slot, |item| {
                                            let item = item.as_mut();
                                            if let Some(item) = item {
                                                let mut location = self.get_location();
                                                location.position.y += 1.7;
                                                let item_entity = Entity::new(
                                                    location,
                                                    self.server
                                                        .entity_registry
                                                        .entity_by_identifier(&Identifier::new(
                                                            "bb", "item",
                                                        ))
                                                        .unwrap(),
                                                    None,
                                                );
                                                let count = if key_mod & KeyboardModifier::CTRL != 0
                                                {
                                                    item.get_count()
                                                } else {
                                                    1
                                                };
                                                item_entity
                                                    .inventory
                                                    .get_full_view()
                                                    .set_item(0, Some(item.copy(count)))
                                                    .unwrap();

                                                item.add_count(-(count as i32));

                                                let rotation = { *self.rotation_shifting.lock() };
                                                let rotation_radians = rotation.0.to_radians();
                                                item_entity.apply_knockback(
                                                    rotation_radians.sin() as f64,
                                                    0.,
                                                    rotation_radians.cos() as f64,
                                                );
                                                *item_entity.rotation_shifting.lock() =
                                                    (rotation.0, false);
                                            }
                                        })
                                        .unwrap();
                                }
                            }
                            KeyboardKey::Tab => {
                                if pressed {
                                    if self.open_inventory.lock().is_some() {
                                        self.set_open_inventory(None);
                                    } else {
                                        if self.entity_data.lock().creative {
                                            self.set_open_inventory(Some((InventoryWrapper::Own(Arc::new(
                                    {
                                        let inventory = Inventory::new(
                                            self,
                                            27,
                                            Some(Box::new(move |inventory: &Inventory, entity: &Entity, slot: u32, _: MouseButton, _: bool| {
                                                let mut entity_data = entity.entity_data.lock();
                                                let hand_empty = entity_data.hand_item.is_none();
                                                if hand_empty {
                                                    entity_data.set_inventory_hand(inventory.get_full_view().get_item(slot).unwrap().clone());
                                                } else {
                                                    entity_data.set_inventory_hand(None);
                                                }
                                                InteractionResult::Consumed
                                            })),
                                            Some(Box::new(|inventory: &Inventory, entity: &Entity, slot: u32, _: i32, y: i32, _: bool| {
                                                let mut entity_data = entity.entity_data.lock();
                                                entity_data.modify_inventory_hand(|item| {
                                                    match &mut *item {
                                                        Some(item) => {
                                                            item.add_count(if y < 0 { -1 } else { 1 });
                                                        }
                                                        None => {
                                                            if y > 0 {
                                                                if let Some(slot_item) = inventory.get_full_view().get_item(slot).unwrap() {
                                                                    *item = Some(slot_item.copy(1))
                                                                }
                                                            }
                                                        }
                                                    }
                                                });
                                                InteractionResult::Consumed
                                            })),
                                            None,
                                        );
                                        let item_registry = &self.server.item_registry;
                                        for (i, id) in item_registry.list().into_iter().enumerate()
                                        {
                                            let item_type = item_registry
                                                .item_by_identifier(id)
                                                .unwrap();
                                            let item_count = item_type.stack_size;
                                            inventory.get_full_view()
                                                .set_item(
                                                    i as u32,
                                                    Some(ItemStack::new(item_type, item_count)),
                                                )
                                                .ok();
                                        }
                                        inventory
                                    }),
                                ), GuiInventoryData{
                                                slots: {
                                                    let mut slots = Vec::with_capacity(27);
                                                    for y in 0..3 {
                                                        for x in 0..9 {
                                                            slots.push((PositionAnchor::Center,
                                                                        ((x-4) as f32 * 130.),
                                                                        (y-1) as f32 * 130.,
                                                            ));
                                                        }
                                                    }
                                                    slots
                                                },
                                                slot_range: 0..27
                                            })));
                                        } else {
                                            self.set_open_inventory(Some((
                                                InventoryWrapper::Entity(self.ptr()),
                                                GuiInventoryData {
                                                    slots: {
                                                        let mut slots = Vec::with_capacity(9);
                                                        for i in 0..9 {
                                                            slots.push((
                                                                PositionAnchor::Center,
                                                                ((i - 4) as f32 * 130.),
                                                                0.,
                                                            ));
                                                        }
                                                        slots
                                                    },
                                                    slot_range: 9..18,
                                                },
                                            )));
                                        }
                                    }
                                }
                            }
                            KeyboardKey::C => {
                                if pressed {
                                    if self.open_inventory.lock().is_some() {
                                        self.set_open_inventory(None);
                                    } else {
                                        let inventory = Inventory::new_owned(
                                        27,
                                        Some(Box::new(
                                            move|inventory: &Inventory, player: &Entity, id: u32, _: MouseButton, _: bool| {
                                                let recipes: Array = inventory
                                                    .get_user_data()
                                                    .get_data_point_ref(&Identifier::new(
                                                        "bb", "recipes",
                                                    ))
                                                    .cloned()
                                                    .unwrap()
                                                    .into_array()
                                                    .unwrap();
                                                if let Some(recipe) = recipes.get(id as usize) {
                                                    let recipe = player
                                                        .server
                                                        .recipes
                                                        .by_id(&recipe.clone().cast::<Identifier>())
                                                        .unwrap();
                                                    if let Ok(_) =
                                                        recipe.consume_inputs(&player.inventory)
                                                    {
                                                        recipe.add_outputs(&player.inventory);
                                                    }
                                                }
                                                InteractionResult::Consumed
                                            },
                                        )),
                                        Some(Box::new(|_, _, _, _, _, _| {
                                            InteractionResult::Ignored
                                        })),
                                        None,
                                    );
                                        let mut recipes_user_map = Vec::new();
                                        for (i, recipe) in self
                                            .server
                                            .recipes
                                            .by_type(&Identifier::new("bb", "crafting"))
                                            .iter()
                                            .enumerate()
                                        {
                                            recipes_user_map.push(Dynamic::from(recipe.id.clone()));
                                            inventory
                                                .get_full_view()
                                                .set_item(i as u32, Some(recipe.get_icon()))
                                                .unwrap();
                                        }
                                        inventory.get_user_data().put_data_point(
                                            &Identifier::new("bb", "recipes"),
                                            Dynamic::from_array(recipes_user_map),
                                        );
                                        self.set_open_inventory(Some((
                                            InventoryWrapper::Own(inventory),
                                            GuiInventoryData {
                                                slots: {
                                                    let mut slots = Vec::with_capacity(27);
                                                    for y in 0..3 {
                                                        for x in 0..9 {
                                                            slots.push((
                                                                PositionAnchor::Center,
                                                                ((x - 4) as f32 * 130.),
                                                                (y - 1) as f32 * 130.,
                                                            ));
                                                        }
                                                    }
                                                    slots
                                                },
                                                slot_range: 0..27,
                                            },
                                        )));
                                    }
                                }
                                /*let mut inventory = self.inventory.lock().unwrap();
                                let recipe = self
                                    .server
                                    .recipes
                                    .by_id(&Identifier::new("example", "planks"))
                                    .unwrap();
                                */
                            }
                            KeyboardKey::Escape => {
                                self.set_open_inventory(None);
                            }
                            _ => {}
                        }
                        if let Some(slot) = key.get_slot() {
                            if pressed {
                                self.entity_data.lock().set_hand_slot(slot as u32);
                            }
                        }
                    }
                    NetworkMessageC2S::GuiClick(element, button, shifting) => {
                        {
                            let slot = self.inventory.resolve_slot(self.get_id(), element.as_str());
                            if let Some(slot) = slot {
                                self.inventory.on_click_slot(self, slot, button, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *self.open_inventory.lock() {
                                let inventory = open_inventory.0.get_inventory();
                                let slot =
                                    inventory.resolve_slot(&open_inventory.1, element.as_str());
                                if let Some(slot) = slot {
                                    inventory.on_click_slot(self, slot, button, shifting);
                                    continue;
                                }
                            }
                        }
                    }
                    NetworkMessageC2S::GuiScroll(element, x, y, shifting) => {
                        {
                            let slot = self.inventory.resolve_slot(self.get_id(), element.as_str());
                            if let Some(slot) = slot {
                                self.inventory.on_scroll_slot(self, slot, x, y, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *self.open_inventory.lock() {
                                let inventory = open_inventory.0.get_inventory();
                                let slot =
                                    inventory.resolve_slot(&open_inventory.1, element.as_str());
                                if let Some(slot) = slot {
                                    inventory.on_scroll_slot(self, slot, x, y, shifting);
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
                        let block_break_time = if self.entity_data.lock().creative {
                            0.
                        } else {
                            let world = self.get_location().chunk.world.clone();
                            let block_state = world.get_block_load(&position).get_block_state();
                            let block_state =
                                world.server.block_registry.state_by_ref(&block_state);
                            let block_tool = &block_state.parent.breaking_data;
                            let item = self.get_hand_item();
                            let tool_data = item
                                .as_ref()
                                .and_then(|item| item.item_type.tool_data.as_ref());
                            let block_break_time = match (&block_tool.1, tool_data) {
                                (Some(block_tool), Some(tool_data)) => {
                                    if (!tool_data.breaks_type(block_tool.0))
                                        || (tool_data.hardness < block_tool.1 && block_tool.1 != 0.)
                                    {
                                        -1.
                                    } else {
                                        tool_data.speed
                                    }
                                }
                                (Some(block_tool), None) => {
                                    if block_tool.1 != 0. {
                                        -1.
                                    } else {
                                        1.
                                    }
                                }
                                _ => tool_data.map(|tool_data| tool_data.speed).unwrap_or(1.),
                            };
                            block_break_time / block_tool.0
                        };
                        if block_break_time >= 0. {
                            self.try_send_message(&NetworkMessageS2C::BlockBreakTimeResponse(
                                id,
                                block_break_time,
                            ))
                            .unwrap();
                        }
                        //todo: check time
                    }
                    NetworkMessageC2S::BreakBlock(block_position) => {
                        let world = &self.get_location().chunk.world;
                        world.break_block(block_position, BlockBreakParameters::from_entity(self));
                    }
                    NetworkMessageC2S::RightClickBlock(block_position, face, shifting) => {
                        let hand_slot = self.entity_data.lock().get_hand_slot();
                        let block = self
                            .get_location()
                            .chunk
                            .world
                            .get_block_load(&block_position);
                        let mut right_click_result = InteractionResult::Ignored;
                        if !shifting {
                            right_click_result = match block {
                                BlockData::Simple(_) => InteractionResult::Ignored,
                                BlockData::Data(block) => block.on_right_click(self),
                            };
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
                                            self.this.upgrade().unwrap(),
                                            block_position,
                                            face,
                                        );
                                }
                            })
                            .unwrap();
                    }
                    NetworkMessageC2S::RightClick(_shifting) => {
                        let hand_slot = self.entity_data.lock().get_hand_slot();
                        let mut right_click_result = InteractionResult::Ignored;
                        self.inventory
                            .get_full_view()
                            .modify_item(hand_slot, |stack| {
                                if let Some(stack) = stack {
                                    //todo: send shifting state
                                    right_click_result = stack
                                        .item_type
                                        .clone()
                                        .on_right_click(stack, self.this.upgrade().unwrap());
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
                        let mut player_data = self.entity_data.lock();
                        let new_slot = player_data.get_hand_slot() as i32 - scroll_y;
                        player_data.set_hand_slot(new_slot as u32);
                    }
                    NetworkMessageC2S::SendMessage(message) => {
                        if message.starts_with("/") {
                            let message = &message[1..].trim_end();
                            let parts: rhai::Array = message
                                .split(" ")
                                .map(|str| Dynamic::from_str(str).unwrap())
                                .collect();
                            self.server.call_event(
                                Identifier::new("bb", "command"),
                                (self.this.upgrade().unwrap(), parts),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
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
        inventory.get_item(self.entity_data.lock().slot).unwrap()
    }
    pub fn remove(&self) {
        self.removed
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
    pub fn is_removed(&self) -> bool {
        self.removed.load(std::sync::atomic::Ordering::Relaxed)
            | self
                .connection
                .lock()
                .as_ref()
                .map(|connection| connection.is_closed())
                .unwrap_or(false)
    }
    pub fn post_remove(&self) {
        if self.is_player() {
            let (world, position) = {
                let location = self.location.lock();
                (location.chunk.world.clone(), location.position)
            };
            let loading_chunks = Entity::get_chunks_to_load_at(&self.server, &position.clone());
            for chunk_position in loading_chunks {
                world.load_chunk(chunk_position).remove_viewer(self, false);
            }
        }
        if let Some(inventory) = self.open_inventory.lock().as_ref() {
            inventory.0.get_inventory().remove_viewer(self.get_id());
        }
    }
    fn is_player(&self) -> bool {
        self.connection.lock().is_some()
    }
    pub fn ptr(&self) -> Arc<Entity> {
        self.this.upgrade().unwrap()
    }
}
impl Animatable for Entity {
    fn send_animation_to_viewers(&self, animation: u32) {
        self.get_location()
            .chunk
            .announce_to_viewers(&NetworkMessageS2C::EntityAnimation(
                self.client_id,
                animation,
            ));
    }
    fn send_animation_to(&self, viewer: &Entity, animation: u32) {
        viewer
            .try_send_message(&NetworkMessageS2C::EntityAnimation(
                self.client_id,
                animation,
            ))
            .ok();
    }
}
pub struct ChunkLoadingManager {
    server: Arc<Server>,
    entity: Weak<Entity>,
    to_load: HashSet<Arc<Chunk>>,
}
impl ChunkLoadingManager {
    pub fn new(entity: Weak<Entity>, server: Arc<Server>) -> Self {
        ChunkLoadingManager {
            server,
            entity,
            to_load: HashSet::new(),
        }
    }
    pub fn load(&mut self, chunk: Arc<Chunk>) {
        self.to_load.insert(chunk);
    }
    pub fn unload(&mut self, chunk: Arc<Chunk>) {
        self.entity
            .upgrade()
            .unwrap()
            .try_send_message(&NetworkMessageS2C::UnloadChunk(chunk.position))
            .ok();
    }
    pub fn tick(&mut self) {
        for chunk in self
            .to_load
            .extract_if(|chunk| chunk.loading_stage.load(Ordering::Relaxed) >= 2)
            .take(
                self.server
                    .settings
                    .get_i64("server.max_chunks_sent_per_tick", 200) as usize,
            )
        {
            let entity = self.entity.upgrade().unwrap();
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
                entity.try_send_message(&load_message).ok();
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
                    .state_by_ref(&world.get_block_load(position).get_block_state()),))
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
    pub fn sync_to(&self, viewer: &Entity) {
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
    fn send_animation_to(&self, viewer: &Entity, animation: u32);
}
#[derive(Clone)]
pub struct Structure {
    id: Identifier,
    blocks: Vec<(BlockPosition, (BlockStateRef, f32))>,
}

impl Structure {
    pub fn from_json(id: Identifier, json: JsonValue, block_registry: &BlockRegistry) -> Self {
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
                }, //todo: place correct state
            ));
        }
        Structure { blocks, id }
    }
    pub fn from_world(
        id: Identifier,
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
        Structure { id, blocks }
    }
    pub fn export(&self, block_registry: &BlockRegistry) -> JsonValue {
        let mut blocks = Vec::new();
        for (position, block) in &self.blocks {
            let state = block_registry.state_by_ref(&block.0);
            blocks.push(object! {
                x:position.x,
                y:position.y,
                z:position.z,
                id:state.parent.id.to_string(),
                state: state.state_id
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

pub struct WorldBlock {
    this: Weak<WorldBlock>,
    pub chunk: Weak<Chunk>,
    pub position: BlockPosition,
    pub state: BlockStateRef,
    pub block: Arc<Block>,
    pub inventory: Inventory,
    user_data: Mutex<UserData>,
    animation_controller: AnimationController<WorldBlock>,
}

impl WorldBlock {
    pub fn new(location: ChunkBlockLocation, state: BlockStateRef) -> Arc<WorldBlock> {
        Arc::new_cyclic(|this| WorldBlock {
            chunk: Arc::downgrade(&location.chunk),
            position: location.position,
            state,
            inventory: Inventory::new(
                WeakInventoryWrapper::Block(this.clone()),
                9,
                None,
                None,
                None,
            ),
            animation_controller: AnimationController::new(this.clone(), 0),
            block: location
                .chunk
                .world
                .server
                .block_registry
                .state_by_ref(&state)
                .parent
                .clone(),
            user_data: Mutex::new(UserData::new()),
            this: this.clone(),
        })
    }
    pub fn on_sent_to_client(&self, player: &Entity) {
        self.animation_controller.sync_to(player);
        for (inventory_index, model_index) in &self.block.item_model_mapping.mapping {
            player
                .try_send_message(&NetworkMessageS2C::BlockItem(
                    self.position,
                    *model_index,
                    self.inventory
                        .get_full_view()
                        .get_item(*inventory_index)
                        .unwrap()
                        .as_ref()
                        .map(|item| item.item_type.client_id),
                ))
                .ok();
        }
    }
    pub fn tick(&self) {
        let recipe_time_identifier = Identifier::new("bb", "recipe_time");
        let mut user_data = self.user_data.lock();
        if let Some(time) = user_data.take_data_point(&recipe_time_identifier) {
            let mut time = time.as_int().unwrap();
            time -= 1;
            user_data.put_data_point(
                &recipe_time_identifier,
                if time <= 0 {
                    let chunk = self.chunk.upgrade().unwrap();
                    let recipes = chunk
                        .world
                        .server
                        .recipes
                        .by_type(&self.block.machine_data.as_ref().unwrap().recipe_type);
                    for recipe in recipes {
                        if recipe.can_craft(&self.inventory) {
                            recipe.consume_inputs(&self.inventory).unwrap();
                            recipe.add_outputs(&self.inventory);
                            break;
                        }
                    }
                    Dynamic::UNIT
                } else {
                    Dynamic::from_int(time)
                },
            );
        } else {
            let chunk = self.chunk.upgrade().unwrap();
            let recipes = chunk
                .world
                .server
                .recipes
                .by_type(&self.block.machine_data.as_ref().unwrap().recipe_type);
            for recipe in recipes {
                if recipe.can_craft(&self.inventory) {
                    user_data.put_data_point(&recipe_time_identifier, Dynamic::from_int(20));
                    break;
                }
            }
        }
    }
    pub fn needs_ticking(&self) -> bool {
        self.block.machine_data.is_some()
    }
    pub fn on_right_click(&self, player: &Entity) -> InteractionResult {
        player.set_open_inventory(Some((
            InventoryWrapper::Block(self.this.upgrade().unwrap()),
            GuiInventoryData {
                slots: {
                    let mut slots = Vec::with_capacity(9);
                    for i in 0..9 {
                        slots.push((PositionAnchor::Center, ((i - 4) as f32 * 130.), 0.));
                    }
                    slots
                },
                slot_range: 0..9,
            },
        )));
        InteractionResult::Consumed
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
            chunk.announce_to_viewers(&NetworkMessageS2C::BlockItem(
                self.position,
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
}
impl Animatable for WorldBlock {
    fn send_animation_to_viewers(&self, animation: u32) {
        self.chunk
            .upgrade()
            .unwrap()
            .announce_to_viewers(&NetworkMessageS2C::BlockAnimation(self.position, animation));
    }
    fn send_animation_to(&self, viewer: &Entity, animation: u32) {
        viewer
            .try_send_message(&NetworkMessageS2C::BlockAnimation(self.position, animation))
            .ok();
    }
}

impl Into<WeakInventoryWrapper> for &WorldBlock {
    fn into(self) -> WeakInventoryWrapper {
        WeakInventoryWrapper::Block(self.this.clone())
    }
}
