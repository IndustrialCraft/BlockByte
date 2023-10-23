use anyhow::{bail, Context, Result};
use block_byte_common::content::{
    ClientBlockCubeRenderData, ClientBlockData, ClientBlockDynamicData,
    ClientBlockFoliageRenderData, ClientBlockRenderDataType, ClientBlockStaticRenderData,
    ClientEntityData,
};
use block_byte_common::messages::MovementType;
use block_byte_common::{BlockPosition, Color, Face, HorizontalFace, Position};
use image::io::Reader;
use image::{ImageOutputFormat, Rgba, RgbaImage};
use json::JsonValue;
use parking_lot::Mutex;
use rhai::plugin::*;
use rhai::{exported_module, Engine, EvalAltResult, FnPtr, FuncArgs, AST};
use splines::{Interpolation, Spline};
use std::{
    cell::OnceCell,
    collections::HashMap,
    fs,
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};
use twox_hash::XxHash64;
use walkdir::WalkDir;

use crate::registry::{BlockMachineData, BlockStateProperty, BlockStatePropertyStorage};
use crate::world::World;
use crate::{
    inventory::{LootTable, Recipe},
    registry::{BlockRegistry, ItemRegistry, ToolData, ToolType},
    util::{Identifier, Location},
    world::{Entity, Structure},
    Server,
};

struct Mod {
    path: PathBuf,
    namespace: String,
}

impl Mod {
    pub fn new(path: &Path) -> Result<Self> {
        let mut path_buf = path.to_path_buf();
        path_buf.push("descriptor.json");
        let descriptor = json::parse(
            std::fs::read_to_string(&path_buf)
                .with_context(|| {
                    format!(
                        "descriptor for mod {} wasn't found",
                        path.file_name().unwrap().to_str().unwrap()
                    )
                })?
                .as_str(),
        )
        .with_context(|| {
            format!(
                "descriptor for mod {} is incorrect",
                path.file_name().unwrap().to_str().unwrap()
            )
        })?;
        path_buf.pop();
        let mod_identifier = descriptor["id"].as_str().unwrap().to_string();
        Ok(Mod {
            path: path.to_path_buf(),
            namespace: mod_identifier,
        })
    }
    pub fn load_scripts(
        &self,
        engine: &Engine,
        script_errors: &mut Vec<(String, Box<EvalAltResult>)>,
    ) {
        for script in WalkDir::new({
            let mut scripts_path = self.path.clone();
            scripts_path.push("scripts");
            scripts_path
        })
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|entry| entry.metadata().unwrap().is_file())
        {
            if let Err(error) = engine.eval_file::<()>(script.into_path()) {
                script_errors.push((self.namespace.clone(), error));
            }
            //todo
        }
    }
    pub fn read_resource(&self, id: Arc<Identifier>) -> Result<Vec<u8>> {
        if id.get_namespace() == &self.namespace {
            let mut full_path = self.path.clone();
            for path_part in id.get_key().split("/") {
                full_path.push(path_part);
            }
            fs::read(full_path).with_context(|| format!("resource {} not found", id))
        } else {
            bail!(
                "identifier {} doesn't have same namespace as mod {} it was requested from",
                id,
                self.namespace
            );
        }
    }
}

pub struct ModManager {
    mods: HashMap<String, Mod>,
}

