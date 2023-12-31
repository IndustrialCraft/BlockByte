use crate::content::{EntityRegistry, ItemRegistry, Texture};
use crate::game::{ClientPlayer, World};
use crate::gui::GUIRenderer;
use crate::model::{Model, ModelInstanceData};
use crate::texture;
use crate::texture::GPUTexture;
use block_byte_common::{Face, Position, TexCoords, Vec3, AABB};
use cgmath::{Matrix4, SquareMatrix};
use image::RgbaImage;
use std::iter;
use std::mem::size_of;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    BindGroup, BlendState, Buffer, BufferUsages, CommandEncoder, Device, LoadOp, Queue, Sampler,
    TextureView,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

pub struct RenderState {
    surface: wgpu::Surface,
    device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    window: Window,
    chunk_render_pipeline: wgpu::RenderPipeline,
    chunk_transparent_render_pipeline: wgpu::RenderPipeline,
    chunk_foliage_render_pipeline: wgpu::RenderPipeline,
    gui_render_pipeline: wgpu::RenderPipeline,
    model_render_pipeline: wgpu::RenderPipeline,
    pub(crate) outline_renderer: OutlineRenderer,
    texture: GPUTexture,
    camera_uniform: CameraUniform,
    camera_buffer: Buffer,
    camera_bind_group: wgpu::BindGroup,
    time_buffer: Buffer,
    time_bind_group: wgpu::BindGroup,
    depth_texture: (wgpu::Texture, Sampler, TextureView),
    pub(crate) mouse: PhysicalPosition<f64>,
}

