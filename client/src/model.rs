use crate::content::{BlockRegistry, BlockRenderDataType, ItemRegistry};
use crate::render::FaceVerticesExtension;
use crate::texture::TextureAtlas;
use block_byte_common::content::{
    ClientItemModel, ModelAnimationData, ModelBone, ModelCubeElement, ModelData, ModelItemElement,
};
use block_byte_common::{Face, Position, TexCoords, Vec3};
use cgmath::{Matrix4, Point3, Rad, SquareMatrix, Transform, Vector3};
use std::collections::HashMap;

pub struct Model {
    data: ModelData,
    texture: TexCoords,
    animations: Vec<u32>,
    items: Vec<String>,
}
impl Model {
    pub fn new(
        data: ModelData,
        texture: TexCoords,
        animations: Vec<String>,
        items: Vec<String>,
    ) -> Self {
        Model {
            texture,
            animations: {
                let mut animations_resolved = Vec::new();
                for animation in animations {
                    animations_resolved.push(
                        data.animations
                            .iter()
                            .position(|anim| anim.0 == animation)
                            .unwrap() as u32,
                    );
                }
                animations_resolved
            },
            data,
            items,
        }
    }
    pub fn get_item_slot(&self, slot: u32) -> Option<&String> {
        self.items.get(slot as usize)
    }
    pub fn add_vertices<F>(
        &self,
        base_matrix: Matrix4<f32>,
        animation: Option<(u32, f32)>,
        items: Option<(&HashMap<String, u32>, ItemTextureResolver)>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        self.add_bone(
            &self.data.root_bone,
            base_matrix,
            animation,
            items,
            vertex_consumer,
        );
    }
    fn add_bone<F>(
        &self,
        bone: &ModelBone,
        parent_transform: Matrix4<f32>,
        animation: Option<(u32, f32)>,
        items: Option<(&HashMap<String, u32>, ItemTextureResolver)>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        let (translate, rotate, scale) = match animation {
            Some((animation, time)) => self
                .animations
                .get(animation as usize)
                .and_then(|animation| bone.animations.get(animation))
                .map(|animation| animation.get_for_time(time))
                .unwrap_or(ModelAnimationData::get_default()),
            None => ModelAnimationData::get_default(),
        };
        let transform =
            parent_transform * Self::create_matrix_trs(&translate, &rotate, &bone.origin, &scale);
        for child_bone in &bone.child_bones {
            self.add_bone(child_bone, transform, animation, items, vertex_consumer);
        }
        for child_cube_element in &bone.cube_elements {
            self.add_cube_element(child_cube_element, transform, vertex_consumer);
        }
        if let Some(items) = items {
            for child_item_element in &bone.item_elements {
                self.add_item_element(child_item_element, transform, items, vertex_consumer);
            }
        }
    }
    fn add_cube_element<F>(
        &self,
        cube_element: &ModelCubeElement,
        parent_transform: Matrix4<f32>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        for face in Face::all() {
            face.add_vertices(
                self.texture.map_sub(&cube_element.texture_by_face(*face)),
                &mut |position, coords| {
                    let position = (parent_transform
                        * Self::create_matrix_trs(
                            &Vec3 {
                                x: 0.,
                                y: 0.,
                                z: 0.,
                            },
                            &cube_element.rotation,
                            &cube_element.origin,
                            &Vec3 {
                                x: 1.,
                                y: 1.,
                                z: 1.,
                            },
                        ))
                    .transform_point(Point3 {
                        x: cube_element.position.x + (position.x as f32 * cube_element.scale.x),
                        y: cube_element.position.y + (position.y as f32 * cube_element.scale.y),
                        z: cube_element.position.z + (position.z as f32 * cube_element.scale.z),
                    });
                    vertex_consumer.call_mut((
                        Position {
                            x: position.x as f64,
                            y: position.y as f64,
                            z: position.z as f64,
                        },
                        coords,
                    ));
                },
            );
        }
    }

    fn add_item_element<F>(
        &self,
        item_element: &ModelItemElement,
        parent_transform: Matrix4<f32>,
        items: (&HashMap<String, u32>, ItemTextureResolver),
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        if let Some(item) = items.0.get(&item_element.name) {
            let texture = items.1.resolve(*item);
            Face::Down.add_vertices(texture, &mut |position, coords| {
                let position = (parent_transform
                    * Self::create_matrix_trs(
                        &Vec3 {
                            x: 0.,
                            y: 0.,
                            z: 0.,
                        },
                        &item_element.rotation,
                        &item_element.origin,
                        &Vec3 {
                            x: 1.,
                            y: 1.,
                            z: 1.,
                        },
                    ))
                .transform_point(Point3 {
                    x: item_element.position.x + (position.x as f32 * item_element.size.x),
                    y: item_element.position.y + (position.z as f32 * item_element.size.y),
                    z: item_element.position.z,
                });
                vertex_consumer.call_mut((
                    Position {
                        x: position.x as f64,
                        y: position.y as f64,
                        z: position.z as f64,
                    },
                    coords,
                ));
            });
        }
    }
    pub fn create_matrix_trs(
        translation: &Vec3,
        rotation: &Vec3,
        origin: &Vec3,
        scale: &Vec3,
    ) -> Matrix4<f32> {
        let origin = Matrix4::from_translation(Vector3::new(origin.x, origin.y, origin.z));
        Matrix4::from_translation(Vector3::new(translation.x, translation.y, translation.z))
            * (origin
                * Matrix4::from_angle_x(Rad(rotation.x))
                * Matrix4::from_angle_y(Rad(rotation.y))
                * Matrix4::from_angle_z(Rad(rotation.z))
                * origin.invert().unwrap())
            * Matrix4::from_nonuniform_scale(scale.x, scale.y, scale.z)
    }
    pub fn get_animation_length(&self, animation: u32) -> Option<f32> {
        self.data
            .animations
            .get(*self.animations.get(animation as usize).unwrap() as usize)
            .map(|animation| animation.1)
    }
}
#[derive(Copy, Clone)]
pub struct ItemTextureResolver<'a> {
    pub texture_atlas: &'a TextureAtlas,
    pub item_registry: &'a ItemRegistry,
    pub block_registry: &'a BlockRegistry,
}
impl<'a> ItemTextureResolver<'a> {
    pub fn resolve(&self, item_id: u32) -> TexCoords {
        match &self.item_registry.get_item(item_id).model {
            ClientItemModel::Texture(texture) => self.texture_atlas.get(texture),
            ClientItemModel::Block(block_id) => {
                match &self.block_registry.get_block(*block_id).block_type {
                    BlockRenderDataType::Air => self.texture_atlas.missing_texture,
                    BlockRenderDataType::Cube(cube_data) => cube_data.front,
                    BlockRenderDataType::Static(_) => self.texture_atlas.missing_texture,
                    BlockRenderDataType::Foliage(_) => self.texture_atlas.missing_texture,
                }
            }
        }
    }
}
