use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::Hash,
    io::Write,
    ops::DerefMut,
    sync::{
        atomic::{AtomicBool, AtomicU32},
        Arc, Mutex, Weak,
    },
};

use array_init::array_init;
use atomic_counter::{AtomicCounter, ConsistentCounter, RelaxedCounter};
use endio::LEWrite;
use libflate::zlib::Encoder;
use uuid::Uuid;

use crate::{
    net::{NetworkMessageS2C, PlayerConnection},
    util::{ChunkLocation, ChunkPosition, Location, Position},
    Server,
};

pub struct World {
    server: Arc<Server>,
    this: Weak<Self>,
    chunks: Mutex<HashMap<ChunkPosition, Arc<Chunk>>>,
    unload_timer: RelaxedCounter,
}
impl World {
    const UNLOAD_TIME: usize = 100;
    pub fn new(server: Arc<Server>) -> Arc<Self> {
        Arc::new_cyclic(|this| World {
            this: this.clone(),
            chunks: Mutex::new(HashMap::new()),
            server,
            unload_timer: RelaxedCounter::new(0),
        })
    }
    pub fn load_chunk(&self, position: ChunkPosition) -> Arc<Chunk> {
        let mut chunks = self.chunks.lock().unwrap();
        if let Some(chunk) = chunks.get(&position) {
            return chunk.clone();
        }
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
    viewers: Mutex<HashSet<ChunkViewer>>,
    unload_timer: RelaxedCounter,
}
impl Chunk {
    const UNLOAD_TIME: usize = 40;
    pub fn new(position: ChunkPosition, world: Arc<World>) -> Arc<Self> {
        let chunk = Arc::new(Chunk {
            position,
            world,
            blocks: Mutex::new(array_init(|_| {
                array_init(|_| array_init(|_| BlockData::Simple(0)))
            })),
            unload_timer: RelaxedCounter::new(0),
            entities: Mutex::new(Vec::new()),
            viewers: Mutex::new(HashSet::new()),
        });
        chunk.set_block(0, 0, 0, BlockData::Simple(5));
        chunk
    }
    pub fn set_block(&self, offset_x: u8, offset_y: u8, offset_z: u8, block: BlockData) {
        self.announce_to_viewers(NetworkMessageS2C::SetBlock(
            self.position.x * 16 + offset_x as i32,
            self.position.y * 16 + offset_y as i32,
            self.position.z * 16 + offset_z as i32,
            block.get_client_id(),
        ));
        self.blocks.lock().unwrap()[offset_x as usize][offset_y as usize][offset_z as usize] =
            block;
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
        let mut encoder = Encoder::new(Vec::new()).unwrap();
        for id in blocks {
            encoder.write_be(id).unwrap();
        }
        match viewer.data.lock().unwrap().deref_mut() {
            EntityData::Player(connection) => {
                connection.send(&NetworkMessageS2C::LoadChunk(
                    self.position.x,
                    self.position.y,
                    self.position.z,
                    encoder.finish().into_result().unwrap(),
                ));
            }
            _ => panic!("tried to add non player entity as chunk viewer"),
        }
        self.viewers
            .lock()
            .unwrap()
            .insert(ChunkViewer::new(viewer).unwrap());
    }
    fn remove_viewer(&self, viewer: Arc<Entity>) {
        use std::ops::DerefMut;
        match viewer.data.lock().unwrap().deref_mut() {
            EntityData::Player(connection) => {
                connection.send(&NetworkMessageS2C::UnloadChunk(
                    self.position.x,
                    self.position.y,
                    self.position.z,
                ));
            }
            _ => panic!("tried to remove non player entity from chunk viewers list"),
        }
        self.viewers
            .lock()
            .unwrap()
            .remove(&ChunkViewer::new(viewer).unwrap());
    }
    pub fn announce_to_viewers(&self, message: NetworkMessageS2C) {
        for viewer in self.viewers.lock().unwrap().iter() {
            viewer.send(&message);
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
        for entity in entities {
            entity.tick();
        }
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
    player: Arc<Entity>,
}
impl ChunkViewer {
    pub fn new(player: Arc<Entity>) -> Result<Self, ()> {
        use std::ops::Deref;
        let result = match player.data.lock().unwrap().deref() {
            EntityData::Player(_) => Ok(()),
            _ => Err(()),
        };
        result.map(|_| ChunkViewer { player })
    }
    pub fn send(&self, message: &NetworkMessageS2C) {
        use std::ops::DerefMut;
        match self.player.data.lock().unwrap().deref_mut() {
            EntityData::Player(connection) => {
                connection.send(&message);
            }
            _ => unreachable!(),
        }
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
pub enum EntityData {
    Player(PlayerConnection),
}
impl EntityData {
    pub fn get_type(&self) -> u32 {
        match self {
            Self::Player(_) => 0,
        }
    }
}
pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    teleport: Mutex<Option<ChunkLocation>>,
    data: Mutex<EntityData>,
    removed: AtomicBool,
    client_id: u32,
    id: Uuid,
}
const ENTITY_CLIENT_ID_GENERATOR: AtomicU32 = AtomicU32::new(0);
impl Entity {
    pub fn new<T: Into<ChunkLocation>>(location: T, data: EntityData) -> Arc<Entity> {
        let location: ChunkLocation = location.into();
        let position = location.position;
        let chunk = location.chunk.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            location: Mutex::new(location),
            data: Mutex::new(data),
            removed: AtomicBool::new(false),
            this: weak.clone(),
            client_id: ENTITY_CLIENT_ID_GENERATOR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            id: Uuid::new_v4(),
            teleport: Mutex::new(None),
        });
        chunk.add_entity(entity.clone());
        let add_message = entity.create_add_message(position);
        for viewer in chunk.viewers.lock().unwrap().iter() {
            viewer.send(&add_message);
        }
        for chunk_position in Entity::get_chunks_to_load_at(&position) {
            chunk
                .world
                .load_chunk(chunk_position)
                .add_viewer(entity.clone());
        }
        entity
    }
    pub fn create_add_message(&self, position: Position) -> NetworkMessageS2C {
        NetworkMessageS2C::AddEntity(
            self.data.lock().unwrap().get_type(),
            self.client_id,
            position.x,
            position.y,
            position.z,
            0., /*todo rotation*/
            0,  /*todo animation*/
            0.,
        )
    }
    pub fn teleport<T: Into<ChunkLocation>>(&self, location: T) {
        let location: ChunkLocation = location.into();
        let position = location.position.clone();
        self.move_to(location);
        use std::ops::DerefMut;
        match self.data.lock().unwrap().deref_mut() {
            EntityData::Player(connection) => connection.send(&NetworkMessageS2C::TeleportPlayer(
                position.x, position.y, position.z,
            )),
            _ => {}
        }
    }
    pub fn move_to<T: Into<ChunkLocation>>(&self, location: T) {
        *self.teleport.lock().unwrap() = Some(location.into());
    }
    pub fn get_chunks_to_load_at(position: &Position) -> HashSet<ChunkPosition> {
        let chunk_pos = position.to_chunk_pos();
        let vertical_view_distance = 20;
        let horizontal_view_distance = 16;
        let mut positions = HashSet::new();
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
        use std::ops::Deref;
        let location = self.location.lock().unwrap();
        let location: &ChunkLocation = location.deref().into();
        location.clone()
    }
    pub fn tick(&self) {
        use std::ops::Deref;
        use std::ops::DerefMut;

        if let Some(teleport_location) = self.teleport.lock().unwrap().deref() {
            let old_location = self.location.lock().unwrap().clone();
            let new_location: ChunkLocation = teleport_location.clone();

            *self.location.lock().unwrap() = new_location.clone();

            if !Arc::ptr_eq(&old_location.chunk, &new_location.chunk) {
                new_location.chunk.add_entity(self.this.upgrade().unwrap());

                {
                    let old_viewers = old_location.chunk.viewers.lock().unwrap();
                    let new_viewers = new_location.chunk.viewers.lock().unwrap();
                    let add_message = self.create_add_message(new_location.position);
                    let delete_message = NetworkMessageS2C::DeleteEntity(self.client_id);
                    for viewer in old_viewers.difference(&new_viewers) {
                        viewer.send(&delete_message);
                    }
                    for viewer in new_viewers.difference(&old_viewers) {
                        viewer.send(&add_message);
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
                    0., /*todo:rotation*/
                ));
        }
        *self.teleport.lock().unwrap() = None;

        match self.data.lock().unwrap().deref_mut() {
            EntityData::Player(connection) => {
                while let Some(message) = connection.try_recv() {
                    match message {
                        crate::net::NetworkMessageC2S::PlayerPosition(
                            x,
                            y,
                            z,
                            shift,
                            rotation,
                            moved,
                        ) => {
                            self.move_to(&Location {
                                position: Position { x, y, z },
                                world: self.location.lock().unwrap().chunk.world.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    pub fn remove(&self) {
        self.removed
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
    pub fn is_removed(&self) -> bool {
        self.removed.load(std::sync::atomic::Ordering::Relaxed) | {
            let data = self.data.lock().unwrap();
            use std::ops::Deref;
            match data.deref() {
                EntityData::Player(connection) => connection.is_closed(),
                _ => false,
            }
        }
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
        use std::ops::Deref;
        match self.data.lock().unwrap().deref() {
            EntityData::Player(_) => true,
            _ => false,
        }
    }
}
impl Hash for Entity {
    fn hash<H: ~const std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}