impl RenderState {
    pub(crate) async fn new(window: Window, texture_image: RgbaImage) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            dx12_shader_compiler: Default::default(),
        });
        let surface = unsafe { instance.create_surface(&window) }.unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    features: wgpu::Features::DEPTH_CLIP_CONTROL,
                    limits: if cfg!(target_arch = "wasm32") {
                        wgpu::Limits::downlevel_webgl2_defaults()
                    } else {
                        wgpu::Limits::default()
                    },
                    label: None,
                },
                None,
            )
            .await
            .unwrap();
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let texture = GPUTexture::from_image(&device, &queue, &texture_image, Some("main texture"));
        let chunk_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Chunk Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("assets/chunk_shader.wgsl").into()),
        });
        let model_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Model Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("assets/model_shader.wgsl").into()),
        });
        let gui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GUI Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("assets/gui_shader.wgsl").into()),
        });
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GUI Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("assets/outline_shader.wgsl").into()),
        });
        let camera_uniform = CameraUniform::new();
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("camera_bind_group_layout"),
            });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
            label: Some("camera_bind_group"),
        });
        let time_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Time Buffer"),
            contents: bytemuck::cast_slice(&[0f32]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let time_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("time_bind_group_layout"),
            });
        let time_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &time_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: time_buffer.as_entire_binding(),
            }],
            label: Some("time_bind_group"),
        });
        let depth_texture = texture::create_depth_texture(&device, &config, "depth_texture");
        let chunk_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Chunk Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                    &time_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let model_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Model Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let chunk_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Chunk Render Pipeline"),
                layout: Some(&chunk_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &chunk_shader,
                    entry_point: "vs_main",
                    buffers: &[ChunkVertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &chunk_shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
            });
        let chunk_transparent_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Chunk Transparent Render Pipeline"),
                layout: Some(&chunk_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &chunk_shader,
                    entry_point: "vs_main",
                    buffers: &[ChunkVertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &chunk_shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
            });
        let chunk_foliage_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Chunk Foliage Render Pipeline"),
                layout: Some(&chunk_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &chunk_shader,
                    entry_point: "vs_main",
                    buffers: &[ChunkVertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &chunk_shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
            });
        let gui_render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GUI Render Pipeline Layout"),
                bind_group_layouts: &[&texture.texture_bind_group_layout],
                push_constant_ranges: &[],
            });
        let gui_render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("GUI Render Pipeline"),
            layout: Some(&gui_render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &gui_shader,
                entry_point: "vs_main",
                buffers: &[GUIVertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &gui_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: true,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });
        let model_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Model Render Pipeline"),
                layout: Some(&model_render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &model_shader,
                    entry_point: "vs_main",
                    buffers: &[Vertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &model_shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
            });
        let outline_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Outline Render Pipeline Layout"),
                bind_group_layouts: &[&camera_bind_group_layout],
                push_constant_ranges: &[],
            });
        let outline_render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Outline Render Pipeline"),
                layout: Some(&outline_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &outline_shader,
                    entry_point: "vs_main",
                    buffers: &[OutlineVertex::desc()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &outline_shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview: None,
            });
        Self {
            window,
            surface,
            queue,
            config,
            size,
            chunk_render_pipeline,
            chunk_transparent_render_pipeline,
            chunk_foliage_render_pipeline,
            gui_render_pipeline,
            model_render_pipeline,
            outline_renderer: OutlineRenderer::new(outline_render_pipeline, &device),
            texture,
            camera_uniform,
            camera_buffer,
            camera_bind_group,
            time_bind_group,
            time_buffer,
            depth_texture,
            mouse: PhysicalPosition::new(0., 0.),
            device,
        }
    }

    pub fn window(&self) -> &Window {
        &self.window
    }
    pub fn device(&self) -> &Device {
        &self.device
    }
    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.depth_texture =
                texture::create_depth_texture(&self.device, &self.config, "depth_texture");
        }
    }

    pub fn render(
        &mut self,
        camera: &ClientPlayer,
        world: &mut World,
        gui: &mut GUIRenderer,
        item_registry: &ItemRegistry,
        entity_registry: &EntityRegistry,
        viewmodel: Option<(&Model, &ModelInstanceData)>,
        time: f32,
    ) -> Result<(), wgpu::SurfaceError> {
        self.camera_uniform
            .load_view_proj_matrix(camera, self.size.width as f32 / self.size.height as f32);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );
        self.queue
            .write_buffer(&self.time_buffer, 0, bytemuck::cast_slice(&[time]));

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.2,
                            b: 0.3,
                            a: 1.0,
                        }),
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.chunk_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            render_pass.set_bind_group(2, &self.time_bind_group, &[]);

            world.tick(&self.device);
            for chunk in &mut world.chunks {
                if let Some(vertex_buffer) = chunk.1.get_vertices().0 {
                    render_pass.set_vertex_buffer(0, vertex_buffer.0);
                    render_pass.draw(0..vertex_buffer.1, 0..1);
                }
            }
        }
        let (model_buffer, model_vertex_count) = {
            let mut vertices = Vec::new();
            for (block_position, dynamic_block_data) in &world.dynamic_blocks {
                let dynamic_data = world
                    .block_registry
                    .get_block(dynamic_block_data.id)
                    .dynamic
                    .as_ref()
                    .unwrap();
                dynamic_data.add_vertices(
                    Matrix4::identity(),
                    &dynamic_block_data.model_instance,
                    Some(item_registry),
                    &mut |position, coords| {
                        vertices.push(Vertex {
                            position: [
                                (block_position.x as f64 + position.x) as f32 + 0.5,
                                (block_position.y as f64 + position.y) as f32,
                                (block_position.z as f64 + position.z) as f32 + 0.5,
                            ],
                            tex_coords: [coords.0, coords.1],
                        })
                    },
                );
            }
            for (_, entity) in &world.entities {
                let entity_data = entity_registry.get_entity(entity.type_id);
                entity_data.model.add_vertices(
                    Model::create_matrix_trs(
                        &Vec3 {
                            x: (entity.position.x + (entity_data.hitbox_w / 2.)) as f32,
                            y: entity.position.y as f32,
                            z: (entity.position.z + (entity_data.hitbox_d / 2.)) as f32,
                        },
                        &Vec3 {
                            x: 0.,
                            y: (entity.rotation + 180.).to_radians(),
                            z: 0.,
                        },
                        &Vec3::ZERO,
                        &Vec3::ONE,
                    ),
                    &entity.model_instance,
                    Some(item_registry),
                    &mut |position, coords| {
                        vertices.push(Vertex {
                            position: [position.x as f32, position.y as f32, position.z as f32],
                            tex_coords: [coords.0, coords.1],
                        })
                    },
                );
            }
            let buffer = self.device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Model Buffer"),
                usage: BufferUsages::VERTEX,
                contents: bytemuck::cast_slice(vertices.as_slice()),
            });
            (buffer, vertices.len() as u32)
        };
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Model Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.model_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);

            render_pass.set_vertex_buffer(0, model_buffer.slice(..));
            render_pass.draw(0..model_vertex_count, 0..1);
        }
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Foliage Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: LoadOp::Load,
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: LoadOp::Load,
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.chunk_foliage_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            render_pass.set_bind_group(2, &self.time_bind_group, &[]);
            for chunk in &mut world.chunks {
                if let Some(vertex_buffer) = chunk.1.get_vertices().2 {
                    render_pass.set_vertex_buffer(0, vertex_buffer.0);
                    render_pass.draw(0..vertex_buffer.1, 0..1);
                }
            }
        }
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Transparent Chunk Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: LoadOp::Load,
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: LoadOp::Load,
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.chunk_transparent_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            render_pass.set_bind_group(2, &self.time_bind_group, &[]);

            for chunk in &mut world.chunks {
                if let Some(vertex_buffer) = chunk.1.get_vertices().1 {
                    render_pass.set_vertex_buffer(0, vertex_buffer.0);
                    render_pass.draw(0..vertex_buffer.1, 0..1);
                }
            }
        }
        self.outline_renderer
            .render(&mut encoder, &view, &self.camera_bind_group);

        self.queue.submit(iter::once(encoder.finish()));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let viewmodel = {
            match viewmodel {
                Some((viewmodel, viewmodel_instance)) => {
                    let mut vertices = Vec::new();
                    viewmodel.add_vertices(
                        Model::create_matrix_trs(&Vec3::ZERO, &Vec3::ZERO, &Vec3::ZERO, &Vec3::ONE),
                        viewmodel_instance,
                        Some(item_registry),
                        &mut |position, coords| {
                            vertices.push(Vertex {
                                position: [position.x as f32, position.y as f32, position.z as f32],
                                tex_coords: [coords.0, coords.1],
                            })
                        },
                    );
                    let buffer = self.device.create_buffer_init(&BufferInitDescriptor {
                        label: Some("ViewModel Buffer"),
                        usage: BufferUsages::VERTEX,
                        contents: bytemuck::cast_slice(vertices.as_slice()),
                    });
                    Some((buffer, vertices.len() as u32))
                }
                None => None,
            }
        };
        if let Some(viewmodel) = &viewmodel {
            self.camera_uniform
                .load_viewmodel_matrix(self.size.width as f32 / self.size.height as f32);
            self.queue.write_buffer(
                &self.camera_buffer,
                0,
                bytemuck::cast_slice(&[self.camera_uniform]),
            );
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ViewModel Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.model_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);

            render_pass.set_vertex_buffer(0, viewmodel.0.slice(..));
            render_pass.draw(0..viewmodel.1, 0..1);
        }

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GUI Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.2,
                    depth_ops: Some(wgpu::Operations {
                        load: LoadOp::Clear(0.),
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });
            render_pass.set_pipeline(&self.gui_render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            let (buffer, vertex_count) =
                gui.draw(&self.device, item_registry, self.mouse, self.size);
            render_pass.set_vertex_buffer(0, buffer);
            render_pass.draw(0..vertex_count, 0..1);
        }

        self.queue.submit(iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}
