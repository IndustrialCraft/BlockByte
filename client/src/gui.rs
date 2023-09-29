use crate::content::{BlockRegistry, BlockRenderDataType, ItemRegistry};
use crate::render::GUIVertex;
use crate::texture::TextureAtlas;
use block_byte_common::content::ClientItemModel;
use block_byte_common::gui::{GUIComponent, GUIElement, PositionAnchor};
use block_byte_common::{Color, TexCoords, Vec2};
use std::collections::HashMap;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{Buffer, BufferAddress, BufferDescriptor, BufferSlice, BufferUsages, Device, Queue};
use winit::dpi::{PhysicalPosition, PhysicalSize};

pub struct GUIRenderer {
    elements: HashMap<String, GUIElement>,
    buffer: Buffer,
    gui_scale: f32,
    texture_atlas: TextureAtlas,
    cursor_locked: bool,
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
            gui_scale: 1. / 1000.,
            cursor_locked: true,
        }
    }
    pub fn set_element(&mut self, id: String, element: GUIElement) {
        self.elements.insert(id, element);
    }
    pub fn get_element(&mut self, id: String) -> Option<&mut GUIElement> {
        self.elements.get_mut(id.as_str())
    }
    pub fn remove_elements(&mut self, id: &str) {
        self.elements
            .extract_if(|element_id, _| element_id.starts_with(id))
            .count();
    }
    pub fn set_cursor_locked(&mut self, locked: bool) {
        self.cursor_locked = locked;
    }
    pub fn is_cursor_locked(&self) -> bool {
        self.cursor_locked
    }
    pub fn get_mouse_position(
        &self,
        physical_position: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
    ) -> Vec2 {
        let x = physical_position.x / size.width as f64;
        let y = physical_position.y / size.height as f64;
        Vec2 {
            x: ((x * 2.) - 1.) as f32,
            y: ((2. - (y * 2.)) - 1.) as f32,
        }
    }
    pub fn get_selected(
        &self,
        mouse: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
    ) -> Option<String> {
        let mouse = self.get_mouse_position(mouse, size);
        let aspect_ratio = size.width as f32 / size.height as f32;
        for (id, element) in &self.elements {
            let size = match &element.component_type {
                GUIComponent::ImageComponent { size, .. } => size,
                GUIComponent::TextComponent { .. } => unimplemented!(),
                GUIComponent::SlotComponent { size, .. } => size,
            };
            if Self::mouse_hovers(
                mouse,
                element.anchor,
                Vec2 {
                    x: element.position.x as f32,
                    y: element.position.y as f32,
                },
                *size,
                self.gui_scale,
                aspect_ratio,
            ) {
                return Some(id.clone());
            }
        }
        None
    }
    pub fn draw(
        &mut self,
        device: &Device,
        item_registry: &ItemRegistry,
        block_registry: &BlockRegistry,
        mouse: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
    ) -> (BufferSlice, u32) {
        let aspect_ratio = size.width as f32 / size.height as f32;
        let mouse = self.get_mouse_position(mouse, size);
        let mut vertices: Vec<GUIVertex> = Vec::new();
        //todo: sort by z position
        for element in self.elements.values() {
            //todo: mouse
            match &element.component_type {
                GUIComponent::ImageComponent { texture: uv, size } => {
                    Self::add_rect_vertices(
                        &mut vertices,
                        element.anchor,
                        Vec2 {
                            x: element.position.x as f32,
                            y: element.position.y as f32,
                        },
                        *size,
                        self.texture_atlas.get(uv.as_str()),
                        element.base_color,
                        aspect_ratio,
                        self.gui_scale,
                        mouse,
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
                            element.anchor,
                            Vec2 {
                                x: element.position.x as f32,
                                y: element.position.y as f32,
                            },
                            *size,
                            self.texture_atlas.get(background.as_str()),
                            element.base_color,
                            aspect_ratio,
                            self.gui_scale,
                            mouse,
                        );
                    }
                    if let Some(item_id) = item_id.as_ref() {
                        let item = item_registry.get_item(*item_id);
                        match &item.model {
                            ClientItemModel::Texture(texture) => {
                                Self::add_rect_vertices(
                                    &mut vertices,
                                    element.anchor,
                                    Vec2 {
                                        x: element.position.x as f32,
                                        y: element.position.y as f32,
                                    },
                                    *size,
                                    self.texture_atlas.get(texture.as_str()),
                                    element.base_color,
                                    aspect_ratio,
                                    self.gui_scale,
                                    mouse,
                                );
                            }
                            ClientItemModel::Block(block) => {
                                match &block_registry.get_block(*block).block_type {
                                    BlockRenderDataType::Air => {}
                                    BlockRenderDataType::Cube(data) => {
                                        Self::add_rect_vertices(
                                            &mut vertices,
                                            element.anchor,
                                            Vec2 {
                                                x: element.position.x as f32,
                                                y: element.position.y as f32,
                                            },
                                            *size,
                                            data.front,
                                            element.base_color,
                                            aspect_ratio,
                                            self.gui_scale,
                                            mouse,
                                        );
                                    }
                                    BlockRenderDataType::Static(_) => {}
                                    BlockRenderDataType::Foliage(_) => {}
                                }
                            }
                        }
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
    fn mouse_hovers(
        mouse: Vec2,
        anchor: PositionAnchor,
        center: Vec2,
        size: Vec2,
        gui_scale: f32,
        aspect_ratio: f32,
    ) -> bool {
        if anchor == PositionAnchor::Cursor {
            return false;
        }
        let anchor = anchor.get_center(mouse);
        let position = Vec2 {
            x: anchor.x + ((center.x - (size.x / 2.)) * gui_scale) / aspect_ratio,
            y: (anchor.y + ((center.y - (size.y / 2.)) * gui_scale)),
        };
        let size = Vec2 {
            x: size.x * gui_scale / aspect_ratio,
            y: size.y * gui_scale,
        };
        mouse.x >= position.x
            && mouse.x <= position.x + size.x
            && mouse.y >= position.y
            && mouse.y <= position.y + size.y
    }
    fn add_rect_vertices(
        vertices: &mut Vec<GUIVertex>,
        anchor: PositionAnchor,
        center: Vec2,
        size: Vec2,
        uv: TexCoords,
        color: Color,
        aspect_ratio: f32,
        gui_scale: f32,
        mouse: Vec2,
    ) {
        let anchor = anchor.get_center(mouse);
        let position = Vec2 {
            x: anchor.x + ((center.x - (size.x / 2.)) * gui_scale) / aspect_ratio,
            y: (anchor.y + ((center.y - (size.y / 2.)) * gui_scale)),
        };
        let size = Vec2 {
            x: size.x * gui_scale / aspect_ratio,
            y: size.y * gui_scale,
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
