use anyhow::{anyhow, bail, Context, Result};
use block_byte_common::content::{
    ClientBlockCubeRenderData, ClientBlockData, ClientBlockDynamicData,
    ClientBlockFoliageRenderData, ClientBlockRenderDataType, ClientBlockStaticRenderData,
    ClientEntityData, ClientTexture, Transformation,
};
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
use std::fs::FileType;
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
use uuid::Uuid;
use walkdir::WalkDir;

use crate::inventory::{GUILayout, InventoryWrapper, ItemStack, ModGuiViewer, OwnedInventoryView};
use crate::registry::{
    Block, BlockState, BlockStateProperty, BlockStatePropertyStorage, BlockStateRef,
    InteractionResult,
};
use crate::util::BlockLocation;
use crate::world::{BlockNetwork, PlayerData, UserData, World, WorldBlock};
use crate::{
    inventory::{LootTable, Recipe},
    registry::{BlockRegistry, ItemRegistry, ToolData, ToolType},
    util::{Identifier, Location},
    world::{Entity, Structure},
    Server,
};

pub enum ContentType {
    Json(JsonValue),
    Binary(Vec<u8>),
}

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
    pub fn load_content<F: Fn(&str, Identifier) -> Option<JsonValue>>(
        &self,
        resource_type: &str,
        json_base_provider: F,
    ) -> HashMap<Identifier, ContentType> {
        let mut content = HashMap::new();
        let path = {
            let mut path = self.path.clone();
            path.push(resource_type);
            path
        };
        for file in WalkDir::new(&path) {
            if let Ok(file) = file {
                if file.file_type().is_file() {
                    content.insert(
                        Identifier::new(
                            &self.namespace,
                            pathdiff::diff_paths(file.path(), &path)
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .split_once(".")
                                .unwrap()
                                .0,
                        ),
                        if file.file_name().to_str().unwrap().ends_with(".json") {
                            ContentType::Json(Self::recursively_load_json(
                                resource_type,
                                json::parse(fs::read_to_string(file.path()).unwrap().as_str())
                                    .unwrap(),
                                &json_base_provider,
                            ))
                        } else {
                            ContentType::Binary(fs::read(file.path()).unwrap())
                        },
                    );
                }
            }
        }
        content
    }
    fn recursively_load_json<F: Fn(&str, Identifier) -> Option<JsonValue>>(
        resource_type: &str,
        json: JsonValue,
        json_base_provider: &F,
    ) -> JsonValue {
        if let Some(base) = json["base"].as_str() {
            let base_json = Self::recursively_load_json(
                resource_type,
                json_base_provider(resource_type, Identifier::parse(base).unwrap()).unwrap(),
                json_base_provider,
            );
            patch_up_json(base_json, json)
        } else {
            json
        }
    }
    /*fn read_json_resource(resource_type: &str, id: Identifier) -> Result<JsonValue> {
            let mut full_path = self.path.clone();
            full_path.push(resource_type);
            for path_part in id.get_key().split("/") {
                full_path.push(path_part);
            }
            fs::read_to_string(full_path)
                .with_context(|| format!("resource {} not found", id))
                .and_then(|data| json::parse(&data).map_err(|_| anyhow!("malformed json")))
    }*/
}

pub struct ModManager {
    mods: HashMap<String, Mod>,
}

impl ModManager {
    pub fn load_mods(
        path: &Path,
    ) -> (
        Self,
        Vec<(String, Box<EvalAltResult>)>,
        Engine,
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
        let current_mod_path = Arc::new(Mutex::new(PathBuf::new()));

        let mut modules = Vec::new();

        let mut engine = Engine::new();
        for loaded_mod in &mods {
            {
                let mut path = current_mod_path.lock();
                path.clear();
                path.push(loaded_mod.1.path.clone());
            }
            let script_modules =
                loaded_mod
                    .1
                    .load_scripts(loaded_mod.0.as_str(), &engine, &mut errors);
            for (module_id, module) in script_modules {
                let module = Arc::new(module);
                engine.register_static_module(module_id.as_str(), module.clone());
                modules.push((module_id, module));
            }
        }

        (ModManager { mods }, errors, engine, modules)
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

        engine.register_fn("random_uuid", || Uuid::new_v4().to_string());

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
        Self::load_scripting_object::<BlockNetwork>(engine, &server);
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
    BlockNetwork(Arc<BlockNetwork>),
}
impl UserDataWrapper {
    fn get_user_data(&self) -> MutexGuard<UserData> {
        match self {
            UserDataWrapper::Player(player) => player.user_data.lock(),
            UserDataWrapper::Entity(entity) => entity.user_data.lock(),
            UserDataWrapper::Block(block) => block.user_data.lock(),
            UserDataWrapper::Inventory(inventory) => inventory.get_inventory().user_data.lock(),
            UserDataWrapper::World(world) => world.user_data.lock(),
            UserDataWrapper::BlockNetwork(network) => network.user_data.lock(),
        }
    }
}
impl ScriptingObject for UserDataWrapper {
    fn engine_register_server(engine: &mut Engine, server: &Weak<Server>) {
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
        {
            let server = server.clone();
            engine.register_fn(
                "modify",
                move |user_data: &mut UserDataWrapper, id: &str, callback: FnPtr| {
                    let mut user_data = user_data.get_user_data();
                    let id = Identifier::parse(id).unwrap();
                    let mut data = user_data.take_data_point(&id).unwrap_or(Dynamic::UNIT);
                    let return_value = ScriptCallback::new(callback).call_function(
                        &server.upgrade().unwrap().engine,
                        Some(&mut data),
                        (),
                    );
                    user_data.put_data_point(&id, data);
                    return_value
                },
            );
        }
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

#[derive(Clone, Debug)]
pub struct ScriptCallback {
    pub(crate) function: Option<FnPtr>,
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
fn patch_up_json(mut base: JsonValue, patch: JsonValue) -> JsonValue {
    match (base, patch) {
        (JsonValue::Object(mut base), JsonValue::Object(patch)) => {
            for (name, property) in patch.iter() {
                base.insert(
                    name,
                    if let Some(base) = base.get(name).cloned() {
                        patch_up_json(base, property.clone())
                    } else {
                        property.clone()
                    },
                );
            }
            JsonValue::Object(base)
        }
        (_base, patch) => patch,
    }
}
fn json_to_dynamic(json: JsonValue, engine: &Engine) -> Dynamic {
    use std::str::FromStr;
    if let Some(string) = json.as_str() {
        return if string.starts_with("!") {
            engine.eval(&string[1..]).unwrap()
        } else {
            Dynamic::from_str(string).unwrap()
        };
    }
    match json {
        JsonValue::Null => Dynamic::UNIT,
        JsonValue::Number(number) => Dynamic::from_float(number.into()),
        JsonValue::Boolean(bool) => Dynamic::from_bool(bool),
        JsonValue::Object(object) => {
            let mut output = rhai::Map::new();
            for (name, property) in object.iter() {
                output.insert(name.into(), json_to_dynamic(property.clone(), engine));
            }
            Dynamic::from_map(output)
        }
        JsonValue::Array(array) => {
            let mut output = rhai::Array::new();
            for property in array.into_iter() {
                output.push(json_to_dynamic(property, engine));
            }
            Dynamic::from_array(output)
        }
        _ => unreachable!(),
    }
}
