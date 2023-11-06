use crate::mods::ScriptingObject;
use crate::Server;
use block_byte_common::{BlockPosition, Position};
use rhai::{Dynamic, Engine};
use serde::{Deserialize, Serialize};
use std::sync::Weak;
use std::{fmt::Display, sync::Arc};

use crate::world::{Chunk, World};

#[derive(PartialEq, Eq, Hash, Clone, Debug, Serialize, Deserialize)]
pub struct Identifier {
    pub namespace: String,
    pub key: String,
}
impl Identifier {
    pub fn new<N: Into<String>, K: Into<String>>(namespace: N, key: K) -> Self {
        Identifier {
            namespace: namespace.into(),
            key: key.into(),
        }
    }
    pub fn parse(value: &str) -> Result<Self, ()> {
        let mut split = value.split(":");
        let namespace = split.next().ok_or(())?;
        let key = split.next().ok_or(())?;
        if split.next().is_some() {
            return Err(());
        }
        Ok(Identifier::new(namespace, key))
    }
    pub fn get_namespace(&self) -> &String {
        &self.namespace
    }
    pub fn get_key(&self) -> &String {
        &self.key
    }
}
impl Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.key)
    }
}
impl ScriptingObject for Identifier {
    fn engine_register(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_fn("Identifier", |namespace: &str, key: &str| {
            Identifier::new(namespace, key)
        });
        engine.register_fn("Identifier", |id: &str| match Identifier::parse(id) {
            Ok(id) => Dynamic::from(id),
            Err(_) => Dynamic::UNIT,
        });
        engine.register_fn("to_string", |identifier: &mut Identifier| {
            identifier.to_string()
        });
    }
}

#[derive(Clone)]
pub struct Location {
    pub position: Position,
    pub world: Arc<World>,
}
impl ScriptingObject for Location {
    fn engine_register(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_fn("Location", |position: Position, world: Arc<World>| {
            Location { position, world }
        });
        engine.register_get_set(
            "position",
            |location: &mut Location| location.position,
            |location: &mut Location, position: Position| {
                location.position = position;
            },
        );
        engine.register_get_set(
            "world",
            |location: &mut Location| location.world.clone(),
            |location: &mut Location, world: Arc<World>| {
                location.world = world;
            },
        );
    }
}
impl PartialEq for Location {
    fn eq(&self, other: &Self) -> bool {
        self.position == self.position && Arc::ptr_eq(&self.world, &other.world)
    }
}
impl From<&ChunkLocation> for Location {
    fn from(value: &ChunkLocation) -> Self {
        Location {
            position: value.position,
            world: value.chunk.world.clone(),
        }
    }
}
#[derive(Clone)]
pub struct ChunkLocation {
    pub position: Position,
    pub chunk: Arc<Chunk>,
}
impl ChunkLocation {
    pub fn new(position: Position, chunk: Arc<Chunk>) -> Result<ChunkLocation, ()> {
        if position.to_chunk_pos() == chunk.position {
            Ok(ChunkLocation { position, chunk })
        } else {
            Err(())
        }
    }
}
impl PartialEq for ChunkLocation {
    fn eq(&self, other: &Self) -> bool {
        self.position == self.position && Arc::ptr_eq(&self.chunk, &other.chunk)
    }
}
impl From<&Location> for ChunkLocation {
    fn from(value: &Location) -> Self {
        ChunkLocation {
            position: value.position,
            chunk: value.world.load_chunk(value.position.to_chunk_pos()),
        }
    }
}
#[derive(Clone)]
pub struct BlockLocation {
    pub position: BlockPosition,
    pub world: Arc<World>,
}
impl ScriptingObject for BlockLocation {
    fn engine_register(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_fn(
            "BlockLocation",
            |position: BlockPosition, world: Arc<World>| BlockLocation { position, world },
        );
        engine.register_get_set(
            "position",
            |location: &mut BlockLocation| location.position,
            |location: &mut BlockLocation, position: BlockPosition| {
                location.position = position;
            },
        );
        engine.register_get_set(
            "world",
            |location: &mut BlockLocation| location.world.clone(),
            |location: &mut BlockLocation, world: Arc<World>| {
                location.world = world;
            },
        );
    }
}
impl PartialEq for BlockLocation {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position && Arc::ptr_eq(&self.world, &other.world)
    }
}
impl From<&ChunkBlockLocation> for BlockLocation {
    fn from(value: &ChunkBlockLocation) -> Self {
        BlockLocation {
            position: value.position,
            world: value.chunk.world.clone(),
        }
    }
}
impl From<&ChunkLocation> for BlockLocation {
    fn from(value: &ChunkLocation) -> Self {
        BlockLocation {
            position: value.position.to_block_pos(),
            world: value.chunk.world.clone(),
        }
    }
}
#[derive(Clone)]
pub struct ChunkBlockLocation {
    pub position: BlockPosition,
    pub chunk: Arc<Chunk>,
}
impl ChunkBlockLocation {
    pub fn new(position: BlockPosition, chunk: Arc<Chunk>) -> Result<ChunkBlockLocation, ()> {
        if position.to_chunk_pos() == chunk.position {
            Ok(ChunkBlockLocation { position, chunk })
        } else {
            Err(())
        }
    }
}
impl PartialEq for ChunkBlockLocation {
    fn eq(&self, other: &Self) -> bool {
        self.position == self.position && Arc::ptr_eq(&self.chunk, &other.chunk)
    }
}
impl From<&BlockLocation> for ChunkBlockLocation {
    fn from(value: &BlockLocation) -> Self {
        ChunkBlockLocation {
            position: value.position,
            chunk: value.world.load_chunk(value.position.to_chunk_pos()),
        }
    }
}
