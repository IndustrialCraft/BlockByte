use anyhow::{anyhow, Context, Result};
use bbscript::eval::{ExecutionEnvironment, Function, ScopeStack, ScriptError, ScriptResult};
use bbscript::lex::FilePosition;
use bbscript::variant::{
    Array, FromVariant, FunctionType, FunctionVariant, IntoVariant, Map, Primitive, SharedArray,
    SharedMap, TypeName, Variant,
};
use block_byte_common::content::{
    ClientAnimatedTexture, ClientBlockData, ClientBlockRenderDataType, ClientModel, ClientTexture,
    Transformation,
};
use block_byte_common::gui::PositionAnchor;
use block_byte_common::messages::MovementType;
use block_byte_common::{
    BlockPosition, Color, Direction, Face, HorizontalFace, KeyboardKey, Position,
};
use hex_color::HexColor;
use image::codecs::pnm::GraymapHeader;
use image::io::Reader;
use image::{ImageOutputFormat, Rgba, RgbaImage};
use immutable_string::ImmutableString;
use json::{object, JsonValue};
use parking_lot::lock_api::RawMutex;
use parking_lot::{Mutex, MutexGuard};
use rand::{thread_rng, Rng};
use std::any::Any;
use std::collections::HashSet;
use std::fmt::Display;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};
use strum::IntoEnumIterator;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::inventory::{InventoryWrapper, ItemStack, ModGuiViewer, OwnedInventoryView};
use crate::registry::{BlockState, BlockStateRef, InteractionResult};
use crate::util::BlockLocation;
use crate::world::{BlockNetwork, PlayerData, UserData, World, WorldBlock};
use crate::{
    inventory::Recipe,
    util::{Identifier, Location},
    world::{Entity, Structure},
    Server,
};

