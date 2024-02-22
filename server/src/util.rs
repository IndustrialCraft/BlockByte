use crate::mods::ScriptingObject;
use crate::registry::BlockStateRef;
use crate::Server;
use anyhow::anyhow;
use bbscript::eval::ExecutionEnvironment;
use bbscript::variant::{FromVariant, IntoVariant, Variant};
use block_byte_common::{BlockPosition, Face, Position};
use immutable_string::ImmutableString;
use serde::de::Error;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Formatter;
use std::hash::{Hash, Hasher};
use std::sync::Weak;
use std::{fmt::Display, sync::Arc};

use crate::world::{BlockData, Chunk, World, WorldBlock};

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct Identifier {
    content: ImmutableString,
    split: usize,
}
impl Identifier {
    pub fn new<N: Into<ImmutableString>, K: Into<ImmutableString>>(namespace: N, key: K) -> Self {
        let namespace = namespace.into();
        Identifier {
            split: namespace.len(),
            content: ImmutableString::from(namespace.to_string() + ":" + key.into().as_ref()),
        }
    }
    pub fn parse<V: Into<ImmutableString>>(value: V) -> anyhow::Result<Self> {
        let value = value.into();
        value
            .find(":")
            .map(|id| Identifier {
                content: value,
                split: id,
            })
            .ok_or(anyhow!("missing ':' splitter"))
    }
    pub fn get_namespace(&self) -> &str {
        &self.content[0..self.split]
    }
    pub fn get_key(&self) -> &str {
        &self.content[self.split + 1..self.content.len()]
    }
}
impl Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.content)
    }
}
impl Serialize for Identifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.content.as_ref())
    }
}
impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(IdentifierVisitor)
    }
}
struct IdentifierVisitor;
impl<'de> serde::de::Visitor<'de> for IdentifierVisitor {
    type Value = Identifier;
    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("string")
    }
    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Identifier::parse(v.as_str()).map_err(|_| de::Error::custom("identifier parsing error"))
    }
}
#[derive(Clone)]
pub struct Location {
    pub position: Position,
    pub world: Arc<World>,
}
impl ScriptingObject for Location {
    fn engine_register(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<Location, _>("Location");
        env.register_function("Location", |position: &Position, world: &Arc<World>| {
            Ok(Location {
                position: position.clone(),
                world: world.clone(),
            })
        });
        env.register_member("position", |location: &Location| Some(location.position));
        env.register_member("world", |location: &Location| Some(location.world.clone()));
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
    fn engine_register(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        env.register_custom_name::<BlockLocation, _>("BlockLocation");
        env.register_function(
            "BlockLocation",
            |position: &BlockPosition, world: &Arc<World>| {
                Ok(BlockLocation {
                    position: position.clone(),
                    world: world.clone(),
                })
            },
        );
        env.register_member("position", |location: &BlockLocation| {
            Some(location.position)
        });
        env.register_member("world", |location: &BlockLocation| {
            Some(location.world.clone())
        });
        env.register_method(
            "set_ticking_enabled",
            |location: &BlockLocation, enabled: &bool| {
                if let Some(chunk) = location.world.get_chunk(location.position.to_chunk_pos()) {
                    chunk.set_ticking_enabled(location.position.chunk_offset(), *enabled);
                }
                Ok(())
            },
        );
        env.register_method(
            "set_block",
            |location: &BlockLocation, block: &BlockStateRef, data: &Variant| {
                location
                    .world
                    .set_block(location.position, *block, true, data.clone());
                Ok(())
            },
        );
        env.register_method("get_block_load", |location: &BlockLocation| {
            Ok(location
                .world
                .get_block_load(location.position)
                .get_block_state())
        });
        env.register_method("get_block", |location: &BlockLocation| {
            Ok(Variant::from_option(
                location
                    .world
                    .get_block(&location.position)
                    .map(|block| block.get_block_state()),
            ))
        });
        env.register_method("get_block_data_load", |location: &BlockLocation| {
            Ok(match location.world.get_block_load(location.position) {
                BlockData::Simple(_) => Variant::NULL(),
                BlockData::Data(data) => data.into_variant(),
            })
        });
        env.register_method("get_block_data", |location: &BlockLocation| {
            Ok(location
                .world
                .get_block(&location.position)
                .map(|block| match block {
                    BlockData::Simple(_) => Variant::NULL(),
                    BlockData::Data(data) => data.into_variant(),
                })
                .unwrap_or(Variant::NULL()))
        });
        env.register_method("offset_by_face", |location: &BlockLocation, face: &Face| {
            Ok(BlockLocation {
                position: location.position.offset_by_face(*face),
                world: location.world.clone(),
            })
        });
        {
            let server = server.clone();
            env.register_default_accessor::<BlockLocation, _>(move |this, key| {
                let location = BlockLocation::from_variant(this)?;
                server
                    .upgrade()
                    .unwrap()
                    .block_registry
                    .state_by_ref(
                        location
                            .world
                            .get_block(&location.position)?
                            .get_block_state(),
                    )
                    .parent
                    .static_data
                    .data
                    .get(key.as_ref())
                    .cloned()
            });
        }
        /*{
            let server = server.clone();
            engine.register_indexer_get(move |location: &mut BlockLocation, id: &str| {
                println!("id: {id}");
                location
                    .world
                    .get_block(&location.position)
                    .and_then(|block| {
                        server
                            .upgrade()
                            .unwrap()
                            .block_registry
                            .state_by_ref(block.get_block_state())
                            .parent
                            .static_data
                            .get(id)
                            .cloned()
                    })
                    .unwrap_or(Dynamic::UNIT)
            });
        }*/
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
