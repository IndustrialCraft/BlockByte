#![feature(fn_traits, inline_const, hash_drain_filter, drain_filter)]

mod net;
mod registry;
mod util;
mod world;

use std::{
    collections::HashMap,
    net::TcpListener,
    process,
    sync::{
        atomic::AtomicBool,
        mpsc::{channel, Receiver},
        Arc, Mutex, Weak,
    },
    thread::{self, spawn},
    time::{Duration, Instant},
};

use net::PlayerConnection;
use registry::BlockRegistry;
use util::Identifier;
use world::World;

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
    let server = Server::new(4321);
    let start_time = Instant::now();
    let mut tick_count: u32 = 0;
    while running.load(std::sync::atomic::Ordering::Relaxed) {
        server.tick();
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
    worlds: Mutex<HashMap<Arc<Identifier>, Arc<World>>>,
    new_players: Receiver<Mutex<PlayerConnection>>,
}
impl Server {
    fn new(port: u16) -> Arc<Server> {
        let new_players = Server::create_listener_thread(port);
        Arc::new_cyclic(|this| Server {
            this: this.clone(),
            new_players,
            worlds: Mutex::new(HashMap::new()),
            block_registry: BlockRegistry::new(),
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
    pub fn tick(&self) {
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
    }
    pub fn destroy(&self) {
        for world in self.worlds.lock().unwrap().drain() {
            world.1.destroy();
        }
    }
    fn create_listener_thread(port: u16) -> Receiver<Mutex<PlayerConnection>> {
        let (tx, rx) = channel();
        spawn(move || {
            let server = TcpListener::bind(("127.0.0.1", port)).unwrap();
            for stream in server.incoming() {
                if let Ok(stream) = stream {
                    let tx = tx.clone();
                    spawn(move || {
                        let mut websocket = tungstenite::accept(stream).unwrap();
                        //todo: username and key message
                        websocket.get_mut().set_nonblocking(true).unwrap();
                        tx.send(PlayerConnection::new(websocket)).unwrap();
                    });
                }
            }
        });
        rx
    }
}