pub struct OutlineRenderer {
    buffer: Buffer,
    render: AtomicBool,
    pipeline: wgpu::RenderPipeline,
}
impl OutlineRenderer {
    pub fn new(pipeline: wgpu::RenderPipeline, device: &Device) -> Self {
        Self {
            pipeline,
            buffer: device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Outline Buffer"),
                contents: vec![0u8; 24 * size_of::<OutlineVertex>()].as_slice(),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            }),
            render: AtomicBool::new(false),
        }
    }
    pub fn set_aabb(&self, aabb: Option<AABB>, queue: &Queue) {
        self.render.store(aabb.is_some(), Relaxed);
        if let Some(aabb) = aabb {
            let p000 = OutlineVertex {
                position: [aabb.x as f32, aabb.y as f32, aabb.z as f32],
            };
            let p001 = OutlineVertex {
                position: [aabb.x as f32, aabb.y as f32, (aabb.z + aabb.d) as f32],
            };
            let p010 = OutlineVertex {
                position: [aabb.x as f32, (aabb.y + aabb.h) as f32, aabb.z as f32],
            };
            let p011 = OutlineVertex {
                position: [
                    aabb.x as f32,
                    (aabb.y + aabb.h) as f32,
                    (aabb.z + aabb.d) as f32,
                ],
            };
            let p100 = OutlineVertex {
                position: [(aabb.x + aabb.w) as f32, aabb.y as f32, aabb.z as f32],
            };
            let p101 = OutlineVertex {
                position: [
                    (aabb.x + aabb.w) as f32,
                    aabb.y as f32,
                    (aabb.z + aabb.d) as f32,
                ],
            };
            let p110 = OutlineVertex {
                position: [
                    (aabb.x + aabb.w) as f32,
                    (aabb.y + aabb.h) as f32,
                    aabb.z as f32,
                ],
            };
            let p111 = OutlineVertex {
                position: [
                    (aabb.x + aabb.w) as f32,
                    (aabb.y + aabb.h) as f32,
                    (aabb.z + aabb.d) as f32,
                ],
            };
            let vertices = vec![
                p000, p001, p001, p101, p101, p100, p100, p000, p010, p011, p011, p111, p111, p110,
                p110, p010, p000, p010, p100, p110, p101, p111, p001, p011,
            ];

            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&vertices));
        }
    }
    pub fn render(
        &self,
        encoder: &mut CommandEncoder,
        view: &TextureView,
        camera_bind_group: &BindGroup,
    ) {
        if !self.render.load(Relaxed) {
            return;
        }
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Outline Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: true,
                },
            })],
            depth_stencil_attachment: None,
        });
        render_pass.set_bind_group(0, camera_bind_group, &[]);
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_vertex_buffer(0, self.buffer.slice(..));
        render_pass.draw(0..24, 0..1);
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub render_data: u32,
    pub animation_shift: f32
}
impl ChunkVertex{
    pub fn new(position: Position, coords: [f32;2], render_data: u32, texture: Texture) -> Self{
        match texture{
            Texture::Static { .. } => {
                ChunkVertex{
                    position: [position.x as f32,position.y as f32,position.z as f32],
                    tex_coords: coords,
                    animation_shift: 0.,
                    render_data
                }
            }
            Texture::Animated { stages, time, .. } => {
                ChunkVertex{
                    position: [position.x as f32,position.y as f32,position.z as f32],
                    tex_coords: coords,
                    animation_shift: texture.get_shift(),
                    render_data: render_data | ((stages as u32) << 24) | ((time as u32) << 16),
                }
            }
        }

    }
}
impl ChunkVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32, 3 => Float32];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
}
impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GUIVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub color: u32,
}
impl GUIVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OutlineVertex {
    pub position: [f32; 3],
}
impl OutlineVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;

        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}
