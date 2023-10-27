use crate::gui::TextRenderer;
use crate::model::Model;
use crate::texture::{pack_textures, TextureAtlas};
use ambisonic::rodio::Source;
use ambisonic::{Ambisonic, AmbisonicBuilder, StereoConfig};
use block_byte_common::content::{
    ClientBlockData, ClientBlockRenderDataType, ClientContent, ClientEntityData, ClientItemData,
    ClientItemModel, ModelData,
};
use block_byte_common::{Face, Position, TexCoords, Vec2};
use image::RgbaImage;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

pub fn load_assets(
    zip_path: PathBuf,
    dump_atlas: bool,
) -> (
    RgbaImage,
    TextureAtlas,
    BlockRegistry,
    ItemRegistry,
    EntityRegistry,
    TextRenderer<'static>,
    SoundManager,
) {
    let mut zip =
        zip::ZipArchive::new(std::fs::File::open(zip_path).expect("asset archive not found"))
            .expect("asset archive invalid");
    let mut textures_to_pack = Vec::new();
    let mut models = HashMap::new();

    let mut content = None;
    let mut font = None;

    let mut sound_manager = SoundManager::new();

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
            sound_manager.load_sound(name.replace(".wav", ""), data);
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
            font = Some(TextRenderer {
                font: rusttype::Font::try_from_vec(data).unwrap(),
            });
            continue;
        }
    }
    models.insert(
        "missing".to_string(),
        bitcode::deserialize::<ModelData>(include_bytes!("assets/missing.bbm").as_slice()).unwrap(),
    );
    let font = font.unwrap();
    let content = content.unwrap();
    let (texture_atlas, texture_image) = pack_textures(textures_to_pack, &font.font, dump_atlas);
    let mut block_registry = BlockRegistry { blocks: Vec::new() };
    for block in content.blocks {
        block_registry.add_block(block, &texture_atlas, &models);
    }
    let mut item_registry = ItemRegistry { items: Vec::new() };
    for item in content.items {
        item_registry.add_item(item, &block_registry, &texture_atlas, &texture_image);
    }
    let mut entity_registry = EntityRegistry {
        entities: Vec::new(),
    };
    for entity in content.entities {
        entity_registry.add_entity(entity, &texture_atlas, &models);
    }
    (
        texture_image,
        texture_atlas,
        block_registry,
        item_registry,
        entity_registry,
        font,
        sound_manager,
    )
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
                            Vec::new(),
                            Vec::new(),
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
            dynamic: block_data.dynamic.map(|dynamic| {
                Model::new(
                    models
                        .get(dynamic.model.as_str())
                        .unwrap_or(models.get("missing").unwrap())
                        .clone(),
                    texture_atlas.get(dynamic.texture.as_str()),
                    dynamic.animations,
                    dynamic.items,
                )
            }),
            fluid: block_data.fluid,
            render_data: block_data.render_data,
            selectable: block_data.selectable,
            transparent: block_data.transparent,
            no_collide: block_data.no_collide,
            rotation: block_data.rotation,
        });
    }
}
pub struct BlockData {
    pub block_type: BlockRenderDataType,
    pub dynamic: Option<Model>,
    pub fluid: bool,
    pub render_data: u8,
    pub transparent: bool,
    pub selectable: bool,
    pub no_collide: bool,
    pub rotation: f32,
}
impl BlockData {
    pub fn is_face_full(&self, _face: Face) -> bool {
        if self.transparent {
            return false;
        }
        match self.block_type {
            BlockRenderDataType::Air => false,
            BlockRenderDataType::Cube(_) => true,
            BlockRenderDataType::Static(_) => false,
            BlockRenderDataType::Foliage(_) => false,
        }
    }
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
    pub model: Model,
}
pub struct BlockFoliageRenderData {
    pub texture_1: TexCoords,
    pub texture_2: TexCoords,
    pub texture_3: TexCoords,
    pub texture_4: TexCoords,
}

pub struct ItemData {
    pub name: String,
    pub model: ItemModel,
}
pub enum ItemModel {
    Texture {
        texture: TexCoords,
        sides: (Vec<((u32, u32), Face)>, Vec2),
    },
    Block {
        up: TexCoords,
        front: TexCoords,
        right: TexCoords,
    },
}

