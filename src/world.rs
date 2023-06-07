use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc, Mutex, Weak},
};

use array_init::array_init;
use atomic_counter::{AtomicCounter, RelaxedCounter};

use crate::{
    net::PlayerConnection,
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
            chunks.drain_filter(|_, chunk| chunk.should_unload());
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

pub struct Chunk {
    pub position: ChunkPosition,
    pub world: Arc<World>,
    blocks: Mutex<[[[BlockData; 16]; 16]; 16]>,
    entities: Mutex<Vec<Arc<Entity>>>,
    unload_timer: RelaxedCounter,
}
impl Chunk {
    const UNLOAD_TIME: usize = 40;
    pub fn new(position: ChunkPosition, world: Arc<World>) -> Arc<Self> {
        Arc::new(Chunk {
            position,
            world,
            blocks: Mutex::new(array_init(|_| {
                array_init(|_| array_init(|_| BlockData::Simple(0)))
            })),
            unload_timer: RelaxedCounter::new(0),
            entities: Mutex::new(Vec::new()),
        })
    }
    fn add_entity(&self, entity: Arc<Entity>) {
        self.entities.lock().unwrap().push(entity);
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
        for entity in entities {
            entity.tick();
        }
        self.entities.lock().unwrap().drain_filter(|entity| {
            entity.is_removed() || entity.get_location().chunk.position != self.position
        });
    }
    pub fn should_unload(&self) -> bool {
        self.unload_timer.get() >= Chunk::UNLOAD_TIME
    }
    pub fn destroy(&self) {
        self.entities.lock().unwrap().clear();
    }
}
pub enum EntityData {
    Player(Mutex<PlayerConnection>),
}
pub struct Entity {
    this: Weak<Self>,
    location: Mutex<ChunkLocation>,
    data: Mutex<EntityData>,
    removed: AtomicBool,
}
impl Entity {
    pub fn new<T: Into<ChunkLocation>>(location: T, data: EntityData) -> Arc<Entity> {
        let location = location.into();
        let chunk = location.chunk.clone();
        let entity = Arc::new_cyclic(|weak| Entity {
            location: Mutex::new(location),
            data: Mutex::new(data),
            removed: AtomicBool::new(false),
            this: weak.clone(),
        });
        chunk.add_entity(entity.clone());
        entity
    }
    pub fn teleport<T: Into<ChunkLocation>>(&self, location: T) {
        let new_location: ChunkLocation = location.into();
        if !Arc::ptr_eq(&self.location.lock().unwrap().chunk, &new_location.chunk) {
            new_location.chunk.add_entity(self.this.upgrade().unwrap())
        }
        *self.location.lock().unwrap() = new_location;

        //todo: announce move
    }
    pub fn get_location(&self) -> ChunkLocation {
        use std::ops::Deref;
        let location = self.location.lock().unwrap();
        let location: &ChunkLocation = location.deref().into();
        location.clone()
    }
    pub fn tick(&self) {}
    pub fn remove(&self) {
        self.removed
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
    pub fn is_removed(&self) -> bool {
        self.removed.load(std::sync::atomic::Ordering::Relaxed) | {
            let data = self.data.lock().unwrap();
            use std::ops::Deref;
            match data.deref() {
                EntityData::Player(connection) => connection.lock().unwrap().is_closed(),
                _ => false,
            }
        }
    }
}
