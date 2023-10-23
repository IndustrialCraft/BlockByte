use crate::content::{ItemModel, ItemRegistry};
use crate::render::FaceVerticesExtension;
use block_byte_common::content::{
    ModelAnimationData, ModelBone, ModelCubeElement, ModelData, ModelItemElement,
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
        instance: &ModelInstanceData,
        item_registry: Option<&ItemRegistry>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        self.add_bone(
            &self.data.root_bone,
            base_matrix,
            instance,
            item_registry,
            vertex_consumer,
        );
    }
    fn add_bone<F>(
        &self,
        bone: &ModelBone,
        parent_transform: Matrix4<f32>,
        instance: &ModelInstanceData,
        item_registry: Option<&ItemRegistry>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        let (translate, rotate, scale) = instance
            .animation
            .and_then(|(animation, time)| {
                self.animations
                    .get(animation as usize)
                    .and_then(|animation| bone.animations.get(animation))
                    .map(|animation| animation.get_for_time(time))
            })
            .unwrap_or(ModelAnimationData::get_default());
        let transform =
            parent_transform * Self::create_matrix_trs(&translate, &rotate, &bone.origin, &scale);
        for child_bone in &bone.child_bones {
            self.add_bone(
                child_bone,
                transform,
                instance,
                item_registry,
                vertex_consumer,
            );
        }
        for child_cube_element in &bone.cube_elements {
            self.add_cube_element(child_cube_element, transform, vertex_consumer);
        }
        if let Some(item_registry) = item_registry {
            for child_item_element in &bone.item_elements {
                self.add_item_element(
                    child_item_element,
                    transform,
                    (&instance.items, item_registry),
                    vertex_consumer,
                );
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
                            &Vec3::ZERO,
                            &cube_element.rotation,
                            &cube_element.origin,
                            &Vec3::ONE,
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
        items: (&HashMap<String, u32>, &ItemRegistry),
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        if let Some(item) = items.0.get(&item_element.name) {
            let (main_texture, sides) = match &items.1.get_item(*item).model {
                ItemModel::Texture { texture, sides } => (*texture, Some(sides)),
                ItemModel::Block { front, .. } => (*front, None),
            };
            if let Some(sides) = sides {
                for side in &sides.0 {
                    side.1.add_vertices(
                        TexCoords {
                            u1: 0.,
                            v1: 0.,
                            u2: 0.,
                            v2: 0.,
                        },
                        &mut |position, _coords| {
                            let position = (parent_transform
                                * Self::create_matrix_trs(
                                    &Vec3::ZERO,
                                    &item_element.rotation,
                                    &item_element.origin,
                                    &Vec3::ONE,
                                ))
                            .transform_point(Point3 {
                                x: item_element.position.x
                                    + (((position.x as f32 + side.0 .0 as f32) / sides.1.x)
                                        * item_element.size.x),
                                y: item_element.position.y
                                    + (((position.z as f32 + side.0 .1 as f32) / sides.1.y)
                                        * item_element.size.y),
                                z: item_element.position.z + ((1. - position.y) as f32 / 32.),
                            });
                            vertex_consumer.call_mut((
                                Position {
                                    x: position.x as f64,
                                    y: position.y as f64,
                                    z: position.z as f64,
                                },
                                (
                                    main_texture.u1
                                        + (((side.0 .0 as f32 + 0.5) / sides.1.x)
                                            * (main_texture.u2 - main_texture.u1)),
                                    main_texture.v1
                                        + (((side.0 .1 as f32 + 0.5) / sides.1.y)
                                            * (main_texture.v2 - main_texture.v1)),
                                ),
                            ));
                        },
                    );
                }
            }
            Face::Down.add_vertices(main_texture.flip_horizontally(), &mut |position, coords| {
                let position = (parent_transform
                    * Self::create_matrix_trs(
                        &Vec3::ZERO,
                        &item_element.rotation,
                        &item_element.origin,
                        &Vec3::ONE,
                    ))
                .transform_point(Point3 {
                    x: item_element.position.x + (position.x as f32 * item_element.size.x),
                    y: item_element.position.y + (position.z as f32 * item_element.size.y),
                    z: item_element.position.z + (1. / 32.),
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
            Face::Up.add_vertices(main_texture, &mut |position, coords| {
                let position = (parent_transform
                    * Self::create_matrix_trs(
                        &Vec3::ZERO,
                        &item_element.rotation,
                        &item_element.origin,
                        &Vec3::ONE,
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
                * Matrix4::from_angle_z(Rad(rotation.z))
                * Matrix4::from_angle_y(Rad(rotation.y))
                * Matrix4::from_angle_x(Rad(-rotation.x))
                * origin.invert().unwrap())
            * Matrix4::from_nonuniform_scale(scale.x, scale.y, scale.z)
    }
    pub fn get_animation_length(&self, animation: u32) -> Option<f32> {
        let index = self.animations.get(animation as usize)?;
        self.data
            .animations
            .get(*index as usize)
            .map(|animation| animation.1)
    }
}
pub struct ModelInstanceData {
    pub animation: Option<(u32, f32)>,
    pub items: HashMap<String, u32>,
}
impl ModelInstanceData {
    pub fn new() -> Self {
        ModelInstanceData {
            animation: None,
            items: HashMap::new(),
        }
    }
}
