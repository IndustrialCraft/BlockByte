use crate::content::BlockRenderDataType::Cube;
use crate::content::{BlockRegistry, BlockRenderDataType};
use crate::render::{FaceVerticesExtension, Vertex};
use block_byte_common::{BlockPosition, ChunkPosition, Face, Position};
use cgmath::{ElementWise, InnerSpace, Matrix4, Point3, Vector3};
use std::collections::HashMap;
use std::rc::Rc;
use wgpu::util::DeviceExt;
use wgpu::{Buffer, BufferSlice, Device, Queue};
use winit::event::VirtualKeyCode;

pub struct ClientPlayer {
    pub position: Point3<f32>,
    pub velocity: Vector3<f32>,
    pub pitch_deg: f32,
    pub yaw_deg: f32,
    shifting: bool,
    shifting_animation: f32,
    pub last_moved: bool,
    pub speed: f32,
}
impl ClientPlayer {
    const UP: Vector3<f32> = Vector3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub fn is_shifting(&self) -> bool {
        self.shifting
    }
    pub fn make_front(&self) -> Vector3<f32> {
        let pitch_rad = f32::to_radians(self.pitch_deg);
        let yaw_rad = f32::to_radians(self.yaw_deg);
        Vector3 {
            x: yaw_rad.sin() * pitch_rad.cos(),
            y: pitch_rad.sin(),
            z: yaw_rad.cos() * pitch_rad.cos(),
        }
    }
    pub fn update_orientation(&mut self, d_pitch_deg: f32, d_yaw_deg: f32) {
        self.pitch_deg = (self.pitch_deg + d_pitch_deg).max(-89.0).min(89.0);
        self.yaw_deg = (self.yaw_deg + d_yaw_deg) % 360.0;
    }
    pub fn knockback(&mut self, x: f32, y: f32, z: f32, set: bool) {
        if set {
            self.velocity = Vector3::new(0., 0., 0.);
        }
        self.velocity += Vector3::new(x, y, z);
    }
    pub fn update_position(
        &mut self,
        keys: &std::collections::HashSet<VirtualKeyCode>,
        delta_time: f32,
    ) {
        let mut forward = self.make_front();
        forward.y = 0.;
        let cross_normalized = forward.cross(Self::UP).normalize();
        let mut move_vector = keys.iter().copied().fold(
            Vector3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            |vec, key| match key {
                VirtualKeyCode::W => vec + forward,
                VirtualKeyCode::S => vec - forward,
                VirtualKeyCode::A => vec - cross_normalized,
                VirtualKeyCode::D => vec + cross_normalized,
                _ => vec,
            },
        );
        self.shifting = keys.contains(&VirtualKeyCode::LShift);

        if !(move_vector.x == 0.0 && move_vector.y == 0.0 && move_vector.z == 0.0) {
            move_vector = move_vector.normalize();
        }
        if self.shifting {
            move_vector.y -= 1.;
            move_vector /= 2.;
        }
        if keys.contains(&VirtualKeyCode::Space) {
            move_vector.y += 1.;
        }

        move_vector *= self.speed;
        move_vector *= 5.;

        let total_move = (move_vector + self.velocity) * delta_time;

        self.last_moved = move_vector.magnitude() > 0.;

        let drag_coefficient = 0.025;
        let drag = self
            .velocity
            .mul_element_wise(self.velocity)
            .mul_element_wise(Vector3 {
                x: 1f32.copysign(self.velocity.x),
                y: 1f32.copysign(self.velocity.y),
                z: 1f32.copysign(self.velocity.z),
            })
            * drag_coefficient;
        self.velocity -= drag * delta_time;
        self.position += total_move;
        //self.velocity.y -= delta_time * 15f32;

        self.shifting_animation += (if self.shifting { 1. } else { -1. }) * delta_time * 4.;
        self.shifting_animation = self.shifting_animation.clamp(0., 0.5);
    }
    pub const fn at_position(position: Position) -> Self {
        Self {
            position: Point3 {
                x: position.x as f32,
                y: position.y as f32,
                z: position.z as f32,
            },
            velocity: Vector3::new(0., 0., 0.),
            pitch_deg: 0.0,
            yaw_deg: 0.0,
            shifting: false,
            shifting_animation: 0f32,
            last_moved: false,
            speed: 1.,
        }
    }
    fn eye_height_diff(&self) -> f32 {
        1.75 - self.shifting_animation
    }
    pub fn get_eye(&self) -> Position {
        Position {
            x: self.position.x as f64,
            y: self.position.y as f64,
            z: self.position.z as f64,
        }
        .add(0., self.eye_height_diff() as f64, 0.)
    }
    pub fn create_view_matrix(&self) -> Matrix4<f32> {
        Matrix4::look_at_rh(
            self.position
                + Vector3 {
                    x: 0.,
                    y: self.eye_height_diff(),
                    z: 0.,
                },
            self.position
                + Vector3 {
                    x: 0.,
                    y: self.eye_height_diff(),
                    z: 0.,
                }
                + self.make_front(),
            Self::UP,
        )
    }
    pub fn create_view_matrix_no_pos(&self) -> Matrix4<f32> {
        Matrix4::look_at_rh(
            Point3 {
                x: 0.,
                y: 0.,
                z: 0.,
            },
            Point3 {
                x: 0.,
                y: 0.,
                z: 0.,
            } + self.make_front(),
            Self::UP,
        )
    }
    pub fn create_projection_matrix(&self, aspect: f32) -> Matrix4<f32> {
        cgmath::perspective(cgmath::Deg(90.), aspect, 0.1, 100.)
    }
}
pub struct Chunk {
    position: ChunkPosition,
    blocks: [[[u32; 16]; 16]; 16],
    modified: bool,
    buffer: Option<(Buffer, u32)>,
}
impl Chunk {
    pub fn new(position: ChunkPosition, blocks: [[[u32; 16]; 16]; 16]) -> Self {
        Chunk {
            position,
            blocks,
            modified: true,
            buffer: None,
        }
    }
    pub fn rebuild_chunk_mesh(
        &mut self,
        block_registry: &BlockRegistry,
        device: &Device,
        queue: &Queue,
    ) {
        let mut vertices: Vec<Vertex> = Vec::new();
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    let block = self.blocks[x][y][z];
                    let block = block_registry.get_block(block);
                    match &block.block_type {
                        BlockRenderDataType::Air => {}
                        BlockRenderDataType::Cube(cube_data) => {
                            for face in Face::all() {
                                let neighbor_position = BlockPosition {
                                    x: x as i32,
                                    y: y as i32,
                                    z: z as i32,
                                }
                                .offset_by_face(*face);
                                if neighbor_position.offset_from_origin_chunk().is_none() {
                                    //todo: neighboring chunks
                                    let neighbor_block = block_registry.get_block(
                                        self.blocks[neighbor_position.x as usize]
                                            [neighbor_position.y as usize]
                                            [neighbor_position.z as usize],
                                    );
                                    if neighbor_block.block_type.is_face_full(face.opposite()) {
                                        continue;
                                    }
                                }
                                let texture = cube_data.by_face(*face);
                                let base_position = Position {
                                    x: ((self.position.x * 16) + x as i32) as f64,
                                    y: ((self.position.y * 16) + y as i32) as f64,
                                    z: ((self.position.z * 16) + z as i32) as f64,
                                };
                                face.add_vertices(texture, &mut |position, coords| {
                                    vertices.push(Vertex {
                                        position: [
                                            (base_position.x + position.x) as f32,
                                            (base_position.y + position.y) as f32,
                                            (base_position.z + position.z) as f32,
                                        ],
                                        tex_coords: [coords.0, coords.1],
                                    })
                                });
                            }
                        }
                        BlockRenderDataType::Static(_) => {}
                        BlockRenderDataType::Foliage(_) => {}
                    }
                }
            }
        }
        if vertices.len() == 0 {
            self.buffer = None;
        } else {
            if let Some((buffer, vertex_count)) = &mut self.buffer {
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(vertices.as_slice()));
                *vertex_count = vertices.len() as u32;
            } else {
                self.buffer = Some((
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Chunk Vertex Buffer"),
                        contents: bytemuck::cast_slice(vertices.as_slice()),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                    vertices.len() as u32,
                ));
            }
        }
    }
    pub fn get_vertices(
        &mut self,
        block_registry: &BlockRegistry,
        device: &Device,
        queue: &Queue,
    ) -> Option<(BufferSlice, u32)> {
        if self.modified {
            self.rebuild_chunk_mesh(block_registry, device, queue);
            self.modified = false;
        }
        self.buffer
            .as_ref()
            .map(|buffer| (buffer.0.slice(..), buffer.1))
    }
}
pub struct World {
    pub chunks: HashMap<ChunkPosition, Chunk>,
    pub block_registry: Rc<BlockRegistry>,
}
impl World {
    pub fn new(block_registry: Rc<BlockRegistry>) -> Self {
        World {
            chunks: HashMap::new(),
            block_registry,
        }
    }
    pub fn load_chunk(&mut self, position: ChunkPosition, blocks: [[[u32; 16]; 16]; 16]) {
        self.chunks.insert(position, Chunk::new(position, blocks));
    }
}
