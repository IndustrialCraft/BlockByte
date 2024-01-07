use crate::content::{ItemModel, ItemRegistry};
use crate::render::GUIVertex;
use crate::texture::TextureAtlas;
use block_byte_common::gui::{GUIComponent, GUIElement, PositionAnchor};
use block_byte_common::{Color, TexCoords, Vec2};
use rusttype::Scale;
use std::collections::HashMap;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{Buffer, BufferSlice, BufferUsages, Device};
use winit::dpi::{PhysicalPosition, PhysicalSize};

pub struct GUIRenderer<'a> {
    elements: HashMap<String, GUIElement>,
    buffer: Buffer,
    gui_scale: f32,
    texture_atlas: TextureAtlas,
    cursor_locked: bool,
    text_renderer: TextRenderer<'a>,
    selected: Option<String>,
}
impl<'a> GUIRenderer<'a> {
    pub fn new(
        texture_atlas: TextureAtlas,
        device: &Device,
        text_renderer: TextRenderer<'a>,
    ) -> Self {
        GUIRenderer {
            texture_atlas,
            elements: HashMap::new(),
            buffer: device.create_buffer_init(&BufferInitDescriptor {
                label: Some("gui buffer"),
                contents: &[],
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            }),
            gui_scale: 1. / 700.,
            cursor_locked: true,
            text_renderer,
            selected: None
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
    ) -> Option<(&str, &GUIElement)> {
        let mouse = self.get_mouse_position(mouse, size);
        let aspect_ratio = size.width as f32 / size.height as f32;
        for (id, element) in &self.elements {
            let size = match &element.component_type {
                GUIComponent::ImageComponent { size, .. } => Some(size),
                GUIComponent::TextComponent { .. } => None,
                GUIComponent::SlotComponent { size, .. } => Some(size),
                GUIComponent::LineEdit { size, .. } => Some(size)
            };
            if let Some(size) = size {
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
                    return Some((id.as_str(), element));
                }
            }
        }
        None
    }
    pub fn draw(
        &mut self,
        device: &Device,
        item_registry: &ItemRegistry,
        mouse_physical: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
    ) -> (BufferSlice, u32) {
        let aspect_ratio = size.width as f32 / size.height as f32;
        let mouse = self.get_mouse_position(mouse_physical, size);
        let mut vertices: Vec<GUIVertex> = Vec::new();
        for element in self.elements.values() {
            match &element.component_type {
                GUIComponent::ImageComponent {
                    texture: uv,
                    size,
                    slice,
                } => {
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
                        element.position.z as f32,
                        slice.as_ref(),
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
                            element.position.z as f32,
                            None,
                        );
                    }
                    if let Some(item_id) = item_id.as_ref() {
                        let item = item_registry.get_item(item_id.0);
                        let size = Vec2 {
                            x: size.x * (7. / 8.),
                            y: size.y * (7. / 8.),
                        };
                        match &item.model {
                            ItemModel::Texture { texture, .. } => {
                                Self::add_rect_vertices(
                                    &mut vertices,
                                    element.anchor,
                                    Vec2 {
                                        x: element.position.x as f32,
                                        y: element.position.y as f32,
                                    },
                                    size,
                                    *texture,
                                    Color::WHITE,
                                    aspect_ratio,
                                    self.gui_scale,
                                    mouse,
                                    element.position.z as f32 + 0.1,
                                    None,
                                );
                            }
                            ItemModel::Block { front, .. } => {
                                Self::add_rect_vertices(
                                    &mut vertices,
                                    element.anchor,
                                    Vec2 {
                                        x: element.position.x as f32,
                                        y: element.position.y as f32,
                                    },
                                    size,
                                    *front,
                                    Color::WHITE,
                                    aspect_ratio,
                                    self.gui_scale,
                                    mouse,
                                    element.position.z as f32 + 0.1,
                                    None,
                                );
                            }
                        }
                        if item_id.1 != 1 {
                            let text_size = self.text_renderer.get_size(20., &item_id.1.to_string());
                            self.text_renderer.render(
                                &mut vertices,
                                element.anchor,
                                Vec2 {
                                    x: element.position.x as f32 + (size.x / 2.) - (text_size.x / 2.),
                                    y: element.position.y as f32 - (size.y / 2.) + (text_size.y / 2.),
                                },
                                20.,
                                &item_id.1.to_string(),
                                Color {
                                    r: 0,
                                    g: 0,
                                    b: 0,
                                    a: 255,
                                },
                                &self.texture_atlas,
                                aspect_ratio,
                                self.gui_scale,
                                mouse,
                                element.position.z as f32 + 0.2,
                                true
                            );
                        }
                    }
                }
                GUIComponent::TextComponent { text, font_size } => {
                    self.text_renderer.render(
                        &mut vertices,
                        element.anchor,
                        Vec2 {
                            x: element.position.x as f32,
                            y: element.position.y as f32,
                        },
                        *font_size,
                        text,
                        Color {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 255,
                        },
                        &self.texture_atlas,
                        aspect_ratio,
                        self.gui_scale,
                        mouse,
                        element.position.z as f32,
                        true
                    );
                }

                GUIComponent::LineEdit { text, size } => {
                    Self::add_rect_vertices(&mut vertices, element.anchor, Vec2 {
                        x: element.position.x as f32,
                        y: element.position.y as f32,
                    }, *size, TexCoords::ZERO, Color{
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    }, aspect_ratio, self.gui_scale, mouse, element.position.z as f32 - 0.5, None);
                    self.text_renderer.render(
                        &mut vertices,
                        element.anchor,
                        Vec2 {
                            x: element.position.x as f32,
                            y: element.position.y as f32,
                        },
                        size.y,
                        text,
                        Color {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 255,
                        },
                        &self.texture_atlas,
                        aspect_ratio,
                        self.gui_scale,
                        mouse,
                        element.position.z as f32,
                        true
                    );
                }
            }
        }
        if let Some((_, element)) = self.get_selected(mouse_physical, size) {
            match &element.component_type {
                GUIComponent::SlotComponent { item_id, .. } => {
                    if let Some((item_id, _)) = item_id.as_ref() {
                        let item = item_registry.get_item(*item_id);
                        self.text_renderer.render(
                            &mut vertices,
                            PositionAnchor::Cursor,
                            Vec2 { x: 0., y: 0. },
                            50.,
                            &item.name,
                            Color {
                                r: 0,
                                g: 0,
                                b: 0,
                                a: 255,
                            },
                            &self.texture_atlas,
                            aspect_ratio,
                            self.gui_scale,
                            mouse,
                            100.,
                            true
                        );
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
        depth: f32,
        slice: Option<&(Vec2, Vec2)>,
    ) {
        let depth = depth / 1000.;
        let anchor = anchor.get_center(mouse);
        let position = Vec2 {
            x: anchor.x + ((center.x - (size.x / 2.)) * gui_scale) / aspect_ratio,
            y: (anchor.y + ((center.y - (size.y / 2.)) * gui_scale)),
        };
        let size = Vec2 {
            x: size.x * gui_scale / aspect_ratio,
            y: size.y * gui_scale,
        };
        let slice = slice.unwrap_or(&(Vec2::ZERO, Vec2 { x: 1., y: 1. }));
        let p1 = Vec2 {
            x: position.x + (size.x * slice.0.x),
            y: position.y + (size.y * slice.0.y),
        };
        let p2 = Vec2 {
            x: position.x + (size.x * slice.1.x),
            y: position.y + (size.y * slice.1.y),
        };
        let uv = uv.map_sub(&TexCoords {
            u1: slice.0.x,
            u2: slice.1.x,
            v1: slice.0.y,
            v2: slice.1.y,
        });
        let vertex_4 = GUIVertex {
            position: [p1.x, p1.y, depth],
            tex_coords: [uv.u1, uv.v2],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_3 = GUIVertex {
            position: [p2.x, p1.y, depth],
            tex_coords: [uv.u2, uv.v2],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_2 = GUIVertex {
            position: [p2.x, p2.y, depth],
            tex_coords: [uv.u2, uv.v1],
            color: (color.r as u32)
                + ((color.g as u32) << 8)
                + ((color.b as u32) << 16)
                + ((color.a as u32) << 24),
        };
        let vertex_1 = GUIVertex {
            position: [p1.x, p2.y, depth],
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
pub struct TextRenderer<'a> {
    pub font: rusttype::Font<'a>,
}
impl<'a> TextRenderer<'a> {
    pub fn get_size(&self, size: f32, text: &str) -> Vec2{
        let layout = self
            .font
            .layout(text, Scale::uniform(size), rusttype::Point { x: 0., y: 0. });
        let glyphs: Vec<_> = layout.collect();
        let width: f32 = glyphs
            .iter()
            .map(|glyph| glyph.unpositioned().h_metrics().advance_width)
            .sum();
        let height = glyphs
            .iter()
            .map(|glyph| {
                glyph
                    .unpositioned()
                    .exact_bounding_box()
                    .map(|bb| -bb.min.y + bb.max.y)
                    .unwrap_or(0.)
            })
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.);
        Vec2{
            x: width,
            y: height
        }
    }
    pub fn render(
        &self,
        vertices: &mut Vec<GUIVertex>,
        anchor: PositionAnchor,
        center: Vec2,
        size: f32,
        text: &String,
        color: Color,
        texture_atlas: &TextureAtlas,
        aspect_ratio: f32,
        gui_scale: f32,
        mouse: Vec2,
        depth: f32,
        background: bool
    ) {
        let layout = self
            .font
            .layout(text, Scale::uniform(size), rusttype::Point { x: 0., y: 0. });
        let glyphs: Vec<_> = layout.collect();
        let width: f32 = glyphs
            .iter()
            .map(|glyph| glyph.unpositioned().h_metrics().advance_width)
            .sum();
        let height = glyphs
            .iter()
            .map(|glyph| {
                glyph
                    .unpositioned()
                    .exact_bounding_box()
                    .map(|bb| -bb.min.y + bb.max.y)
                    .unwrap_or(0.)
            })
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.);
        for glyph in glyphs {
            if let Some(bb) = glyph.unpositioned().exact_bounding_box() {
                let texture = texture_atlas
                    .get(("font_".to_string() + glyph.id().0.to_string().as_str()).as_str());
                let size_x = -bb.min.x + bb.max.x;
                let size_y = -bb.min.y + bb.max.y;
                let x = glyph.position().x + center.x + size_x;
                let y = glyph.position().y - bb.min.y + center.y - (height / 2.);
                GUIRenderer::add_rect_vertices(
                    vertices,
                    anchor,
                    Vec2 {
                        x: x - (size_x / 2.) - (width / 2.),
                        y: y - (size_y / 2.),
                    },
                    Vec2 {
                        x: size_x,
                        y: size_y,
                    },
                    texture,
                    color,
                    aspect_ratio,
                    gui_scale,
                    mouse,
                    depth + 0.1,
                    None,
                );
            }
        }
        if background {
            let border = 5. * 2.;
            GUIRenderer::add_rect_vertices(
                vertices,
                anchor,
                Vec2 {
                    x: center.x,
                    y: center.y,
                },
                Vec2 {
                    x: width + border,
                    y: height + border,
                },
                TexCoords::ZERO,
                Color::WHITE,
                aspect_ratio,
                gui_scale,
                mouse,
                depth,
                None,
            );
        }
    }
}
