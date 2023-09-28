use crate::model::Model;
use crate::texture::{pack_textures, TextureAtlas};
use block_byte_common::content::{
    ClientBlockData, ClientBlockRenderDataType, ClientContent, ClientItemData, ModelData,
};
use block_byte_common::{Face, TexCoords};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;

pub fn load_assets(zip_path: &Path) -> (RgbaImage, TextureAtlas, BlockRegistry, ItemRegistry) {
    let mut zip =
        zip::ZipArchive::new(std::fs::File::open(zip_path).expect("asset archive not found"))
            .expect("asset archive invalid");
    let mut textures_to_pack = Vec::new();
    let mut models = HashMap::new();

    let mut content = None;
    let mut font = None;

    for file in 0..zip.len() {
        let mut file = zip.by_index(file).unwrap();
        if !file.is_file() {
            continue;
        }
        let mut data = Vec::new();
        use std::io::Read;
        file.read_to_end(&mut data).unwrap();
        let name = file.name();
        if name.ends_with(".png") {
            textures_to_pack.push((name.replace(".png", ""), data));
            continue;
        }
        if name.ends_with(".wav") {
            //todo
            //sound_manager.load(name.replace(".wav", ""), data);
            continue;
        }
        if name.ends_with(".bbm") {
            if let Ok(model_data) = bitcode::deserialize::<ModelData>(data.as_slice()) {
                models.insert(name.replace(".bbm", ""), model_data);
            }

            continue;
        }
        if name == "content.json" {
            content = Some(
                serde_json::from_str::<ClientContent>(String::from_utf8(data).unwrap().as_str())
                    .unwrap(),
            );
            continue;
        }
        if name == "font.ttf" {
            font = Some(rusttype::Font::try_from_vec(data).unwrap());
            continue;
        }
    }
    models.insert(
        "missing".to_string(),
        bitcode::deserialize::<ModelData>(include_bytes!("assets/missing.bbm").as_slice()).unwrap(),
    );
    let font = font.unwrap();
    let content = content.unwrap();
    let (texture_atlas, texture_image) = pack_textures(textures_to_pack, &font);
    let mut block_registry = BlockRegistry { blocks: Vec::new() };
    for block in content.blocks {
        block_registry.add_block(block, &texture_atlas, &models);
    }
    let mut item_registry = ItemRegistry { items: Vec::new() };
    for item in content.items {
        item_registry.add_item(item);
    }
    (texture_image, texture_atlas, block_registry, item_registry)
}
pub struct BlockRegistry {
    blocks: Vec<BlockData>,
}
impl BlockRegistry {
    pub fn get_block(&self, block: u32) -> &BlockData {
        self.blocks.get(block as usize).unwrap()
    }
    fn add_block(
        &mut self,
        block_data: ClientBlockData,
        texture_atlas: &TextureAtlas,
        models: &HashMap<String, ModelData>,
    ) {
        self.blocks.push(BlockData {
            block_type: match block_data.block_type {
                ClientBlockRenderDataType::Air => BlockRenderDataType::Air,
                ClientBlockRenderDataType::Cube(cube) => {
                    BlockRenderDataType::Cube(BlockCubeRenderData {
                        front: texture_atlas.get(cube.front.as_str()),
                        back: texture_atlas.get(cube.back.as_str()),
                        left: texture_atlas.get(cube.left.as_str()),
                        right: texture_atlas.get(cube.right.as_str()),
                        up: texture_atlas.get(cube.up.as_str()),
                        down: texture_atlas.get(cube.down.as_str()),
                    })
                }
                ClientBlockRenderDataType::Static(static_data) => {
                    BlockRenderDataType::Static(BlockStaticRenderData {
                        model: Model::new(
                            models
                                .get(static_data.model.as_str())
                                .unwrap_or(models.get("missing").unwrap())
                                .clone(),
                            texture_atlas.get(static_data.texture.as_str()),
                        ),
                    })
                }

                ClientBlockRenderDataType::Foliage(foliage) => {
                    BlockRenderDataType::Foliage(BlockFoliageRenderData {
                        texture_1: texture_atlas.get(foliage.texture_1.as_str()),
                        texture_2: texture_atlas.get(foliage.texture_2.as_str()),
                        texture_3: texture_atlas.get(foliage.texture_3.as_str()),
                        texture_4: texture_atlas.get(foliage.texture_4.as_str()),
                    })
                }
            },
            dynamic: block_data.dynamic.map(|dynamic| BlockDynamicData {
                animations: dynamic.animations,
                items: dynamic.items,
            }),
            fluid: block_data.fluid,
            render_data: block_data.render_data,
            selectable: block_data.selectable,
            transparent: block_data.transparent,
        });
    }
}
pub struct BlockData {
    pub block_type: BlockRenderDataType,
    pub dynamic: Option<BlockDynamicData>,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
    pub selectable: bool,
}

pub struct BlockDynamicData {
    //todo: model
    pub animations: Vec<String>,
    pub items: Vec<String>,
}

pub enum BlockRenderDataType {
    Air,
    Cube(BlockCubeRenderData),
    Static(BlockStaticRenderData),
    Foliage(BlockFoliageRenderData),
}
impl BlockRenderDataType {
    pub fn is_face_full(&self, face: Face) -> bool {
        match self {
            BlockRenderDataType::Air => false,
            BlockRenderDataType::Cube(_) => true,
            BlockRenderDataType::Static(_) => false, //todo
            BlockRenderDataType::Foliage(_) => false,
        }
    }
}

pub struct BlockCubeRenderData {
    pub front: TexCoords,
    pub back: TexCoords,
    pub right: TexCoords,
    pub left: TexCoords,
    pub up: TexCoords,
    pub down: TexCoords,
}
impl BlockCubeRenderData {
    pub fn by_face(&self, face: Face) -> TexCoords {
        match face {
            Face::Front => self.front,
            Face::Back => self.back,
            Face::Left => self.left,
            Face::Right => self.right,
            Face::Up => self.up,
            Face::Down => self.down,
        }
    }
}

pub struct BlockStaticRenderData {
    pub model: Model,
}
pub struct BlockFoliageRenderData {
    pub texture_1: TexCoords,
    pub texture_2: TexCoords,
    pub texture_3: TexCoords,
    pub texture_4: TexCoords,
}

pub struct ItemRegistry {
    items: Vec<ClientItemData>,
}
impl ItemRegistry {
    pub fn get_item(&self, item: u32) -> &ClientItemData {
        self.items.get(item as usize).unwrap()
    }
    fn add_item(&mut self, item_data: ClientItemData) {
        self.items.push(item_data);
    }
}
