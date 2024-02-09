#![allow(dead_code)]
#![feature(
    fn_traits,
    inline_const,
    hash_extract_if,
    extract_if,
    const_trait_impl,
    trait_alias
)]
#![feature(let_chains)]

extern crate core;

mod inventory;
mod mods;
mod net;
mod registry;
mod threadpool;
mod util;
mod world;
mod worldgen;

use std::{
    collections::HashMap,
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process,
    sync::{atomic::AtomicBool, Arc, Weak},
    thread::{self, spawn},
    time::{Duration, Instant, SystemTime},
};

use crate::inventory::{GUILayout, Recipe};
use crate::mods::{
    ClientContentData, ContentType, EventManager, IdentifierTag, ScriptCallback, ScriptingObject,
};
use crate::registry::{BlockStateProperty, BlockStatePropertyStorage, RecipeManager, StaticData};
use crate::world::PlayerData;
use block_byte_common::content::{
    ClientBlockData, ClientEntityData, ClientItemData, ClientTexture,
};
use block_byte_common::Position;
use crossbeam_channel::Receiver;
use fxhash::FxHashMap;
use inventory::LootTable;
use json::{object, JsonValue};
use mods::ModManager;
use net::PlayerConnection;
use parking_lot::Mutex;
use registry::{
    Block, BlockRegistry, EntityRegistry, EntityType, Item, ItemModelMapping, ItemRegistry,
};
use rhai::{Dynamic, Engine};
use threadpool::ThreadPool;
use util::{Identifier, Location};
use world::{Entity, Structure, World};
use worldgen::{BasicWorldGenerator, Biome};

fn main() {
    let running = Arc::new(AtomicBool::new(true));
    {
        let ctrlc_running = running.clone();
        ctrlc::set_handler(move || {
            if !ctrlc_running.load(std::sync::atomic::Ordering::Relaxed) {
                process::exit(0);
            }
            ctrlc_running.store(false, std::sync::atomic::Ordering::Relaxed);
        })
        .unwrap();
    }
    {
        let server = Server::new(4321, {
            let mut save = std::env::current_dir().unwrap();
            save.push("save");
            std::fs::create_dir_all(&save).unwrap();
            save
        });
        let start_time = Instant::now();
        let mut tick_count: u32 = 0;
        println!("server started");
        let mut highest_sleep_time = 0;
        while running.load(std::sync::atomic::Ordering::Relaxed) {
            let mspt_timer = Instant::now();
            server.tick();
            if false {
                println!(
                    "mspt: {}",
                    Instant::now().duration_since(mspt_timer).as_micros() as f64 / 1000.
                );
            }
            let sleep_time = (tick_count as i64 * 50)
                - Instant::now().duration_since(start_time).as_millis() as i64;
            if sleep_time > 0 {
                thread::sleep(Duration::from_millis(sleep_time as u64));
            } else if sleep_time < 0 {
                if (-sleep_time) > highest_sleep_time {
                    println!("server is running {}ms behind", -sleep_time);
                }
                highest_sleep_time = -sleep_time;
            }
            server.wait_for_tasks();
            tick_count += 1;
        }
        println!("saving");
        server.destroy();
        server.wait_for_tasks();
        println!("server stopped");
    }
}

pub struct Server {
    this: Weak<Server>,
    block_registry: BlockRegistry,
    item_registry: ItemRegistry,
    entity_registry: EntityRegistry,
    worlds: Mutex<FxHashMap<Identifier, Arc<World>>>,
    new_players: Mutex<Receiver<PlayerConnection>>,
    mods: Mutex<ModManager>,
    client_content: (Vec<u8>, String),
    pub thread_pool: ThreadPool,
    world_generator_template: (Vec<Biome>,),
    structures: HashMap<Identifier, Arc<Structure>>,
    recipes: RecipeManager,
    events: EventManager,
    engine: Engine,
    save_directory: PathBuf,
    settings: ServerSettings,
    loot_tables: HashMap<Identifier, Arc<LootTable>>,
    players: Mutex<Vec<Arc<PlayerData>>>,
    gui_layouts: HashMap<Identifier, Arc<GUILayout>>,
    tags: HashMap<Identifier, Arc<IdentifierTag>>,
}

