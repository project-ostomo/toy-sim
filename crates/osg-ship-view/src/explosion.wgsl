#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, mesh_position_local_to_clip, get_tag},
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var atlas: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var atlas_sampler: sampler;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) brightness: f32,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    out.position = mesh_position_local_to_clip(get_world_from_local(in.instance_index), vec4(in.position, 1.0));
    let tag = get_tag(in.instance_index);
    let tile = f32(tag & 3u);
    out.uv = vec2((tile * 64.0 + 0.5 + in.uv.x * 63.0) / 256.0, (0.5 + in.uv.y * 63.0) / 64.0);
    out.brightness = f32(tag >> 2u) * 64.0;
    return out;
}

@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let texel = textureSample(atlas, atlas_sampler, in.uv);
    return vec4(texel.rgb * texel.a * in.brightness * view.exposure, 0.0);
}
