use array_init::array_init;
use noise::{Fbm, NoiseFn, OpenSimplex};
use splines::{Key, Spline};

use crate::{util::ChunkPosition, world::BlockData};

pub trait WorldGenerator {
    fn generate(&self, position: ChunkPosition) -> [[[BlockData; 16]; 16]; 16];
}
pub struct FlatWorldGenerator {
    pub height: i32,
    pub simple_id: u32,
}
impl WorldGenerator for FlatWorldGenerator {
    fn generate(&self, position: ChunkPosition) -> [[[BlockData; 16]; 16]; 16] {
        array_init(|_| {
            array_init(|i| {
                array_init(|_| {
                    BlockData::Simple(if i as i32 + position.y * 16 < self.height {
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
    land_noise: NoiseWithSize,
    land_height_spline: Spline<f64, f64>,
    land_small_terrain_spline: Spline<f64, f64>,
    small_terrain_noise: NoiseWithSize,
    small_terrain_spline: Spline<f64, f64>,
    mountain_noise: NoiseWithSize,
    mountain_spline: Spline<f64, f64>,
    land_mountain_spline: Spline<f64, f64>,
}
impl BasicWorldGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
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
}
impl WorldGenerator for BasicWorldGenerator {
    fn generate(&self, position: ChunkPosition) -> [[[BlockData; 16]; 16]; 16] {
        let heights: [[i32; 16]; 16] = array_init(|x| {
            array_init(|z| {
                self.get_terrain_height_at(
                    (x as i32) + (position.x * 16),
                    (z as i32) + (position.z * 16),
                )
            })
        });
        array_init(|x| {
            array_init(|i| {
                array_init(|z| {
                    let y = i as i32 + position.y * 16;
                    BlockData::Simple(if y <= heights[x][z] {
                        1
                    } else if y <= 0 {
                        2
                    } else {
                        0
                    })
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
