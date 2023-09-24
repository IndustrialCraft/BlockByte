use crate::render::FaceVerticesExtension;
use block_byte_common::content::{ModelBone, ModelCubeElement, ModelData};
use block_byte_common::{Face, Position, TexCoords};
use cgmath::{Matrix4, Point3, Transform};

pub struct Model {
    data: ModelData,
    texture: TexCoords,
}
impl Model {
    pub fn new(data: ModelData, texture: TexCoords) -> Self {
        Model { data, texture }
    }
    pub fn add_vertices<F>(&self, base_matrix: Matrix4<f32>, vertex_consumer: &mut F)
    where
        F: FnMut(Position, (f32, f32)),
    {
        self.add_bone(&self.data.root_bone, base_matrix, vertex_consumer);
    }
    fn add_bone<F>(&self, bone: &ModelBone, parent_transform: Matrix4<f32>, vertex_consumer: &mut F)
    where
        F: FnMut(Position, (f32, f32)),
    {
        //todo: animations
        let transform = parent_transform;
        for child_bone in &bone.child_bones {
            self.add_bone(child_bone, transform, vertex_consumer);
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
                    //todo: rotation
                    let position = parent_transform.transform_point(Point3 {
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
}
