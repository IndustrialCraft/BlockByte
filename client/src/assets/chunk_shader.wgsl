// Vertex shader
struct CameraUniform {
    view_proj: mat4x4<f32>,
};
@group(1) @binding(0)
var<uniform> camera: CameraUniform;
@group(2) @binding(0)
var<uniform> time: f32;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) render_data: u32,
    @location(3) animation_shift: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) animation_shift: f32
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    let frame_time = f32((model.render_data>>16u) & 255u);
    let stages = (model.render_data>>24u) & 255u;
    out.animation_shift = model.animation_shift * f32(u32((time*1000.)/(frame_time*16.))%stages);
    var position = model.position;
    if ((model.render_data & 255u) == 1u) && ((model.render_data & 512u) > 0u){
        position.y -= (sin(time + position.x + position.z*2.)+1.)/2. * 0.1;
    }
    if ((model.render_data & 255u) == 2u) && ((model.render_data & 512u) > 0u){
        position.x += sin(time) * 0.1;
        position.z += cos(time) * 0.1;
    }
    out.clip_position = camera.view_proj * vec4<f32>(position, 1.0);
    return out;
}


// Fragment shader

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color: vec4<f32> = textureSample(t_diffuse, s_diffuse, vec2(in.tex_coords.x + in.animation_shift, in.tex_coords.y));
    if color.w == 0.{
        discard;
    }
    return color;
}