#[derive(Clone)]
pub struct ClientContentData {
    pub images: HashMap<Identifier, Vec<u8>>,
    pub sounds: HashMap<Identifier, Vec<u8>>,
    pub models: HashMap<Identifier, Vec<u8>>,
}

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
        script_errors: &mut Vec<(String, ScriptError)>,
    ) -> Vec<(String, Function)> {
        let mut functions = Vec::new();
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
            let path = script.into_path();
            let module_name = path.canonicalize().unwrap().to_str().unwrap().to_string();
            let module_path =
                module_name.replace(scripts_path.canonicalize().unwrap().to_str().unwrap(), "");
            let module_name = module_path.replace("/", "::");
            let module_name = module_name.replace(".rhs", "");
            let module_name = format!("{}{}", id, module_name);
            for function in bbscript::parse_source_file(
                std::fs::read_to_string(path).unwrap().as_str(),
                Some(module_name.clone().into()),
                0,
            )
            .unwrap()
            {
                functions.push((format!("{}::{}", module_name, function.name), function));
            }
        }
        functions
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
                            self.namespace.as_str(),
                            pathdiff::diff_paths(file.path(), &path)
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .split_once(".")
                                .unwrap()
                                .0,
                        ),
                        if file.file_name().to_str().unwrap().ends_with(".json") {
                            let mut json =
                                json::parse(fs::read_to_string(file.path()).unwrap().as_str())
                                    .unwrap();
                            if json.remove("abstract").as_bool().unwrap_or(false) {
                                continue;
                            }
                            ContentType::Json(Self::recursively_load_json(
                                resource_type,
                                json,
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
        original_json: JsonValue,
        json_base_provider: &F,
    ) -> JsonValue {
        let mut json = object! {};
        let bases = if let Some(base) = original_json["base"].as_str() {
            vec![base.to_string()]
        } else {
            original_json["base"]
                .members()
                .map(|base| base.as_str().map(|str| str.to_string()))
                .collect::<Option<Vec<String>>>()
                .unwrap()
        };
        for base in bases.into_iter().rev() {
            let patch = Self::recursively_load_json(
                resource_type,
                json_base_provider(resource_type, Identifier::parse(base).unwrap()).unwrap(),
                json_base_provider,
            );
            json = patch_up_json(json, patch);
        }

        patch_up_json(json, original_json)
    }
    fn read_json_resource(&self, resource_type: &str, id: &str) -> Result<JsonValue> {
        let mut full_path = self.path.clone();
        full_path.push(resource_type);
        for path_part in id.split("/") {
            full_path.push(path_part);
        }
        fs::read_to_string(format!("{}.json", full_path.to_str().unwrap()))
            .with_context(|| format!("resource {} not found", id))
            .and_then(|data| json::parse(&data).map_err(|_| anyhow!("malformed json")))
    }
    fn read_image_resource(&self, id: &str) -> Result<ModImage> {
        let mut full_path = self.path.clone();
        full_path.push("images");
        for path_part in id.split("/") {
            full_path.push(path_part);
        }
        Ok(ModImage::load(
            fs::read(&format!("{}.png", full_path.to_str().unwrap())).unwrap(),
            full_path.to_str().unwrap(),
        ))
    }
}

pub struct ModManager {
    mods: HashMap<String, Mod>,
}

impl ModManager {
    pub fn load_mods(path: &Path) -> (Self, Vec<(String, ScriptError)>, ExecutionEnvironment) {
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

        let mut script_environment = ExecutionEnvironment::new();
        for (mod_id, loaded_mod) in &mods {
            let script_modules = loaded_mod.load_scripts(mod_id.as_str(), &mut errors);
            for (name, function) in script_modules {
                script_environment.register_global(
                    name,
                    FunctionVariant {
                        this: Variant::NULL(),
                        function: FunctionType::ScriptFunction(Arc::new(function)),
                    }
                    .into_variant(),
                );
            }
        }

        (ModManager { mods }, errors, script_environment)
    }
    pub fn load_resource_type<F: FnMut(Identifier, ContentType)>(
        &self,
        resource_type: &str,
        mut f: F,
    ) {
        for (_, loaded_mod) in &self.mods {
            for (id, content) in
                loaded_mod.load_content(resource_type, Self::create_json_base_provider(&self.mods))
            {
                f(id, content);
            }
        }
    }
    fn create_json_base_provider<'a>(
        mods: &'a HashMap<String, Mod>,
    ) -> impl Fn(&str, Identifier) -> Option<JsonValue> + 'a {
        move |resource_type, identifier| {
            mods.get(identifier.get_namespace()).and_then(|mod_data| {
                mod_data
                    .read_json_resource(resource_type, identifier.get_key())
                    .ok()
                    .map(|mut json| {
                        json.remove("abstract");
                        json
                    })
            })
        }
    }
    pub fn load_image(&self, id: Identifier) -> Result<ModImage> {
        self.mods
            .get(id.get_namespace())
            .ok_or(anyhow!("mod {} not found", id.get_namespace()))
            .and_then(|mod_data| mod_data.read_image_resource(id.get_key()))
    }
    pub fn init_engine_load(env: &mut ExecutionEnvironment) {
        bbscript::environment::register_defaults(env);
        Self::load_scripting_object::<ClientBlockData>(env);
    }
    pub fn runtime_engine_load(env: &mut ExecutionEnvironment, server: Weak<Server>) {
        {
            let server = server.clone();
            env.register_function(
                "call_event",
                move |id: &ImmutableString, event_data: &Variant| {
                    Ok(server
                        .upgrade()
                        .unwrap()
                        .call_event(Identifier::parse(id.as_ref()).unwrap(), event_data.clone()))
                },
            );
        }
        Self::load_enum::<MovementType>(env, "MovementType");
        Self::load_enum::<Face>(env, "Face");
        Self::load_enum::<PositionAnchor>(env, "PositionAnchor");
        Self::load_enum::<HorizontalFace>(env, "HorizontalFace");
        Self::load_enum::<InteractionResult>(env, "InteractionResult");
        Self::load_enum::<KeyboardKey>(env, "KeyboardKey");

        env.register_function("random_uuid", || {
            Ok(Variant::from_str(Uuid::new_v4().to_string().as_str()))
        });
        env.register_function("random_float", || Ok(thread_rng().gen_range((0.)..1.)));
        env.register_function("random_int", |range: &Range<i64>| {
            Ok(thread_rng().gen_range(range.clone()))
        });

        Self::load_scripting_object_server::<PlayerData>(env, &server);
        Self::load_scripting_object_server::<Entity>(env, &server);
        Self::load_scripting_object_server::<WorldBlock>(env, &server);
        Self::load_scripting_object_server::<World>(env, &server);
        Self::load_scripting_object_server::<Location>(env, &server);
        Self::load_scripting_object_server::<BlockLocation>(env, &server);
        Self::load_scripting_object_server::<Position>(env, &server);
        Self::load_scripting_object_server::<Structure>(env, &server);
        Self::load_scripting_object_server::<BlockPosition>(env, &server);
        Self::load_scripting_object_server::<BlockState>(env, &server);
        Self::load_scripting_object_server::<UserDataWrapper>(env, &server);
        Self::load_scripting_object_server::<InventoryWrapper>(env, &server);
        Self::load_scripting_object_server::<Recipe>(env, &server);
        Self::load_scripting_object_server::<ModGuiViewer>(env, &server);
        Self::load_scripting_object_server::<Transformation>(env, &server);
        Self::load_scripting_object_server::<Face>(env, &server);
        Self::load_scripting_object_server::<HorizontalFace>(env, &server);
        Self::load_scripting_object_server::<IdentifierTag>(env, &server);
        Self::load_scripting_object_server::<ItemStack>(env, &server);
        Self::load_scripting_object_server::<KeyboardKey>(env, &server);
        Self::load_scripting_object_server::<Server>(env, &server);
        Self::load_scripting_object_server::<OwnedInventoryView>(env, &server);
        Self::load_scripting_object_server::<BlockNetwork>(env, &server);
        Self::load_scripting_object_server::<Direction>(env, &server);
        Self::load_scripting_object_server::<ClientBlockData>(env, &server);
        {
            let server = server.clone();
            env.register_function(
                "transaction",
                Box::new(move |mut args: Vec<Variant>| {
                    do_transaction(
                        args.remove(0),
                        args,
                        &server.upgrade().unwrap().script_environment,
                    );
                    Ok(Variant::NULL())
                }) as Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync>,
            );
        }
    }
    fn load_scripting_object_server<T>(env: &mut ExecutionEnvironment, server: &Weak<Server>)
    where
        T: ScriptingObject,
    {
        T::engine_register_server(env, server);
    }
    fn load_scripting_object<T>(env: &mut ExecutionEnvironment)
    where
        T: ScriptingObject,
    {
        T::engine_register(env);
    }
    fn load_enum<T: Display + IntoEnumIterator + IntoVariant>(
        env: &mut ExecutionEnvironment,
        enum_name: &str,
    ) {
        for variant in T::iter() {
            env.register_global(
                format!("{}::{}", enum_name, variant.to_string()),
                variant.into_variant(),
            );
        }
    }
}
pub trait ScriptingObject {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {}
    fn engine_register(env: &mut ExecutionEnvironment) {}
}
impl ScriptingObject for Direction {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<Direction, _>("Direction");
        env.register_function("Direction", |pitch: &f64, yaw: &f64| {
            Ok(Direction {
                pitch: *pitch,
                yaw: *yaw,
            })
        });
        env.register_member("pitch", |direction: &Direction| Some(direction.pitch));
        env.register_member("yaw", |direction: &Direction| Some(direction.yaw));
        env.register_method("operator+", |first: &Direction, other: &Direction| {
            Ok(Direction {
                pitch: first.pitch + other.pitch,
                yaw: first.yaw + other.yaw,
            })
        });
    }
}
impl ScriptingObject for Position {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<Position, _>("Position");
        env.register_function("Position", |x: &f64, y: &f64, z: &f64| {
            Ok(Position {
                x: *x,
                y: *y,
                z: *z,
            })
        });
        env.register_method("operator+", |first: &Position, second: &Position| {
            Ok(*first + *second)
        });
        env.register_method("distance", |first: &Position, other: &Position| {
            Ok(first.distance(other))
        });
        env.register_member("x", |position: &Position| Some(position.x));
        env.register_member("y", |position: &Position| Some(position.y));
        env.register_member("z", |position: &Position| Some(position.z));
        env.register_method("to_block_position", |position: &Position| {
            Ok(position.to_block_pos())
        });
        env.register_method("to_string", |position: &Position| {
            Ok(Variant::from_str(position.to_string().as_str()))
        });
    }
}
impl ScriptingObject for BlockPosition {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<BlockPosition, _>("BlockPosition");
        env.register_function("BlockPosition", |x: &i64, y: &i64, z: &i64| {
            Ok(BlockPosition {
                x: *x as i32,
                y: *y as i32,
                z: *z as i32,
            })
        });
        env.register_method(
            "operator+",
            |first: &BlockPosition, second: &BlockPosition| Ok(*first + *second),
        );
        env.register_method(
            "distance",
            |first: &BlockPosition, other: &BlockPosition| Ok(first.distance(&other)),
        );
        env.register_member("x", |position: &BlockPosition| Some(position.x as i64));
        env.register_member("y", |position: &BlockPosition| Some(position.y as i64));
        env.register_member("z", |position: &BlockPosition| Some(position.z as i64));
        env.register_method("offset_by_face", |position: &BlockPosition, face: &Face| {
            Ok(position.offset_by_face(*face))
        });
        env.register_method("to_string", |position: &BlockPosition| {
            Ok(Variant::from_str(position.to_string().as_str()))
        });
    }
}
impl ScriptingObject for Transformation {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        /*engine.register_type_with_name::<Transformation>("Transformation");
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
        });*/
    }
}
impl ScriptingObject for ClientBlockData {
    fn engine_register(env: &mut ExecutionEnvironment) {
        env.register_function("ClientBlockData", |data: &Map| {
            let block_type = Map::from_variant(data.get("block_type").unwrap()).unwrap();
            let block_type = match ImmutableString::from_option_variant(block_type.get("type"))
                .unwrap()
                .as_ref()
            {
                "Air" => ClientBlockRenderDataType::Air,
                "Cube" => ClientBlockRenderDataType::Cube {
                    front: client_texture_from_variant(block_type.get("front").unwrap()),
                    back: client_texture_from_variant(block_type.get("back").unwrap()),
                    left: client_texture_from_variant(block_type.get("left").unwrap()),
                    right: client_texture_from_variant(block_type.get("right").unwrap()),
                    up: client_texture_from_variant(block_type.get("up").unwrap()),
                    down: client_texture_from_variant(block_type.get("down").unwrap()),
                },
                "Static" => ClientBlockRenderDataType::Static {
                    models: Array::from_variant(block_type.get("models").unwrap())
                        .unwrap()
                        .iter()
                        .map(|variant| {
                            let data = Map::from_variant(variant).unwrap();
                            ClientModel {
                                model: ImmutableString::from_variant(data.get("model").unwrap())
                                    .unwrap()
                                    .to_string(),
                                texture: client_texture_from_variant(data.get("texture").unwrap()),
                                transform: Transformation::identity(),
                            }
                        })
                        .collect(),
                },
                "Foliage" => ClientBlockRenderDataType::Foliage {
                    sides: block_type
                        .get("sides")
                        .map(|texture| client_texture_from_variant(texture)),
                    cross: block_type
                        .get("cross")
                        .map(|texture| client_texture_from_variant(texture)),
                    bottom: block_type
                        .get("bottom")
                        .map(|texture| client_texture_from_variant(texture)),
                },
                _ => panic!(),
            };
            Ok(ClientBlockData {
                block_type,
                dynamic: None,
                fluid: bool::from_option_variant(data.get("fluid"))
                    .cloned()
                    .unwrap_or(false),
                render_data: i64::from_option_variant(data.get("render_data"))
                    .cloned()
                    .unwrap_or(0) as u8,
                transparent: bool::from_option_variant(data.get("transparent"))
                    .cloned()
                    .unwrap_or(false),
                selectable: bool::from_option_variant(data.get("selectable"))
                    .cloned()
                    .unwrap_or(false),
                no_collide: bool::from_option_variant(data.get("no_collide"))
                    .cloned()
                    .unwrap_or(false),
            })
        });
    }
}
fn client_texture_from_variant(variant: &Variant) -> ClientTexture {
    if let Some(texture) = ImmutableString::from_variant(variant) {
        ClientTexture::String(texture.as_ref().to_string())
    } else {
        let data = Map::from_variant(variant).unwrap();
        ClientTexture::Struct(ClientAnimatedTexture {
            id: ImmutableString::from_option_variant(data.get("id"))
                .unwrap()
                .to_string(),
            time: u8::from_option_variant(data.get("time")).cloned().unwrap(),
            stages: u8::from_option_variant(data.get("stages"))
                .cloned()
                .unwrap(),
        })
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
    fn engine_register_server(env: &mut ExecutionEnvironment, server: &Weak<Server>) {
        {
            let server = server.clone();
            env.register_function("Tag", move |id: &ImmutableString| {
                Ok(Variant::from_option(
                    server
                        .upgrade()
                        .unwrap()
                        .tags
                        .get(&Identifier::parse(id.as_ref()).unwrap())
                        .cloned(),
                ))
            });
        }
        env.register_method(
            "contains",
            |tag: &Arc<IdentifierTag>, id: &ImmutableString| {
                Ok(tag.contains(&Identifier::parse(id.as_ref()).unwrap()))
            },
        );
        {
            let server = server.clone();
            env.register_method(
                "contains",
                move |tag: &Arc<IdentifierTag>, block: &BlockStateRef| {
                    Ok(tag.contains(
                        &server
                            .upgrade()
                            .unwrap()
                            .block_registry
                            .state_by_ref(*block)
                            .parent
                            .id,
                    ))
                },
            );
        }
    }
}
impl ScriptingObject for KeyboardKey {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<KeyboardKey, _>("KeyboardKey");
        env.register_method("to_string", |key: &KeyboardKey| {
            Ok(Variant::from_str(format!("{:?}", key).as_str()))
        });
        env.register_method("operator==", |first: &KeyboardKey, second: &KeyboardKey| {
            Ok(first == second)
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
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_custom_name::<UserDataWrapper, _>("UserData");
        env.register_method("get", |this: &UserDataWrapper, key: &ImmutableString| {
            Ok(Variant::from_option(
                this.get_user_data()
                    .0
                    .get(&Identifier::parse(key.clone()).unwrap())
                    .cloned(),
            ))
        });
        env.register_method(
            "set",
            |this: &UserDataWrapper, key: &ImmutableString, value: &Variant| {
                this.get_user_data()
                    .0
                    .insert(Identifier::parse(key.clone()).unwrap(), value.clone());
                Ok(())
            },
        );
        /*env.register_default_accessor::<UserDataWrapper, _>(|this, name| {
            UserDataWrapper::from_variant(this)
                .unwrap()
                .get_user_data()
                .0
                .get(name.as_ref())
                .cloned()
        });
        env.register_setter::<UserDataWrapper, _>(|this, name, value| {
            UserDataWrapper::from_variant(this)
                .unwrap()
                .get_user_data()
                .0
                .insert(name, value.clone());
        });*/
        /*env.register_indexer_get_set(
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
            env.register_fn(
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
        }*/
    }
}
impl ScriptingObject for Face {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_method("to_horizontal_face", |this: &Face| {
            Ok(Variant::from_option(this.to_horizontal_face()))
        });
    }
}
impl ScriptingObject for HorizontalFace {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_method("to_face", |this: &HorizontalFace| Ok(this.to_face()));
    }
}