pub struct ItemRegistry {
    items: Vec<ItemData>,
}
impl ItemRegistry {
    pub fn get_item(&self, item: u32) -> &ItemData {
        self.items.get(item as usize).unwrap()
    }
    fn is_pixel_full(image: &RgbaImage, texture: TexCoords, coords: (i32, i32)) -> bool {
        let width = ((texture.u2 - texture.u1) * image.width() as f32) as u32;
        let height = ((texture.v2 - texture.v1) * image.height() as f32) as u32;
        let x = (texture.u1 * image.width() as f32) as u32;
        let y = (texture.v1 * image.width() as f32) as u32;
        if coords.0 < 0 || coords.1 < 0 || coords.0 >= width as i32 || coords.1 >= height as i32 {
            return false;
        }
        image.get_pixel(x + coords.0 as u32, y + coords.1 as u32).0[3] > 0
    }
    fn add_item(
        &mut self,
        item_data: ClientItemData,
        block_registry: &BlockRegistry,
        texture_atlas: &TextureAtlas,
        image: &RgbaImage,
    ) {
        self.items.push(ItemData {
            name: item_data.name,
            model: match item_data.model {
                ClientItemModel::Texture(texture) => {
                    let texture = texture_atlas.get(texture.as_str());
                    let mut sides = Vec::new();
                    let width = (texture.u2 - texture.u1) * image.width() as f32;
                    let height = (texture.v2 - texture.v1) * image.height() as f32;
                    for x in 0..width as u32 {
                        for y in 0..height as u32 {
                            let this_full =
                                Self::is_pixel_full(image, texture, (x as i32, y as i32));
                            if this_full {
                                for face in &[Face::Front, Face::Back, Face::Left, Face::Right] {
                                    let face_offset = face.get_offset();
                                    let side_full = Self::is_pixel_full(
                                        image,
                                        texture,
                                        (x as i32 + face_offset.x, y as i32 + face_offset.z),
                                    );
                                    if !side_full {
                                        sides.push(((x, y), *face));
                                    }
                                }
                            }
                        }
                    }
                    ItemModel::Texture {
                        texture,
                        sides: (
                            sides,
                            Vec2 {
                                x: width,
                                y: height,
                            },
                        ),
                    }
                }
                ClientItemModel::Block(block) => {
                    let block = block_registry.get_block(block);
                    match &block.block_type {
                        BlockRenderDataType::Cube(cube_data) => ItemModel::Block {
                            front: cube_data.front,
                            up: cube_data.up,
                            right: cube_data.right,
                        },
                        _ => ItemModel::Texture {
                            texture: texture_atlas.missing_texture,
                            sides: (Vec::new(), Vec2::ZERO),
                        },
                    }
                }
            },
        });
    }
}

pub struct EntityRegistry {
    entities: Vec<EntityData>,
}
impl EntityRegistry {
    pub fn get_entity(&self, entity: u32) -> &EntityData {
        self.entities.get(entity as usize).unwrap()
    }
    fn add_entity(
        &mut self,
        entity_data: ClientEntityData,
        texture_atlas: &TextureAtlas,
        models: &HashMap<String, ModelData>,
    ) {
        self.entities.push(EntityData {
            model: Model::new(
                models
                    .get(entity_data.model.as_str())
                    .unwrap_or(models.get("missing").unwrap())
                    .clone(),
                texture_atlas.get(entity_data.texture.as_str()),
                entity_data.animations,
                entity_data.items,
            ),
            hitbox_w: entity_data.hitbox_w,
            hitbox_h: entity_data.hitbox_h,
            hitbox_d: entity_data.hitbox_d,
            hitbox_h_shifting: entity_data.hitbox_h_shifting,
            viewmodel: entity_data.viewmodel.as_ref().map(|viewmodel| {
                Model::new(
                    models
                        .get(viewmodel.0.as_str())
                        .unwrap_or(models.get("missing").unwrap())
                        .clone(),
                    texture_atlas.get(viewmodel.1.as_str()),
                    viewmodel.2.clone(),
                    viewmodel.3.clone(),
                )
            }),
        });
    }
}
pub struct EntityData {
    pub model: Model,
    pub hitbox_w: f64,
    pub hitbox_h: f64,
    pub hitbox_d: f64,
    pub hitbox_h_shifting: f64,
    pub viewmodel: Option<Model>,
}

//todo: better audio
pub struct SoundManager {
    scene: Ambisonic,
    //sources: HashMap<String, SamplesConverter<Decoder<Cursor<Vec<u8>>>, f32>>,
    sources: HashMap<String, Sound>,
}
impl SoundManager {
    pub fn new() -> Self {
        let scene = AmbisonicBuilder::default().build();
        SoundManager {
            scene,
            sources: HashMap::new(),
        }
    }
    pub fn load_sound(&mut self, id: String, data: Vec<u8>) {
        self.sources.insert(id, Sound(Arc::new(data)));
    }
    pub fn play_sound(
        &mut self,
        id: &str,
        position: Position,
        gain: f32,
        pitch: f32,
        relative: bool,
    ) {
        let controller = self.scene.play_at(
            ambisonic::rodio::Decoder::new(self.sources.get(id).unwrap().cursor())
                .unwrap()
                .convert_samples(),
            [position.x as f32, position.y as f32, position.z as f32],
        );
    }
}
pub struct Sound(Arc<Vec<u8>>);
impl AsRef<[u8]> for Sound {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Sound {
    pub fn cursor(self: &Self) -> Cursor<Sound> {
        Cursor::new(Sound(self.0.clone()))
    }
}
