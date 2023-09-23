use image::{DynamicImage, Rgba, RgbaImage};
use rusttype::{GlyphId, Point, Scale};
use std::collections::HashMap;
use std::path::Path;
use texture_packer::exporter::ImageExporter;
use texture_packer::importer::ImageImporter;
use wgpu::{BindGroup, BindGroupLayout};

pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub texture_bind_group_layout: BindGroupLayout,
    pub diffuse_bind_group: BindGroup,
}

impl Texture {
    pub fn from_image(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &RgbaImage,
        label: Option<&str>,
    ) -> Self {
        let dimensions = rgba.dimensions();
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                aspect: wgpu::TextureAspect::All,
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
            },
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * dimensions.0),
                rows_per_image: Some(dimensions.1),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("texture_bind_group_layout"),
            });

        let diffuse_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("diffuse_bind_group"),
        });

        Self {
            texture,
            view,
            sampler,
            texture_bind_group_layout,
            diffuse_bind_group,
        }
    }
}

pub fn pack_textures(
    textures: Vec<(String, Vec<u8>)>,
    font: &rusttype::Font,
) -> (TextureAtlas, RgbaImage) {
    let mut texture_map = HashMap::new();
    let mut packer =
        texture_packer::TexturePacker::new_skyline(texture_packer::TexturePackerConfig {
            max_width: 2048,
            max_height: 2048,
            allow_rotation: false,
            texture_outlines: false,
            border_padding: 0,
            texture_padding: 0,
            trim: false,
            texture_extrusion: 0,
        });
    for (name, data) in textures {
        if let Ok(texture) = ImageImporter::import_from_memory(data.as_slice()) {
            packer.pack_own(name, texture).unwrap();
        }
    }
    {
        let glyphs: Vec<_> = (0..font.glyph_count())
            .map(|i| {
                font.glyph(GlyphId(i as u16))
                    .scaled(Scale::uniform(30.))
                    .positioned(Point { x: 0., y: 0. })
            })
            .collect();
        for g in glyphs.iter().enumerate() {
            if let Some(bb) = g.1.pixel_bounding_box() {
                let mut font_texture =
                    DynamicImage::new_rgba8(bb.width() as u32, bb.height() as u32);
                let font_buffer = match &mut font_texture {
                    DynamicImage::ImageRgba8(buffer) => buffer,
                    _ => panic!(),
                };
                g.1.draw(|x, y, v| {
                    font_buffer.put_pixel(x, y, Rgba([0, 0, 0, (v * 255f32) as u8]));
                    //font_buffer.put_pixel(x, y, Rgba([(v * 255f32) as u8, 0, 0, 255]));
                });
                packer
                    .pack_own("font_".to_string() + g.0.to_string().as_str(), font_texture)
                    .unwrap();
            }
        }
    }
    packer
        .pack_own(
            "missing".to_string(),
            ImageImporter::import_from_memory(include_bytes!("assets/missing.png"))
                .expect("missing texture corrupted"),
        )
        .unwrap();
    for (name, frame) in packer.get_frames() {
        use texture_packer::texture::Texture;

        let texture = TexCoords {
            u1: frame.frame.x as f32 / packer.width() as f32,
            v1: frame.frame.y as f32 / packer.height() as f32,
            u2: (frame.frame.x + frame.frame.w) as f32 / packer.width() as f32,
            v2: (frame.frame.y + frame.frame.h) as f32 / packer.height() as f32,
        };
        texture_map.insert(name.to_string(), texture);
    }
    let exporter = ImageExporter::export(&packer).unwrap();
    exporter.save(Path::new("textureatlasdump.png")).unwrap();
    (
        TextureAtlas {
            missing_texture: texture_map.get("missing").unwrap().clone(),
            textures: texture_map,
        },
        exporter.to_rgba8(),
    )
}
#[derive(Copy, Clone)]
pub struct TexCoords {
    pub u1: f32,
    pub v1: f32,
    pub u2: f32,
    pub v2: f32,
}
pub struct TextureAtlas {
    textures: HashMap<String, TexCoords>,
    missing_texture: TexCoords,
}
impl TextureAtlas {
    pub fn get(&self, texture: &str) -> TexCoords {
        *self.textures.get(texture).unwrap_or(&self.missing_texture)
    }
}
