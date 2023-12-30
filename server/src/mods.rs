use anyhow::{bail, Context, Result};
use block_byte_common::content::{ClientBlockCubeRenderData, ClientBlockData, ClientBlockDynamicData, ClientBlockFoliageRenderData, ClientBlockRenderDataType, ClientBlockStaticRenderData, ClientEntityData, ClientTexture, Transformation};
use block_byte_common::messages::MovementType;
use block_byte_common::{BlockPosition, Color, Face, HorizontalFace, KeyboardKey, Position, Vec3};
use image::io::Reader;
use image::{ImageOutputFormat, Rgba, RgbaImage};
use json::JsonValue;
use parking_lot::{Mutex, MutexGuard, RwLock};
use rhai::plugin::*;
use rhai::{
    exported_module, Engine, EvalAltResult, FnPtr, FuncArgs, GlobalRuntimeState, Scope, StaticVec,
    AST,
};
use splines::{Interpolation, Spline};
use std::collections::HashSet;
use std::ops::RangeInclusive;
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

use crate::inventory::{GUILayout, InventoryWrapper, ItemStack, ModGuiViewer, OwnedInventoryView};
use crate::registry::{
    Block, BlockState, BlockStateProperty, BlockStatePropertyStorage, BlockStateRef,
    InteractionResult,
};
use crate::util::BlockLocation;
use crate::world::{PlayerData, UserData, World, WorldBlock};
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
        id: &str,
        engine: &Engine,
        script_errors: &mut Vec<(String, Box<EvalAltResult>)>,
    ) -> Vec<(String, Module)> {
        let mut modules = Vec::new();
        let scripts_path = {
            let mut scripts_path = self.path.clone();
            scripts_path.push("scripts");
            scripts_path
        };
        for script in WalkDir::new(&scripts_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|entry| entry.metadata().unwrap().is_file())
        {
            let parsed = engine.compile_file(script.clone().into_path()).unwrap();
            let module_name = script
                .into_path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let module_path =
                module_name.replace(scripts_path.canonicalize().unwrap().to_str().unwrap(), "");
            let module_name = module_path.replace("/", "::");
            let module_name = module_name.replace(".rhs", "");
            let module_name = format!("{}{}", id, module_name);
            match Module::eval_ast_as_new(Scope::new(), &parsed, engine) {
                Ok(module) => modules.push((module_name, module)),
                Err(error) => script_errors.push((format!("{}{}", id, module_path), error)),
            }
        }
        modules
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
        HashMap<Identifier, BlockBuilder>,
        HashMap<Identifier, ItemBuilder>,
        HashMap<Identifier, EntityBuilder>,
        ClientContentData,
        Vec<BiomeBuilder>,
        EventManager,
        Vec<(String, Box<EvalAltResult>)>,
        Arc<RwLock<Engine>>,
        Vec<(String, Arc<Module>)>,
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
        let blocks = Arc::new(Mutex::new(HashMap::new()));
        let items = Arc::new(Mutex::new(HashMap::new()));
        let entities = Arc::new(Mutex::new(HashMap::new()));
        let biomes = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(EventManager::new()));
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
            .register_fn("add_property_bool", BlockBuilder::add_property_bool)
            .register_fn("add_property_number", BlockBuilder::add_property_number)
            .register_fn("on_tick", BlockBuilder::on_tick)
            .register_fn("on_right_click", BlockBuilder::on_right_click)
            .register_fn("on_left_click", BlockBuilder::on_left_click)
            .register_fn("on_neighbor_update", BlockBuilder::on_neighbor_update)
            .register_fn("on_place", BlockBuilder::on_place)
            .register_fn("on_destroy", BlockBuilder::on_destroy)
            .register_fn("create_air", ModClientBlockData::create_air)
            .register_fn("create_cube", ModClientBlockData::create_cube)
            .register_fn("create_static", ModClientBlockData::create_static)
            .register_fn("create_static", ModClientBlockData::create_static_transform)
            .register_fn("add_static_model", ModClientBlockData::add_static_model)
            .register_fn("create_foliage", ModClientBlockData::create_foliage)
            .register_fn("fluid", ModClientBlockData::fluid)
            .register_fn("no_collide", ModClientBlockData::no_collide)
            .register_fn("transparent", ModClientBlockData::transparent)
            .register_fn("selectable", ModClientBlockData::selectable)
            .register_fn("render_data", ModClientBlockData::render_data)
            .register_fn("dynamic", ModClientBlockData::dynamic)
            .register_fn(
                "dynamic_add_animation",
                ModClientBlockData::dynamic_add_animation,
            )
            .register_fn("dynamic_add_item", ModClientBlockData::dynamic_add_item)
            .register_fn("data_container", BlockBuilder::mark_data_container)
            .register_fn("register", move |this: &mut BlockBuilder, id: &str| {
                let id = Identifier::parse(id).unwrap();
                registered_blocks.lock().insert(id.clone(), this.clone());
                RegisteredBlock { id }
            })
            .register_fn(
                "register_item",
                move |this: &mut RegisteredBlock, item_id: &str, name: &str| {
                    let mut item_builder = ItemBuilder::new();
                    item_builder = ItemBuilder::client_name(&mut item_builder, name);
                    item_builder = ItemBuilder::client_model_block(
                        &mut item_builder,
                        this.id.to_string().as_str(),
                    );
                    item_builder =
                        ItemBuilder::place(&mut item_builder, this.id.to_string().as_str());
                    registered_items_from_blocks
                        .lock()
                        .insert(Identifier::parse(item_id).unwrap(), item_builder);
                },
            );
        loading_engine.register_fn("register_event", move |event: &str, callback: FnPtr| {
            let mut registerd_events = registered_events.lock();
            registerd_events.register(
                Identifier::parse(event).unwrap(),
                ScriptCallback::new(callback),
            );
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
            .register_fn("register", move |this: &mut ItemBuilder, id: &str| {
                registered_items
                    .lock()
                    .insert(Identifier::parse(id).unwrap(), this.clone())
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
            .register_fn("register", move |this: &mut EntityBuilder, id: &str| {
                registered_entities
                    .lock()
                    .insert(Identifier::parse(id).unwrap(), this.clone())
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
        loading_engine.register_static_module("Face", exported_module!(FaceModule).into());
        loading_engine.register_static_module(
            "HorizontalFace",
            exported_module!(HorizontalFaceModule).into(),
        );

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
            ModImage::load(std::fs::read(full_path).unwrap(), path)
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

        Transformation::engine_register(&mut loading_engine);
        HorizontalFace::engine_register(&mut loading_engine);
        Identifier::engine_register(&mut loading_engine);

        let loading_engine = Arc::new(RwLock::new(loading_engine));

        let mut modules = Vec::new();

        for loaded_mod in &mods {
            {
                let mut path = current_mod_path.lock();
                path.clear();
                path.push(loaded_mod.1.path.clone());
            }
            let script_modules = loaded_mod.1.load_scripts(
                loaded_mod.0.as_str(),
                &loading_engine.read(),
                &mut errors,
            );
            for (module_id, module) in script_modules {
                let module = Arc::new(module);
                loading_engine
                    .write()
                    .register_static_module(module_id.as_str(), module.clone());
                modules.push((module_id, module));
            }
        }
        let events = (*events.lock()).clone();

        let call_events = events.clone();
        let call_engine = loading_engine.clone();
        loading_engine
            .write()
            .register_fn("call_event", move |id: &str, args: Dynamic| {
                call_events.call_event(Identifier::parse(id).unwrap(), args, &call_engine.read())
            });

        let _ = events.call_event(
            Identifier::new("bb", "init"),
            Dynamic::UNIT,
            &loading_engine.read(),
        );

        let blocks = blocks.lock().clone();
        let items = items.lock().clone();
        let entities = entities.lock().clone();
        let biomes = biomes
            .lock()
            .iter()
            .map(|biome| biome.lock().clone())
            .collect();
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
            modules,
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
                    Arc::new(Structure::from_json(json, block_registry)),
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
    pub fn load_gui_layouts(&self) -> HashMap<Identifier, Arc<GUILayout>> {
        let mut layouts = HashMap::new();
        for loaded_mod in &self.mods {
            let mut path = loaded_mod.1.path.clone();
            path.push("gui");
            for layout_path in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|entry| entry.metadata().unwrap().is_file())
            {
                let path_diff = pathdiff::diff_paths(layout_path.path(), &path).unwrap();
                let id = path_diff
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .split_once(".")
                    .unwrap()
                    .0;
                let id = Identifier::new(loaded_mod.1.namespace.clone(), id);
                let json = fs::read_to_string(layout_path.path()).unwrap();
                layouts.insert(id.clone(), Arc::new(GUILayout::load(json.as_str())));
            }
        }
        layouts
    }
    pub fn load_tags(&self) -> HashMap<Identifier, Arc<IdentifierTag>> {
        let mut tags = HashMap::new();
        for loaded_mod in &self.mods {
            let mut path = loaded_mod.1.path.clone();
            path.push("tags");
            for tag_path in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|entry| entry.metadata().unwrap().is_file())
            {
                let path_diff = pathdiff::diff_paths(tag_path.path(), &path).unwrap();
                let id = path_diff
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .split_once(".")
                    .unwrap()
                    .0;
                let id = Identifier::new(loaded_mod.1.namespace.clone(), id);
                let json = fs::read_to_string(tag_path.path()).unwrap();
                tags.insert(
                    id.clone(),
                    IdentifierTag::load(json::parse(json.as_str()).unwrap()),
                );
            }
        }
        tags
    }
    pub fn runtime_engine_load(engine: &mut Engine, server: Weak<Server>) {
        {
            let server = server.clone();
            engine.register_fn("Server", move || server.upgrade().unwrap());
        }
        {
            let server = server.clone();
            engine.register_fn("call_event", move |id: &str, event_data: Dynamic| {
                server
                    .upgrade()
                    .unwrap()
                    .call_event(Identifier::parse(id).unwrap(), event_data)
            });
        }
        engine.register_static_module("MovementType", exported_module!(MovementTypeModule).into());
        engine.register_static_module("Face", exported_module!(FaceModule).into());
        engine.register_static_module(
            "PositionAnchorModule",
            exported_module!(PositionAnchorModule).into(),
        );
        engine.register_static_module(
            "HorizontalFace",
            exported_module!(HorizontalFaceModule).into(),
        );
        engine.register_static_module(
            "InteractionResult",
            exported_module!(InteractionResultModule).into(),
        );
        engine.register_static_module("KeyboardKey", exported_module!(KeyboardKeyModule).into());

        Self::load_scripting_object::<PlayerData>(engine, &server);
        Self::load_scripting_object::<Entity>(engine, &server);
        Self::load_scripting_object::<WorldBlock>(engine, &server);
        Self::load_scripting_object::<World>(engine, &server);
        Self::load_scripting_object::<Location>(engine, &server);
        Self::load_scripting_object::<BlockLocation>(engine, &server);
        Self::load_scripting_object::<Position>(engine, &server);
        Self::load_scripting_object::<Identifier>(engine, &server);
        Self::load_scripting_object::<Structure>(engine, &server);
        Self::load_scripting_object::<BlockPosition>(engine, &server);
        Self::load_scripting_object::<BlockState>(engine, &server);
        Self::load_scripting_object::<Block>(engine, &server);
        Self::load_scripting_object::<UserDataWrapper>(engine, &server);
        Self::load_scripting_object::<InventoryWrapper>(engine, &server);
        Self::load_scripting_object::<Recipe>(engine, &server);
        Self::load_scripting_object::<ModGuiViewer>(engine, &server);
        Self::load_scripting_object::<Transformation>(engine, &server);
        Self::load_scripting_object::<Face>(engine, &server);
        Self::load_scripting_object::<HorizontalFace>(engine, &server);
        Self::load_scripting_object::<IdentifierTag>(engine, &server);
        Self::load_scripting_object::<ItemStack>(engine, &server);
        Self::load_scripting_object::<KeyboardKey>(engine, &server);
        Self::load_scripting_object::<Server>(engine, &server);
        Self::load_scripting_object::<OwnedInventoryView>(engine, &server);
    }
    fn load_scripting_object<T>(engine: &mut Engine, server: &Weak<Server>)
    where
        T: ScriptingObject,
    {
        T::engine_register_server(engine, server);
        T::engine_register(engine);
    }
}
pub trait ScriptingObject {
    #[allow(unused)]
    fn engine_register_server(engine: &mut Engine, server: &Weak<Server>) {}
    #[allow(unused)]
    fn engine_register(engine: &mut Engine) {}
}
impl ScriptingObject for Position {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<Position>("Position");
        engine.register_fn("Position", |x: f64, y: f64, z: f64| Position { x, y, z });
        engine.register_fn("+", |first: Position, second: Position| first + second);
        engine.register_fn("*", |first: Position, scalar: f64| first.multiply(scalar));
        engine.register_fn("distance", |first: &mut Position, other: Position| {
            first.distance(&other)
        });
        engine.register_get_set(
            "x",
            |position: &mut Position| position.x,
            |position: &mut Position, x: f64| position.x = x,
        );
        engine.register_get_set(
            "y",
            |position: &mut Position| position.y,
            |position: &mut Position, y: f64| position.y = y,
        );
        engine.register_get_set(
            "z",
            |position: &mut Position| position.z,
            |position: &mut Position, z: f64| position.z = z,
        );
        engine.register_fn("to_block_position", |position: &mut Position| {
            position.to_block_pos()
        });
        engine.register_fn("to_string", |position: &mut Position| position.to_string());
    }
}
impl ScriptingObject for BlockPosition {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<BlockPosition>("BlockPosition");
        engine.register_fn("BlockPosition", |x: i64, y: i64, z: i64| BlockPosition {
            x: x as i32,
            y: y as i32,
            z: z as i32,
        });
        engine.register_fn("+", |first: BlockPosition, second: BlockPosition| {
            first + second
        });
        engine.register_fn(
            "distance",
            |first: &mut BlockPosition, other: BlockPosition| first.distance(&other),
        );
        engine.register_get_set(
            "x",
            |position: &mut BlockPosition| position.x as i64,
            |position: &mut BlockPosition, x: i64| {
                position.x = x as i32;
            },
        );
        engine.register_get_set(
            "y",
            |position: &mut BlockPosition| position.y as i64,
            |position: &mut BlockPosition, y: i64| {
                position.y = y as i32;
            },
        );
        engine.register_get_set(
            "z",
            |position: &mut BlockPosition| position.z as i64,
            |position: &mut BlockPosition, z: i64| {
                position.z = z as i32;
            },
        );
        engine.register_fn(
            "offset_by_face",
            |position: &mut BlockPosition, face: Face| position.offset_by_face(face),
        );
        engine.register_fn("to_string", |position: &mut BlockPosition| {
            position.to_string()
        });
    }
}
impl ScriptingObject for Transformation {
    fn engine_register(engine: &mut Engine) {
        engine.register_type_with_name::<Transformation>("Transformation");
        engine.register_fn("transform_from_rotation", |x: f64, y: f64, z: f64| {
            Transformation {
                position: Vec3::ZERO,
                rotation: Vec3 {
                    x: x as f32,
                    y: y as f32,
                    z: z as f32,
                },
                scale: Vec3::ONE,
                origin: Vec3::ZERO,
            }
        });
        engine.register_fn("transform_rotation_from_face", |face: Face| {
            Transformation {
                position: Vec3::ZERO,
                rotation: match face {
                    Face::Front => Vec3::ZERO,
                    Face::Back => Vec3 {
                        x: 0.,
                        y: 180f32.to_radians(),
                        z: 0.,
                    },
                    Face::Up => Vec3 {
                        x: 0.,
                        y: 0.,
                        z: 90f32.to_radians(),
                    },
                    Face::Down => Vec3 {
                        x: 0.,
                        y: 0.,
                        z: 270f32.to_radians(),
                    },
                    Face::Left => Vec3 {
                        x: 0.,
                        y: 90f32.to_radians(),
                        z: 0.,
                    },
                    Face::Right => Vec3 {
                        x: 0.,
                        y: 270f32.to_radians(),
                        z: 0.,
                    },
                },
                scale: Vec3::ONE,
                origin: Vec3 {
                    x: 0.,
                    y: 0.5,
                    z: 0.,
                },
            }
        });
        engine.register_fn("transform_rotation_from_face_up", |face: Face| {
            Transformation {
                position: Vec3::ZERO,
                rotation: match face {
                    Face::Back => Vec3 {
                        x: 90f32.to_radians(),
                        y: 0.,
                        z: 0.,
                    },
                    Face::Front => Vec3 {
                        x: 270f32.to_radians(),
                        y: 0.,
                        z: 0.,
                    },
                    Face::Up => Vec3::ZERO,
                    Face::Down => Vec3 {
                        x: 180f32.to_radians(),
                        y: 0.,
                        z: 0.,
                    },
                    Face::Right => Vec3 {
                        x: 0.,
                        y: 0.,
                        z: 270f32.to_radians(),
                    },
                    Face::Left => Vec3 {
                        x: 0.,
                        y: 0.,
                        z: 90f32.to_radians(),
                    },
                },
                scale: Vec3::ONE,
                origin: Vec3 {
                    x: 0.,
                    y: 0.5,
                    z: 0.,
                },
            }
        });
    }
}
pub struct IdentifierTag {
    ids: HashSet<Identifier>,
}
impl IdentifierTag {
    pub fn load(json: JsonValue) -> Arc<Self> {
        Arc::new(IdentifierTag {
            ids: json
                .members()
                .into_iter()
                .map(|id| Identifier::parse(id.as_str().unwrap()).unwrap())
                .collect(),
        })
    }
    pub fn contains(&self, id: &Identifier) -> bool {
        self.ids.contains(id)
    }
    pub fn list(&self) -> Vec<Identifier> {
        self.ids.iter().cloned().collect()
    }
}
impl ScriptingObject for IdentifierTag {
    fn engine_register_server(engine: &mut Engine, server: &Weak<Server>) {
        {
            let server = server.clone();
            engine.register_fn("Tag", move |id: &str| {
                server
                    .upgrade()
                    .unwrap()
                    .tags
                    .get(&Identifier::parse(id).unwrap())
                    .map(|tag| Dynamic::from(tag.clone()))
                    .unwrap_or(Dynamic::UNIT)
            });
        }
        engine.register_fn("contains", |tag: &mut Arc<IdentifierTag>, id: &str| {
            tag.contains(&Identifier::parse(id).unwrap())
        });
        {
            let server = server.clone();
            engine.register_fn(
                "contains",
                move |tag: &mut Arc<IdentifierTag>, block: BlockStateRef| {
                    tag.contains(
                        &server
                            .upgrade()
                            .unwrap()
                            .block_registry
                            .state_by_ref(block)
                            .parent
                            .id,
                    )
                },
            );
        }
    }
}
impl ScriptingObject for KeyboardKey {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<KeyboardKey>("KeyboardKey");
        engine.register_fn("to_string", |key: &mut KeyboardKey| format!("{:?}", key));
        engine.register_fn("==", |first: KeyboardKey, second: KeyboardKey| {
            first == second
        });
    }
}
#[derive(Clone)]
pub enum UserDataWrapper {
    Player(Arc<PlayerData>),
    Entity(Arc<Entity>),
    Block(Arc<WorldBlock>),
    Inventory(InventoryWrapper),
    World(Arc<World>),
}
impl UserDataWrapper {
    fn get_user_data(&self) -> MutexGuard<UserData> {
        match self {
            UserDataWrapper::Player(player) => player.user_data.lock(),
            UserDataWrapper::Entity(entity) => entity.user_data.lock(),
            UserDataWrapper::Block(block) => block.user_data.lock(),
            UserDataWrapper::Inventory(inventory) => inventory.get_inventory().user_data.lock(),
            UserDataWrapper::World(world) => world.user_data.lock(),
        }
    }
}
impl ScriptingObject for UserDataWrapper {
    fn engine_register_server(engine: &mut Engine, _server: &Weak<Server>) {
        engine.register_type_with_name::<UserDataWrapper>("UserData");
        engine.register_indexer_get_set(
            |user_data: &mut UserDataWrapper, id: &str| {
                user_data
                    .get_user_data()
                    .get_data_point_ref(&Identifier::parse(id).unwrap())
                    .cloned()
                    .unwrap_or(Dynamic::UNIT)
            },
            |user_data: &mut UserDataWrapper, id: &str, value: Dynamic| {
                user_data
                    .get_user_data()
                    .put_data_point(&Identifier::parse(id).unwrap(), value);
            },
        );
    }
}
impl ScriptingObject for Face {
    fn engine_register(engine: &mut Engine) {
        engine.register_fn("to_horizontal_face", |this: Face| {
            this.to_horizontal_face()
                .map(|face| Dynamic::from(face))
                .unwrap_or(Dynamic::UNIT)
        });
    }
}
impl ScriptingObject for HorizontalFace {
    fn engine_register(engine: &mut Engine) {
        engine.register_fn("to_face", |this: HorizontalFace| this.to_face());
    }
}