impl ModManager {
    pub fn load_mods(
        path: &Path,
    ) -> (
        Self,
        Vec<BlockBuilder>,
        Vec<ItemBuilder>,
        Vec<EntityBuilder>,
        ClientContentData,
        Vec<BiomeBuilder>,
        HashMap<Identifier, Vec<ScriptCallback>>,
        Vec<(String, Box<EvalAltResult>)>,
        Engine,
    ) {
        let mut errors = Vec::new();
        let mut mods = HashMap::new();
        for mod_path in std::fs::read_dir(path).unwrap() {
            let mod_path = mod_path.unwrap();
            let path = mod_path.path();
            let name = mod_path.file_name().to_str().unwrap().to_string();
            if let Ok(loaded_mod) = Mod::new(path.as_path()) {
                if mods.contains_key(&loaded_mod.namespace) {
                    panic!("mod {} loaded twice", loaded_mod.namespace);
                }
                mods.insert(loaded_mod.namespace.clone(), loaded_mod);
            } else {
                println!("loading mod '{}' failed", name);
            }
        }
        let mut loading_engine = Engine::new();
        let current_mod_path = Arc::new(Mutex::new(PathBuf::new()));
        let content = Arc::new(Mutex::new(ClientContentData::new()));
        let blocks = Arc::new(Mutex::new(Vec::new()));
        let items = Arc::new(Mutex::new(Vec::new()));
        let entities = Arc::new(Mutex::new(Vec::new()));
        let biomes = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(HashMap::new()));
        let registered_blocks = blocks.clone();
        let registered_items = items.clone();
        let registered_items_from_blocks = items.clone();
        let registered_entities = entities.clone();
        let registered_biomes = biomes.clone();
        let registered_events = events.clone();
        loading_engine
            .register_type_with_name::<BlockBuilder>("BlockBuilder")
            .register_fn("create_block", BlockBuilder::new)
            .register_fn(
                "add_property_horizontal_face",
                BlockBuilder::add_property_horizontal_face,
            )
            .register_fn("add_property_face", BlockBuilder::add_property_face)
            .register_fn("breaking_tool", BlockBuilder::breaking_tool)
            .register_fn("loot", BlockBuilder::loot)
            .register_fn("machine", BlockBuilder::machine)
            .register_fn("breaking_speed", BlockBuilder::breaking_speed)
            .register_fn("create_air", ModClientBlockData::create_air)
            .register_fn("create_cube", ModClientBlockData::create_cube)
            .register_fn("create_static", ModClientBlockData::create_static)
            .register_fn("create_foliage", ModClientBlockData::create_foliage)
            .register_fn("fluid", ModClientBlockData::fluid)
            .register_fn("hangs_on", ModClientBlockData::hangs_on)
            .register_fn("no_collide", ModClientBlockData::no_collide)
            .register_fn("transparent", ModClientBlockData::transparent)
            .register_fn("selectable", ModClientBlockData::selectable)
            .register_fn("render_data", ModClientBlockData::render_data)
            .register_fn("dynamic", ModClientBlockData::dynamic)
            .register_fn("rotation", ModClientBlockData::rotation)
            .register_fn(
                "dynamic_add_animation",
                ModClientBlockData::dynamic_add_animation,
            )
            .register_fn("dynamic_add_item", ModClientBlockData::dynamic_add_item)
            .register_fn("data_container", BlockBuilder::mark_data_container)
            .register_fn("register", move |this: &mut Arc<Mutex<BlockBuilder>>| {
                registered_blocks.lock().push(this.clone())
            })
            .register_fn(
                "register_item",
                move |this: &mut Arc<Mutex<BlockBuilder>>,
                      item_id: &str,
                      name: &str|
                      -> Arc<Mutex<BlockBuilder>> {
                    let mut item_builder = ItemBuilder::new(item_id);
                    ItemBuilder::client_name(&mut item_builder, name);
                    let block_id = { this.lock().id.to_string() };
                    ItemBuilder::client_model_block(&mut item_builder, block_id.as_str());
                    ItemBuilder::place(&mut item_builder, block_id.as_str());
                    registered_items_from_blocks.lock().push(item_builder);
                    this.clone()
                },
            );
        loading_engine.register_fn("register_event", move |event: &str, callback: FnPtr| {
            let mut registerd_events = registered_events.lock();
            registerd_events
                .entry(Identifier::parse(event).unwrap())
                .or_insert(Vec::new())
                .push(ScriptCallback::new(callback));
        });
        loading_engine
            .register_type_with_name::<ItemBuilder>("ItemBuilder")
            .register_fn("create_item", ItemBuilder::new)
            .register_fn("tool", ItemBuilder::tool)
            .register_fn("tool_add_type", ItemBuilder::tool_add_type)
            .register_fn("client_name", ItemBuilder::client_name)
            .register_fn("client_model_texture", ItemBuilder::client_model_texture)
            .register_fn("client_model_block", ItemBuilder::client_model_block)
            .register_fn("place", ItemBuilder::place)
            .register_fn("on_right_click", ItemBuilder::on_right_click)
            .register_fn("stack_size", ItemBuilder::stack_size)
            .register_fn("register", move |this: &mut Arc<Mutex<ItemBuilder>>| {
                registered_items.lock().push(this.clone())
            });
        loading_engine
            .register_type_with_name::<EntityBuilder>("EntityBuilder")
            .register_fn("create_entity", EntityBuilder::new)
            .register_fn("client_model", EntityBuilder::client_model)
            .register_fn("client_viewmodel", EntityBuilder::client_viewmodel)
            .register_fn(
                "client_viewmodel_add_item",
                EntityBuilder::client_viewmodel_add_item,
            )
            .register_fn(
                "client_viewmodel_add_animation",
                EntityBuilder::client_viewmodel_add_animation,
            )
            .register_fn("client_hitbox", EntityBuilder::client_hitbox)
            .register_fn("client_add_animation", EntityBuilder::client_add_animation)
            .register_fn("client_add_item", EntityBuilder::client_add_item)
            .register_fn("tick", EntityBuilder::tick)
            .register_fn("register", move |this: &mut Arc<Mutex<EntityBuilder>>| {
                registered_entities.lock().push(this.clone())
            });
        loading_engine
            .register_type_with_name::<BiomeBuilder>("BiomeBuilder")
            .register_fn("create_biome", BiomeBuilder::new)
            .register_fn("add_structure", BiomeBuilder::add_structure)
            .register_fn("spline_add_height", BiomeBuilder::spline_add_height)
            .register_fn("spline_add_land", BiomeBuilder::spline_add_land)
            .register_fn(
                "spline_add_temperature",
                BiomeBuilder::spline_add_temperature,
            )
            .register_fn("spline_add_moisture", BiomeBuilder::spline_add_moisture)
            .register_fn("register", move |this: &mut Arc<Mutex<BiomeBuilder>>| {
                registered_biomes.lock().push(this.clone())
            });
        loading_engine.register_static_module("ToolType", exported_module!(ToolTypeModule).into());

        /*loading_engine.register_fn(
            "create_biome",
            move |top: String, middle: String, bottom: String| {
                registered_biomes
                    .lock()
                    .unwrap()
                    .push((top, middle, bottom));
            },
        );*/