impl CameraUniform {
    fn new() -> Self {
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }
    fn load_view_proj_matrix(&mut self, camera: &ClientPlayer, aspect_ratio: f32) {
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * ClientPlayer::create_projection_matrix(aspect_ratio)
            * camera.create_view_matrix())
        .into();
    }
    fn load_viewmodel_matrix(&mut self, aspect_ratio: f32) {
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * ClientPlayer::create_projection_matrix(aspect_ratio)
            * ClientPlayer::create_default_view_matrix())
        .into();
    }
    #[rustfmt::skip]
    pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 0.5, 0.5,
        0.0, 0.0, 0.0, 1.0,
    );
}
pub trait FaceVerticesExtension {
    fn add_vertices<F>(&self, coords: TexCoords, vertex_consumer: &mut F)
    where
        F: FnMut(Position, (f32, f32));
}
impl FaceVerticesExtension for Face {
    fn add_vertices<F>(&self, coords: TexCoords, vertex_consumer: &mut F)
    where
        F: FnMut(Position, (f32, f32)),
    {
        let (first, second, third, fourth) = match self {
            Face::Front => (
                Position {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Position {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Back => (
                Position {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Position {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Position {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Up => (
                Position {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Position {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
            ),
            Face::Down => (
                Position {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Position {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
            Face::Left => (
                Position {
                    x: 0.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 0.,
                    y: 1.,
                    z: 1.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 1.,
                },
                Position {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
            ),
            Face::Right => (
                Position {
                    x: 1.,
                    y: 1.,
                    z: 1.,
                },
                Position {
                    x: 1.,
                    y: 1.,
                    z: 0.,
                },
                Position {
                    x: 1.,
                    y: 0.,
                    z: 0.,
                },
                Position {
                    x: 1.,
                    y: 0.,
                    z: 1.,
                },
            ),
        };
        vertex_consumer.call_mut((first, (coords.u1, coords.v1)));
        vertex_consumer.call_mut((fourth, (coords.u1, coords.v2)));
        vertex_consumer.call_mut((third, (coords.u2, coords.v2)));

        vertex_consumer.call_mut((third, (coords.u2, coords.v2)));
        vertex_consumer.call_mut((second, (coords.u2, coords.v1)));
        vertex_consumer.call_mut((first, (coords.u1, coords.v1)));
    }
}