impl Server {
    fn new(port: u16, save_directory: PathBuf) -> Arc<Server> {
        let (mod_manager, errors, mut engine) = ModManager::load_mods(Path::new("mods"));
        for error in &errors {
            println!("script error at {}: {}", error.0, error.1.to_string());
        }
        if errors.len() > 0 {
            println!("server stopped because of mod errors");
            process::exit(0);
        }
        let mut block_registry = BlockRegistry::new();
        let mut item_registry = ItemRegistry::new();
        let mut entity_registry = EntityRegistry::new();
        let mut biomes = Vec::new();
        let mut structures = HashMap::new();
        let mut events = EventManager::new();
        let mut recipes = HashMap::new();
        let mut loot_tables = HashMap::new();
        let mut gui_layouts = HashMap::new();
        let mut tags = HashMap::new();
        mod_manager.load_resource_type("blocks", |id, content| match content {
            ContentType::Json(mut json) => {
                let properties = {
                    let mut properties = BlockStatePropertyStorage::new();
                    match json.remove("properties") {
                        JsonValue::Object(json_properties) => {
                            for (name, property) in json_properties.iter() {
                                let property = if let Some(string) = property.as_str() {
                                    if let Some((start, end)) = string.split_once("..=") {
                                        BlockStateProperty::Number(
                                            start.parse::<i32>().unwrap()
                                                ..=end.parse::<i32>().unwrap(),
                                        )
                                    } else {
                                        match string {
                                            "bool" => BlockStateProperty::Bool,
                                            "Face" => BlockStateProperty::Face,
                                            "HorizontalFace" => BlockStateProperty::HorizontalFace,
                                            _ => panic!(),
                                        }
                                    }
                                } else {
                                    if property.is_array() {
                                        BlockStateProperty::String(
                                            property
                                                .members()
                                                .map(|element| {
                                                    element.as_str().unwrap().to_string()
                                                })
                                                .collect(),
                                        )
                                    } else {
                                        panic!()
                                    }
                                };
                                properties.register_property(name.to_string(), property);
                            }
                        }
                        JsonValue::Null => {}
                        _ => panic!(),
                    }
                    properties
                };
                let client_block_data: ClientBlockData =
                    serde_json::from_str(json.remove("client").to_string().as_str()).unwrap();
                let static_data = StaticData {
                    data: {
                        mods::json_to_dynamic(json, &engine)
                            .try_cast::<rhai::Map>()
                            .unwrap()
                            .into_iter()
                            .map(|(name, value)| (name.to_string(), value))
                            .collect()
                    },
                };
                block_registry
                    .register(
                        id,
                        |default_state, id| {
                            Arc::new(Block {
                                id,
                                default_state,
                                data_container: None,
                                item_model_mapping: ItemModelMapping {
                                    mapping: HashMap::new(),
                                },
                                properties,
                                networks: HashMap::new(),
                                static_data,
                            })
                        },
                        |_id, _block| client_block_data.clone(), //todo: state function
                    )
                    .unwrap();
            }
            ContentType::Binary(_) => unimplemented!(),
        });
        mod_manager.load_resource_type("items", |id, content| match content {
            ContentType::Json(mut json) => {
                let stack_size = json.remove("stackSize").as_u32().unwrap_or(1);
                let client_data: ClientItemData =
                    serde_json::from_str(json.remove("client").to_string().as_str()).unwrap();
                let static_data = StaticData {
                    data: {
                        mods::json_to_dynamic(json, &engine)
                            .try_cast::<rhai::Map>()
                            .unwrap()
                            .into_iter()
                            .map(|(name, value)| (name.to_string(), value))
                            .collect()
                    },
                };
                item_registry
                    .register(id.clone(), move |client_id| {
                        Arc::new(Item {
                            id,
                            client_data,
                            client_id,
                            stack_size,
                            static_data,
                        })
                    })
                    .unwrap();
            }
            ContentType::Binary(_) => unimplemented!(),
        });
        mod_manager.load_resource_type("entities", |id, content| match content {
            ContentType::Json(mut json) => {
                let client_data: ClientEntityData =
                    serde_json::from_str(json.remove("client").to_string().as_str()).unwrap();
                let static_data = StaticData {
                    data: {
                        mods::json_to_dynamic(json, &engine)
                            .try_cast::<rhai::Map>()
                            .unwrap()
                            .into_iter()
                            .map(|(name, value)| (name.to_string(), value))
                            .collect()
                    },
                };
                entity_registry
                    .register(id.clone(), move |client_id| {
                        Arc::new(EntityType {
                            id,
                            client_id,
                            client_data,
                            item_model_mapping: ItemModelMapping {
                                mapping: HashMap::new(),
                            },
                            static_data,
                        })
                    })
                    .unwrap();
            }
            ContentType::Binary(_) => unimplemented!(),
        });
        entity_registry
            .register(Identifier::new("bb", "item"), |client_id| {
                Arc::new(EntityType {
                    id: Identifier::new("bb", "item"),
                    client_id,
                    client_data: ClientEntityData {
                        model: "bb:item".to_string(),
                        texture: ClientTexture::String("".to_string()),
                        hitbox_w: 0.5,
                        hitbox_h: 0.1,
                        hitbox_d: 0.5,
                        hitbox_h_shifting: 0.1,
                        animations: vec![],
                        items: vec!["main".to_string()],
                        viewmodel: None,
                    },
                    item_model_mapping: ItemModelMapping {
                        mapping: {
                            let mut mapping = HashMap::new();
                            mapping.insert(0, 0);
                            mapping
                        },
                    },
                    static_data: StaticData {
                        data: HashMap::new(),
                    },
                })
            })
            .unwrap();
        mod_manager.load_resource_type("structures", |id, content| match content {
            ContentType::Json(json) => {
                structures.insert(id, Arc::new(Structure::from_json(json, &block_registry)));
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("biomes", |id, content| match content {
            ContentType::Json(json) => {
                biomes.push(Biome::from_json(&json, &block_registry, &structures));
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("recipes", |id, content| match content {
            ContentType::Json(json) => {
                recipes.insert(
                    id.clone(),
                    Arc::new(Recipe::from_json(id, json, &item_registry)),
                );
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("loot_tables", |id, content| match content {
            ContentType::Json(json) => {
                loot_tables.insert(id, Arc::new(LootTable::from_json(json, &item_registry)));
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("gui", |id, content| match content {
            ContentType::Json(json) => {
                gui_layouts.insert(
                    id,
                    Arc::new(serde_json::from_str(json.to_string().as_str()).unwrap()),
                );
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("tags", |id, content| match content {
            ContentType::Json(json) => {
                tags.insert(id, IdentifierTag::load(json));
            }
            ContentType::Binary(_) => {}
        });
        mod_manager.load_resource_type("events", |_, content| match content {
            ContentType::Json(_) => {}
            ContentType::Binary(text) => {
                let text = String::from_utf8(text).unwrap();
                let (id, event) = text.split_once("\n").unwrap();
                events.register(
                    Identifier::parse(&id[1..]).unwrap(),
                    ScriptCallback::new(engine.eval(event).unwrap()),
                );
            }
        });
        let mut client_content_data = ClientContentData {
            images: HashMap::new(),
            sounds: HashMap::new(),
            models: HashMap::new(),
        };
        mod_manager.load_resource_type("images", |id, content| match content {
            ContentType::Json(_) => todo!(),
            ContentType::Binary(data) => {
                client_content_data.images.insert(id, data);
            }
        });
        mod_manager.load_resource_type("sounds", |id, content| match content {
            ContentType::Json(_) => todo!(),
            ContentType::Binary(data) => {
                client_content_data.sounds.insert(id, data);
            }
        });
        mod_manager.load_resource_type("models", |id, content| match content {
            ContentType::Json(_) => todo!(),
            ContentType::Binary(data) => {
                client_content_data.models.insert(id, data);
            }
        });
        client_content_data.models.insert(
            Identifier::new("bb", "item"),
            include_bytes!("assets/item_model.bbm").to_vec(),
        );
        let client_content = {
            let client_content = registry::ClientContentGenerator::generate_zip(
                &block_registry,
                &item_registry,
                &entity_registry,
                client_content_data,
            );
            let hash = sha256::digest(client_content.as_slice());
            (client_content, hash)
        };
        {
            let mut content = save_directory.clone();
            content.push("content.zip");
            fs::write(content, &client_content.0).unwrap();
        }
        Arc::new_cyclic(|this| Server {
            this: this.clone(),
            new_players: Mutex::new(Server::create_listener_thread(this.clone(), port)),
            worlds: Mutex::new(FxHashMap::default()),
            item_registry,
            entity_registry,
            mods: Mutex::new(mod_manager),
            client_content,
            thread_pool: ThreadPool::new(4),
            world_generator_template: (biomes,),
            block_registry,
            structures,
            recipes: RecipeManager::new(recipes),
            events,
            engine: {
                ModManager::runtime_engine_load(&mut engine, this.clone());
                engine
            },
            settings: {
                let path = {
                    let mut path = save_directory.clone();
                    path.push("settings.txt");
                    path
                };
                if path.exists() {
                    ServerSettings::load_from_string(fs::read_to_string(path).unwrap())
                } else {
                    ServerSettings::new()
                }
            },
            save_directory,
            loot_tables,
            players: Mutex::new(Vec::new()),
            gui_layouts,
            tags,
        })
    }
    pub fn export_file(&self, filename: String, data: Vec<u8>) {
        let path = {
            let mut path = self.save_directory.clone();
            path.push(filename);
            path
        };
        fs::write(path, data).unwrap();
    }
    pub fn get_or_create_world(&self, identifier: Identifier) -> Arc<World> {
        let mut worlds = self.worlds.lock();
        if let Some(world) = worlds.get(&identifier) {
            return world.clone();
        }
        let world = World::new(
            self.this.upgrade().unwrap(),
            Box::new(BasicWorldGenerator::new(
                1,
                self.world_generator_template.0.clone(),
            )),
            identifier.clone(),
        );
        worlds.insert(identifier, world.clone());
        world
    }
    pub fn get_world(&self, identifier: Arc<Identifier>) -> Option<Arc<World>> {
        let worlds = self.worlds.lock();
        worlds.get(&identifier).map(|world| world.clone())
    }
    pub fn get_spawn_location(&self) -> Location {
        Location {
            position: Position {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            world: self.get_or_create_world(Identifier::new("bb", "lobby")),
        }
    }
    pub fn call_event(&self, id: Identifier, event_data: Dynamic) -> Dynamic {
        self.events.call_event(id, event_data, &self.engine)
    }
    pub fn tick(&self) {
        while let Ok(connection) = self.new_players.lock().try_recv() {
            let player = {
                let mut event_data = rhai::Map::new();
                event_data.insert("location".into(), Dynamic::from(self.get_spawn_location()));
                let event_data = self.call_event(
                    Identifier::new("bb", "resolve_spawn_location"),
                    event_data.into(),
                );
                let event_data =
                    self.call_event(Identifier::new("bb", "player_join"), event_data.into());
                let mut event_data: rhai::Map = event_data.try_cast().unwrap();
                let entity = event_data
                    .remove("entity")
                    .and_then(|entity| entity.try_cast::<Arc<Entity>>())
                    .expect("player entity not specified");

                let player = PlayerData::new(connection, self.ptr(), entity);
                self.players.lock().push(player.clone());

                player
            };
            {
                let mut event_data = rhai::Map::new();
                event_data.insert("player".into(), Dynamic::from(player));
                let _ =
                    self.call_event(Identifier::new("bb", "post_player_join"), event_data.into());
            }
        }
        for player in &*self.players.lock() {
            player.tick();
        }
        for world in self.worlds.lock().values() {
            world.tick();
        }
        self.worlds
            .lock()
            .extract_if(|_, world| world.should_unload())
            .count();
        self.players
            .lock()
            .extract_if(|player| player.connection.lock().is_closed())
            .count();
    }
    pub fn wait_for_tasks(&self) {
        while !self.thread_pool.all_tasks_finished() {
            thread::yield_now();
        }
    }
    pub fn destroy(&self) {
        for world in self.worlds.lock().drain() {
            world.1.destroy();
        }
        std::fs::write(
            {
                let mut path = self.save_directory.clone();
                path.push("settings.txt");
                path
            },
            self.settings.save_to_string(),
        )
        .unwrap();
    }
    fn create_listener_thread(game_server: Weak<Server>, port: u16) -> Receiver<PlayerConnection> {
        let (tx, rx) = crossbeam_channel::unbounded();
        spawn(move || {
            let server = TcpListener::bind(("0.0.0.0", port)).unwrap();
            for stream in server.incoming() {
                if let Ok(stream) = stream {
                    let tx = tx.clone();
                    let server = game_server.upgrade().unwrap();
                    spawn(move || {
                        let websocket = tungstenite::accept(stream).unwrap();
                        let player_connection = PlayerConnection::new(websocket);
                        if let Ok(mut connection) = player_connection {
                            match connection.1 {
                                0 => tx.send(connection.0).unwrap(),
                                1 => {
                                    let json = object! {
                                        motd: server.settings.get("server.motd", "test server").clone(),
                                        time: SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string(),
                                        client_content_hash: server.client_content.1.clone()
                                    };
                                    connection.0.send_json(json);
                                }
                                2 => connection.0.send_binary(&server.client_content.0),
                                _ => {}
                            }
                        }
                    });
                }
            }
        });
        rx
    }
    pub fn ptr(&self) -> Arc<Server> {
        self.this.upgrade().unwrap()
    }
}
impl ScriptingObject for Server {
    fn engine_register_server(engine: &mut Engine, server: &Weak<Server>) {
        {
            let server = server.clone();
            engine.register_fn("list_items", move || {
                server
                    .upgrade()
                    .unwrap()
                    .item_registry
                    .list()
                    .map(|id| Dynamic::from(id.to_string()))
                    .collect::<rhai::Array>()
            });
        }
    }
}
pub struct ServerSettings {
    settings: Mutex<HashMap<String, String>>,
}
impl ServerSettings {
    pub fn new() -> Self {
        Self {
            settings: Mutex::new(HashMap::new()),
        }
    }
    pub fn load_from_string(input: String) -> Self {
        let mut settings = HashMap::new();
        for line in input.lines() {
            let (key, value) = line.split_once("=").unwrap();
            settings.insert(key.to_string(), value.to_string());
        }
        Self {
            settings: Mutex::new(settings),
        }
    }
    pub fn get(&self, key: &str, default: &str) -> String {
        let mut settings = self.settings.lock();
        settings
            .entry(key.to_string())
            .or_insert_with(|| default.to_string())
            .clone()
    }
    pub fn get_i64(&self, key: &str, default: i64) -> i64 {
        let mut settings = self.settings.lock();
        settings
            .entry(key.to_string())
            .or_insert_with(|| default.to_string())
            .parse()
            .unwrap_or(default)
    }
    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        let mut settings = self.settings.lock();
        settings
            .entry(key.to_string())
            .or_insert_with(|| default.to_string())
            .parse()
            .unwrap_or(default)
    }
    pub fn save_to_string(&self) -> String {
        let mut output = String::new();
        let settings = self.settings.lock();
        let mut settings: Vec<_> = settings.iter().collect();
        settings.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in settings {
            output.push_str(key);
            output.push('=');
            output.push_str(value);
            output.push('\n');
        }
        output
    }
}