        let mut content_register = |name: &str, content_type: ContentType| {
            let register_current_mod_path = current_mod_path.clone();
            let register_content = content.clone();
            loading_engine.register_fn(name, move |id: &str, path: &str| {
                let start_path = { register_current_mod_path.lock().clone() };
                let mut full_path = start_path.clone();
                full_path.push(path);
                if !full_path.starts_with(start_path) {
                    panic!("path traversal attack");
                }
                register_content.lock().by_type(content_type).insert(
                    Identifier::parse(id).unwrap(),
                    std::fs::read(full_path).unwrap(),
                );
            });
        };
        content_register.call_mut(("register_image", ContentType::Image));
        content_register.call_mut(("register_sound", ContentType::Sound));
        content_register.call_mut(("register_model", ContentType::Model));

        let register_content = content.clone();
        loading_engine.register_fn("register_image", move |id: &str, image: ModImage| {
            register_content
                .lock()
                .images
                .insert(Identifier::parse(id).unwrap(), image.export());
        });

        let image_mod_path = current_mod_path.clone();
        loading_engine.register_fn("load_image", move |path: &str| {
            let start_path = { image_mod_path.lock().clone() };
            let mut full_path = start_path.clone();
            full_path.push(path);
            if !full_path.starts_with(start_path) {
                panic!("path traversal attack");
            }
            ModImage::load(std::fs::read(full_path).unwrap())
        });
        loading_engine.register_fn("multiply", |first: &mut ModImage, second: ModImage| {
            first.multiply(&second)
        });
        loading_engine.register_fn("overlay", |first: &mut ModImage, second: ModImage| {
            first.overlay(&second)
        });
        loading_engine.register_fn("color", |image: &mut ModImage, color: Color| {
            image.color(color)
        });
        loading_engine.register_fn("create_color", |r: f64, g: f64, b: f64, a: f64| Color {
            r: (r * 255.) as u8,
            g: (g * 255.) as u8,
            b: (b * 255.) as u8,
            a: (a * 255.) as u8,
        });

