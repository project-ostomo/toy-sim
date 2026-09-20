#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_tag},
    mesh_view_bindings::view,
}

struct GlintMaterial { gain: vec4<f32>, }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GlintMaterial;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) luminance: f32,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    let world = get_world_from_local(in.instance_index);
    let centre = view.clip_from_world * world[3];
    let offset = in.position.xy * 8.0 / max(view.viewport.zw, vec2(1.0));
    out.position = vec4(centre.xy + offset * centre.w, centre.zw);
    out.uv = in.uv;
    out.luminance = bitcast<f32>(get_tag(in.instance_index));
    return out;
}

@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let xy = in.uv * 2.0 - 1.0;
    let r2 = dot(xy, xy);
    let psf = (exp(-48.0 * r2) + 0.08 * exp(-8.0 * r2)) * (1.0 - smoothstep(0.8, 1.0, r2));
    return vec4(vec3(1.0, 0.95, 0.88) * in.luminance * material.gain.rgb * psf, 0.0);
}
