use crate::{
    registry::{BlockRegistry, BlockStateRef},
    util::Identifier,
    world::{BlockData, Chunk, Structure},
};
use array_init::array_init;
use block_byte_common::BlockPosition;
use moka::sync::Cache;
use noise::{Fbm, NoiseFn, OpenSimplex};
use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use splines::{Key, Spline};
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
    land_height_spline: Spline<f64, f64>,
    land_small_terrain_spline: Spline<f64, f64>,
    small_terrain_noise: NoiseWithSize,
    small_terrain_spline: Spline<f64, f64>,
    mountain_noise: NoiseWithSize,
    mountain_spline: Spline<f64, f64>,
    land_mountain_spline: Spline<f64, f64>,
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
            land_height_spline: Spline::from_vec(vec![
                Key::new(-1., -100., splines::Interpolation::Linear),
                Key::new(-0.05, -30., splines::Interpolation::Linear),
                Key::new(-0.005, -10., splines::Interpolation::Linear),
                Key::new(0.005, 0., splines::Interpolation::Linear),
                Key::new(1., 0., splines::Interpolation::Linear),
            ]),
            land_small_terrain_spline: Spline::from_vec(vec![
                Key::new(-1., 1., splines::Interpolation::Linear),
                Key::new(-0.05, 1., splines::Interpolation::Linear),
                Key::new(-0.005, 0., splines::Interpolation::Linear),
                Key::new(0.005, 0., splines::Interpolation::Linear),
                Key::new(0.05, 1., splines::Interpolation::Linear),
                Key::new(1., 1., splines::Interpolation::Linear),
            ]),
            small_terrain_noise: NoiseWithSize::new((seed * 24245) as u32, 80.),
            small_terrain_spline: Spline::from_vec(vec![
                Key::new(-1., 0., splines::Interpolation::Linear),
                Key::new(-0.2, 4., splines::Interpolation::Linear),
                Key::new(1., 10., splines::Interpolation::Linear),
            ]),
            mountain_noise: NoiseWithSize::new((seed * 38468) as u32, 600.),
            mountain_spline: Spline::from_vec(vec![
                Key::new(0.0, 0., splines::Interpolation::Linear),
                Key::new(0.1, 25., splines::Interpolation::Linear),
                Key::new(0.4, 25., splines::Interpolation::Linear),
                Key::new(0.6, 100., splines::Interpolation::Linear),
                Key::new(1., 300., splines::Interpolation::Linear),
            ]),
            land_mountain_spline: Spline::from_vec(vec![
                Key::new(0.005, 0., splines::Interpolation::Linear),
                Key::new(0.01, 0.3, splines::Interpolation::Linear),
                Key::new(0.05, 1., splines::Interpolation::Linear),
                Key::new(1., 1., splines::Interpolation::Linear),
            ]),
            temperature_noise: NoiseWithSize::new((seed * 15618236) as u32, 1000.),
            moisture_noise: NoiseWithSize::new((seed * 7489223) as u32, 1000.),
            biomes,
            column_cache: ThreadLocal::new(),
            column_cache_common: Mutex::new(Cache::new(100000)),
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
        let temperature = self.temperature_noise.get(x, z);
        let moisture = self.moisture_noise.get(x, z);
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
    pub fn get_splined(&self, x: f64, z: f64, spline: &Spline<f64, f64>) -> f64 {
        spline
            .clamped_sample(self.noise.get([x / self.size, z / self.size]))
            .unwrap()
    }
}
#[derive(Clone)]
pub struct Biome {
    top_block: BlockStateRef,
    middle_block: BlockStateRef,
    bottom_block: BlockStateRef,
    water_block: BlockStateRef,
    land_noise_spline: Spline<f64, f64>,
    height_spline: Spline<f64, f64>,
    temperature_noise_spline: Spline<f64, f64>,
    moisture_noise_spline: Spline<f64, f64>,
    structures: Vec<(f32, Arc<Structure>)>,
}
impl Biome {
    pub fn new(
        block_registry: &BlockRegistry,
        top_block: Identifier,
        middle_block: Identifier,
        bottom_block: Identifier,
        water_block: Identifier,
        land_noise_spline: Spline<f64, f64>,
        height_spline: Spline<f64, f64>,
        temperature_noise_spline: Spline<f64, f64>,
        moisture_noise_spline: Spline<f64, f64>,
        structures: Vec<(f32, Arc<Structure>)>,
    ) -> Self {
        Biome {
            top_block: block_registry
                .block_by_identifier(&top_block)
                .unwrap()
                .get_default_state_ref(),
            middle_block: block_registry
                .block_by_identifier(&middle_block)
                .unwrap()
                .get_default_state_ref(),
            bottom_block: block_registry
                .block_by_identifier(&bottom_block)
                .unwrap()
                .get_default_state_ref(),
            water_block: block_registry
                .block_by_identifier(&water_block)
                .unwrap()
                .get_default_state_ref(),
            land_noise_spline,
            height_spline,
            temperature_noise_spline,
            moisture_noise_spline,
            structures,
        }
    }
    pub fn get_structures(&self) -> &Vec<(f32, Arc<Structure>)> {
        &self.structures
    }
    pub fn get_fitness(&self, land: f64, height: f64, temperature: f64, moisture: f64) -> f64 {
        let fitness = self.land_noise_spline.clamped_sample(land).unwrap_or(1.)
            * self.height_spline.clamped_sample(height).unwrap_or(1.)
            * self
                .temperature_noise_spline
                .clamped_sample(temperature)
                .unwrap_or(1.)
            * self
                .moisture_noise_spline
                .clamped_sample(moisture)
                .unwrap_or(1.);
        fitness
    }
}
