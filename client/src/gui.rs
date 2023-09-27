use crate::content::ItemRegistry;
use crate::render::GUIVertex;
use crate::texture::TextureAtlas;
use block_byte_common::content::ClientItemModel;
use block_byte_common::gui::{GUIComponent, GUIElement};
use block_byte_common::{Color, TexCoords, Vec2};
use std::collections::HashMap;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{Buffer, BufferAddress, BufferDescriptor, BufferSlice, BufferUsages, Device, Queue};

pub struct GUIRenderer {
    elements: HashMap<String, GUIElement>,
    buffer: Buffer,
    gui_scale: f32,
    texture_atlas: TextureAtlas,
}
impl GUIRenderer {
    pub fn new(texture_atlas: TextureAtlas, device: &Device) -> Self {
        GUIRenderer {
            texture_atlas,
            elements: HashMap::new(),
            buffer: device.create_buffer_init(&BufferInitDescriptor {
                label: Some("gui buffer"),
                contents: &[],
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            }),
            gui_scale: 1. / 250.,
        }
    }
    pub fn set_element(&mut self, id: String, element: GUIElement) {
        self.elements.insert(id, element);
    }
    pub fn remove_elements(&mut self, id: &str) {
        self.elements
            .extract_if(|element_id, _| element_id.starts_with(id))
            .count();
    }
    pub fn draw(&mut self, device: &Device, item_registry: &ItemRegistry) -> (BufferSlice, u32) {
        let mut vertices: Vec<GUIVertex> = Vec::new();
        //todo: sort by z position
        for element in self.elements.values() {
            //todo: mouse
            let center_position = element.anchor.get_center(Vec2 { x: 0., y: 0. });
            let center_position = Vec2 {
                x: center_position.x + (element.position.x * self.gui_scale as f64) as f32,
                y: center_position.y + (element.position.y * self.gui_scale as f64) as f32,
            };
            match &element.component_type {
                GUIComponent::ImageComponent { texture: uv, size } => {
                    Self::add_rect_vertices(
                        &mut vertices,
                        center_position,
                        Vec2 {
                            x: size.x * self.gui_scale,
                            y: size.y * self.gui_scale,
                        },
                        self.texture_atlas.get(uv.as_str()),
                        element.base_color,
                    );
                }
                GUIComponent::SlotComponent {
                    background,
                    size,
                    item_id,
                } => {
                    if !background.is_empty() {
                        Self::add_rect_vertices(
                            &mut vertices,
                            center_position,
                            Vec2 {
                                x: size.x * self.gui_scale,
                                y: size.y * self.gui_scale,
                            },
                            self.texture_atlas.get(background.as_str()),
                            element.base_color,
                        );
                    }
                    let item = item_registry.get_item(*item_id);
                    match &item.model {
                        ClientItemModel::Texture(texture) => {
                            Self::add_rect_vertices(
                                &mut vertices,
                                center_position,
                                Vec2 {
                                    x: size.x * self.gui_scale,
                                    y: size.y * self.gui_scale,
                                },
                                self.texture_atlas.get(texture.as_str()),
                                element.base_color,
                            );
                        }
                        ClientItemModel::Block(_) => {}
                    }
                }
                _ => {}
            }
        }
        self.buffer = device.create_buffer_init(&BufferInitDescriptor {
            label: Some("gui buffer"),
            contents: bytemuck::cast_slice(vertices.as_slice()),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });
        (self.buffer.slice(..), vertices.len() as u32)
    }
    fn add_rect_vertices(
        vertices: &mut Vec<GUIVertex>,
        center: Vec2,
        size: Vec2,
        uv: TexCoords,
        color: Color,
    ) {
        let position = Vec2 {
            x: center.x - (size.x / 2.),
            y: center.y - (size.y / 2.),
        };
        let vertex_4 = GUIVertex {
            position: [position.x, position.y],
            tex_coords: [uv.u1, uv.v2],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_3 = GUIVertex {
            position: [position.x + size.x, position.y],
            tex_coords: [uv.u2, uv.v2],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_2 = GUIVertex {
            position: [position.x + size.x, position.y + size.y],
            tex_coords: [uv.u2, uv.v1],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_1 = GUIVertex {
            position: [position.x, position.y + size.y],
            tex_coords: [uv.u1, uv.v1],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        vertices.push(vertex_1);
        vertices.push(vertex_4);
        vertices.push(vertex_3);

        vertices.push(vertex_3);
        vertices.push(vertex_2);
        vertices.push(vertex_1);
    }
}
