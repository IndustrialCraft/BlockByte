use array_init::array_init;

use crate::{util::ChunkPosition, world::BlockData};

pub trait WorldGenerator {
    fn generate(&self, position: ChunkPosition) -> [[[BlockData; 16]; 16]; 16];
}
pub struct FlatWorldGenerator {
    pub height: i32,
    pub simple_id: u32,
}
impl WorldGenerator for FlatWorldGenerator {
    fn generate(&self, position: ChunkPosition) -> [[[BlockData; 16]; 16]; 16] {
        array_init(|_| {
            array_init(|i| {
                array_init(|_| {
                    BlockData::Simple(if i as i32 + position.y * 16 < self.height {
                        self.simple_id
                    } else {
                        0
                    })
                })
            })
        })
    }
}
