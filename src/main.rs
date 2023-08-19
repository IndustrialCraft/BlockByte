#![allow(unused_variables)]
#![feature(fn_traits, inline_const, hash_extract_if, extract_if, const_trait_impl)]

mod inventory;
mod mods;
mod net;
mod registry;
mod threadpool;
mod util;
mod world;
mod worldgen;

use std::{
    cell::RefCell,
    collections::HashMap,
    net::TcpListener,
    path::{Path, PathBuf},
    process,
    sync::{atomic::AtomicBool, Arc, Mutex, Weak},
    thread::{self, spawn},
    time::{Duration, Instant, SystemTime},
};

use crossbeam_channel::Receiver;
use fxhash::FxHashMap;
use json::object;
use mods::{ModManager, ScriptCallback};
use net::PlayerConnection;
use registry::{Block, BlockRegistry, BlockState, EntityRegistry, EntityType, Item, ItemRegistry};
use rhai::{Engine, FuncArgs};
use splines::Spline;
use threadpool::ThreadPool;
use util::{Identifier, Location, Position};
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
        let server = Server::new(4321, "test server".to_string(), {
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
            server.tick();
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
    motd: String,
    client_content: (Vec<u8>, String),
    pub thread_pool: ThreadPool,
    world_generator_template: (Vec<Biome>,),
    structures: HashMap<Identifier, Arc<Structure>>,
    events: HashMap<Identifier, Vec<ScriptCallback>>,
    engine: Engine,
    save_directory: PathBuf,
}

impl Server {
    fn new(port: u16, motd: String, save_directory: PathBuf) -> Arc<Server> {
        let loaded_mods = ModManager::load_mods(Path::new("mods"));
        for error in &loaded_mods.7 {
            println!("script error from mod {}: {}", error.0, error.1.to_string());
        }
        if loaded_mods.7.len() > 0 {
            println!("server stopped because of mod errors");
            process::exit(0);
        }
        let block_registry = RefCell::new(BlockRegistry::new());
        let item_registry = RefCell::new(ItemRegistry::new());
        let entity_registry = RefCell::new(EntityRegistry::new());
        for block_data in loaded_mods.1 {
            block_registry
                .borrow_mut()
                .register(block_data.id.clone(), |id| {
                    let block = Arc::new(Block {
                        id: block_data.id,
                        default_state: id,
                    });
                    let state = vec![BlockState {
                        state_id: id,
                        client_data: block_data.client,
                        parent: block.clone(),
                    }];
                    (block, state)
                })
                .unwrap();
        }
        for item_data in loaded_mods.2 {
            item_registry
                .borrow_mut()
                .register(item_data.id, |id| {
                    Arc::new(Item {
                        id,
                        client_data: item_data.client,
                        place_block: item_data.place.map(|place| {
                            block_registry
                                .borrow()
                                .block_by_identifier(&place)
                                .unwrap()
                                .clone()
                        }),
                        on_right_click: item_data
                            .on_right_click
                            .map(|right_click| ScriptCallback::new(right_click)),
                    })
                })
                .unwrap();
        }
        for entity_data in loaded_mods.3 {
            entity_registry
                .borrow_mut()
                .register(entity_data.id, |id| {
                    Arc::new(EntityType {
                        id,
                        client_data: entity_data.client,
                        ticker: Mutex::new(
                            entity_data.ticker.map(|ticker| ScriptCallback::new(ticker)),
                        ),
                    })
                })
                .unwrap();
        }
        let client_content = {
            let client_content = registry::ClientContent::generate_zip(
                &block_registry.borrow(),
                &item_registry.borrow(),
                &entity_registry.borrow(),
                loaded_mods.4,
            );
            let hash = sha256::digest(client_content.as_slice());
            (client_content, hash)
        };
        let structures = loaded_mods.0.load_structures(&block_registry.borrow());
        let block_registry = block_registry.into_inner();
        Arc::new_cyclic(|this| Server {
            this: this.clone(),
            new_players: Mutex::new(Server::create_listener_thread(this.clone(), port)),
            worlds: Mutex::new(FxHashMap::default()),
            item_registry: item_registry.into_inner(),
            entity_registry: entity_registry.into_inner(),
            mods: Mutex::new(loaded_mods.0),
            motd,
            client_content,
            thread_pool: ThreadPool::new(4),
            world_generator_template: (loaded_mods
                .5
                .iter()
                .map(|biome_template| {
                    Biome::new(
                        &block_registry,
                        biome_template.top_block.clone(),
                        biome_template.middle_block.clone(),
                        biome_template.bottom_block.clone(),
                        biome_template.water_block.clone(),
                        Spline::from_vec(biome_template.spline_land.clone()),
                        Spline::from_vec(biome_template.spline_height.clone()),
                        Spline::from_vec(biome_template.spline_temperature.clone()),
                        Spline::from_vec(biome_template.spline_moisture.clone()),
                        biome_template
                            .structures
                            .iter()
                            .map(|(chance, id)| (*chance, structures.get(id).unwrap().clone()))
                            .collect(),
                    )
                })
                .collect(),),
            block_registry,
            structures,
            events: loaded_mods.6,
            engine: {
                let mut engine = Engine::new();
                ModManager::runtime_engine_load(&mut engine, this.clone());
                engine
            },
            save_directory,
        })
    }
    pub fn call_event(&self, engine: &Engine, event: Identifier, args: impl FuncArgs + Clone) {
        if let Some(event_list) = self.events.get(&event) {
            for event in event_list {
                event.call(engine, args.clone());
            }
        }
    }
    pub fn get_or_create_world(&self, identifier: Identifier) -> Arc<World> {
        let mut worlds = self.worlds.lock().unwrap();
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
        let worlds = self.worlds.lock().unwrap();
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
    pub fn tick(&self) {
        while let Ok(connection) = self.new_players.lock().unwrap().try_recv() {
            let player = Entity::new(
                &self.get_spawn_location(),
                self.entity_registry
                    .entity_by_identifier(&Identifier::new("bb", "player"))
                    .unwrap()
                    .clone(),
                Some(connection),
            );
            self.call_event(
                &self.engine,
                Identifier::new("bb", "player_join"),
                (player,),
            );
        }
        let worlds: Vec<Arc<World>> = self
            .worlds
            .lock()
            .unwrap()
            .values()
            .map(|w| w.clone())
            .collect();
        for world in worlds {
            world.tick();
        }
        self.worlds
            .lock()
            .unwrap()
            .extract_if(|_, world| world.should_unload())
            .count();
    }
    pub fn wait_for_tasks(&self) {
        while !self.thread_pool.all_tasks_finished() {
            thread::yield_now();
        }
    }
    pub fn destroy(&self) {
        for world in self.worlds.lock().unwrap().drain() {
            world.1.destroy();
        }
    }
    fn create_listener_thread(game_server: Weak<Server>, port: u16) -> Receiver<PlayerConnection> {
        let (tx, rx) = crossbeam_channel::unbounded();
        spawn(move || {
            let server = TcpListener::bind(("127.0.0.1", port)).unwrap();
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
                                        motd: server.motd.clone(),
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
}