        for loaded_mod in &mods {
            {
                let mut path = current_mod_path.lock();
                path.clear();
                path.push(loaded_mod.1.path.clone());
            }
            loaded_mod.1.load_scripts(&loading_engine, &mut errors);
        }
        let blocks = blocks
            .lock()
            .iter()
            .map(|block| block.lock().clone())
            .collect();
        let items = items
            .lock()
            .iter()
            .map(|item| item.lock().clone())
            .collect();
        let entities = entities
            .lock()
            .iter()
            .map(|entity| entity.lock().clone())
            .collect();
        let biomes = biomes
            .lock()
            .iter()
            .map(|biome| biome.lock().clone())
            .collect();
        let events = (*events.lock()).clone();
        //println!("{blocks:#?}\n{items:#?}\n{entities:#?}");
        let content = content.lock().clone();
        (
            ModManager { mods },
            blocks,
            items,
            entities,
            content,
            biomes,
            events,
            errors,
            loading_engine,
        )
    }
    pub fn load_structures(
        &self,
        block_registry: &BlockRegistry,
    ) -> HashMap<Identifier, Arc<Structure>> {
        let mut structures = HashMap::new();
        for loaded_mod in &self.mods {
            let mut path = loaded_mod.1.path.clone();
            path.push("structures");
            for structure_path in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|entry| entry.metadata().unwrap().is_file())
            {
                let path_diff = pathdiff::diff_paths(structure_path.path(), &path).unwrap();
                let id = path_diff
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .split_once(".")
                    .unwrap()
                    .0;
                let id = Identifier::new(loaded_mod.1.namespace.clone(), id);
                let json = json::parse(fs::read_to_string(structure_path.path()).unwrap().as_str())
                    .unwrap();
                structures.insert(
                    id.clone(),
                    Arc::new(Structure::from_json(id, json, block_registry)),
                );
            }
        }
        structures
    }
    pub fn load_loot_tables(
        &self,
        item_registry: &ItemRegistry,
    ) -> HashMap<Identifier, Arc<LootTable>> {
        let mut loot_tables = HashMap::new();
        for loaded_mod in &self.mods {
            let mut path = loaded_mod.1.path.clone();
            path.push("loot_tables");
            for loot_table_path in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|entry| entry.metadata().unwrap().is_file())
            {
                let path_diff = pathdiff::diff_paths(loot_table_path.path(), &path).unwrap();
                let id = path_diff
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .split_once(".")
                    .unwrap()
                    .0;
                let id = Identifier::new(loaded_mod.1.namespace.clone(), id);
                let json =
                    json::parse(fs::read_to_string(loot_table_path.path()).unwrap().as_str())
                        .unwrap();
                loot_tables.insert(
                    id.clone(),
                    Arc::new(LootTable::from_json(json, item_registry)),
                );
            }
        }
        loot_tables
    }
    pub fn load_recipes(&self, item_registry: &ItemRegistry) -> HashMap<Identifier, Arc<Recipe>> {
        let mut recipes = HashMap::new();
        for loaded_mod in &self.mods {
            let mut path = loaded_mod.1.path.clone();
            path.push("recipes");
            for recipe_path in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|entry| entry.metadata().unwrap().is_file())
            {
                let path_diff = pathdiff::diff_paths(recipe_path.path(), &path).unwrap();
                let id = path_diff
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .split_once(".")
                    .unwrap()
                    .0;
                let id = Identifier::new(loaded_mod.1.namespace.clone(), id);
                let json =
                    json::parse(fs::read_to_string(recipe_path.path()).unwrap().as_str()).unwrap();
                recipes.insert(
                    id.clone(),
                    Arc::new(Recipe::from_json(id, json, item_registry)),
                );
            }
        }
        recipes
    }
    pub fn runtime_engine_load(engine: &mut Engine, server: Weak<Server>) {
        engine.register_type_with_name::<Position>("Position");
        engine.register_type_with_name::<BlockPosition>("BlockPosition");
        {
            let server = server.clone();
            engine.register_fn("Server", move || server.upgrade().unwrap());
        }
        engine.register_fn("take_data_point", |entity: &mut Arc<Entity>, id: &str| {
            entity
                .user_data
                .lock()
                .take_data_point(&Identifier::parse(id).unwrap())
        });
        engine.register_fn("get_data_point", |entity: &mut Arc<Entity>, id: &str| {
            entity
                .user_data
                .lock()
                .get_data_point_ref(&Identifier::parse(id).unwrap())
                .cloned()
                .unwrap_or(Dynamic::UNIT)
        });
        engine.register_fn(
            "insert_data_point",
            |entity: &mut Arc<Entity>, id: &str, value: Dynamic| {
                entity
                    .user_data
                    .lock()
                    .put_data_point(&Identifier::parse(id).unwrap(), value)
            },
        );
        //engine.register_type_with_name::<Arc<Entity>>("Entity");
        engine.register_fn("send_message", |entity: Arc<Entity>, text: &str| {
            entity.send_chat_message(text.to_string());
        });
        engine.register_fn("get_position", |entity: Arc<Entity>| {
            entity.get_location().position
        });
        engine.register_fn("get_rotation", |entity: Arc<Entity>| {
            entity.get_rotation() as f64
        });
        engine.register_fn("get_world", |entity: Arc<Entity>| {
            entity.get_location().chunk.world.clone()
        });
        engine.register_fn("abilities", |entity: Arc<Entity>| PlayerAbilitiesWrapper {
            entity,
        });
        engine.register_fn(
            "teleport_position",
            |entity: &mut Arc<Entity>, position: Position| {
                let position = position.clone();
                let chunk = entity.get_location().chunk.clone();
                let location = Location {
                    position,
                    world: chunk.world.clone(),
                };
                entity.teleport(&location, None);
            },
        );
        engine.register_fn(
            "teleport_position_rotation",
            |entity: &mut Arc<Entity>, position: Position, rotation: f64| {
                let position = position.clone();
                let chunk = entity.get_location().chunk.clone();
                let location = Location {
                    position,
                    world: chunk.world.clone(),
                };
                entity.teleport(&location, Some((rotation as f32, false)));
            },
        );
        engine.register_fn("is_shifting", |entity: &mut Arc<Entity>| {
            entity.is_shifting()
        });

        engine.register_fn("Position", |x: f64, y: f64, z: f64| Position { x, y, z });
        engine.register_fn("to_block_position", |position: &mut Position| {
            position.to_block_pos()
        });
        engine.register_fn("BlockPosition", |x: i64, y: i64, z: i64| BlockPosition {
            x: x as i32,
            y: y as i32,
            z: z as i32,
        });
        engine.register_fn("to_string", |block_position: &mut BlockPosition| {
            block_position.to_string()
        });
        engine.register_fn("+", |first: Position, second: Position| first + second);
        engine.register_fn("*", |first: Position, scalar: f64| first.multiply(scalar));
        engine.register_fn("distance", Position::distance);
        engine.register_get_set("x", Position::get_x, Position::set_x);
        engine.register_get_set("y", Position::get_y, Position::set_y);
        engine.register_get_set("z", Position::get_z, Position::set_z);

        engine.register_fn(
            "get_structure",
            |world: &mut Arc<World>,
             first: BlockPosition,
             second: BlockPosition,
             origin: BlockPosition| {
                Arc::new(Structure::from_world(
                    Identifier::new("bb", "script_requested"),
                    &world,
                    first,
                    second,
                    origin,
                ))
            },
        );
        engine.register_fn(
            "place_structure",
            |world: &mut Arc<World>, structure: Arc<Structure>, position: BlockPosition| {
                world.place_structure(position, &structure, true);
            },
        );
        {
            let server = server.clone();
            engine.register_fn(
                "export_structure",
                move |structure: &mut Arc<Structure>, name: &str| {
                    let server = server.upgrade().unwrap();
                    let json = structure.export(&server.block_registry);
                    server.export_file(name.to_string(), json.to_string().as_bytes().to_vec());
                },
            );
        }

        engine
            .register_type_with_name::<PlayerAbilitiesWrapper>("PlayerAbilities")
            .register_fn("speed", PlayerAbilitiesWrapper::set_speed)
            .register_fn("movement_type", PlayerAbilitiesWrapper::set_movement_type)
            .register_fn("creative", PlayerAbilitiesWrapper::set_creative);
        engine.register_static_module("MovementType", exported_module!(MovementTypeModule).into());
    }
}

