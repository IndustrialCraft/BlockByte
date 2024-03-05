use crate::util::Identifier;
use crate::{
    registry::{BlockRegistry, BlockStateRef},
    world::{BlockData, Chunk, Structure},
};
use array_init::array_init;
use block_byte_common::BlockPosition;
use json::JsonValue;
use moka::sync::Cache;
use noise::{Fbm, NoiseFn, OpenSimplex, Seedable};
use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::sync::Arc;
use thread_local::ThreadLocal;

pub struct WorldGeneratorType {
    land: NoiseConfig,
    terrain: NoiseConfig,
    temperature: NoiseConfig,
    moisture: NoiseConfig,
    biomes: Vec<Biome>,
}
impl WorldGeneratorType {
    pub fn new(biomes: Vec<Biome>) -> Arc<WorldGeneratorType> {
        Arc::new(Self {
            land: NoiseConfig::new(4561561, 5000., Spline::new(vec![])),
            terrain: NoiseConfig::new(
                24245,
                500.,
                Spline::new(vec![SplinePoint::new(-1., 0.), SplinePoint::new(1., 100.)]),
            ),
            temperature: NoiseConfig::new(
                15618236,
                1000.,
                Spline::new(vec![SplinePoint::new(-1., 0.), SplinePoint::new(1., 100.)]),
            ),
            moisture: NoiseConfig::new(
                7489223,
                1000.,
                Spline::new(vec![SplinePoint::new(-1., 0.), SplinePoint::new(1., 100.)]),
            ),
            biomes,
        })
    }
    /*pub fn from_json(json: JsonValue) -> Arc<WorldGeneratorType> {
        WorldGeneratorType::new(json["biomes"].members().map(|biome| biome.as_str()))
    }*/
}

pub struct WorldGenerator {
    seed: u64,
    generator_type: Arc<WorldGeneratorType>,
    column_cache: ThreadLocal<Cache<(i32, i32), [[(i32, usize); 16]; 16]>>,
    column_cache_common: Mutex<Cache<(i32, i32), [[(i32, usize); 16]; 16]>>,
    land: NoiseProvider,
    terrain: NoiseProvider,
    temperature: NoiseProvider,
    moisture: NoiseProvider,
}
impl WorldGenerator {
    pub fn new(seed: u64, generator_type: Arc<WorldGeneratorType>) -> Self {
        Self {
            seed,
            column_cache: ThreadLocal::new(),
            column_cache_common: Mutex::new(Cache::new(2048)),
            land: generator_type.land.instantiate(seed as u32),
            terrain: generator_type.terrain.instantiate(seed as u32),
            temperature: generator_type.temperature.instantiate(seed as u32),
            moisture: generator_type.moisture.instantiate(seed as u32),
            generator_type,
        }
    }
    pub fn get_terrain_height_at(&self, x: i32, z: i32) -> i32 {
        let x = x as f64;
        let z = z as f64;
        self.terrain.get(x, z) as i32
    }
    pub fn get_biome_at(&self, x: i32, z: i32, height: i32) -> usize {
        let height = height as f64;
        let x = x as f64;
        let z = z as f64;
        let land = self.land.get(x, z);
        let temperature = self.temperature.get(x, z);
        let moisture = self.moisture.get(x, z);
        let biome = self
            .generator_type
            .biomes
            .iter()
            .enumerate()
            .map(|(id, biome)| (id, biome.get_fitness(land, height, temperature, moisture)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        biome.0
    }
    pub fn generate(&self, chunk: &Arc<Chunk>) -> [[[BlockData; 16]; 16]; 16] {
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
                    let biome = self.generator_type.biomes.get(biome).unwrap();
                    if i == 0 {
                        for (chance, structure) in biome.get_structures() {
                            if height / 16 == position.y && structure_rng.gen_bool(*chance) {
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
struct NoiseConfig {
    spline: Spline,
    seed_shift: u32,
    size: f64,
}
impl NoiseConfig {
    pub fn new(seed_shift: u32, size: f64, spline: Spline) -> NoiseConfig {
        Self {
            seed_shift,
            size,
            spline,
        }
    }
    pub fn instantiate(&self, seed: u32) -> NoiseProvider {
        NoiseProvider {
            size: self.size,
            spline: self.spline.clone(),
            noise: Fbm::new(seed ^ self.seed_shift),
        }
    }
}
struct NoiseProvider {
    noise: Fbm<OpenSimplex>,
    size: f64,
    spline: Spline,
}
impl NoiseProvider {
    pub fn get(&self, x: f64, z: f64) -> f64 {
        let noise = self.noise.get([x / self.size, z / self.size]);
        self.spline.sample(noise).unwrap_or(noise)
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
