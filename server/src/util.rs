use crate::mods::ScriptingObject;
use crate::registry::BlockStateRef;
use crate::Server;
use block_byte_common::{BlockPosition, Face, Position};
use rhai::{Dynamic, Engine};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::sync::Weak;
use std::{fmt::Display, sync::Arc};

use crate::world::{BlockData, Chunk, World, WorldBlock};

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
    fn engine_register(_engine: &mut Engine) {}
}

#[derive(Clone)]
pub struct Location {
    pub position: Position,
    pub world: Arc<World>,
}
impl ScriptingObject for Location {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<Location>("Location");
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
#[derive(Clone, Eq)]
pub struct BlockLocation {
    pub position: BlockPosition,
    pub world: Arc<World>,
}
impl BlockLocation {
    pub fn get_data(&self) -> Option<Arc<WorldBlock>> {
        match self.world.get_block(&self.position).unwrap() {
            BlockData::Data(data) => Some(data.clone()),
            BlockData::Simple(_) => None,
        }
    }
}
impl ScriptingObject for BlockLocation {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<BlockLocation>("BlockLocation");
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
        engine.register_fn(
            "set_block",
            |location: &mut BlockLocation, block: BlockStateRef| {
                location
                    .world
                    .set_block(location.position, block, true, None);
            },
        );
        engine.register_fn("get_block_load", |location: &mut BlockLocation| {
            location
                .world
                .get_block_load(location.position)
                .get_block_state()
        });
        engine.register_fn("get_block", |location: &mut BlockLocation| {
            location
                .world
                .get_block(&location.position)
                .map(|block| Dynamic::from(block.get_block_state()))
                .unwrap_or(Dynamic::UNIT)
        });
        engine.register_fn(
            "get_block_data_load",
            |location: &mut BlockLocation| match location.world.get_block_load(location.position) {
                BlockData::Simple(_) => Dynamic::UNIT,
                BlockData::Data(data) => Dynamic::from(data),
            },
        );
        engine.register_fn("get_block_data", |location: &mut BlockLocation| {
            location
                .world
                .get_block(&location.position)
                .map(|block| match block {
                    BlockData::Simple(_) => Dynamic::UNIT,
                    BlockData::Data(data) => Dynamic::from(data),
                })
                .unwrap_or(Dynamic::UNIT)
        });
        engine.register_fn(
            "offset_by_face",
            |location: &mut BlockLocation, face: Face| BlockLocation {
                position: location.position.offset_by_face(face),
                world: location.world.clone(),
            },
        );
    }
}
impl PartialEq for BlockLocation {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position && Arc::ptr_eq(&self.world, &other.world)
    }
}
impl Hash for BlockLocation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.position.hash(state);
        self.world.id.hash(state);
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