#[derive(Clone)]
pub struct BiomeBuilder {
    pub id: Identifier,
    pub top_block: Identifier,
    pub middle_block: Identifier,
    pub bottom_block: Identifier,
    pub water_block: Identifier,
    pub spline_height: Vec<splines::Key<f64, f64>>,
    pub spline_land: Vec<splines::Key<f64, f64>>,
    pub spline_temperature: Vec<splines::Key<f64, f64>>,
    pub spline_moisture: Vec<splines::Key<f64, f64>>,
    pub structures: Vec<(f32, Identifier)>,
}

impl BiomeBuilder {
    pub fn new(id: &str, top: &str, middle: &str, bottom: &str, water: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(BiomeBuilder {
            id: Identifier::parse(id).unwrap(),
            top_block: Identifier::parse(top).unwrap(),
            middle_block: Identifier::parse(middle).unwrap(),
            bottom_block: Identifier::parse(bottom).unwrap(),
            water_block: Identifier::parse(water).unwrap(),
            spline_height: Vec::new(),
            spline_land: Vec::new(),
            spline_temperature: Vec::new(),
            spline_moisture: Vec::new(),
            structures: Vec::new(),
        }))
    }
    pub fn add_structure(this: &mut Arc<Mutex<Self>>, chance: f64, id: &str) -> Arc<Mutex<Self>> {
        this.lock()
            .structures
            .push((chance as f32, Identifier::parse(id).unwrap()));
        this.clone()
    }
    pub fn spline_add_height(
        this: &mut Arc<Mutex<Self>>,
        key: f64,
        value: f64,
    ) -> Arc<Mutex<Self>> {
        this.lock()
            .spline_height
            .push(splines::Key::new(key, value, Interpolation::Linear));
        this.clone()
    }
    pub fn spline_add_land(this: &mut Arc<Mutex<Self>>, key: f64, value: f64) -> Arc<Mutex<Self>> {
        this.lock()
            .spline_land
            .push(splines::Key::new(key, value, Interpolation::Linear));
        this.clone()
    }
    pub fn spline_add_temperature(
        this: &mut Arc<Mutex<Self>>,
        key: f64,
        value: f64,
    ) -> Arc<Mutex<Self>> {
        this.lock()
            .spline_temperature
            .push(splines::Key::new(key, value, Interpolation::Linear));
        this.clone()
    }
    pub fn spline_add_moisture(
        this: &mut Arc<Mutex<Self>>,
        key: f64,
        value: f64,
    ) -> Arc<Mutex<Self>> {
        this.lock()
            .spline_moisture
            .push(splines::Key::new(key, value, Interpolation::Linear));
        this.clone()
    }
}

#[derive(Clone, Debug)]
pub struct BlockBuilder {
    pub id: Identifier,
    pub client: ScriptCallback,
    pub data_container: bool,
    pub breaking_data: (f32, Option<(ToolType, f32)>),
    pub loot: Option<Identifier>,
    pub machine_data: Option<BlockMachineData>,
    pub properties: BlockStatePropertyStorage,
}

