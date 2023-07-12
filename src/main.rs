#![feature(
    fn_traits,
    inline_const,
    hash_drain_filter,
    drain_filter,
    const_trait_impl
)]

mod inventory;
mod mods;
mod net;
mod registry;
mod util;
mod world;
mod worldgen;

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::Hash,
    net::TcpListener,
    path::Path,
    process,
    sync::{atomic::AtomicBool, Arc, Mutex, Weak},
    thread::{self, spawn},
    time::{Duration, Instant, SystemTime},
};

use crossbeam_channel::{Receiver, Sender};
use fxhash::FxHashMap;
use json::object;
use mods::{
    BlockRegistryWrapper, ClientContentDataWrapper, EntityRegistryWrapper, ItemRegistryWrapper,
    ModManager,
};
use net::PlayerConnection;
use registry::{BlockRegistry, EntityRegistry, ItemRegistry};
use threadpool::ThreadPool;
use util::{Identifier, Location, Position};
use world::{Entity, World};

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
    let server = Server::new(4321, "test server".to_string());
    let start_time = Instant::now();
    let mut tick_count: u32 = 0;
    println!("server started");
    while running.load(std::sync::atomic::Ordering::Relaxed) {
        server.tick();
        //println!("tick: {}", tick_count);
        while tick_count as u128 * 50 > Instant::now().duration_since(start_time).as_millis() {
            thread::sleep(Duration::from_millis(1));
        }
        tick_count += 1;
    }
    server.destroy();
}

pub struct Server {
    this: Weak<Server>,
    block_registry: BlockRegistry,
    item_registry: ItemRegistry,
    entity_registry: EntityRegistry,
    worlds: Mutex<FxHashMap<Arc<Identifier>, Arc<World>>>,
    new_players: Mutex<Receiver<PlayerConnection>>,
    mods: Mutex<ModManager>,
    motd: String,
    client_content: (Vec<u8>, String),
    thread_pool: Mutex<ThreadPool>,
    pub thread_pool_tasks: Sender<Box<dyn FnOnce() + Send>>,
    thread_pool_tasks_rc: Receiver<Box<dyn FnOnce() + Send>>,
}

impl Server {
    fn new(port: u16, motd: String) -> Arc<Server> {
        let mods = ModManager::load_mods(Path::new("mods"));
        let block_registry = RefCell::new(BlockRegistry::new());
        let item_registry = RefCell::new(ItemRegistry::new());
        let entity_registry = RefCell::new(EntityRegistry::new());
        mods.call_event(
            "blockRegistryInit",
            BlockRegistryWrapper {
                block_registry: &block_registry,
            },
        );
        mods.call_event(
            "itemRegistryInit",
            ItemRegistryWrapper {
                block_registry: &block_registry.borrow(),
                item_registry: &item_registry,
            },
        );
        mods.call_event(
            "entityRegistryInit",
            EntityRegistryWrapper {
                entity_registry: &entity_registry,
            },
        );
        let client_content = {
            let client_content = RefCell::new(mods::ClientContentData::new());
            mods.call_event(
                "clientContentInit",
                ClientContentDataWrapper {
                    client_content: &client_content,
                },
            );
            let client_content = registry::ClientContent::generate_zip(
                &block_registry.borrow(),
                &item_registry.borrow(),
                &entity_registry.borrow(),
                client_content.into_inner(),
            );
            let hash = sha256::digest(client_content.as_slice());
            (client_content, hash)
        };
        let (thread_pool_tasks, thread_pool_tasks_rc) = crossbeam_channel::unbounded();
        Arc::new_cyclic(|this| Server {
            this: this.clone(),
            new_players: Mutex::new(Server::create_listener_thread(this.clone(), port)),
            worlds: Mutex::new(FxHashMap::default()),
            block_registry: block_registry.into_inner(),
            item_registry: item_registry.into_inner(),
            entity_registry: entity_registry.into_inner(),
            mods: Mutex::new(mods),
            motd,
            client_content,
            thread_pool: Mutex::new(ThreadPool::with_name(
                "blockbyte_server_thread_pool".to_string(),
                4,
            )),
            thread_pool_tasks,
            thread_pool_tasks_rc,
        })
    }
    pub fn get_or_create_world(&self, identifier: Arc<Identifier>) -> Arc<World> {
        let mut worlds = self.worlds.lock().unwrap();
        if let Some(world) = worlds.get(&identifier) {
            return world.clone();
        }
        let world = World::new(self.this.upgrade().unwrap());
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
            Entity::new(
                &self.get_spawn_location(),
                self.entity_registry
                    .entity_by_identifier(&Identifier::new("test", "player"))
                    .unwrap()
                    .clone(),
                Some(connection),
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
            .drain_filter(|_, world| world.should_unload());
        while self.thread_pool_tasks_rc.len() > 0 {
            while let Ok(task) = self.thread_pool_tasks_rc.try_recv() {
                self.thread_pool
                    .lock()
                    .unwrap()
                    .execute(|| task.call_once(()));
            }
            self.thread_pool.lock().unwrap().join();
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
