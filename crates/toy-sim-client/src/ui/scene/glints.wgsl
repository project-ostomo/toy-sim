#import bevy_pbr::{forward_io::Vertex, mesh_view_bindings::view}

struct GlintMaterial { gain: vec4<f32>, }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GlintMaterial;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    let camera_relative = (view.view_from_world * vec4(in.position, 0.0)).xyz;
    out.position = view.clip_from_view * vec4(camera_relative, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let xy = in.uv * 2.0 - 1.0;
    let r2 = dot(xy, xy);
    let psf = (exp(-48.0 * r2) + 0.08 * exp(-8.0 * r2)) * (1.0 - smoothstep(0.8, 1.0, r2));
    return vec4(in.color.rgb * material.gain.rgb * psf, 0.0);
}
