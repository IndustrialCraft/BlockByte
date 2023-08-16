use std::sync::{atomic::AtomicI32, Arc, Weak};

use crossbeam_channel::*;
use rhai::Engine;

use crate::{mods::ModManager, Server};

pub struct ThreadPool {
    transmitter: Sender<Box<dyn FnOnce(&Engine) + Send>>,
    queued: Arc<AtomicI32>,
}
impl ThreadPool {
    pub fn new(workers: u32, server: Weak<Server>) -> Self {
        let (transmitter, receiver) = crossbeam_channel::unbounded();
        let queued = Arc::new(AtomicI32::new(0));
        for _ in 0..workers {
            Worker::spawn(receiver.clone(), queued.clone(), server.clone());
        }
        ThreadPool {
            transmitter,
            queued,
        }
    }
    pub fn execute(&self, job: Box<dyn FnOnce(&Engine) + Send>) {
        self.queued
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.transmitter.send(job).unwrap();
    }
    pub fn all_tasks_finished(&self) -> bool {
        self.queued.load(std::sync::atomic::Ordering::SeqCst) == 0
    }
}
struct Worker {
    receiver: Receiver<Box<dyn FnOnce(&Engine) + Send>>,
    queued: Arc<AtomicI32>,
    engine: Engine,
}
impl Worker {
    pub fn spawn(
        receiver: Receiver<Box<dyn FnOnce(&Engine) + Send>>,
        queued: Arc<AtomicI32>,
        server: Weak<Server>,
    ) {
        std::thread::spawn(move || {
            let mut worker = Worker {
                receiver,
                queued,
                engine: Engine::new(),
            };
            ModManager::runtime_engine_load(&mut worker.engine, server);
            while let Ok(job) = worker.receiver.recv() {
                job.call_once((&worker.engine,));
                worker
                    .queued
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
    }
}