#[derive(Clone)]
pub struct BiomeBuilder {
    pub id: Identifier,
    pub top_block: String,
    pub middle_block: String,
    pub bottom_block: String,
    pub water_block: String,
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
            top_block: top.to_string(),
            middle_block: middle.to_string(),
            bottom_block: bottom.to_string(),
            water_block: water.to_string(),
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
#[derive(Clone)]
pub struct RegisteredBlock {
    id: Identifier,
}
#[derive(Clone, Debug)]
pub struct BlockBuilder {
    pub client: ScriptCallback,
    pub data_container: Option<(u32,)>,
    pub properties: BlockStatePropertyStorage,
    pub on_tick: ScriptCallback,
    pub on_right_click: ScriptCallback,
    pub on_left_click: ScriptCallback,
    pub on_neighbor_update: ScriptCallback,
    pub on_place: ScriptCallback,
    pub on_destroy: ScriptCallback,
}

impl BlockBuilder {
    pub fn new(client: FnPtr) -> Self {
        BlockBuilder {
            client: ScriptCallback::new(client),
            data_container: None,
            properties: BlockStatePropertyStorage::new(),
            on_tick: ScriptCallback::empty(),
            on_right_click: ScriptCallback::empty(),
            on_left_click: ScriptCallback::empty(),
            on_neighbor_update: ScriptCallback::empty(),
            on_place: ScriptCallback::empty(),
            on_destroy: ScriptCallback::empty(),
        }
    }
    pub fn add_property_horizontal_face(&mut self, name: &str) -> Self {
        let mut this = self.clone();
        this.properties
            .register_property(name.to_string(), BlockStateProperty::HorizontalFace);
        this
    }
    pub fn add_property_face(&mut self, name: &str) -> Self {
        let mut this = self.clone();
        this.properties
            .register_property(name.to_string(), BlockStateProperty::Face);
        this
    }
    pub fn add_property_bool(&mut self, name: &str) -> Self {
        let mut this = self.clone();
        this.properties
            .register_property(name.to_string(), BlockStateProperty::Bool);
        this
    }
    pub fn add_property_number(&mut self, name: &str, range: RangeInclusive<i64>) -> Self {
        let mut this = self.clone();
        this.properties.register_property(
            name.to_string(),
            BlockStateProperty::Number((*range.start() as i32)..=(*range.end() as i32)),
        );
        this
    }
    pub fn on_tick(&mut self, ticker: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_tick = ScriptCallback::new(ticker);
        this
    }
    pub fn on_right_click(&mut self, click_action: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_right_click = ScriptCallback::new(click_action);
        this
    }
    pub fn on_left_click(&mut self, click_action: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_left_click = ScriptCallback::new(click_action);
        this
    }
    pub fn on_neighbor_update(&mut self, neighbor_update: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_neighbor_update = ScriptCallback::new(neighbor_update);
        this
    }
    pub fn on_place(&mut self, place_action: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_place = ScriptCallback::new(place_action);
        this
    }
    pub fn on_destroy(&mut self, destroy_action: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_destroy = ScriptCallback::new(destroy_action);
        this
    }
    pub fn mark_data_container(&mut self, inventory_size: i64) -> Self {
        let mut this = self.clone();
        this.data_container = Some((inventory_size as u32,));
        this
    }
}
#[derive(Clone)]
pub struct ModClientBlockData {
    pub client: ClientBlockData,
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
            front: ClientTexture::Static{id:front.to_string()},
            back: ClientTexture::Static{id:back.to_string()},
            right: ClientTexture::Static{id:right.to_string()},
            left: ClientTexture::Static{id:left.to_string()},
            up: ClientTexture::Static{id: up.to_string()},
            down: ClientTexture::Static{id:down.to_string()},
        }))
    }
    pub fn create_static(model: &str, texture: &str) -> Self {
        Self::new(ClientBlockRenderDataType::Static(
            ClientBlockStaticRenderData {
                models: vec![(
                    model.to_string(),
                    ClientTexture::Static{id:texture.to_string()},
                    Transformation::identity(),
                )],
            },
        ))
    }
    pub fn create_static_transform(model: &str, texture: &str, transform: Transformation) -> Self {
        Self::new(ClientBlockRenderDataType::Static(
            ClientBlockStaticRenderData {
                models: vec![(model.to_string(), ClientTexture::Static{id:texture.to_string()}, transform)],
            },
        ))
    }
    pub fn add_static_model(
        &mut self,
        model: &str,
        texture: &str,
        transform: Transformation,
    ) -> Self {
        match &mut self.client.block_type {
            ClientBlockRenderDataType::Static(models) => {
                models
                    .models
                    .push((model.to_string(), ClientTexture::Static{id:texture.to_string()}, transform));
            }
            _ => panic!(),
        }
        self.clone()
    }
    pub fn create_foliage(
        texture_1: &str,
        texture_2: &str,
        texture_3: &str,
        texture_4: &str,
    ) -> Self {
        Self::new(ClientBlockRenderDataType::Foliage(
            ClientBlockFoliageRenderData {
                texture_1: ClientTexture::Static{id:texture_1.to_string()},
                texture_2: ClientTexture::Static{id:texture_2.to_string()},
                texture_3: ClientTexture::Static{id:texture_3.to_string()},
                texture_4: ClientTexture::Static{id:texture_4.to_string()},
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
            },
        }
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
    pub fn dynamic(&mut self, model: &str, texture: &str) -> Self {
        self.client.dynamic = Some(ClientBlockDynamicData {
            model: model.to_string(),
            texture: ClientTexture::Static{id:texture.to_string()},
            animations: Vec::new(),
            items: Vec::new(),
        });
        self.clone()
    }
    pub fn dynamic_add_animation(&mut self, animation: &str) -> Self {
        if let Some(dynamic) = &mut self.client.dynamic {
            dynamic.animations.push(animation.to_string());
        }
        self.clone()
    }
    pub fn dynamic_add_item(&mut self, item: &str) -> Self {
        if let Some(dynamic) = &mut self.client.dynamic {
            dynamic.items.push(item.to_string());
        }
        self.clone()
    }
}
#[derive(Clone)]
pub struct ItemBuilder {
    pub client: ClientModItemData,
    pub place: Option<String>,
    pub on_right_click: Option<FnPtr>,
    pub stack_size: u32,
    pub tool: Option<ToolData>,
}

#[derive(Clone, Debug)]
pub struct ClientModItemData {
    pub name: Option<String>,
    pub model: ClientModItemModel,
}

#[derive(Clone, Debug)]
pub enum ClientModItemModel {
    Texture(String),
    Block(String),
}

impl ItemBuilder {
    pub fn new() -> Self {
        ItemBuilder {
            client: ClientModItemData {
                name: None,
                model: ClientModItemModel::Texture(String::new()),
            },
            place: None,
            on_right_click: None,
            stack_size: 20,
            tool: None,
        }
    }
    pub fn tool(&mut self, durability: i64, speed: f64, hardness: f64) -> Self {
        let mut this = self.clone();
        this.tool = Some(ToolData {
            durability: durability as u32,
            speed: speed as f32,
            hardness: hardness as f32,
            type_bitmap: 0u8,
        });
        this.stack_size = 1;
        this
    }
    pub fn tool_add_type(&mut self, tool_type: ToolType) -> Self {
        let mut this = self.clone();
        if let Some(tool) = &mut this.tool {
            tool.add_type(tool_type);
        }
        this
    }
    pub fn client_name(&mut self, name: &str) -> Self {
        let mut this = self.clone();
        this.client.name = Some(name.to_string());
        this
    }
    pub fn client_model_texture(&mut self, texture: &str) -> Self {
        let mut this = self.clone();
        this.client.model = ClientModItemModel::Texture(texture.to_string());
        this
    }
    pub fn client_model_block(&mut self, block: &str) -> Self {
        let mut this = self.clone();
        this.client.model = ClientModItemModel::Block(block.to_string());
        this
    }
    pub fn place(&mut self, place: &str) -> Self {
        let mut this = self.clone();
        this.place = Some(place.to_string());
        this
    }
    pub fn on_right_click(&mut self, callback: FnPtr) -> Self {
        let mut this = self.clone();
        this.on_right_click = Some(callback);
        this
    }
    pub fn stack_size(&mut self, stack_size: u32) -> Self {
        let mut this = self.clone();
        if this.tool.is_none() {
            this.stack_size = stack_size;
        } else {
            panic!("setting stack size of tool");
        }
        this
    }
}

#[derive(Clone)]
pub struct EntityBuilder {
    pub client: ClientEntityData,
    pub ticker: Option<FnPtr>,
}

impl EntityBuilder {
    pub fn new() -> Self {
        EntityBuilder {
            client: ClientEntityData {
                model: String::new(),
                texture: ClientTexture::Static{id:String::new()},
                hitbox_w: 1.,
                hitbox_h: 1.,
                hitbox_d: 1.,
                hitbox_h_shifting: 0.75,
                animations: Vec::new(),
                items: Vec::new(),
                viewmodel: None,
            },
            ticker: None,
        }
    }
    pub fn client_viewmodel(&mut self, model: &str, texture: &str) -> Self {
        let mut this = self.clone();
        this.client.viewmodel = Some((
            model.to_string(),
            ClientTexture::Static{id:texture.to_string()},
            Vec::new(),
            Vec::new(),
        ));
        this
    }
    pub fn client_viewmodel_add_animation(&mut self, animation: &str) -> Self {
        let mut this = self.clone();
        this.client
            .viewmodel
            .as_mut()
            .unwrap()
            .2
            .push(animation.to_string());
        this
    }
    pub fn client_viewmodel_add_item(&mut self, item: &str) -> Self {
        let mut this = self.clone();
        this.client
            .viewmodel
            .as_mut()
            .unwrap()
            .3
            .push(item.to_string());
        this
    }
    pub fn tick(&mut self, callback: FnPtr) -> Self {
        let mut this = self.clone();
        this.ticker = Some(callback);
        this
    }
    pub fn client_model(&mut self, model: &str, texture: &str) -> Self {
        let mut this = self.clone();
        this.client.model = model.to_string();
        this.client.texture = ClientTexture::Static{id:texture.to_string()};
        this
    }
    pub fn client_hitbox(&mut self, width: f64, height: f64, depth: f64) -> Self {
        let mut this = self.clone();
        this.client.hitbox_w = width;
        this.client.hitbox_h = height;
        this.client.hitbox_d = depth;
        this.client.hitbox_h_shifting = height * 0.75;
        this
    }
    pub fn client_add_animation(&mut self, animation: &str) -> Self {
        let mut this = self.clone();
        this.client.animations.push(animation.to_string());
        this
    }
    pub fn client_add_item(&mut self, item: &str) -> Self {
        let mut this = self.clone();
        this.client.items.push(item.to_string());
        this
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
    function: Option<FnPtr>,
}

impl ScriptCallback {
    const AST: OnceCell<AST> = OnceCell::new();
    pub fn new(function: FnPtr) -> Self {
        Self {
            function: Some(function),
        }
    }
    pub fn empty() -> Self {
        Self { function: None }
    }
    pub fn call_function(
        &self,
        engine: &Engine,
        this: Option<&mut Dynamic>,
        args: impl FuncArgs,
    ) -> Dynamic {
        if let Some(function) = &self.function {
            let mut arg_values = StaticVec::new_const();
            args.parse(&mut arg_values);
            let global = &mut GlobalRuntimeState::new(engine);
            let context = (engine, "", None, &*global, rhai::Position::NONE).into();
            match function.call_raw(&context, this, arg_values) {
                Ok(ret) => ret,
                Err(error) => {
                    println!("callback error: {error:#?}");
                    Dynamic::UNIT
                }
            }
        } else {
            Dynamic::UNIT
        }
    }
    pub fn call_action(
        &self,
        engine: &Engine,
        this: Option<&mut Dynamic>,
        args: impl FuncArgs,
    ) -> InteractionResult {
        self.call_function(engine, this, args)
            .try_cast::<InteractionResult>()
            .unwrap_or(InteractionResult::Ignored)
    }
    pub fn is_empty(&self) -> bool {
        self.function.is_none()
    }
}
#[derive(Clone)]
pub struct EventManager {
    events: HashMap<Identifier, Vec<ScriptCallback>>,
}
impl EventManager {
    pub fn new() -> Self {
        EventManager {
            events: HashMap::new(),
        }
    }
    pub fn call_event(&self, id: Identifier, mut event_data: Dynamic, engine: &Engine) -> Dynamic {
        if let Some(event_list) = self.events.get(&id) {
            for event in event_list {
                let _ = event.call_function(engine, Some(&mut event_data), ());
            }
        }
        event_data
    }
    pub fn register(&mut self, id: Identifier, callback: ScriptCallback) {
        self.events.entry(id).or_insert(Vec::new()).push(callback);
    }
}
#[export_module]
#[allow(non_snake_case)]
mod PositionAnchorModule {
    use block_byte_common::gui::PositionAnchor;

    #[allow(non_upper_case_globals)]
    pub const Top: PositionAnchor = PositionAnchor::Top;
    #[allow(non_upper_case_globals)]
    pub const Bottom: PositionAnchor = PositionAnchor::Bottom;
    #[allow(non_upper_case_globals)]
    pub const Left: PositionAnchor = PositionAnchor::Left;
    #[allow(non_upper_case_globals)]
    pub const Right: PositionAnchor = PositionAnchor::Right;
    #[allow(non_upper_case_globals)]
    pub const TopLeft: PositionAnchor = PositionAnchor::TopLeft;
    #[allow(non_upper_case_globals)]
    pub const TopRight: PositionAnchor = PositionAnchor::TopRight;
    #[allow(non_upper_case_globals)]
    pub const BottomLeft: PositionAnchor = PositionAnchor::BottomLeft;
    #[allow(non_upper_case_globals)]
    pub const BottomRight: PositionAnchor = PositionAnchor::BottomRight;
    #[allow(non_upper_case_globals)]
    pub const Center: PositionAnchor = PositionAnchor::Center;
    #[allow(non_upper_case_globals)]
    pub const Cursor: PositionAnchor = PositionAnchor::Cursor;
}
#[export_module]
#[allow(non_snake_case)]
mod FaceModule {
    #[allow(non_upper_case_globals)]
    pub const Front: Face = Face::Front;
    #[allow(non_upper_case_globals)]
    pub const Back: Face = Face::Back;
    #[allow(non_upper_case_globals)]
    pub const Left: Face = Face::Left;
    #[allow(non_upper_case_globals)]
    pub const Right: Face = Face::Right;
    #[allow(non_upper_case_globals)]
    pub const Up: Face = Face::Up;
    #[allow(non_upper_case_globals)]
    pub const Down: Face = Face::Down;
}
#[export_module]
#[allow(non_snake_case)]
mod HorizontalFaceModule {
    #[allow(non_upper_case_globals)]
    pub const Front: HorizontalFace = HorizontalFace::Front;
    #[allow(non_upper_case_globals)]
    pub const Back: HorizontalFace = HorizontalFace::Back;
    #[allow(non_upper_case_globals)]
    pub const Left: HorizontalFace = HorizontalFace::Left;
    #[allow(non_upper_case_globals)]
    pub const Right: HorizontalFace = HorizontalFace::Right;
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
mod InteractionResultModule {
    use crate::registry::InteractionResult;

    #[allow(non_upper_case_globals)]
    pub const Ignored: InteractionResult = InteractionResult::Ignored;
    #[allow(non_upper_case_globals)]
    pub const Consumed: InteractionResult = InteractionResult::Consumed;
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

#[export_module]
#[allow(non_snake_case)]
mod KeyboardKeyModule {
    use block_byte_common::KeyboardKey;

    #[allow(non_upper_case_globals)]
    pub const Tab: KeyboardKey = KeyboardKey::Tab;
    #[allow(non_upper_case_globals)]
    pub const C: KeyboardKey = KeyboardKey::C;
    #[allow(non_upper_case_globals)]
    pub const Escape: KeyboardKey = KeyboardKey::Escape;
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
    pub fn load(data: Vec<u8>, name: &str) -> ModImage {
        ModImage {
            image: Reader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .unwrap()
                .decode()
                .expect(format!("couldn't load {}", name).as_str())
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