#[derive(Clone)]
pub struct ScriptCallback {
    pub function: Option<Arc<Function>>,
}

impl ScriptCallback {
    pub fn new(function: Arc<Function>) -> Self {
        Self {
            function: Some(function),
        }
    }
    pub fn from_function_variant(function: &FunctionVariant) -> Self {
        match &function.function {
            FunctionType::ScriptFunction(function) => Self {
                function: Some(function.clone()),
            },
            FunctionType::RustFunction(_) => panic!(),
        }
    }
    pub fn empty() -> Self {
        Self { function: None }
    }
    pub fn call_function(
        &self,
        env: &ExecutionEnvironment,
        this: Option<Variant>,
        args: Vec<Variant>,
    ) -> ScriptResult {
        if let Some(function) = &self.function {
            let stack = ScopeStack::new();
            if let Some(this) = this {
                stack.set_variable_top("this".into(), this);
            }
            function.run(Some(&stack), args, env)
        } else {
            Ok(Variant::NULL())
        }
    }
    pub fn call_action(
        &self,
        env: &ExecutionEnvironment,
        this: Option<Variant>,
        args: Vec<Variant>,
    ) -> Result<InteractionResult, ScriptError> {
        let variant = self.call_function(env, this, args)?;
        Ok(InteractionResult::from_variant(&variant)
            .cloned()
            .unwrap_or(InteractionResult::Ignored))
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
    pub fn call_event(&self, id: Identifier, event_data: Variant, env: &ExecutionEnvironment) {
        if let Some(event_list) = self.events.get(&id) {
            for event in event_list {
                event
                    .call_function(env, Some(event_data.clone()), vec![])
                    .unwrap();
            }
        }
    }
    pub fn register(&mut self, id: Identifier, callback: ScriptCallback) {
        self.events.entry(id).or_insert(Vec::new()).push(callback);
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
    pub fn from_json<F: Fn(Identifier) -> ModImage>(json: JsonValue, loader: &F) -> ModImage {
        let image = json["image"].as_str().unwrap();
        let mut image = loader(Identifier::parse(image).unwrap());
        for overlay in json["overlays"].members() {
            image = image.overlay(&ModImage::from_json(overlay.clone(), loader));
        }

        let color = json["color"].as_str();
        if let Some(color) = color {
            let color = HexColor::parse(color).unwrap();
            image = image.color(Color {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            });
        }
        let mask = json["mask"].as_str();
        if let Some(mask) = mask {
            image = image.multiply(&loader(Identifier::parse(mask).unwrap()));
        }
        image
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
trait TransactionLock {
    fn commit(&self);
    fn cancel(&self);
}
#[derive(Clone)]
struct TransactionData {
    locks: Arc<(Vec<Arc<dyn TransactionLock + Send + Sync>>, AtomicBool)>,
}
impl ScriptingObject for TransactionData {
    fn engine_register_server(env: &mut ExecutionEnvironment, _server: &Weak<Server>) {
        env.register_method("commit", |this: &TransactionData| {
            match this
                .locks
                .1
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(_) => {
                    for lock in this.locks.0.iter() {
                        lock.commit();
                    }
                    Ok(())
                }
                Err(_) => Err(ScriptError::runtime("double commit", FilePosition::INVALID)),
            }
        });
    }
}
fn do_transaction(function: Variant, locks: Vec<Variant>, environment: &ExecutionEnvironment) {
    let mut args = Vec::new();
    let mut transaction_locks = Vec::new();
    for lock in locks {
        let lock = transaction_lock(lock).unwrap();
        args.push(lock.clone().into_variant());
        transaction_locks.push(lock);
    }
    let transaction_locks = Arc::new((transaction_locks, AtomicBool::new(false)));
    let function = function.function_set_this(
        TransactionData {
            locks: transaction_locks.clone(),
        }
        .into_variant(),
    );
    function
        .call(args, environment, &FilePosition::INVALID)
        .unwrap();
    if !transaction_locks.1.load(Ordering::Acquire) {
        for lock in transaction_locks.0.iter() {
            lock.cancel();
        }
    }
}
fn transaction_lock(variant: Variant) -> Option<Arc<dyn TransactionLock + Send + Sync>> {
    if let Some(map) = SharedMap::from_variant(&variant) {
        return Some(LockedSharedMap::lock(map));
    }
    None
}
struct LockedSharedMap {
    map: SharedMap,
    guard: Mutex<
        Option<
            TransactionMutex<
                Mutex<HashMap<ImmutableString, Variant>>,
                HashMap<ImmutableString, Variant>,
            >,
        >,
    >,
    local: Mutex<HashMap<ImmutableString, Variant>>,
}
impl LockedSharedMap {
    pub fn lock(ptr: &SharedMap) -> Arc<Self> {
        Arc::new(Self {
            map: ptr.clone(),
            guard: Mutex::new(Some(TransactionMutex::lock(ptr.clone()))),
            local: Mutex::new(HashMap::new()),
        })
    }
}
impl TransactionLock for LockedSharedMap {
    fn commit(&self) {
        let mut guard_lock = self.guard.lock();
        if guard_lock.is_some() {
            guard_lock
                .as_ref()
                .unwrap()
                .get()
                .extend(self.local.lock().drain());
            *guard_lock = None;
        }
    }
    fn cancel(&self) {
        *self.guard.lock() = None;
    }
}

struct TransactionMutex<A: TransactionLockable<T>, T> {
    ptr: Arc<A>,
    _pd: PhantomData<T>,
}
impl<A: TransactionLockable<T>, T> TransactionMutex<A, T> {
    pub fn lock(ptr: Arc<A>) -> TransactionMutex<A, T> {
        unsafe {
            ptr.get_lock().raw().lock();
        }
        TransactionMutex {
            ptr,
            _pd: PhantomData::default(),
        }
    }
    pub fn get(&self) -> &mut T {
        unsafe { &mut *self.ptr.get_lock().data_ptr() }
    }
}
impl<A: TransactionLockable<T>, T> Drop for TransactionMutex<A, T> {
    fn drop(&mut self) {
        unsafe {
            self.ptr.get_lock().raw().unlock();
        }
    }
}

trait TransactionLockable<T> {
    fn get_lock(&self) -> &Mutex<T>;
}
impl<T> TransactionLockable<T> for Mutex<T> {
    fn get_lock(&self) -> &Mutex<T> {
        self
    }
}

fn patch_up_json(base: JsonValue, patch: JsonValue) -> JsonValue {
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
pub fn json_to_variant(json: JsonValue, script_environment: &ExecutionEnvironment) -> Variant {
    if let Some(string) = json.as_str() {
        return if string.starts_with("!") {
            //engine.eval(&string[1..]).unwrap()
            FunctionVariant {
                function: FunctionType::ScriptFunction(Arc::new(
                    bbscript::parse_source_file(&string[1..], None, 0)
                        .expect(&string[1..])
                        .remove(0),
                )),
                this: Variant::NULL(),
            }
            .into_variant()
        } else if string.starts_with("@") {
            script_environment
                .get_global(&string[1..].into())
                .cloned()
                .unwrap()
        } else {
            Variant::from_str(string)
        };
    }
    match json {
        JsonValue::Null => Variant::NULL(),
        JsonValue::Number(number) => Into::<f64>::into(number).into_variant(),
        JsonValue::Boolean(bool) => bool.into_variant(),
        JsonValue::Object(object) => {
            let mut output: HashMap<ImmutableString, _> = HashMap::new();
            for (name, property) in object.iter() {
                output.insert(
                    name.into(),
                    json_to_variant(property.clone(), script_environment),
                );
            }
            Arc::new(output).into_variant()
        }
        JsonValue::Array(array) => array
            .into_iter()
            .map(|entry| json_to_variant(entry, script_environment))
            .collect::<Array>()
            .into_variant(),
        _ => unreachable!(),
    }
}
