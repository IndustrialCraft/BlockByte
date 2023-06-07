use std::{cell::RefCell, sync::Arc};

use crate::world::{Chunk, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Front = 0,
    Back = 1,
    Up = 2,
    Down = 3,
    Left = 4,
    Right = 5,
}
impl Face {
    const FACES: [Face; 6] = [
        Face::Front,
        Face::Back,
        Face::Left,
        Face::Right,
        Face::Up,
        Face::Down,
    ];
    pub fn all() -> &'static [Face; 6] {
        &Face::FACES
    }
    #[inline(always)]
    pub fn get_offset(&self) -> BlockPosition {
        match self {
            Self::Front => BlockPosition { x: 0, y: 0, z: -1 },
            Self::Back => BlockPosition { x: 0, y: 0, z: 1 },
            Self::Left => BlockPosition { x: -1, y: 0, z: 0 },
            Self::Right => BlockPosition { x: 1, y: 0, z: 0 },
            Self::Up => BlockPosition { x: 0, y: 1, z: 0 },
            Self::Down => BlockPosition { x: 0, y: -1, z: 0 },
        }
    }
    #[inline(always)]
    pub fn opposite(&self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Front => Self::Back,
            Self::Back => Self::Front,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Position {
    pub fn add_other(&self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
    pub fn add(&self, x: f32, y: f32, z: f32) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl std::ops::Add for BlockPosition {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        BlockPosition {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
impl BlockPosition {
    #[inline(always)]
    pub fn offset_from_origin_chunk(&self) -> Option<Face> {
        if self.x < 0 {
            return Some(Face::Left);
        }
        if self.x >= 16 {
            return Some(Face::Right);
        }
        if self.y < 0 {
            return Some(Face::Down);
        }
        if self.y >= 16 {
            return Some(Face::Up);
        }
        if self.z < 0 {
            return Some(Face::Front);
        }
        if self.z >= 16 {
            return Some(Face::Back);
        }
        return None;
    }
    #[inline(always)]
    pub fn chunk_offset(&self) -> (u8, u8, u8) {
        (
            self.x.rem_euclid(16) as u8,
            self.y.rem_euclid(16) as u8,
            self.z.rem_euclid(16) as u8,
        )
    }
    #[inline(always)]
    pub fn to_chunk_pos(&self) -> ChunkPosition {
        ChunkPosition {
            x: ((self.x as f32) / 16f32).floor() as i32,
            y: ((self.y as f32) / 16f32).floor() as i32,
            z: ((self.z as f32) / 16f32).floor() as i32,
        }
    }
    #[inline(always)]
    pub fn to_position(&self) -> Position {
        Position {
            x: self.x as f32,
            y: self.y as f32,
            z: self.z as f32,
        }
    }
}
impl Position {
    #[inline(always)]
    pub fn to_chunk_pos(&self) -> ChunkPosition {
        ChunkPosition {
            x: ((self.x as f32) / 16f32).floor() as i32,
            y: ((self.y as f32) / 16f32).floor() as i32,
            z: ((self.z as f32) / 16f32).floor() as i32,
        }
    }
    #[inline(always)]
    pub fn to_block_pos(&self) -> BlockPosition {
        BlockPosition {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChunkPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl ChunkPosition {
    pub fn with_offset(&self, face: &Face) -> Self {
        let offset = face.get_offset();
        ChunkPosition {
            x: self.x + offset.x,
            y: self.y + offset.y,
            z: self.z + offset.z,
        }
    }
    pub fn add(&self, x: i32, y: i32, z: i32) -> Self {
        ChunkPosition {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
    pub fn distance_squared(&self, other: &ChunkPosition) -> u32 {
        let xd = self.x - other.x;
        let yd = self.y - other.y;
        let zd = self.z - other.z;
        (xd * xd + yd * yd + zd * zd) as u32
    }
}
#[derive(PartialEq, Eq, Hash)]
pub struct Identifier {
    namespace: String,
    key: String,
}
impl Identifier {
    pub fn new<N: Into<String>, K: Into<String>>(namespace: N, key: K) -> Arc<Self> {
        Arc::new(Identifier {
            namespace: namespace.into(),
            key: key.into(),
        })
    }
    pub fn parse(value: String) -> Result<Arc<Self>, ()> {
        let mut split = value.split(":");
        let namespace = split.next().ok_or(())?;
        let key = split.next().ok_or(())?;
        if split.next().is_some() {
            return Err(());
        }
        Ok(Identifier::new(namespace, key))
    }
}

pub struct Location {
    pub position: Position,
    pub world: Arc<World>,
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
        if position.to_chunk_pos() != chunk.position {
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
