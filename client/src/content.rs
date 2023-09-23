use crate::texture::{pack_textures, TexCoords, TextureAtlas};
use block_byte_common::content::{ClientBlockData, ClientBlockRenderDataType, ClientContent};
use block_byte_common::Face;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;

pub fn load_assets(zip_path: &Path) -> (RgbaImage, BlockRegistry) {
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
            models.insert(name.replace(".bbm", ""), data);
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
    let font = font.unwrap();
    let content = content.unwrap();
    let (texture_atlas, texture_image) = pack_textures(textures_to_pack, &font);
    let mut block_registry = BlockRegistry { blocks: Vec::new() };
    for block in content.blocks {
        block_registry.add_block(block, &texture_atlas)
    }
    //let content = load_content(content.unwrap(), &texture_atlas, &texture, models);
    (texture_image, block_registry)
}
pub struct BlockRegistry {
    blocks: Vec<BlockData>,
}
impl BlockRegistry {
    pub fn get_block(&self, block: u32) -> &BlockData {
        self.blocks.get(block as usize).unwrap()
    }
    fn add_block(&mut self, block_data: ClientBlockData, texture_atlas: &TextureAtlas) {
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
                    BlockRenderDataType::Static(BlockStaticRenderData {})
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
    //todo: model
}
pub struct BlockFoliageRenderData {
    pub texture_1: TexCoords,
    pub texture_2: TexCoords,
    pub texture_3: TexCoords,
    pub texture_4: TexCoords,
}
