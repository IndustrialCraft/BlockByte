use crate::game::{ClientPlayer, World};
use crate::texture;
use crate::texture::{TexCoords, Texture};
use block_byte_common::{Face, Position};
use image::RgbaImage;
use std::iter;
use wgpu::util::DeviceExt;
use wgpu::{Buffer, Sampler, TextureView};
use winit::dpi::PhysicalSize;
use winit::window::Window;

pub struct RenderState {
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    window: Window,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: Buffer,
    texture: Texture,
    camera_uniform: CameraUniform,
    camera_buffer: Buffer,
    camera_bind_group: wgpu::BindGroup,
    depth_texture: (wgpu::Texture, Sampler, TextureView),
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
                    features: wgpu::Features::empty(),
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
        let texture = Texture::from_image(&device, &queue, &texture_image, Some("main texture"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
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

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture.texture_bind_group_layout,
                    &camera_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
        let depth_texture = texture::create_depth_texture(&device, &config, "depth_texture");
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            window,
            surface,
            device,
            queue,
            config,
            size,
            render_pipeline,
            vertex_buffer,
            texture,
            camera_uniform,
            camera_buffer,
            camera_bind_group,
            depth_texture,
        }
    }

    pub fn window(&self) -> &Window {
        &self.window
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
    ) -> Result<(), wgpu::SurfaceError> {
        self.camera_uniform
            .update_view_proj(camera, self.size.width as f32 / self.size.height as f32);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

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
                label: Some("Render Pass"),
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
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.texture.diffuse_bind_group, &[]);
            render_pass.set_bind_group(1, &self.camera_bind_group, &[]);
            world.tick(&self.device, &self.queue);
            for chunk in &mut world.chunks {
                if let Some(vertex_buffer) = chunk.1.get_vertices() {
                    render_pass.set_vertex_buffer(0, vertex_buffer.0);
                    render_pass.draw(0..vertex_buffer.1, 0..1);
                }
            }
        }

        self.queue.submit(iter::once(encoder.finish()));
        output.present();

        Ok(())
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

const VERTICES: &[Vertex] = &[
    Vertex {
        position: [0.0, 0.5, 1.0],
        tex_coords: [0.5, 1.],
    },
    Vertex {
        position: [-0.5, -0.5, 1.0],
        tex_coords: [0., 0.],
    },
    Vertex {
        position: [0.5, -0.5, 1.0],
        tex_coords: [1., 0.],
    },
];

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}
impl CameraUniform {
    fn new() -> Self {
        use cgmath::SquareMatrix;
        Self {
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }
    fn update_view_proj(&mut self, camera: &ClientPlayer, aspect_ratio: f32) {
        self.view_proj = (Self::OPENGL_TO_WGPU_MATRIX
            * camera.create_projection_matrix(aspect_ratio)
            * camera.create_view_matrix())
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