impl BlockBuilder {
    pub fn new(id: &str, client: FnPtr) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(BlockBuilder {
            id: Identifier::parse(id).unwrap(),
            client: ScriptCallback::new(client),
            data_container: false,
            breaking_data: (1., None),
            loot: None,
            machine_data: None,
            properties: BlockStatePropertyStorage::new(),
        }))
    }
    pub fn add_property_horizontal_face(
        this: &mut Arc<Mutex<Self>>,
        name: &str,
    ) -> Arc<Mutex<Self>> {
        this.lock()
            .properties
            .register_property(name.to_string(), BlockStateProperty::HorizontalFace);
        this.clone()
    }
    pub fn add_property_face(this: &mut Arc<Mutex<Self>>, name: &str) -> Arc<Mutex<Self>> {
        this.lock()
            .properties
            .register_property(name.to_string(), BlockStateProperty::Face);
        this.clone()
    }
    pub fn machine(
        this: &mut Arc<Mutex<Self>>,
        recipe_type: &str,
        base_speed: f64,
        tier: i64,
    ) -> Arc<Mutex<Self>> {
        this.lock().machine_data = Some(BlockMachineData {
            recipe_type: Identifier::parse(recipe_type).unwrap(),
            base_speed: base_speed as f32,
            tier: tier as u32,
        });
        this.clone()
    }
    pub fn breaking_speed(this: &mut Arc<Mutex<Self>>, breaking_speed: f64) -> Arc<Mutex<Self>> {
        this.lock().breaking_data.0 = breaking_speed as f32;
        this.clone()
    }

    pub fn loot(this: &mut Arc<Mutex<Self>>, id: &str) -> Arc<Mutex<Self>> {
        this.lock().loot = Some(Identifier::parse(id).unwrap());
        this.clone()
    }
    pub fn breaking_tool(
        this: &mut Arc<Mutex<Self>>,
        tool_type: ToolType,
        hardness: f64,
    ) -> Arc<Mutex<Self>> {
        this.lock().breaking_data.1 = Some((tool_type, hardness as f32));
        this.clone()
    }
    pub fn mark_data_container(this: &mut Arc<Mutex<Self>>) -> Arc<Mutex<Self>> {
        this.lock().data_container = true;
        this.clone()
    }
}
#[derive(Clone)]
pub struct ModClientBlockData {
    pub client: ClientBlockData,
    pub hangs_on: Option<Face>,
}
impl ModClientBlockData {
    pub fn create_air() -> Self {
        ModClientBlockData::new(ClientBlockRenderDataType::Air)
    }
    pub fn create_cube(
        front: &str,
        back: &str,
        right: &str,
        left: &str,
        up: &str,
        down: &str,
    ) -> Self {
        Self::new(ClientBlockRenderDataType::Cube(ClientBlockCubeRenderData {
            front: front.to_string(),
            back: back.to_string(),
            right: right.to_string(),
            left: left.to_string(),
            up: up.to_string(),
            down: down.to_string(),
        }))
    }
    pub fn create_static(model: &str, texture: &str) -> Self {
        Self::new(ClientBlockRenderDataType::Static(
            ClientBlockStaticRenderData {
                model: model.to_string(),
                texture: texture.to_string(),
            },
        ))
    }
    pub fn create_foliage(
        texture_1: &str,
        texture_2: &str,
        texture_3: &str,
        texture_4: &str,
    ) -> Self {
        Self::new(ClientBlockRenderDataType::Foliage(
            ClientBlockFoliageRenderData {
                texture_1: texture_1.to_string(),
                texture_2: texture_2.to_string(),
                texture_3: texture_3.to_string(),
                texture_4: texture_4.to_string(),
            },
        ))
    }
    pub fn new(block_type: ClientBlockRenderDataType) -> Self {
        ModClientBlockData {
            client: ClientBlockData {
                block_type,
                dynamic: None,
                transparent: false,
                selectable: true,
                fluid: false,
                no_collide: false,
                render_data: 0,
                rotation: 0.,
            },
            hangs_on: None,
        }
    }
    pub fn hangs_on(&mut self, hangs_on: Face) -> Self {
        self.hangs_on = Some(hangs_on);
        self.clone()
    }
    pub fn no_collide(&mut self) -> Self {
        self.client.no_collide = true;
        self.clone()
    }
    pub fn fluid(&mut self, fluid: bool) -> Self {
        self.client.fluid = fluid;
        self.clone()
    }
    pub fn transparent(&mut self, transparent: bool) -> Self {
        self.client.transparent = transparent;
        self.clone()
    }
    pub fn selectable(&mut self, selectable: bool) -> Self {
        self.client.selectable = selectable;
        self.clone()
    }
    pub fn render_data(&mut self, render_data: i64) -> Self {
        self.client.render_data = render_data as u8;
        self.clone()
    }
    pub fn rotation(&mut self, face: HorizontalFace) -> Self {
        self.client.rotation = match face {
            HorizontalFace::Front => 0.,
            HorizontalFace::Right => 90.,
            HorizontalFace::Back => 180.,
            HorizontalFace::Left => 270.,
        };
        self.clone()
    }
    pub fn dynamic(&mut self, model: &str, texture: &str) -> Self {
        self.client.dynamic = Some(ClientBlockDynamicData {
            model: model.to_string(),
            texture: texture.to_string(),
            animations: Vec::new(),
            items: Vec::new(),
        });
        self.clone()
    }
    pub fn dynamic_add_animation(&mut self, animation: &str) -> Self {
        //todo: result
        if let Some(dynamic) = &mut self.client.dynamic {
            dynamic.animations.push(animation.to_string());
        }
        self.clone()
    }
    pub fn dynamic_add_item(&mut self, item: &str) -> Self {
        //todo: result
        if let Some(dynamic) = &mut self.client.dynamic {
            dynamic.items.push(item.to_string());
        }
        self.clone()
    }
}
#[derive(Clone)]
pub struct ItemBuilder {
    pub id: Identifier,
    pub client: ClientModItemData,
    pub place: Option<Identifier>,
    pub on_right_click: Option<FnPtr>,
    pub stack_size: u32,
    pub tool: Option<ToolData>,
}

#[derive(Clone, Debug)]
pub struct ClientModItemData {
    pub name: String,
    pub model: ClientModItemModel,
}

#[derive(Clone, Debug)]
pub enum ClientModItemModel {
    Texture(String),
    Block(String),
}

