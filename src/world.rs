use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicU32},
        Arc, Mutex, Weak,
    },
    thread,
    time::Instant,
};

use array_init::array_init;
use atomic_counter::{AtomicCounter, RelaxedCounter};
use endio::LEWrite;
use fxhash::{FxHashMap, FxHashSet};
use json::{array, object, JsonValue};
use libflate::zlib::Encoder;
use rand::Rng;
use rhai::{Engine, AST};
use uuid::Uuid;

use crate::{
    inventory::{Inventory, InventoryViewer, ItemStack},
    net::{NetworkMessageS2C, PlayerConnection},
    registry::{BlockRegistry, BlockStateRef, EntityData, InteractionResult},
    util::{BlockPosition, ChunkLocation, ChunkPosition, Identifier, Location, Position},
    worldgen::{BasicWorldGenerator, FlatWorldGenerator, WorldGenerator},
    Server,
};

pub struct World {
    server: Arc<Server>,
    this: Weak<Self>,
    chunks: Mutex<FxHashMap<ChunkPosition, Arc<Chunk>>>,
    unload_timer: RelaxedCounter,
    world_generator: Box<dyn WorldGenerator + Send + Sync>,
    unloaded_structure_placements:
        Mutex<HashMap<ChunkPosition, Vec<(BlockPosition, Arc<Structure>)>>>,
}
impl World {
    const UNLOAD_TIME: usize = 1000;
    pub fn new(
        server: Arc<Server>,
        world_generator: Box<dyn WorldGenerator + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| World {
            this: this.clone(),
            chunks: Mutex::new(FxHashMap::default()),
            server,
            unload_timer: RelaxedCounter::new(0),
            world_generator,
            unloaded_structure_placements: Mutex::new(HashMap::new()),
        })
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
            match chunk {
                Some(chunk) => {
                    chunk.place_structure(position, structure.clone());
                }
                None => {
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
    pub fn set_block(&self, position: BlockPosition, block: BlockData) {
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
        F: FnOnce(BlockData) -> Option<BlockData>,
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
        let chunk = Chunk::new(position, self.this.upgrade().unwrap());
        let mut chunks = self.chunks.lock().unwrap();
        chunks.insert(position, chunk.clone());
        if let Some(placement_list) = {
            self.unloaded_structure_placements
                .lock()
                .unwrap()
                .remove(&position)
        } {
            for (position, structure) in placement_list {
                chunk.place_structure(position, structure);
            }
        }
        chunk
            .generating
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
            chunks.drain_filter(|_, chunk| {
                let should_unload = chunk.should_unload();
                if should_unload {
                    chunk.destroy();
                }
                should_unload
            });
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
    Data,
}
impl BlockData {
    pub fn get_client_id(&self) -> u32 {
        match self {
            Self::Simple(id) => *id,
            Self::Data => todo!(),
        }
    }
}

pub struct Chunk {
    pub position: ChunkPosition,
    pub world: Arc<World>,
    blocks: Mutex<[[[BlockData; 16]; 16]; 16]>,
    entities: Mutex<Vec<Arc<Entity>>>,
    viewers: Mutex<FxHashSet<ChunkViewer>>,
    unload_timer: RelaxedCounter,
    generating: AtomicBool,
}
impl Chunk {
    const UNLOAD_TIME: usize = 200;
    pub fn new(position: ChunkPosition, world: Arc<World>) -> Arc<Self> {
        let chunk = Arc::new(Chunk {
            position,
            blocks: Mutex::new(world.world_generator.generate(position, &world)),
            world,
            unload_timer: RelaxedCounter::new(0),
            entities: Mutex::new(Vec::new()),
            viewers: Mutex::new(FxHashSet::default()),
            generating: AtomicBool::new(true),
        });
        chunk
    }
    pub fn place_structure(&self, position: BlockPosition, structure: Arc<Structure>) {
        structure.place(
            |block_position, block| {
                if block_position.to_chunk_pos() == self.position {
                    let offset = block_position.chunk_offset();
                    self.set_block(offset.0, offset.1, offset.2, block.to_block_data());
                }
            },
            position,
        );
    }
    pub fn set_block(&self, offset_x: u8, offset_y: u8, offset_z: u8, block: BlockData) {
        if !self.generating.load(std::sync::atomic::Ordering::SeqCst) {
            self.announce_to_viewers(NetworkMessageS2C::SetBlock(
                self.position.x * 16 + offset_x as i32,
                self.position.y * 16 + offset_y as i32,
                self.position.z * 16 + offset_z as i32,
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
        let mut blocks: Vec<u32> = Vec::with_capacity(16 * 16 * 16);
        {
            let real_blocks = self.blocks.lock().unwrap();
            for x in 0..16 {
                for y in 0..16 {
                    for z in 0..16 {
                        blocks.push(real_blocks[x][y][z].get_client_id());
                    }
                }
            }
        }
        let thread_viewer = viewer.clone();
        let position = self.position.clone();
        self.world
            .server
            .thread_pool_tasks
            .send(Box::new(move |_| {
                let mut encoder = Encoder::new(Vec::new()).unwrap();
                for id in blocks {
                    encoder.write_be(id).unwrap();
                }
                thread_viewer
                    .try_send_message(&NetworkMessageS2C::LoadChunk(
                        position.x,
                        position.y,
                        position.z,
                        encoder.finish().into_result().unwrap(),
                    ))
                    .ok();
            }))
            .unwrap();

        for entity in self.entities.lock().unwrap().iter() {
            if Arc::ptr_eq(entity, &viewer) {
                continue;
            }
            viewer.try_send_message(&entity.create_add_message(entity.get_location().position));
        }
        self.viewers
            .lock()
            .unwrap()
            .insert(ChunkViewer::new(viewer));
    }
    fn remove_viewer(&self, viewer: Arc<Entity>) {
        viewer.try_send_message(&NetworkMessageS2C::UnloadChunk(
            self.position.x,
            self.position.y,
            self.position.z,
        ));
        for entity in self.entities.lock().unwrap().iter() {
            if Arc::ptr_eq(entity, &viewer) {
                continue;
            }
            viewer.try_send_message(&NetworkMessageS2C::DeleteEntity(entity.client_id));
        }
        self.viewers
            .lock()
            .unwrap()
            .remove(&ChunkViewer::new(viewer));
    }
    pub fn announce_to_viewers(&self, message: NetworkMessageS2C) {
        for viewer in self.viewers.lock().unwrap().iter() {
            viewer.player.try_send_message(&message);
        }
    }
    pub fn tick(&self) {
        self.unload_timer.inc();

        let entities: Vec<_> = self
            .entities
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.clone())
            .collect();
        if self.viewers.lock().unwrap().len() > 0 {
            self.unload_timer.reset();
        }
        self.world
            .server
            .thread_pool_tasks
            .send(Box::new(move |engine| {
                for entity in entities {
                    entity.tick(engine);
                }
            }))
            .unwrap();

        let mut removed_entities = Vec::new();
        self.entities.lock().unwrap().drain_filter(|entity| {
            let not_same_chunk = entity.get_location().chunk.position != self.position;
            let removed = entity.is_removed();
            if removed && !not_same_chunk {
                removed_entities.push(entity.clone());
            }
            removed || not_same_chunk
        });
        for entity in removed_entities {
            entity.post_remove();
        }
    }
    pub fn should_unload(&self) -> bool {
        self.unload_timer.get() >= Chunk::UNLOAD_TIME
    }
    pub fn destroy(&self) {
        self.entities.lock().unwrap().clear();
        self.viewers.lock().unwrap().clear();
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
    fn hash<H: ~const std::hash::Hasher>(&self, state: &mut H) {
        self.player.id.hash(state)
    }
}
impl PartialEq for ChunkViewer {
    fn eq(&self, other: &Self) -> bool {
        self.player.id == other.player.id
    }
}
impl Eq for ChunkViewer {}

pub struct PlayerData {
    player: Weak<Entity>,
    connection: PlayerConnection,
    slot: u32,
}
impl PlayerData {
    pub fn new(player: Weak<Entity>, connection: PlayerConnection) -> Self {
        PlayerData {
            player,
            connection,
            slot: u32::MAX,
        }
    }
    pub fn set_hand_slot(&mut self, slot: u32) {
        let player = self.player.upgrade().unwrap();
        let inventory = player.inventory.lock().unwrap();
        let slot = if slot == u32::MAX {
            inventory.get_size() - 1
        } else {
            slot % inventory.get_size()
        };
        let inventory_viewer = inventory.get_viewer(&player).unwrap();
        self.connection.send(&NetworkMessageS2C::GuiData(
                object! {id: inventory_viewer.get_slot_id(self.slot), type: "editElement", data_type: "color", color: array![1, 1, 1, 1]},
            ));
        self.slot = slot;
        self.connection.send(&NetworkMessageS2C::GuiData(
                object! {id: inventory_viewer.get_slot_id(self.slot), type: "editElement", data_type: "color", color: array![1, 0, 0, 1]},
            ));
    }
}

pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    rotation: Mutex<f32>,
    teleport: Mutex<Option<ChunkLocation>>,
    entity_type: Arc<EntityData>,
    player_data: Mutex<Option<PlayerData>>,
    removed: AtomicBool,
    client_id: u32,
    id: Uuid,
    animation_controller: Mutex<AnimationController>,
    inventory: Mutex<Inventory>,
}
static ENTITY_CLIENT_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);
impl Entity {
    pub fn new<T: Into<ChunkLocation>>(
        location: T,
        entity_type: Arc<EntityData>,
        player_data: Option<PlayerConnection>,
    ) -> Arc<Entity> {
        let location: ChunkLocation = location.into();
        let position = location.position;
        let chunk = location.chunk.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            location: Mutex::new(location),
            entity_type,
            removed: AtomicBool::new(false),
            this: weak.clone(),
            client_id: ENTITY_CLIENT_ID_GENERATOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            id: Uuid::new_v4(),
            teleport: Mutex::new(None),
            player_data: Mutex::new(
                player_data.map(|connection| PlayerData::new(weak.clone(), connection)),
            ),
            rotation: Mutex::new(0.),
            animation_controller: Mutex::new(AnimationController::new(weak.clone(), 1)),
            inventory: Mutex::new(Inventory::new(9)),
        });
        {
            let item_registry = &chunk.world.server.item_registry;
            let mut inventory = entity.inventory.lock().unwrap();
            for (i, id) in item_registry.list().into_iter().enumerate() {
                inventory
                    .set_item(
                        i as u32,
                        Some(ItemStack::new(
                            item_registry.item_by_identifier(id).unwrap().clone(),
                            5,
                        )),
                    )
                    .unwrap();
            }
        }
        entity
            .inventory
            .lock()
            .unwrap()
            .add_viewer(InventoryViewer::new(Arc::downgrade(&entity), {
                let mut slots = Vec::with_capacity(9);
                for i in 0..9 {
                    slots.push(((i as f32 * 0.13) - (4.5 * 0.13), -0.5));
                }
                slots
            })); //todo: only add if is player

        if let Some(player_data) = entity.player_data.lock().unwrap().as_mut() {
            player_data.set_hand_slot(0);
        }
        chunk.add_entity(entity.clone());
        let add_message = entity.create_add_message(position);
        for viewer in chunk.viewers.lock().unwrap().iter() {
            viewer.player.try_send_message(&add_message);
        }
        for chunk_position in Entity::get_chunks_to_load_at(&position) {
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
    pub fn get_id(&self) -> &Uuid {
        &self.id
    }
    pub fn try_send_message(&self, message: &NetworkMessageS2C) -> Result<(), ()> {
        if let Some(player) = self.player_data.lock().unwrap().deref_mut() {
            player.connection.send(message);
            Ok(())
        } else {
            Err(())
        }
    }
    pub fn create_add_message(&self, position: Position) -> NetworkMessageS2C {
        let animation_controller = self.animation_controller.lock().unwrap();
        NetworkMessageS2C::AddEntity(
            self.entity_type.id,
            self.client_id,
            position.x,
            position.y,
            position.z,
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
            position.x, position.y, position.z,
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
    pub fn get_chunks_to_load_at(position: &Position) -> FxHashSet<ChunkPosition> {
        let chunk_pos = position.to_chunk_pos();
        let vertical_view_distance = 15;
        let horizontal_view_distance = 8;
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
        let location: &ChunkLocation = location.deref().into();
        location.clone()
    }
    pub fn tick(&self, engine: &Engine) {
        if let Some(teleport_location) = self.teleport.lock().unwrap().deref() {
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
                        viewer.player.try_send_message(&delete_message);
                    }
                    for viewer in new_viewers.difference(&old_viewers) {
                        viewer.player.try_send_message(&add_message);
                    }
                }
                let is_player = self.is_player();
                if is_player {
                    let old_loaded = Entity::get_chunks_to_load_at(&old_location.position);
                    let new_loaded = Entity::get_chunks_to_load_at(&new_location.position);

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
                    new_location.position.x,
                    new_location.position.y,
                    new_location.position.z,
                    *self.rotation.lock().unwrap(),
                ));
        }
        {
            *self.teleport.lock().unwrap() = None;
        }
        if let Some(ticker) = self.entity_type.ticker.lock().unwrap().deref() {
            ticker.call::<()>(engine, &AST::empty(), ()).unwrap();
        }
        if self.is_player() {
            let messages = self
                .player_data
                .lock()
                .unwrap()
                .deref_mut()
                .as_mut()
                .unwrap()
                .connection
                .receive_messages();
            for message in messages {
                match message {
                    crate::net::NetworkMessageC2S::PlayerPosition(
                        x,
                        y,
                        z,
                        shift,
                        rotation,
                        moved,
                    ) => {
                        self.move_to(
                            &Location {
                                position: Position { x, y, z },
                                world: { self.location.lock().unwrap().chunk.world.clone() },
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
                        world.set_block(block_position, BlockData::Simple(0));
                    }
                    crate::net::NetworkMessageC2S::RightClickBlock(x, y, z, face, _) => {
                        let hand_slot = self.player_data.lock().unwrap().as_ref().unwrap().slot;
                        let mut right_click_result = InteractionResult::Ignored;
                        self.inventory
                            .lock()
                            .unwrap()
                            .modify_item(hand_slot, |stack| {
                                if let Some(stack) = stack {
                                    right_click_result =
                                        stack.item_type.clone().on_right_click_block(
                                            stack,
                                            self.this.upgrade().unwrap(),
                                            BlockPosition { x, y, z },
                                            face,
                                        );
                                }
                            })
                            .unwrap();
                    }
                    crate::net::NetworkMessageC2S::MouseScroll(scroll_x, scroll_y) => {
                        let mut player_data = self.player_data.lock().unwrap();
                        let player_data = player_data.as_mut().unwrap();
                        let new_slot = player_data.slot as i32 - scroll_y;
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
                .player_data
                .lock()
                .unwrap()
                .as_ref()
                .map(|connection| connection.connection.is_closed())
                .unwrap_or(false)
    }
    pub fn post_remove(&self) {
        if self.is_player() {
            let (world, position) = {
                let location = self.location.lock().unwrap();
                (location.chunk.world.clone(), location.position)
            };
            let loading_chunks = Entity::get_chunks_to_load_at(&position.clone());
            for chunk_position in loading_chunks {
                world
                    .load_chunk(chunk_position)
                    .remove_viewer(self.this.upgrade().unwrap());
            }
        }
    }
    fn is_player(&self) -> bool {
        self.player_data.lock().unwrap().is_some()
    }
}
impl Hash for Entity {
    fn hash<H: ~const std::hash::Hasher>(&self, state: &mut H) {
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
