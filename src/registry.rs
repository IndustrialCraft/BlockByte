use std::{collections::HashMap, sync::Arc};

use json::{array, object, JsonValue};

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
            .register(Identifier::new("bb", "air"), |id| {
                let block = Arc::new(Block { default_state: id });
                let state = vec![BlockState {
                    state_id: id,
                    client_data: ClientBlockRenderData {
                        block_type: ClientBlockRenderDataType::Air,
                        fluid: false,
                        render_data: 0,
                        transparent: false,
                    },
                    parent: block.clone(),
                }];
                (block, state)
            })
            .expect("couldn't register air");
        block_registry
    }
    pub fn register<F>(&mut self, id: Arc<Identifier>, creator: F) -> Result<u32, ()>
    where
        F: FnOnce(u32) -> (Arc<Block>, Vec<BlockState>),
    {
        if self.blocks.get(&id).is_some() {
            return Err(());
        }
        let numeric_id = self.id_generator;
        let (block, mut block_states) = creator.call_once((self.id_generator,));
        self.blocks.insert(id, block);
        self.states.append(&mut block_states);
        self.id_generator += block_states.len() as u32;
        Ok(numeric_id)
    }
}

pub struct Block {
    pub default_state: u32,
}

pub struct BlockState {
    pub state_id: u32,
    pub client_data: ClientBlockRenderData,
    pub parent: Arc<Block>,
}
impl BlockState {
    pub fn get_full_id(&self) -> u32 {
        self.state_id
    }
}

pub struct ClientBlockRenderData {
    pub block_type: ClientBlockRenderDataType,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
}
pub enum ClientBlockRenderDataType {
    Air,
    Cube(ClientBlockCubeRenderData),
}
pub struct ClientBlockCubeRenderData {
    pub front: String,
    pub back: String,
    pub right: String,
    pub left: String,
    pub up: String,
    pub down: String,
}

pub struct ClientContent {}
impl ClientContent {
    pub fn generate_content(block_registry: &BlockRegistry) -> JsonValue {
        let mut blocks = array![];
        for block in block_registry.states.iter().skip(1).enumerate() {
            let client_data = &block.1.client_data;
            let mut model_json = object! {
                transparent: client_data.transparent,
                fluid: client_data.fluid,
                render_data: client_data.render_data

            };
            match &client_data.block_type {
                ClientBlockRenderDataType::Air => {}
                ClientBlockRenderDataType::Cube(cube_data) => {
                    model_json.insert("type", "cube").unwrap();
                    model_json.insert("north", cube_data.front.clone()).unwrap();
                    model_json.insert("south", cube_data.back.clone()).unwrap();
                    model_json.insert("right", cube_data.right.clone()).unwrap();
                    model_json.insert("left", cube_data.left.clone()).unwrap();
                    model_json.insert("up", cube_data.up.clone()).unwrap();
                    model_json.insert("down", cube_data.down.clone()).unwrap();
                }
            }
            blocks
                .push(object! {id: block.0+1,
                    model: model_json
                })
                .unwrap();
        }
        object! {
            blocks: blocks
        }
    }
}