impl ItemBuilder {
    pub fn new(id: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(ItemBuilder {
            client: ClientModItemData {
                name: id.to_string(),
                model: ClientModItemModel::Texture(String::new()),
            },
            place: None,
            id: Identifier::parse(id).unwrap(),
            on_right_click: None,
            stack_size: 20,
            tool: None,
        }))
    }
    pub fn tool(
        this: &mut Arc<Mutex<Self>>,
        durability: i64,
        speed: f64,
        hardness: f64,
    ) -> Arc<Mutex<Self>> {
        let mut locked = this.lock();
        locked.tool = Some(ToolData {
            durability: durability as u32,
            speed: speed as f32,
            hardness: hardness as f32,
            type_bitmap: 0u8,
        });
        locked.stack_size = 1;
        this.clone()
    }
    pub fn tool_add_type(this: &mut Arc<Mutex<Self>>, tool_type: ToolType) -> Arc<Mutex<Self>> {
        if let Some(tool) = &mut this.lock().tool {
            tool.add_type(tool_type);
        }
        this.clone()
    }
    pub fn client_name(this: &mut Arc<Mutex<Self>>, name: &str) -> Arc<Mutex<Self>> {
        this.lock().client.name = name.to_string();
        this.clone()
    }
    pub fn client_model_texture(this: &mut Arc<Mutex<Self>>, texture: &str) -> Arc<Mutex<Self>> {
        this.lock().client.model = ClientModItemModel::Texture(texture.to_string());
        this.clone()
    }
    pub fn client_model_block(this: &mut Arc<Mutex<Self>>, block: &str) -> Arc<Mutex<Self>> {
        this.lock().client.model = ClientModItemModel::Block(block.to_string());
        this.clone()
    }
    pub fn place(this: &mut Arc<Mutex<Self>>, place: &str) -> Arc<Mutex<Self>> {
        this.lock().place = Some(Identifier::parse(place).unwrap());
        this.clone()
    }
    pub fn on_right_click(this: &mut Arc<Mutex<Self>>, callback: FnPtr) -> Arc<Mutex<Self>> {
        this.lock().on_right_click = Some(callback);
        this.clone()
    }
    pub fn stack_size(this: &mut Arc<Mutex<Self>>, stack_size: u32) -> Arc<Mutex<Self>> {
        let mut locked = this.lock();
        if locked.tool.is_none() {
            locked.stack_size = stack_size;
        } else {
            panic!("setting stack size of tool");
        }
        this.clone()
    }
}

#[derive(Clone)]
pub struct EntityBuilder {
    pub id: Identifier,
    pub client: ClientEntityData,
    pub ticker: Option<FnPtr>,
}

impl EntityBuilder {
    pub fn new(id: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(EntityBuilder {
            id: Identifier::parse(id).unwrap(),
            client: ClientEntityData {
                model: String::new(),
                texture: String::new(),
                hitbox_w: 1.,
                hitbox_h: 1.,
                hitbox_d: 1.,
                hitbox_h_shifting: 0.75,
                animations: Vec::new(),
                items: Vec::new(),
                viewmodel: None,
            },
            ticker: None,
        }))
    }
    pub fn client_viewmodel(
        this: &mut Arc<Mutex<Self>>,
        model: &str,
        texture: &str,
    ) -> Arc<Mutex<Self>> {
        this.lock().client.viewmodel = Some((
            model.to_string(),
            texture.to_string(),
            Vec::new(),
            Vec::new(),
        ));
        this.clone()
    }
    pub fn client_viewmodel_add_animation(
        this: &mut Arc<Mutex<Self>>,
        animation: &str,
    ) -> Arc<Mutex<Self>> {
        this.lock()
            .client
            .viewmodel
            .as_mut()
            .unwrap()
            .2
            .push(animation.to_string());
        this.clone()
    }
    pub fn client_viewmodel_add_item(this: &mut Arc<Mutex<Self>>, item: &str) -> Arc<Mutex<Self>> {
        this.lock()
            .client
            .viewmodel
            .as_mut()
            .unwrap()
            .3
            .push(item.to_string());
        this.clone()
    }
    pub fn tick(this: &mut Arc<Mutex<Self>>, callback: FnPtr) -> Arc<Mutex<Self>> {
        this.lock().ticker = Some(callback);
        this.clone()
    }
    pub fn client_model(
        this: &mut Arc<Mutex<Self>>,
        model: &str,
        texture: &str,
    ) -> Arc<Mutex<Self>> {
        {
            let mut borrowed = this.lock();
            borrowed.client.model = model.to_string();
            borrowed.client.texture = texture.to_string();
        }
        this.clone()
    }
    pub fn client_hitbox(
        this: &mut Arc<Mutex<Self>>,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Arc<Mutex<Self>> {
        {
            let mut borrowed = this.lock();
            borrowed.client.hitbox_w = width;
            borrowed.client.hitbox_h = height;
            borrowed.client.hitbox_d = depth;
            borrowed.client.hitbox_h_shifting = height * 0.75;
        }
        this.clone()
    }
    pub fn client_add_animation(this: &mut Arc<Mutex<Self>>, animation: &str) -> Arc<Mutex<Self>> {
        this.lock().client.animations.push(animation.to_string());
        this.clone()
    }
    pub fn client_add_item(this: &mut Arc<Mutex<Self>>, item: &str) -> Arc<Mutex<Self>> {
        this.lock().client.items.push(item.to_string());
        this.clone()
    }
}

#[derive(Clone)]
pub struct ClientContentData {
    pub images: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
    pub sounds: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
    pub models: HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>>,
}

impl ClientContentData {
    pub fn new() -> Self {
        ClientContentData {
            images: Default::default(),
            sounds: Default::default(),
            models: Default::default(),
        }
    }
    fn by_type(
        &mut self,
        content_type: ContentType,
    ) -> &mut HashMap<Identifier, Vec<u8>, BuildHasherDefault<XxHash64>> {
        match content_type {
            ContentType::Image => &mut self.images,
            ContentType::Sound => &mut self.sounds,
            ContentType::Model => &mut self.models,
        }
    }
}

