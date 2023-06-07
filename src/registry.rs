use std::{collections::HashMap, sync::Arc};

use crate::util::Identifier;

pub struct BlockRegistry {
    blocks: HashMap<Arc<Identifier>, Arc<Block>>,
    states: Vec<BlockState>,
    id_generator: u32,
}
impl BlockRegistry {
    pub fn new() -> Self {
        let mut block_registry = BlockRegistry {
            blocks: HashMap::new(),
            states: Vec::new(),
            id_generator: 0,
        };
        block_registry
            .register(Identifier::new("bb".to_string(), "air".to_string()), |id| {
                let block = Arc::new(Block { block_id: *id });
                let state = vec![BlockState {
                    state_id: *id,
                    parent: block.clone(),
                }];
                *id += 1;
                (block, state)
            })
            .expect("couldn't register air");
        block_registry
    }
    pub fn register<F>(&mut self, id: Arc<Identifier>, creator: F) -> Result<u32, ()>
    where
        F: FnOnce(&mut u32) -> (Arc<Block>, Vec<BlockState>),
    {
        if self.blocks.get(&id).is_some() {
            return Err(());
        }
        let numeric_id = self.id_generator;
        let (block, mut block_states) = creator.call_once((&mut self.id_generator,));
        self.blocks.insert(id, block);
        self.states.append(&mut block_states);
        Ok(numeric_id)
    }
}

pub struct Block {
    block_id: u32,
}

pub struct BlockState {
    state_id: u32,
    parent: Arc<Block>,
}
impl BlockState {
    pub fn get_full_id(&self) -> u32 {
        self.parent.block_id + self.state_id
    }
}
