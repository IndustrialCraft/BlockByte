use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8},
        Arc, Mutex, Weak,
    },
};

use array_init::array_init;
use atomic_counter::{AtomicCounter, RelaxedCounter};
use endio::{BERead, LEWrite};
use flate2::Compression;
use fxhash::{FxHashMap, FxHashSet};
use json::{array, object, JsonValue};

use uuid::Uuid;

use crate::{
    inventory::{Inventory, InventoryWrapper, ItemStack},
    net::{self, MouseButton, MovementType, NetworkMessageS2C, PlayerConnection},
    registry::{BlockRegistry, BlockStateRef, EntityType, InteractionResult},
    util::{
        BlockPosition, ChunkBlockLocation, ChunkLocation, ChunkPosition, Identifier, Location,
        Position,
    },
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
                        self.unloaded_structure_placements.lock().unwrap();

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
    pub fn set_block(&self, position: BlockPosition, block: BlockStateRef) {
        let chunk_offset = position.chunk_offset();
        self.load_chunk(position.to_chunk_pos()).set_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
            block,
        );
    }
    pub fn get_block(&self, position: BlockPosition) -> BlockData {
        let chunk_offset = position.chunk_offset();
        self.load_chunk(position.to_chunk_pos()).get_block(
            chunk_offset.0,
            chunk_offset.1,
            chunk_offset.2,
        )
    }
    pub fn replace_block<F>(&self, position: BlockPosition, replacer: F)
    where
        F: FnOnce(BlockData) -> Option<BlockStateRef>,
    {
        let chunk_offset = position.chunk_offset();
        let chunk = self.load_chunk(position.to_chunk_pos());
        let new_block =
            replacer.call_once((chunk.get_block(chunk_offset.0, chunk_offset.1, chunk_offset.2),));
        if let Some(new_block) = new_block {
            chunk.set_block(chunk_offset.0, chunk_offset.1, chunk_offset.2, new_block);
        }
    }
    pub fn load_chunk(&self, position: ChunkPosition) -> Arc<Chunk> {
        {
            let chunks = self.chunks.lock().unwrap();
            if let Some(chunk) = chunks.get(&position) {
                return chunk.clone();
            }
        }
        let mut chunks = self.chunks.lock().unwrap();
        let chunk = Chunk::new(position, self.this.upgrade().unwrap());
        chunks.insert(position, chunk.clone());
        chunk
    }
    pub fn get_chunk(&self, position: ChunkPosition) -> Option<Arc<Chunk>> {
        let chunks = self.chunks.lock().unwrap();
        chunks.get(&position).map(|c| c.clone())
    }
    pub fn tick(&self) {
        let chunks_to_tick: Vec<Arc<Chunk>> = self
            .chunks
            .lock()
            .unwrap()
            .values()
            .map(|c| c.clone())
            .collect();
        for chunk in chunks_to_tick {
            chunk.tick();
        }
        let non_empty = {
            let mut chunks = self.chunks.lock().unwrap();
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
        for chunk in self.chunks.lock().unwrap().drain() {
            chunk.1.destroy();
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
}

pub struct Chunk {
    pub position: ChunkPosition,
    pub world: Arc<World>,
    blocks: Mutex<[[[BlockData; 16]; 16]; 16]>,
    entities: Mutex<Vec<Arc<Entity>>>,
    viewers: Mutex<FxHashSet<ChunkViewer>>,
    unload_timer: AtomicU8,
    loading_stage: AtomicU8,
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
            this: this.clone(),
        });
        let gen_chunk = chunk.clone();
        world.clone().server.thread_pool.execute(Box::new(move || {
            {
                let save_path = gen_chunk.get_chunk_path();
                if save_path.exists() {
                    let data = std::fs::read(save_path).unwrap(); //todo: if save data is corrupted, regenerate chunk
                    let mut data = data.as_slice();
                    let block_map_len: u32 = data.read_be().unwrap();
                    let mut blocks = Vec::with_capacity(block_map_len as usize);
                    let block_registry = &gen_chunk.world.server.block_registry;
                    for _ in 0..block_map_len {
                        blocks.push(
                            block_registry
                                .block_by_identifier(
                                    &Identifier::parse(net::read_string(&mut data).as_str())
                                        .unwrap(),
                                )
                                .unwrap(),
                        );
                    }
                    let chunk = gen_chunk.clone();
                    *gen_chunk.blocks.lock().unwrap() = array_init(|x| {
                        array_init(|y| {
                            array_init(|z| {
                                let block_id: u16 = data.read_be().unwrap();
                                let block = blocks.get(block_id as usize).unwrap();
                                let block_data = block.get_default_state_ref().create_block_data(
                                    &chunk,
                                    BlockPosition {
                                        x: (position.x * 16) + x as i32,
                                        y: (position.y * 16) + y as i32,
                                        z: (position.z * 16) + z as i32,
                                    },
                                );
                                if let BlockData::Data(block) = &block_data {
                                    let length: u32 = data.read_be().unwrap();
                                    //todo: optimize
                                    let mut block_data: Vec<u8> =
                                        Vec::with_capacity(length as usize);
                                    for _ in 0..length {
                                        block_data.push(data.read_be().unwrap());
                                    }
                                    block.deserialize(block_data.as_slice());
                                }
                                block_data
                            })
                        })
                    });
                } else {
                    *gen_chunk.blocks.lock().unwrap() =
                        gen_chunk.world.world_generator.generate(&gen_chunk);
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
                    .unwrap()
                    .remove(&position)
            } {
                for (position, structure) in placement_list {
                    gen_chunk.place_structure(position, structure);
                }
            }
            gen_chunk
                .loading_stage
                .store(2, std::sync::atomic::Ordering::SeqCst);
            let mut data = Vec::with_capacity(16 * 16 * 16 * 4);
            {
                let real_blocks = gen_chunk.blocks.lock().unwrap();
                for x in 0..16 {
                    for y in 0..16 {
                        for z in 0..16 {
                            data.write_be(real_blocks[x][y][z].get_client_id()).unwrap();
                        }
                    }
                }
            }
            //let mut encoder = Encoder::new().unwrap();
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
            std::io::copy(&mut data.as_slice(), &mut encoder).unwrap();
            let load_message = NetworkMessageS2C::LoadChunk(
                position.x,
                position.y,
                position.z,
                encoder.finish().unwrap(),
            );
            for viewer in gen_chunk.viewers.lock().unwrap().iter() {
                viewer.player.try_send_message(&load_message).ok();
            }
        }));
        chunk
    }
    pub fn place_structure(&self, position: BlockPosition, structure: Arc<Structure>) {
        structure.place(
            |block_position, block| {
                if block_position.to_chunk_pos() == self.position {
                    let offset = block_position.chunk_offset();
                    self.set_block(offset.0, offset.1, offset.2, block);
                }
            },
            position,
        );
    }
    pub fn set_block(&self, offset_x: u8, offset_y: u8, offset_z: u8, block: BlockStateRef) {
        let block_position = BlockPosition {
            x: self.position.x * 16 + offset_x as i32,
            y: self.position.y * 16 + offset_y as i32,
            z: self.position.z * 16 + offset_z as i32,
        };
        let block = block.create_block_data(&self.this.upgrade().unwrap(), block_position);
        if self.loading_stage.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            self.announce_to_viewers(NetworkMessageS2C::SetBlock(
                block_position.x,
                block_position.y,
                block_position.z,
                block.get_client_id(),
            ));
        }
        self.blocks.lock().unwrap()[offset_x as usize][offset_y as usize][offset_z as usize] =
            block;
    }
    pub fn get_block(&self, offset_x: u8, offset_y: u8, offset_z: u8) -> BlockData {
        self.blocks.lock().unwrap()[offset_x as usize][offset_y as usize][offset_z as usize].clone()
    }
    fn add_entity(&self, entity: Arc<Entity>) {
        self.entities.lock().unwrap().push(entity);
    }
    fn add_viewer(&self, viewer: Arc<Entity>) {
        if self.loading_stage.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            let mut data = Vec::with_capacity(16 * 16 * 16 * 4);
            {
                let real_blocks = self.blocks.lock().unwrap();
                for x in 0..16 {
                    for y in 0..16 {
                        for z in 0..16 {
                            data.write_be(real_blocks[x][y][z].get_client_id()).unwrap();
                        }
                    }
                }
            }
            let thread_viewer = viewer.clone();
            let position = self.position.clone();
            self.world.server.thread_pool.execute(Box::new(move || {
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
                std::io::copy(&mut data.as_slice(), &mut encoder).unwrap();
                let load_message = NetworkMessageS2C::LoadChunk(
                    position.x,
                    position.y,
                    position.z,
                    encoder.finish().unwrap(),
                );
                thread_viewer.try_send_message(&load_message).ok();
            }));
        }
        for entity in self.entities.lock().unwrap().iter() {
            if Arc::ptr_eq(entity, &viewer) {
                continue;
            }
            viewer
                .try_send_message(&entity.create_add_message(entity.get_location().position))
                .unwrap();
        }
        self.viewers
            .lock()
            .unwrap()
            .insert(ChunkViewer::new(viewer));
    }
    fn remove_viewer(&self, viewer: Arc<Entity>) {
        viewer
            .try_send_message(&NetworkMessageS2C::UnloadChunk(
                self.position.x,
                self.position.y,
                self.position.z,
            ))
            .unwrap();
        for entity in self.entities.lock().unwrap().iter() {
            if Arc::ptr_eq(entity, &viewer) {
                continue;
            }
            viewer
                .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                .unwrap();
        }
        self.viewers
            .lock()
            .unwrap()
            .remove(&ChunkViewer::new(viewer));
    }
    pub fn announce_to_viewers(&self, message: NetworkMessageS2C) {
        for viewer in self.viewers.lock().unwrap().iter() {
            viewer.player.try_send_message(&message).unwrap();
        }
    }
    pub fn tick(&self) {
        self.unload_timer
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut removed_entities = Vec::new();
        self.entities
            .lock()
            .unwrap()
            .extract_if(|entity| {
                let new_location = entity.get_location();
                let not_same_chunk = new_location.chunk.position != self.position;
                if not_same_chunk {
                    for viewer in self
                        .viewers
                        .lock()
                        .unwrap()
                        .difference(&new_location.chunk.viewers.lock().unwrap())
                    {
                        viewer
                            .player
                            .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                            .unwrap();
                    }
                }
                let removed = entity.is_removed();
                if removed && !not_same_chunk {
                    removed_entities.push(entity.clone());
                    for viewer in self.viewers.lock().unwrap().iter() {
                        viewer
                            .player
                            .try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id))
                            .unwrap();
                    }
                }
                removed || not_same_chunk
            })
            .count();
        for entity in removed_entities {
            entity.post_remove();
        }

        let entities: Vec<_> = self
            .entities
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.clone())
            .collect();
        if self.viewers.lock().unwrap().len() > 0 {
            self.unload_timer
                .store(0, std::sync::atomic::Ordering::Relaxed);
        }
        if self.needs_ticking() {
            self.world.server.thread_pool.execute(Box::new(move || {
                for entity in entities {
                    entity.tick();
                }
            }));
        }
    }
    pub fn needs_ticking(&self) -> bool {
        self.entities.lock().unwrap().len() > 0
    }
    pub fn should_unload(&self) -> bool {
        self.unload_timer.load(std::sync::atomic::Ordering::Relaxed) >= Chunk::UNLOAD_TIME
    }
    pub fn destroy(&self) {
        let chunk = self.this.upgrade().unwrap();
        if !self.world.temporary {
            self.world.server.thread_pool.execute(Box::new(move || {
                {
                    let mut data = Vec::new();
                    let mut block_data = Vec::with_capacity(16 * 16 * 16 * 2);
                    let mut block_map = FxHashMap::default();
                    let blocks = chunk.blocks.lock().unwrap();
                    let block_registry = &chunk.world.server.block_registry;
                    for x in 0..16 {
                        for y in 0..16 {
                            for z in 0..16 {
                                let block = &blocks[x][y][z];
                                let (block_state_ref, serialized_block) = match block {
                                    BlockData::Simple(id) => {
                                        (BlockStateRef::from_state_id(*id), None)
                                    }
                                    BlockData::Data(block) => {
                                        (block.state, Some(block.serialize()))
                                    }
                                };
                                let block = block_registry.state_by_ref(&block_state_ref);
                                let block_id = &block.parent.id; //todo: save state
                                let block_map_len = block_map.len();
                                let numeric_id =
                                    *block_map.entry(block_id).or_insert(block_map_len);
                                block_data.write_be(numeric_id as u16).unwrap();
                                if let Some(mut serialized_block) = serialized_block {
                                    block_data.write_be(serialized_block.len() as u32).unwrap();
                                    block_data.append(&mut serialized_block);
                                }
                            }
                        }
                    }
                    data.write_be(block_map.len() as u32).unwrap();
                    let mut block_map: Vec<_> = block_map.iter().collect();
                    block_map.sort_by(|first, second| first.1.cmp(second.1));
                    for block_map_entry in block_map {
                        net::write_string(&mut data, &block_map_entry.0.to_string());
                    }
                    data.append(&mut block_data);
                    std::fs::write(chunk.get_chunk_path(), data).unwrap();
                }
            }));
        }
        self.entities.lock().unwrap().clear();
        self.viewers.lock().unwrap().clear();
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
struct ChunkViewer {
    pub player: Arc<Entity>,
}
impl ChunkViewer {
    pub fn new(player: Arc<Entity>) -> Self {
        ChunkViewer { player }
    }
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
        let inventory = player.inventory.lock().unwrap();
        let slot = if slot == u32::MAX {
            inventory.get_size() - 1
        } else {
            slot % inventory.get_size()
        };
        player.try_send_message(&NetworkMessageS2C::GuiData(
                object! {id: inventory.get_slot_id(self.slot), type: "editElement", data_type: "color", color: array![1, 1, 1, 1]},
            )).ok();
        self.slot = slot;
        player.try_send_message(&NetworkMessageS2C::GuiData(
                object! {id: inventory.get_slot_id(self.slot), type: "editElement", data_type: "color", color: array![1, 0, 0, 1]},
            )).ok();
    }
    pub fn get_hand_slot(&self) -> u32 {
        self.slot
    }
}

pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    rotation: Mutex<f32>,
    teleport: Mutex<Option<ChunkLocation>>,
    entity_type: Arc<EntityType>,
    pub entity_data: Mutex<EntityData>,
    removed: AtomicBool,
    client_id: u32,
    id: Uuid,
    animation_controller: Mutex<AnimationController>,
    pub inventory: Mutex<Inventory>,
    pub server: Arc<Server>,
    open_inventory: Mutex<Option<InventoryWrapper>>,
    pub connection: Mutex<Option<PlayerConnection>>,
}
static ENTITY_CLIENT_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);
impl Entity {
    pub fn new<T: Into<ChunkLocation>>(
        location: T,
        entity_type: Arc<EntityType>,
        connection: Option<PlayerConnection>,
    ) -> Arc<Entity> {
        let location: ChunkLocation = location.into();
        let position = location.position;
        let chunk = location.chunk.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            server: location.chunk.world.server.clone(),
            location: Mutex::new(location),
            entity_type,
            removed: AtomicBool::new(false),
            this: weak.clone(),
            client_id: ENTITY_CLIENT_ID_GENERATOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            id: Uuid::new_v4(),
            teleport: Mutex::new(None),
            entity_data: Mutex::new(EntityData::new(weak.clone())),
            rotation: Mutex::new(0.),
            animation_controller: Mutex::new(AnimationController::new(weak.clone(), 1)),
            inventory: Mutex::new(Inventory::new(
                9,
                || {
                    let mut slots = Vec::with_capacity(9);
                    for i in 0..9 {
                        slots.push(((i as f32 * 0.13) - (4.5 * 0.13), -0.5));
                    }
                    slots
                },
                None,
                None,
            )),
            open_inventory: Mutex::new(None),
            connection: Mutex::new(connection),
        });
        entity
            .try_send_message(&NetworkMessageS2C::TeleportPlayer(
                position.x as f32,
                position.y as f32,
                position.z as f32,
                0.,
            ))
            .unwrap();

        entity
            .inventory
            .lock()
            .unwrap()
            .add_viewer(entity.this.upgrade().unwrap()); //todo: only add if is player

        {
            let mut entity_data = entity.entity_data.lock().unwrap();
            entity_data.set_hand_slot(0);
            Inventory::set_cursor(&mut *entity_data);
        }
        chunk.add_entity(entity.clone());
        let add_message = entity.create_add_message(position);
        for viewer in chunk.viewers.lock().unwrap().iter() {
            viewer.player.try_send_message(&add_message).unwrap();
        }
        for chunk_position in Entity::get_chunks_to_load_at(&chunk.world.server, &position) {
            chunk
                .world
                .load_chunk(chunk_position)
                .add_viewer(entity.clone());
        }

        /*entity.try_send_message(&NetworkMessageS2C::PlayerAbilities(
            1.,
            crate::net::MovementType::NoClip,
        ));*/
        entity
    }
    pub fn get_rotation(&self) -> f32 {
        *self.rotation.lock().unwrap()
    }
    pub fn set_open_inventory(&self, new_inventory: Option<InventoryWrapper>) {
        let mut current_inventory = self.open_inventory.lock().unwrap();
        if let Some(current_inventory) = &*current_inventory {
            current_inventory
                .get_inventory()
                .unwrap()
                .remove_viewer(self.this.upgrade().unwrap());
        }
        if let Some(new_inventory) = &new_inventory {
            new_inventory
                .get_inventory()
                .unwrap()
                .add_viewer(self.this.upgrade().unwrap());
        } else {
            let mut player_data = self.entity_data.lock().unwrap();
            //todo: drop hand item
            player_data.set_inventory_hand(None);
            Inventory::set_cursor(&mut *player_data);
        }
        self.try_send_message(&NetworkMessageS2C::GuiData(
            object! {"type":"setCursorLock",lock:new_inventory.is_none()},
        ))
        .unwrap();
        *current_inventory = new_inventory;
    }
    pub fn get_id(&self) -> &Uuid {
        &self.id
    }
    pub fn try_send_message(&self, message: &NetworkMessageS2C) -> Result<(), ()> {
        if let Some(connection) = &mut *self.connection.lock().unwrap() {
            connection.send(message);
            Ok(())
        } else {
            Err(())
        }
    }
    pub fn send_chat_message(&self, text: String) {
        self.try_send_message(&NetworkMessageS2C::ChatMessage(text))
            .ok();
    }
    pub fn create_add_message(&self, position: Position) -> NetworkMessageS2C {
        let animation_controller = self.animation_controller.lock().unwrap();
        NetworkMessageS2C::AddEntity(
            self.entity_type.id,
            self.client_id,
            position.x as f32,
            position.y as f32,
            position.z as f32,
            *self.rotation.lock().unwrap(),
            animation_controller.animation,
            animation_controller.animation_start_time,
        )
    }
    pub fn teleport<T: Into<ChunkLocation>>(&self, location: T, rotation: Option<f32>) {
        let location: ChunkLocation = location.into();
        let position = location.position.clone();
        self.move_to(location, rotation);
        self.try_send_message(&NetworkMessageS2C::TeleportPlayer(
            position.x as f32,
            position.y as f32,
            position.z as f32,
            rotation.unwrap_or(f32::NAN),
        ))
        .ok();
    }
    pub fn move_to<T: Into<ChunkLocation>>(&self, location: T, rotation: Option<f32>) {
        {
            *self.teleport.lock().unwrap() = Some(location.into());
        }
        if let Some(rotation) = rotation {
            *self.rotation.lock().unwrap() = rotation;
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
        let location = self.location.lock().unwrap();
        location.clone()
    }
    pub fn tick(&self) {
        let teleport_location = {
            self.teleport
                .lock()
                .unwrap()
                .as_ref()
                .map(|loc| loc.clone())
        };
        if let Some(teleport_location) = teleport_location {
            let old_location = { self.location.lock().unwrap().clone() };
            let new_location: ChunkLocation = teleport_location.clone();
            {
                *self.location.lock().unwrap() = new_location.clone();
            }
            if !Arc::ptr_eq(&old_location.chunk, &new_location.chunk) {
                new_location.chunk.add_entity(self.this.upgrade().unwrap());

                {
                    let old_viewers = old_location.chunk.viewers.lock().unwrap();
                    let new_viewers = new_location.chunk.viewers.lock().unwrap();
                    let add_message = self.create_add_message(new_location.position);
                    let delete_message = NetworkMessageS2C::DeleteEntity(self.client_id);
                    for viewer in old_viewers.difference(&new_viewers) {
                        viewer.player.try_send_message(&delete_message).unwrap();
                    }
                    for viewer in new_viewers.difference(&old_viewers) {
                        viewer.player.try_send_message(&add_message).unwrap();
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
                                .remove_viewer(self.this.upgrade().unwrap());
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
                            world
                                .load_chunk(pos.clone())
                                .remove_viewer(self.this.upgrade().unwrap());
                        }
                        for pos in new_loaded.difference(&old_loaded) {
                            world
                                .load_chunk(pos.clone())
                                .add_viewer(self.this.upgrade().unwrap());
                        }
                    }
                }
            }
            new_location
                .chunk
                .announce_to_viewers(NetworkMessageS2C::MoveEntity(
                    self.client_id,
                    new_location.position.x as f32,
                    new_location.position.y as f32,
                    new_location.position.z as f32,
                    *self.rotation.lock().unwrap(),
                ));
        }
        {
            *self.teleport.lock().unwrap() = None;
        }
        if let Some(ticker) = &*self.entity_type.ticker.lock().unwrap() {
            ticker.call(&self.server.engine, (self.this.upgrade().unwrap(),));
        }
        if self.is_player() {
            let messages = self
                .connection
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .receive_messages();
            for message in messages {
                match message {
                    net::NetworkMessageC2S::Keyboard(key, release, repeat) => match key {
                        9 => {
                            if self.entity_data.lock().unwrap().creative {
                                self.set_open_inventory(Some(InventoryWrapper::Own(Arc::new(
                                    Mutex::new({
                                        let mut inventory = Inventory::new(
                                            27,
                                            || {
                                                let mut slots = Vec::with_capacity(27);
                                                for y in 0..3 {
                                                    for x in 0..9 {
                                                        slots.push((
                                                            (x as f32 * 0.13) - (4.5 * 0.13),
                                                            y as f32 * 0.15,
                                                        ));
                                                    }
                                                }
                                                slots
                                            },
                                            Some(Arc::new(move|inventory: &mut Inventory, entity: &Entity, slot:u32, _:MouseButton, _:bool| {
                                                let mut entity_data = entity.entity_data.lock().unwrap();
                                                let hand_empty = entity_data.hand_item.is_none();
                                                if hand_empty{
                                                    entity_data.set_inventory_hand(inventory.get_item(slot).unwrap().clone());
                                                } else {
                                                    entity_data.set_inventory_hand(None);
                                                }
                                                InteractionResult::Consumed
                                            })),
                                            Some(Arc::new(|inventory: &mut Inventory, entity: &Entity, slot: u32, _:i32, y:i32, _: bool|{
                                                let mut entity_data = entity.entity_data.lock().unwrap();
                                                entity_data.modify_inventory_hand(|item|{
                                                    match &mut *item{
                                                        Some(item) => {
                                                            item.add_count(if y < 0 {-1} else {1});
                                                        }
                                                        None => {
                                                            if y > 0{
                                                                if let Some(slot_item) = inventory.get_item(slot).unwrap(){
                                                                    *item = Some(slot_item.copy(1))
                                                                }
                                                            }
                                                        }
                                                    }
                                                });
                                                InteractionResult::Consumed
                                            })),
                                        );
                                        let item_registry = &self.server.item_registry;
                                        for (i, id) in item_registry.list().into_iter().enumerate()
                                        {
                                            let item_type = item_registry
                                                .item_by_identifier(id)
                                                .unwrap()
                                                .clone();
                                            let item_count = item_type.stack_size;
                                            inventory
                                                .set_item(
                                                    i as u32,
                                                    Some(ItemStack::new(item_type, item_count)),
                                                )
                                                .ok();
                                        }
                                        inventory
                                    }),
                                ))));
                            } else {
                                self.set_open_inventory(Some(InventoryWrapper::Own(Arc::new(
                                    Mutex::new(Inventory::new(
                                        9,
                                        || {
                                            let mut slots = Vec::with_capacity(9);
                                            for i in 0..9 {
                                                slots.push(((i as f32 * 0.13) - (4.5 * 0.13), 0.));
                                            }
                                            slots
                                        },
                                        None,
                                        None,
                                    )),
                                ))));
                            }
                        }
                        103 => {
                            let mut inventory = self.inventory.lock().unwrap();
                            let recipe = self
                                .server
                                .recipes
                                .get(&Identifier::new("example", "planks"))
                                .unwrap();
                            if let Ok(_) = recipe.consume_inputs(&mut inventory) {
                                recipe.add_outputs(&mut inventory);
                            }
                        }
                        49..=57 => {
                            self.entity_data
                                .lock()
                                .unwrap()
                                .set_hand_slot((key - 49) as u32);
                        }
                        _ => {}
                    },
                    net::NetworkMessageC2S::GuiClose => {
                        self.set_open_inventory(None);
                    }
                    net::NetworkMessageC2S::GuiClick(element, button, shifting) => {
                        {
                            let mut player_inventory = self.inventory.lock().unwrap();
                            let slot = player_inventory.resolve_slot(element.as_str());
                            if let Some(slot) = slot {
                                player_inventory.on_click_slot(self, slot, button, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *self.open_inventory.lock().unwrap()
                            {
                                let mut open_inventory = open_inventory.get_inventory().unwrap();
                                let slot = open_inventory.resolve_slot(element.as_str());
                                if let Some(slot) = slot {
                                    open_inventory.on_click_slot(self, slot, button, shifting);
                                    continue;
                                }
                            }
                        }
                    }
                    net::NetworkMessageC2S::GuiScroll(element, x, y, shifting) => {
                        {
                            let mut player_inventory = self.inventory.lock().unwrap();
                            let slot = player_inventory.resolve_slot(element.as_str());
                            if let Some(slot) = slot {
                                player_inventory.on_scroll_slot(self, slot, x, y, shifting);
                                continue;
                            }
                        }
                        {
                            if let Some(open_inventory) = &mut *self.open_inventory.lock().unwrap()
                            {
                                let mut open_inventory = open_inventory.get_inventory().unwrap();
                                let slot = open_inventory.resolve_slot(element.as_str());
                                if let Some(slot) = slot {
                                    open_inventory.on_scroll_slot(self, slot, x, y, shifting);
                                    continue;
                                }
                            }
                        }
                    }
                    crate::net::NetworkMessageC2S::PlayerPosition(
                        x,
                        y,
                        z,
                        shift,
                        rotation,
                        moved,
                    ) => {
                        let world = { self.location.lock().unwrap().chunk.world.clone() };
                        self.move_to(
                            &Location {
                                position: Position {
                                    x: x as f64,
                                    y: y as f64,
                                    z: z as f64,
                                },
                                world,
                            },
                            Some(rotation),
                        );
                        self.animation_controller
                            .lock()
                            .unwrap()
                            .set_animation(Some(if moved { 2 } else { 1 }));
                    }
                    crate::net::NetworkMessageC2S::RequestBlockBreakTime(id, _) => {
                        self.try_send_message(&NetworkMessageS2C::BlockBreakTimeResponse(id, 1.))
                            .unwrap();
                        //todo: check time
                    }
                    crate::net::NetworkMessageC2S::BreakBlock(x, y, z) => {
                        let block_position = BlockPosition { x, y, z };
                        let world = &self.get_location().chunk.world;
                        world.set_block(block_position, BlockStateRef::from_state_id(0));
                    }
                    crate::net::NetworkMessageC2S::RightClickBlock(x, y, z, face, shifting) => {
                        let block_position = BlockPosition { x, y, z };
                        let hand_slot = self.entity_data.lock().unwrap().get_hand_slot();
                        let block = self.get_location().chunk.world.get_block(block_position);
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
                            .lock()
                            .unwrap()
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
                    crate::net::NetworkMessageC2S::RightClick(shifting) => {
                        let hand_slot = self.entity_data.lock().unwrap().get_hand_slot();
                        let mut right_click_result = InteractionResult::Ignored;
                        self.inventory
                            .lock()
                            .unwrap()
                            .modify_item(hand_slot, |stack| {
                                if let Some(stack) = stack {
                                    right_click_result = stack
                                        .item_type
                                        .clone()
                                        .on_right_click(stack, self.this.upgrade().unwrap());
                                }
                            })
                            .unwrap();
                    }
                    crate::net::NetworkMessageC2S::MouseScroll(scroll_x, scroll_y) => {
                        let mut player_data = self.entity_data.lock().unwrap();
                        let new_slot = player_data.get_hand_slot() as i32 - scroll_y;
                        player_data.set_hand_slot(new_slot as u32);
                    }
                    _ => {}
                }
            }
        }
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
                .unwrap()
                .as_ref()
                .map(|connection| connection.is_closed())
                .unwrap_or(false)
    }
    pub fn post_remove(&self) {
        if self.is_player() {
            let (world, position) = {
                let location = self.location.lock().unwrap();
                (location.chunk.world.clone(), location.position)
            };
            let loading_chunks = Entity::get_chunks_to_load_at(&self.server, &position.clone());
            for chunk_position in loading_chunks {
                world
                    .load_chunk(chunk_position)
                    .remove_viewer(self.this.upgrade().unwrap());
            }
        }
    }
    fn is_player(&self) -> bool {
        self.connection.lock().unwrap().is_some()
    }
}
impl Hash for Entity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}
pub struct AnimationController {
    entity: Weak<Entity>,
    animation: u32,
    animation_start_time: f32,
    default_animation: u32,
}
impl AnimationController {
    pub fn new(entity: Weak<Entity>, default_animation: u32) -> Self {
        AnimationController {
            entity,
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
            let entity = self.entity.upgrade().unwrap();
            entity.location.lock().unwrap().chunk.announce_to_viewers(
                NetworkMessageS2C::EntityAnimation(entity.client_id, self.animation),
            );
        }
    }
}

pub struct Structure {
    id: Identifier,
    blocks: HashMap<BlockPosition, BlockStateRef>,
}
impl Structure {
    pub fn from_json(id: Identifier, json: JsonValue, block_registry: &BlockRegistry) -> Self {
        let mut blocks = HashMap::new();
        for block in json["blocks"].members() {
            blocks.insert(
                BlockPosition {
                    x: block["x"].as_i32().unwrap(),
                    y: block["y"].as_i32().unwrap(),
                    z: block["z"].as_i32().unwrap(),
                },
                block_registry
                    .block_by_identifier(&Identifier::parse(block["id"].as_str().unwrap()).unwrap())
                    .unwrap()
                    .get_default_state_ref(),
            );
        }
        Structure { blocks, id }
    }
    pub fn place<F>(&self, mut placer: F, position: BlockPosition)
    where
        F: FnMut(BlockPosition, BlockStateRef),
    {
        for (block_position, block) in &self.blocks {
            placer.call_mut((block_position.clone() + position, block.clone()));
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
    chunk: Weak<Chunk>,
    position: BlockPosition,
    state: BlockStateRef,
    pub inventory: Mutex<Inventory>,
}
impl WorldBlock {
    pub fn new(location: ChunkBlockLocation, state: BlockStateRef) -> Arc<WorldBlock> {
        Arc::new_cyclic(|this| WorldBlock {
            chunk: Arc::downgrade(&location.chunk),
            position: location.position,
            state,
            inventory: Mutex::new(Inventory::new(
                9,
                || {
                    let mut slots = Vec::with_capacity(9);
                    for i in 0..9 {
                        slots.push(((i as f32 * 0.13) - (4.5 * 0.13), 0.));
                    }
                    slots
                },
                None,
                None,
            )),
            this: this.clone(),
        })
    }
    pub fn on_right_click(&self, player: &Entity) -> InteractionResult {
        player.set_open_inventory(Some(InventoryWrapper::Block(self.this.upgrade().unwrap())));
        player.send_chat_message("clicked".to_string());
        InteractionResult::Consumed
    }
    pub fn serialize(&self) -> Vec<u8> {
        let mut block_data = Vec::new();
        let inventory = self.inventory.lock().unwrap();
        inventory.serialize(&mut block_data);
        block_data
    }
    pub fn deserialize(&self, mut data: &[u8]) {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.deserialize(
            &mut data,
            &self.chunk.upgrade().unwrap().world.server.item_registry,
        );
    }
}