#[derive(Clone, Copy)]
enum ContentType {
    Image,
    Sound,
    Model,
}

#[derive(Clone, Debug)]
pub struct ScriptCallback {
    function: FnPtr,
}

impl ScriptCallback {
    const AST: OnceCell<AST> = OnceCell::new();
    pub fn new(function: FnPtr) -> Self {
        Self { function }
    }
    pub fn call(&self, engine: &Engine, args: impl FuncArgs) -> Dynamic {
        match self
            .function
            .call::<Dynamic>(engine, Self::AST.get_or_init(|| AST::empty()), args)
        {
            Ok(ret) => ret,
            Err(error) => {
                println!("callback error: {error:#?}");
                Dynamic::UNIT
            }
        }
    }
}

#[derive(Clone)]
pub struct PlayerAbilitiesWrapper {
    pub entity: Arc<Entity>,
}

impl PlayerAbilitiesWrapper {
    pub fn set_speed(&mut self, speed: f64) {
        self.entity.entity_data.set_speed(speed as f32);
    }
    pub fn set_movement_type(&mut self, move_type: MovementType) {
        self.entity.entity_data.set_move_type(move_type);
    }
    pub fn set_creative(&mut self, creative: bool) {
        *self.entity.entity_data.creative.lock() = creative;
    }
}

#[export_module]
#[allow(non_snake_case)]
mod MovementTypeModule {
    #[allow(non_upper_case_globals)]
    pub const Normal: MovementType = MovementType::Normal;
    #[allow(non_upper_case_globals)]
    pub const Fly: MovementType = MovementType::Fly;
    #[allow(non_upper_case_globals)]
    pub const NoClip: MovementType = MovementType::NoClip;
}

#[export_module]
#[allow(non_snake_case)]
mod ToolTypeModule {
    use crate::registry::ToolType;

    #[allow(non_upper_case_globals)]
    pub const Axe: ToolType = ToolType::Axe;
    #[allow(non_upper_case_globals)]
    pub const Shovel: ToolType = ToolType::Shovel;
    #[allow(non_upper_case_globals)]
    pub const Pickaxe: ToolType = ToolType::Pickaxe;
    #[allow(non_upper_case_globals)]
    pub const Wrench: ToolType = ToolType::Wrench;
    #[allow(non_upper_case_globals)]
    pub const Knife: ToolType = ToolType::Knife;
}

pub fn spline_from_json(json: &JsonValue) -> Spline<f64, f64> {
    if json.is_number() {
        Spline::from_vec(vec![splines::Key {
            t: 0.,
            value: json.as_u32().unwrap() as f64,
            interpolation: splines::Interpolation::Linear,
        }])
    } else {
        Spline::from_vec(
            json.entries()
                .map(|(key, value)| {
                    let key: f64 = key.parse().unwrap();
                    let value = value.as_u32().unwrap() as f64;
                    splines::Key {
                        t: key,
                        value,
                        interpolation: splines::Interpolation::Linear,
                    }
                })
                .collect(),
        )
    }
}
#[derive(Clone)]
pub struct ModImage {
    image: RgbaImage,
}
impl ModImage {
    pub fn load(data: Vec<u8>) -> ModImage {
        ModImage {
            image: Reader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .unwrap()
                .decode()
                .unwrap()
                .into_rgba8(),
        }
    }
    pub fn color(&self, color: Color) -> ModImage {
        let mut image = self.image.clone();
        for pixel in image.pixels_mut() {
            pixel.0 = (Color::from_array(pixel.0) * color).to_array();
        }
        ModImage { image }
    }
    pub fn remove_background(&self, threshold: u8) -> ModImage {
        let mut image = self.image.clone();
        for pixel in image.pixels_mut() {
            pixel.0[3] = if pixel.0[3] >= threshold { 255 } else { 0 };
        }
        ModImage { image }
    }
    pub fn multiply(&self, other: &ModImage) -> ModImage {
        if self.image.width() != other.image.width() || self.image.height() != other.image.height()
        {
            panic!("cannot multiply images with different dimensions");
        }
        let mut image = self.image.clone();
        for x in 0..self.image.width() {
            for y in 0..self.image.height() {
                image.put_pixel(
                    x,
                    y,
                    Rgba(
                        (Color::from_array(self.image.get_pixel(x, y).0)
                            * Color::from_array(other.image.get_pixel(x, y).0))
                        .to_array(),
                    ),
                );
            }
        }
        ModImage { image }
    }
    pub fn overlay(&self, overlay: &ModImage) -> ModImage {
        if self.image.width() != overlay.image.width()
            || self.image.height() != overlay.image.height()
        {
            panic!("cannot overlay images with different dimensions");
        }
        let mut image = self.image.clone();
        for x in 0..self.image.width() {
            for y in 0..self.image.height() {
                let overlay_color = overlay.image.get_pixel(x, y).0;
                if overlay_color[3] > 0 {
                    image.put_pixel(x, y, Rgba(overlay_color));
                }
            }
        }
        ModImage { image }
    }
    pub fn export(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        self.image
            .write_to(
                &mut std::io::Cursor::new(&mut buffer),
                ImageOutputFormat::Png,
            )
            .unwrap();
        buffer
    }
}
