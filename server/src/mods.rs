use anyhow::{bail, Context, Result};
use bbscript::eval::{ExecutionEnvironment, Function, ScriptResult};
use bbscript::variant::{Primitive, Variant};
use block_byte_common::content::{
    ClientBlockCubeRenderData, ClientBlockData, ClientBlockDynamicData,
    ClientBlockFoliageRenderData, ClientBlockRenderDataType, ClientBlockStaticRenderData,
    ClientEntityData, ClientTexture, Transformation,
};
use block_byte_common::gui::PositionAnchor;
use block_byte_common::messages::MovementType;
use block_byte_common::{BlockPosition, Color, Face, HorizontalFace, KeyboardKey, Position, Vec3};
use image::io::Reader;
use image::{ImageOutputFormat, Rgba, RgbaImage};
use immutable_string::ImmutableString;
use json::JsonValue;
use parking_lot::{Mutex, MutexGuard, RwLock};
use splines::{Interpolation, Spline};
use std::collections::HashSet;
use std::fmt::Display;
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
        script_errors: &mut Vec<String>,
    ) -> Vec<(String, Vec<Function>)> {
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
            match bbscript::parse_source_file(
                std::fs::read_to_string(script.clone().into_path()).as_str(),
            ) {
                Ok(parsed) => {
                    let module_name = script
                        .into_path()
                        .canonicalize()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string();
                    let module_path = module_name
                        .replace(scripts_path.canonicalize().unwrap().to_str().unwrap(), "");
                    let module_name = module_path.replace("/", "::");
                    let module_name = module_name.replace(".bbs", "");
                    let module_name = format!("{}{}", id, module_name);
                    modules.push((module_name, parsed));
                }
                Err(error) => script_errors.push(error),
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
    pub fn load_mods(path: &Path) -> (Self, Vec<String>, Vec<(String, Function)>) {
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
        let mut modules = Vec::new();

        let mut functions = Vec::new();
        for loaded_mod in &mods {
            for script_modules in loaded_mod
                .1
                .load_scripts(loaded_mod.0.as_str(), &mut errors)
            {
                functions.push((
                    format!("{}::{}", loaded_mod.0, script_modules.0),
                    script_modules.1,
                ));
            }
        }
        (ModManager { mods }, errors, modules)
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
    pub fn runtime_engine_load(envirnoment: &mut ExecutionEnvironment, server: Weak<Server>) {
        Self::load_enum::<MovementType>(envirnoment, "MovementType");
        Self::load_enum::<Face>(envirnoment, "Face");
        Self::load_enum::<PositionAnchor>(envirnoment, "PositionAnchor");
        Self::load_enum::<HorizontalFace>(envirnoment, "HorizontalFace");
        Self::load_enum::<InteractionResult>(envirnoment, "InteractionResult");
        Self::load_enum::<KeyboardKey>(envirnoment, "KeyboardKey");

        envirnoment.register_method("random_uuid", || {
            Ok(Variant::Primitive(Box::new(ImmutableString::new(
                Uuid::new_v4().to_string(),
            ))))
        });

        Self::load_scripting_object::<PlayerData>(envirnoment, &server);
        Self::load_scripting_object::<Entity>(envirnoment, &server);
        Self::load_scripting_object::<WorldBlock>(envirnoment, &server);
        Self::load_scripting_object::<World>(envirnoment, &server);
        Self::load_scripting_object::<Location>(envirnoment, &server);
        Self::load_scripting_object::<BlockLocation>(envirnoment, &server);
        Self::load_scripting_object::<Position>(envirnoment, &server);
        Self::load_scripting_object::<Identifier>(envirnoment, &server);
        Self::load_scripting_object::<Structure>(envirnoment, &server);
        Self::load_scripting_object::<BlockPosition>(envirnoment, &server);
        Self::load_scripting_object::<BlockState>(envirnoment, &server);
        Self::load_scripting_object::<Block>(envirnoment, &server);
        Self::load_scripting_object::<UserDataWrapper>(envirnoment, &server);
        Self::load_scripting_object::<InventoryWrapper>(envirnoment, &server);
        Self::load_scripting_object::<Recipe>(envirnoment, &server);
        Self::load_scripting_object::<ModGuiViewer>(envirnoment, &server);
        Self::load_scripting_object::<Transformation>(envirnoment, &server);
        Self::load_scripting_object::<Face>(envirnoment, &server);
        Self::load_scripting_object::<HorizontalFace>(envirnoment, &server);
        Self::load_scripting_object::<IdentifierTag>(envirnoment, &server);
        Self::load_scripting_object::<ItemStack>(envirnoment, &server);
        Self::load_scripting_object::<KeyboardKey>(envirnoment, &server);
        Self::load_scripting_object::<Server>(envirnoment, &server);
        Self::load_scripting_object::<OwnedInventoryView>(envirnoment, &server);
        Self::load_scripting_object::<BlockNetwork>(envirnoment, &server);
    }
    fn load_scripting_object<T>(envirnoment: &mut ExecutionEnvironment, server: &Weak<Server>)
    where
        T: ScriptingObject,
    {
        T::engine_register_server(envirnoment, server);
        T::engine_register(envirnoment);
    }
    fn load_enum<T>(environment: &mut ExecutionEnvironment, name: &str)
    where
        T: ScriptingEnum,
    {
    }
}
pub trait ScriptingEnum {
    fn list_variants();
}
pub trait ScriptingObject {
    fn engine_register(environment: &mut ExecutionEnvironment, _server: &Weak<Server>);

    fn register_to_string<T: Display + Primitive>(environment: &mut ExecutionEnvironment) {
        environment.register_method("to_string", |this: &T, _| {
            ImmutableString::new(this.to_string())
        });
    }
}
impl ScriptingObject for Position {
    fn engine_register(environment: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        environment.register_function("Position", |args| {
            let (x, y, z): (&f64, &f64, &f64) =
                bbscript::variant::convert_variant_list_3(&args[..])?;
            Ok(Variant::new_primitive(Position {
                x: *x,
                y: *y,
                z: *z,
            }))
        });
        environment.register_member("x", |position: &Position| {
            Some(Variant::new_primitive(position.x))
        });
        environment.register_member("y", |position: &Position| {
            Some(Variant::new_primitive(position.y))
        });
        environment.register_member("z", |position: &Position| {
            Some(Variant::new_primitive(position.z))
        });
        environment.register_method("distance", |position: &Position, args| {
            let (other,): (&Position,) = bbscript::variant::convert_variant_list_1(&args[..])?;
            Ok(Variant::new_primitive(position.distance(other)))
        });
        environment.register_method("to_block_pos", |position: &Position, _| {
            Ok(Variant::new_primitive(position.to_block_pos()))
        });
        Self::register_to_string::<Position>(environment);
    }
}
impl ScriptingObject for BlockPosition {
    fn engine_register(environment: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        environment.register_function("Position", |args| {
            let (x, y, z): (&i64, &i64, &i64) =
                bbscript::variant::convert_variant_list_3(&args[..])?;
            Ok(Variant::new_primitive(BlockPosition {
                x: *x as i32,
                y: *y as i32,
                z: *z as i32,
            }))
        });
        environment.register_member("x", |position: &BlockPosition| {
            Some(Variant::new_primitive(position.x))
        });
        environment.register_member("y", |position: &BlockPosition| {
            Some(Variant::new_primitive(position.y))
        });
        environment.register_member("z", |position: &BlockPosition| {
            Some(Variant::new_primitive(position.z))
        });
        environment.register_method("distance", |position: &BlockPosition, args| {
            let (other,): (&BlockPosition,) = bbscript::variant::convert_variant_list_1(&args[..])?;
            Ok(Variant::new_primitive(position.distance(other)))
        });
        environment.register_method("offset_by_face", |position: &BlockPosition, args| {
            let (face,): (&Face,) = bbscript::variant::convert_variant_list_1(&args[..])?;
            Ok(Variant::new_primitive(position.offset_by_face(*face)))
        });
        Self::register_to_string::<BlockPosition>(environment);
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
                    .get_data_point_ref(id)
                    .cloned()
                    .unwrap_or(Dynamic::UNIT)
            },
            |user_data: &mut UserDataWrapper, id: &str, value: Dynamic| {
                user_data.get_user_data().put_data_point(id, value);
            },
        );
        {
            let server = server.clone();
            engine.register_fn(
                "modify",
                move |user_data: &mut UserDataWrapper, id: &str, callback: FnPtr| {
                    let mut user_data = user_data.get_user_data();
                    let mut data = user_data.take_data_point(id).unwrap_or(Dynamic::UNIT);
                    let return_value = ScriptCallback::new(callback).call_function(
                        &server.upgrade().unwrap().engine,
                        Some(&mut data),
                        (),
                    );
                    user_data.put_data_point(id, data);
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
#[derive(Clone, Debug)]
pub struct BlockBuilder {
    pub client: ScriptCallback,
    pub data_container: Option<(u32,)>,
    pub properties: BlockStatePropertyStorage,
    pub networks: HashMap<Identifier, ScriptCallback>,
    pub on_tick: ScriptCallback,
    pub on_right_click: ScriptCallback,
    pub on_left_click: ScriptCallback,
    pub on_neighbor_update: ScriptCallback,
    pub on_place: ScriptCallback,
    pub on_destroy: ScriptCallback,
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
#[derive(Clone)]
pub struct EntityBuilder {
    pub client: ClientEntityData,
    pub ticker: Option<FnPtr>,
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
