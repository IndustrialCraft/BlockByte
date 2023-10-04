use crate::content::{BlockRegistry, BlockRenderDataType};
use crate::render::{FaceVerticesExtension, Vertex};
use block_byte_common::messages::MovementType;
use block_byte_common::{BlockPosition, ChunkPosition, Face, FaceStorage, Position, AABB};
use cgmath::{ElementWise, InnerSpace, Matrix4, Point3, SquareMatrix, Vector3};
use log::warn;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wgpu::util::DeviceExt;
use wgpu::{Buffer, BufferSlice, Device};
use winit::event::VirtualKeyCode;

pub struct ClientPlayer {
    pub position: Point3<f32>,
    pub velocity: Vector3<f32>,
    pub pitch_deg: f32,
    pub yaw_deg: f32,
    shifting: bool,
    shifting_animation: f32,
    pub last_moved: bool,
    speed: f32,
    movement_type: MovementType,
    block_registry: Rc<BlockRegistry>,
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
    pub fn get_eye(&self) -> Position {
        Position {
            x: self.position.x as f64,
            y: self.position.y as f64,
            z: self.position.z as f64,
        }
        .add(0.3, self.eye_height_diff() as f64, 0.3)
    }
    pub fn update_position(
        &mut self,
        keys: &std::collections::HashSet<VirtualKeyCode>,
        delta_time: f32,
        world: &World,
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
        let position = Position {
            x: self.position.x as f64,
            y: self.position.y as f64,
            z: self.position.z as f64,
        };
        self.shifting = keys.contains(&VirtualKeyCode::LShift);
        if !self.shifting {
            let collides = self.collides_at(position, world);
            self.shifting = true;
            let collides_shifting = self.collides_at(position, world);
            self.shifting = collides && (!collides_shifting);
        }

        if !(move_vector.x == 0.0 && move_vector.y == 0.0 && move_vector.z == 0.0) {
            move_vector = move_vector.normalize();
        }
        if self.shifting {
            move_vector /= 2.;
        }

        if self.movement_type == MovementType::Normal {
            if keys.contains(&VirtualKeyCode::Space) {
                let block = world.get_block(position.to_block_pos()).unwrap_or(0);
                let block = self.block_registry.get_block(block);
                if block.fluid {
                    move_vector.y += 1.;
                    self.velocity.y = 0.;
                } else {
                    if self.collides_at(position.add(0., -0.2, 0.), world) {
                        self.velocity.y = 5.5;
                    }
                }
            }
        } else {
            if keys.contains(&VirtualKeyCode::Space) {
                move_vector.y += 1.;
            }
            if keys.contains(&VirtualKeyCode::LShift) {
                move_vector.y -= 1.;
            }
        }

        move_vector *= self.speed;
        move_vector *= 5.;

        let mut total_move = (move_vector + self.velocity) * delta_time;

        self.last_moved = move_vector.magnitude() > 0.;

        if (total_move.x != 0.
            && self.shifting
            && self.collides_at(position.add(0., -0.1, 0.), world))
            && !self.collides_at(position.add(total_move.x as f64, -0.1, 0.), world)
        {
            total_move.x = 0.;
            self.velocity.x = 0.;
        }
        if (total_move.z != 0.
            && self.shifting
            && self.collides_at(position.add(total_move.x as f64, -0.1, 0.), world))
            && !self.collides_at(
                position.add(total_move.x as f64, -0.1, total_move.z as f64),
                world,
            )
        {
            total_move.z = 0.;
            self.velocity.z = 0.;
        }

        if self.collides_at(position.add(total_move.x as f64, 0., 0.), world) {
            total_move.x = 0.;
            self.velocity.x = 0.;
        }
        if self.collides_at(
            position.add(total_move.x as f64, total_move.y as f64, 0.),
            world,
        ) {
            total_move.y = 0.;
            self.velocity.y = 0.;
        }
        if self.collides_at(
            position.add(
                total_move.x as f64,
                total_move.y as f64,
                total_move.z as f64,
            ),
            world,
        ) {
            total_move.z = 0.;
            self.velocity.z = 0.;
        }

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
        if self.movement_type == MovementType::Normal {
            self.velocity.y -= delta_time * 15f32;
        }

        self.shifting_animation += (if self.shifting { 1. } else { -1. }) * delta_time * 4.;
        self.shifting_animation = self.shifting_animation.clamp(0., 0.5);
    }
    fn collides_at(&self, position: Position, world: &World) -> bool {
        if self.movement_type == MovementType::NoClip {
            return false;
        }
        let bounding_box = AABB {
            x: position.x,
            y: position.y,
            z: position.z,
            w: 0.6,
            h: 1.95 - if self.shifting { 0.5 } else { 0. },
            d: 0.6,
        };
        for block_pos in bounding_box.iter_blocks() {
            if world.get_block(block_pos).map_or(true, |block| {
                let block = self.block_registry.get_block(block);
                !block.fluid && !block.no_collide
            }) {
                return true;
            }
        }
        return false;
    }
    pub const fn at_position(position: Position, block_registry: Rc<BlockRegistry>) -> Self {
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
            movement_type: MovementType::NoClip,
            block_registry,
        }
    }
    pub fn set_abilities(&mut self, speed: f32, movement_type: MovementType) {
        self.speed = speed;
        self.movement_type = movement_type;
    }
    fn eye_height_diff(&self) -> f32 {
        1.75 - self.shifting_animation
    }
    pub fn create_view_matrix(&self) -> Matrix4<f32> {
        let eye = self.get_eye();
        let eye = Point3 {
            x: eye.x as f32,
            y: eye.y as f32,
            z: eye.z as f32,
        };
        Matrix4::look_at_rh(eye, eye + self.make_front(), Self::UP)
    }
    pub fn create_projection_matrix(&self, aspect: f32) -> Matrix4<f32> {
        cgmath::perspective(cgmath::Deg(90.), aspect, 0.001, 1000.)
    }
}
pub struct DynamicBlockData {
    pub id: u32,
    pub animation: Option<(u32, f32)>,
    pub items: HashMap<String, u32>,
}
pub struct Chunk {
    position: ChunkPosition,
    blocks: [[[u32; 16]; 16]; 16],
    buffer: Option<(Buffer, u32)>,
    transparent_buffer: Option<(Buffer, u32)>,
    foliage_buffer: Option<(Buffer, u32)>,
}
impl Chunk {
    pub fn new(position: ChunkPosition, blocks: [[[u32; 16]; 16]; 16]) -> Self {
        Chunk {
            position,
            blocks,
            buffer: None,
            transparent_buffer: None,
            foliage_buffer: None,
        }
    }
    pub fn rebuild_chunk_mesh(
        &mut self,
        block_registry: &BlockRegistry,
        device: &Device,
        neighbor_chunks: FaceStorage<&Chunk>,
    ) {
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut transparent_vertices: Vec<Vertex> = Vec::new();
        let mut foliage_vertices: Vec<Vertex> = Vec::new();
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    let block = self.blocks[x][y][z];
                    let block = block_registry.get_block(block);
                    let base_position = Position {
                        x: ((self.position.x * 16) + x as i32) as f64,
                        y: ((self.position.y * 16) + y as i32) as f64,
                        z: ((self.position.z * 16) + z as i32) as f64,
                    };
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
                                let neighbor_offset = neighbor_position.chunk_offset();
                                let neighbor_chunk =
                                    match neighbor_position.offset_from_origin_chunk() {
                                        Some(face) => *neighbor_chunks.by_face(face),
                                        None => self,
                                    };

                                let neighbor_block = block_registry.get_block(
                                    neighbor_chunk.blocks[neighbor_offset.0 as usize]
                                        [neighbor_offset.1 as usize]
                                        [neighbor_offset.2 as usize],
                                );
                                if neighbor_block.is_face_full(face.opposite())
                                    || (neighbor_block.transparent && block.transparent)
                                {
                                    continue;
                                }

                                let texture = cube_data.by_face(*face);
                                face.add_vertices(texture, &mut |position, coords| {
                                    (if block.transparent {
                                        &mut transparent_vertices
                                    } else {
                                        &mut vertices
                                    })
                                    .push(Vertex {
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
                        BlockRenderDataType::Static(model) => {
                            model.model.add_vertices(
                                Matrix4::identity(),
                                None,
                                None,
                                &mut |position, coords| {
                                    vertices.push(Vertex {
                                        position: [
                                            (base_position.x + position.x) as f32 + 0.5,
                                            (base_position.y + position.y) as f32,
                                            (base_position.z + position.z) as f32 + 0.5,
                                        ],
                                        tex_coords: [coords.0, coords.1],
                                    })
                                },
                            );
                        }
                        BlockRenderDataType::Foliage(foliage) => {
                            for face in &[Face::Front, Face::Back, Face::Left, Face::Right] {
                                face.add_vertices(
                                    match face {
                                        Face::Front => foliage.texture_1,
                                        Face::Back => foliage.texture_2,
                                        Face::Left => foliage.texture_3,
                                        Face::Right => foliage.texture_4,
                                        _ => unreachable!(),
                                    },
                                    &mut |position, coords| {
                                        let shift = face.opposite().get_offset();
                                        foliage_vertices.push(Vertex {
                                            position: [
                                                (base_position.x + position.x) as f32
                                                    + (shift.x as f32 * 0.3),
                                                (base_position.y + position.y) as f32,
                                                (base_position.z + position.z) as f32
                                                    + (shift.z as f32 * 0.3),
                                            ],
                                            tex_coords: [coords.0, coords.1],
                                        });
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        if vertices.len() == 0 {
            self.buffer = None;
        } else {
            self.buffer = Some((
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Vertex Buffer"),
                    contents: bytemuck::cast_slice(vertices.as_slice()),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                }),
                vertices.len() as u32,
            ));
        }
        if transparent_vertices.len() == 0 {
            self.transparent_buffer = None;
        } else {
            self.transparent_buffer = Some((
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Transparent Vertex Buffer"),
                    contents: bytemuck::cast_slice(transparent_vertices.as_slice()),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                }),
                transparent_vertices.len() as u32,
            ));
        }
        if foliage_vertices.len() == 0 {
            self.foliage_buffer = None;
        } else {
            self.foliage_buffer = Some((
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Transparent Vertex Buffer"),
                    contents: bytemuck::cast_slice(foliage_vertices.as_slice()),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                }),
                foliage_vertices.len() as u32,
            ));
        }
    }
    pub fn get_vertices(
        &mut self,
    ) -> (
        Option<(BufferSlice, u32)>,
        Option<(BufferSlice, u32)>,
        Option<(BufferSlice, u32)>,
    ) {
        (
            self.buffer
                .as_ref()
                .map(|buffer| (buffer.0.slice(..), buffer.1)),
            self.transparent_buffer
                .as_ref()
                .map(|buffer| (buffer.0.slice(..), buffer.1)),
            self.foliage_buffer
                .as_ref()
                .map(|buffer| (buffer.0.slice(..), buffer.1)),
        )
    }
}
pub struct World {
    pub chunks: HashMap<ChunkPosition, Chunk>,
    pub block_registry: Rc<BlockRegistry>,
    pub modified_chunks: HashSet<ChunkPosition>,
    pub dynamic_blocks: HashMap<BlockPosition, DynamicBlockData>,
    pub entities: HashMap<u32, EntityData>,
}
impl World {
    pub fn new(block_registry: Rc<BlockRegistry>) -> Self {
        World {
            chunks: HashMap::new(),
            block_registry,
            modified_chunks: HashSet::new(),
            dynamic_blocks: HashMap::new(),
            entities: HashMap::new(),
        }
    }
    pub fn tick(&mut self, device: &Device) {
        let max_chunk_meshes_per_frame = 200;
        for chunk_position in self
            .modified_chunks
            .extract_if(|_| true)
            .take(max_chunk_meshes_per_frame)
        {
            if let Some([chunk, front, back, left, right, up, down]) = self.chunks.get_many_mut([
                &chunk_position,
                &chunk_position.with_offset(&Face::Front),
                &chunk_position.with_offset(&Face::Back),
                &chunk_position.with_offset(&Face::Left),
                &chunk_position.with_offset(&Face::Right),
                &chunk_position.with_offset(&Face::Up),
                &chunk_position.with_offset(&Face::Down),
            ]) {
                chunk.rebuild_chunk_mesh(
                    &self.block_registry,
                    device,
                    FaceStorage {
                        front,
                        back,
                        left,
                        right,
                        up,
                        down,
                    },
                );
            }
        }
    }
    pub fn load_chunk(&mut self, position: ChunkPosition, blocks: [[[u32; 16]; 16]; 16]) {
        self.chunks.insert(position, Chunk::new(position, blocks));
        self.modified_chunks.insert(position);
        for face in Face::all() {
            self.modified_chunks.insert(position.with_offset(face));
        }
    }
    pub fn unload_chunk(&mut self, position: ChunkPosition) {
        self.chunks.remove(&position);
        self.dynamic_blocks
            .extract_if(|block_position, _| block_position.to_chunk_pos() == position)
            .count();
    }
    pub fn get_dynamic_block_data(
        &mut self,
        block_position: BlockPosition,
    ) -> Option<&mut DynamicBlockData> {
        let block_id = {
            match self.get_block(block_position) {
                Some(block_id) => block_id,
                None => return None,
            }
        };
        if self.block_registry.get_block(block_id).dynamic.is_none() {
            return None;
        }
        Some(
            self.dynamic_blocks
                .entry(block_position)
                .or_insert_with(|| DynamicBlockData {
                    id: block_id,
                    animation: None,
                    items: HashMap::new(),
                }),
        )
    }
    pub fn set_block(&mut self, position: BlockPosition, id: u32) {
        let chunk_position = position.to_chunk_pos();
        let offset = position.chunk_offset();
        if let Some(chunk) = self.chunks.get_mut(&chunk_position) {
            chunk.blocks[offset.0 as usize][offset.1 as usize][offset.2 as usize] = id;
            for face in Face::all() {
                if position.offset_by_face(*face).to_chunk_pos() != chunk_position {
                    self.modified_chunks
                        .insert(chunk_position.with_offset(face));
                }
            }
            self.modified_chunks.insert(chunk_position);
        } else {
            warn!("setting block in unloaded chunk");
        }
        self.dynamic_blocks.remove(&position);
    }
    pub fn get_block(&self, position: BlockPosition) -> Option<u32> {
        let chunk = position.to_chunk_pos();
        let offset = position.chunk_offset();
        self.chunks
            .get(&chunk)
            .map(|chunk| chunk.blocks[offset.0 as usize][offset.1 as usize][offset.2 as usize])
    }
    pub fn raycast(
        &self,
        max_distance: f64,
        start_position: Position,
        direction: Vector3<f32>,
    ) -> Option<(BlockPosition, Face)> {
        let mut output = None;
        voxel_tile_raycast::voxel_raycast(
            nalgebra::Vector3::new(start_position.x, start_position.y, start_position.z),
            nalgebra::Vector3::new(direction.x as f64, direction.y as f64, direction.z as f64),
            max_distance,
            |index, _hit_pos, hit_normal| {
                let block_position = BlockPosition {
                    x: index.x,
                    y: index.y,
                    z: index.z,
                };
                let block = self.get_block(block_position);
                if self.block_registry.get_block(block.unwrap_or(0)).selectable {
                    output = Some((
                        block_position,
                        Face::all()
                            .iter()
                            .find(|face| {
                                let offset = face.get_offset();
                                offset.x == hit_normal.x
                                    && offset.y == hit_normal.y
                                    && offset.z == hit_normal.z
                            })
                            .cloned()
                            .unwrap_or(Face::Up),
                    ));
                    true
                } else {
                    false
                }
            },
        );
        output
    }
}
pub struct EntityData {
    pub type_id: u32,
    pub position: Position,
    pub rotation: f32,
    pub animation: Option<(u32, f32)>,
    pub items: HashMap<String, u32>,
}
