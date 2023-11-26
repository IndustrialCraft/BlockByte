// Vertex shader
// Vertex shader

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) tex_coords: vec2<f32>,
    @location(2) color: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
    @location(1) color: u32
}

@vertex
fn vs_main(
    model: VertexInput,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coords = model.tex_coords;
    out.clip_position = vec4<f32>(model.position, 1.0); // 2.
    out.color = model.color;
    return out;
}


// Fragment shader

@group(0) @binding(0)
var t_diffuse: texture_2d<f32>;
@group(0)@binding(1)
var s_diffuse: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coloring = vec4(f32(in.color&0xFFu)/255.,f32((in.color>>8u)&0xFFu)/255.,f32((in.color>>16u)&0xFFu)/255.,f32((in.color>>24u)&0xFFu)/255.);
    let texture = textureSample(t_diffuse, s_diffuse, in.tex_coords);
    if in.tex_coords.x == 0. && in.tex_coords.y == 0.{
        return coloring;
    } else {
        let color: vec4<f32> = texture * coloring;
        if color.w == 0.{
            discard;
        }
        return color;
    }
}