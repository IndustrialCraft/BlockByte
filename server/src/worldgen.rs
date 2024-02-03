use crate::util::Identifier;
use crate::{
    registry::{BlockRegistry, BlockStateRef},
    world::{BlockData, Chunk, Structure},
};
use array_init::array_init;
use block_byte_common::BlockPosition;
use json::JsonValue;
use moka::sync::Cache;
use noise::{Fbm, NoiseFn, OpenSimplex};
use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::sync::Arc;
use thread_local::ThreadLocal;

pub trait WorldGenerator {
    fn generate(&self, chunk: &Arc<Chunk>) -> [[[BlockData; 16]; 16]; 16];
}
pub struct FlatWorldGenerator {
    pub height: i32,
    pub simple_id: u32,
}
impl WorldGenerator for FlatWorldGenerator {
    fn generate(&self, chunk: &Arc<Chunk>) -> [[[BlockData; 16]; 16]; 16] {
        array_init(|_| {
            array_init(|i| {
                array_init(|_| {
                    BlockData::Simple(if i as i32 + chunk.position.y * 16 < self.height {
                        self.simple_id
                    } else {
                        0
                    })
                })
            })
        })
    }
}

pub struct BasicWorldGenerator {
    seed: u64,
    land_noise: NoiseWithSize,
    land_height_spline: Spline,
    land_small_terrain_spline: Spline,
    small_terrain_noise: NoiseWithSize,
    small_terrain_spline: Spline,
    mountain_noise: NoiseWithSize,
    mountain_spline: Spline,
    land_mountain_spline: Spline,
    temperature_noise: NoiseWithSize,
    moisture_noise: NoiseWithSize,
    biomes: Vec<Biome>,
    column_cache: ThreadLocal<Cache<(i32, i32), [[(i32, usize); 16]; 16]>>,
    column_cache_common: Mutex<Cache<(i32, i32), [[(i32, usize); 16]; 16]>>,
}
impl BasicWorldGenerator {
    pub fn new(seed: u64, biomes: Vec<Biome>) -> Self {
        Self {
            seed,
            land_noise: NoiseWithSize::new((seed * 4561561) as u32, 5000.),
            land_height_spline: Spline::new(vec![
                SplinePoint::new(-1., -100.),
                SplinePoint::new(-0.05, -30.),
                SplinePoint::new(-0.005, -10.),
                SplinePoint::new(0.005, 0.),
                SplinePoint::new(1., 0.),
            ]),
            land_small_terrain_spline: Spline::new(vec![
                SplinePoint::new(-1., 1.),
                SplinePoint::new(-0.05, 1.),
                SplinePoint::new(-0.005, 0.),
                SplinePoint::new(0.005, 0.),
                SplinePoint::new(0.05, 1.),
                SplinePoint::new(1., 1.),
            ]),
            small_terrain_noise: NoiseWithSize::new((seed * 24245) as u32, 80.),
            small_terrain_spline: Spline::new(vec![
                SplinePoint::new(-1., 0.),
                SplinePoint::new(-0.2, 4.),
                SplinePoint::new(1., 10.),
            ]),
            mountain_noise: NoiseWithSize::new((seed * 38468) as u32, 600.),
            mountain_spline: Spline::new(vec![
                SplinePoint::new(0.0, 0.),
                SplinePoint::new(0.1, 25.),
                SplinePoint::new(0.4, 25.),
                SplinePoint::new(0.6, 100.),
                SplinePoint::new(1., 300.),
            ]),
            land_mountain_spline: Spline::new(vec![
                SplinePoint::new(0.005, 0.),
                SplinePoint::new(0.01, 0.3),
                SplinePoint::new(0.05, 1.),
                SplinePoint::new(1., 1.),
            ]),
            temperature_noise: NoiseWithSize::new((seed * 15618236) as u32, 1000.),
            moisture_noise: NoiseWithSize::new((seed * 7489223) as u32, 1000.),
            biomes,
            column_cache: ThreadLocal::new(),
            column_cache_common: Mutex::new(Cache::new(2048)),
        }
    }
    pub fn get_terrain_height_at(&self, x: i32, z: i32) -> i32 {
        let x = x as f64;
        let z = z as f64;
        (self.land_noise.get_splined(x, z, &self.land_height_spline)
            + (self
                .land_noise
                .get_splined(x, z, &self.land_small_terrain_spline)
                * self
                    .small_terrain_noise
                    .get_splined(x, z, &self.small_terrain_spline))
            + (self
                .land_noise
                .get_splined(x, z, &self.land_mountain_spline)
                * self.mountain_noise.get_splined(x, z, &self.mountain_spline))) as i32
    }
    pub fn get_biome_at(&self, x: i32, z: i32, height: i32) -> usize {
        let height = height as f64;
        let x = x as f64;
        let z = z as f64;
        let land = self.land_noise.get(x, z);
        let temperature = (self.temperature_noise.get(x, z) + 1.) / 2.;
        let moisture = (self.moisture_noise.get(x, z) + 1.) / 2.;
        let biome = self
            .biomes
            .iter()
            .enumerate()
            .map(|(id, biome)| (id, biome.get_fitness(land, height, temperature, moisture)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        biome.0
    }
}
impl WorldGenerator for BasicWorldGenerator {
    fn generate(&self, chunk: &Arc<Chunk>) -> [[[BlockData; 16]; 16]; 16] {
        let position = chunk.position;
        let cache = self
            .column_cache
            .get_or(|| self.column_cache_common.lock().clone());

        let column_data: [[(i32, usize); 16]; 16] =
            cache.get_with((position.x, position.z), || {
                array_init(|x| {
                    array_init(|z| {
                        let total_x = (x as i32) + (position.x * 16);
                        let total_z = (z as i32) + (position.z * 16);
                        let terrain_height = self.get_terrain_height_at(total_x, total_z);
                        (
                            terrain_height,
                            self.get_biome_at(total_x, total_z, terrain_height),
                        )
                    })
                })
            });
        let mut structure_rng = rand::rngs::StdRng::seed_from_u64(
            41516516 * self.seed
                + (position.x * 41156) as u64
                + (position.y * 261265) as u64
                + (position.z * 156415) as u64,
        );
        array_init(|x| {
            array_init(|i| {
                array_init(|z| {
                    let y = i as i32 + position.y * 16;
                    let (height, biome) = column_data[x][z];
                    let biome = self.biomes.get(biome).unwrap();
                    if i == 0 {
                        for (chance, structure) in biome.get_structures() {
                            if height / 16 == position.y && structure_rng.gen_bool(*chance as f64) {
                                chunk.world.place_structure(
                                    BlockPosition {
                                        x: (x as i32) + (position.x * 16),
                                        y: height + 1,
                                        z: (z as i32) + (position.z * 16),
                                    },
                                    structure,
                                    false,
                                );
                            }
                        }
                    }
                    let block_position = BlockPosition {
                        x: (position.x * 16) + x as i32,
                        y,
                        z: (position.z * 16) + z as i32,
                    };
                    if y > height {
                        if y > 0 {
                            BlockData::Simple(0)
                        } else {
                            biome.water_block.create_block_data(chunk, block_position)
                        }
                    } else if y == height {
                        biome.top_block.create_block_data(chunk, block_position)
                    } else if y >= height - 4 {
                        biome.middle_block.create_block_data(chunk, block_position)
                    } else {
                        biome.bottom_block.create_block_data(chunk, block_position)
                    }

                    /*BlockData::Simple(if y <= heights[x][z] {
                        1
                    } else if y <= 0 {
                        2
                    } else {
                        0
                    })*/
                })
            })
        })
    }
}

struct NoiseWithSize {
    noise: Fbm<OpenSimplex>,
    size: f64,
}
impl NoiseWithSize {
    pub fn new(seed: u32, size: f64) -> Self {
        Self {
            noise: Fbm::new(seed),
            size,
        }
    }
    pub fn get(&self, x: f64, z: f64) -> f64 {
        self.noise.get([x / self.size, z / self.size])
    }
    pub fn get_splined(&self, x: f64, z: f64, spline: &Spline) -> f64 {
        spline
            .sample(self.noise.get([x / self.size, z / self.size]))
            .unwrap()
    }
}
#[derive(Clone)]
pub struct Biome {
    top_block: BlockStateRef,
    middle_block: BlockStateRef,
    bottom_block: BlockStateRef,
    water_block: BlockStateRef,
    land_noise_spline: Spline,
    height_spline: Spline,
    temperature_noise_spline: Spline,
    moisture_noise_spline: Spline,
    structures: Vec<(f64, Arc<Structure>)>,
}
impl Biome {
    pub fn from_json(
        json: &JsonValue,
        block_registry: &BlockRegistry,
        structures: &HashMap<Identifier, Arc<Structure>>,
    ) -> Self {
        Biome {
            top_block: block_registry
                .state_from_string(json["top"].as_str().unwrap())
                .unwrap(),
            middle_block: block_registry
                .state_from_string(json["middle"].as_str().unwrap())
                .unwrap(),
            bottom_block: block_registry
                .state_from_string(json["bottom"].as_str().unwrap())
                .unwrap(),
            water_block: block_registry
                .state_from_string(json["water"].as_str().unwrap())
                .unwrap(),
            land_noise_spline: Spline::from_json(&json["land"]),
            height_spline: Spline::from_json(&json["height"]),
            temperature_noise_spline: Spline::from_json(&json["temperature"]),
            moisture_noise_spline: Spline::from_json(&json["moisture"]),
            structures: json["structures"]
                .members()
                .map(|structure| {
                    (
                        structure["chance"].as_f64().unwrap(),
                        structures
                            .get(&Identifier::parse(structure["id"].as_str().unwrap()).unwrap())
                            .unwrap()
                            .clone(),
                    )
                })
                .collect(),
        }
    }
    pub fn get_structures(&self) -> &Vec<(f64, Arc<Structure>)> {
        &self.structures
    }
    pub fn get_fitness(&self, land: f64, height: f64, temperature: f64, moisture: f64) -> f64 {
        let fitness = self.land_noise_spline.sample(land).unwrap_or(1.)
            * self.height_spline.sample(height).unwrap_or(1.)
            * self
                .temperature_noise_spline
                .sample(temperature)
                .unwrap_or(1.)
            * self.moisture_noise_spline.sample(moisture).unwrap_or(1.);
        fitness
    }
}
#[derive(Copy, Clone)]
pub struct SplinePoint {
    key: f64,
    left: f64,
    right: f64,
}
impl SplinePoint {
    pub fn new(key: f64, value: f64) -> SplinePoint {
        SplinePoint {
            key,
            left: value,
            right: value,
        }
    }
}
#[derive(Clone)]
pub struct Spline {
    points: Vec<SplinePoint>,
}
impl Spline {
    pub fn new(mut points: Vec<SplinePoint>) -> Self {
        points.sort_by(|a, b| a.key.total_cmp(&b.key));
        Spline { points }
    }
    pub fn from_json(json: &JsonValue) -> Self {
        if json.is_null() {
            return Spline {
                points: vec![SplinePoint::new(0., 1.)],
            };
        }
        if let Some(value) = json.as_f64() {
            return Spline {
                points: vec![SplinePoint::new(0., value)],
            };
        }
        let mut points = Vec::new();
        for point in json.members() {
            let key = point["key"].as_f64().unwrap();
            if let Some(value) = point["value"].as_f64() {
                points.push(SplinePoint {
                    key,
                    left: value,
                    right: value,
                });
            } else {
                points.push(SplinePoint {
                    key,
                    left: point["left"].as_f64().unwrap(),
                    right: point["right"].as_f64().unwrap(),
                });
            }
        }
        Spline::new(points)
    }
    pub fn sample(&self, key: f64) -> Option<f64> {
        if self.points.len() == 0 {
            return None;
        }
        let mut first = None;
        let mut second = None;
        for point in &self.points {
            if point.key < key {
                first = Some(point);
            } else {
                second = Some(point);
                break;
            }
        }
        if first.is_none() {
            return Some(second.unwrap().left);
        }
        if second.is_none() {
            return Some(first.unwrap().right);
        }
        let first = first.unwrap();
        let second = second.unwrap();
        let lerp_val = (key - first.key) / (second.key - first.key);
        Some((first.right * (1. - lerp_val)) + (second.left * lerp_val))
    }
}
