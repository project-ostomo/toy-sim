#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, get_tag, mesh_position_local_to_clip},
    mesh_view_bindings::view,
}

struct ThermalMaterial {
    colors: array<vec4<f32>, 256>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: ThermalMaterial;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) temperature: f32,
    @location(1) @interpolate(flat) inside: u32,
    @location(2) @interpolate(flat) strength: f32,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    out.position = mesh_position_local_to_clip(
        get_world_from_local(in.instance_index), vec4(in.position, 1.0),
    );
    let tag = get_tag(in.instance_index);
    out.temperature = f32(tag & 65535u);
    out.strength = f32(tag >> 16u) / 65535.0;
    let camera = (get_local_from_world(in.instance_index) * vec4(view.world_position, 1.0)).xyz;
    out.inside = select(0u, 1u, length(camera) < 1.0);
    return out;
}

@fragment
fn fragment(in: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if front == (in.inside != 0u) || in.temperature < 300.0 {
        discard;
    }

    let entry = clamp((in.temperature - 300.0) * 255.0 / 9700.0, 0.0, 255.0);
    let low = u32(floor(entry));
    let rgb = mix(material.colors[low].rgb, material.colors[min(low + 1u, 255u)].rgb, fract(entry));
    return vec4(rgb * 0.8 * in.strength * view.exposure, 0.0);
}
