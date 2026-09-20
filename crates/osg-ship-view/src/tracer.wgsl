#import bevy_pbr::{forward_io::Vertex, mesh_view_bindings::view}

struct TracerMaterial { emission: vec4<f32>, }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: TracerMaterial;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}
@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    // CPU vertices are relative to this camera. Rotate without subtracting two
    // large world translations in the shader.
    let camera_relative = (view.view_from_world * vec4(in.position, 0.0)).xyz;
    out.position = view.clip_from_view * vec4(camera_relative, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}
@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let across = abs(in.uv.y * 2.0 - 1.0);
    let glow = exp(-4.0 * across * across) + 3.0 * exp(-80.0 * across * across);
    let along = 0.2 + 0.8 * in.uv.x;
    return vec4(material.emission.rgb * material.emission.a * in.color.rgb * in.color.a
        * glow * along * view.exposure, 0.0);
}
