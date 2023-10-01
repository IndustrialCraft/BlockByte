use crate::render::FaceVerticesExtension;
use block_byte_common::content::{ModelAnimationData, ModelBone, ModelCubeElement, ModelData};
use block_byte_common::{Face, Position, TexCoords, Vec3};
use cgmath::{Matrix4, Point3, Rad, SquareMatrix, Transform, Vector3};

pub struct Model {
    data: ModelData,
    texture: TexCoords,
}
impl Model {
    pub fn new(data: ModelData, texture: TexCoords) -> Self {
        Model { data, texture }
    }
    pub fn add_vertices<F>(
        &self,
        base_matrix: Matrix4<f32>,
        animation: Option<(u32, f32)>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        self.add_bone(
            &self.data.root_bone,
            base_matrix,
            animation,
            vertex_consumer,
        );
    }
    fn add_bone<F>(
        &self,
        bone: &ModelBone,
        parent_transform: Matrix4<f32>,
        animation: Option<(u32, f32)>,
        vertex_consumer: &mut F,
    ) where
        F: FnMut(Position, (f32, f32)),
    {
        let (translate, rotate, scale) = match animation {
            Some((animation, time)) => bone
                .animations
                .get(&animation)
                .map(|animation| animation.get_for_time(time))
                .unwrap_or(ModelAnimationData::get_default()),
            None => ModelAnimationData::get_default(),
        };
        let transform =
            parent_transform * Self::create_matrix_trs(&translate, &rotate, &bone.origin, &scale);
        for child_bone in &bone.child_bones {
            self.add_bone(child_bone, transform, animation, vertex_consumer);
        }
        for child_cube_element in &bone.cube_elements {
            self.add_cube_element(child_cube_element, transform, vertex_consumer);
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
    fn create_matrix_trs(
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
            .get(animation as usize)
            .map(|animation| animation.1)
    }
}
