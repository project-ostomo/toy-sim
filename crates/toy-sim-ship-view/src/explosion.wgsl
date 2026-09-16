#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, mesh_position_local_to_clip},
    mesh_view_bindings::view,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> radiance: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> optical: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> flash: vec4<f32>;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec3<f32>,
    @location(1) @interpolate(flat) camera: vec3<f32>,
}
@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    out.position = mesh_position_local_to_clip(get_world_from_local(in.instance_index), vec4(in.position, 1.0));
    out.local = in.position;
    out.camera = (get_local_from_world(in.instance_index) * vec4(view.world_position, 1.0)).xyz;
    return out;
}
@fragment
fn fragment(in: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let inside = length(in.camera) < 1.0;
    if front == inside { discard; }
    let direction = normalize(in.local - in.camera);
    let b = dot(in.camera, direction);
    let c = dot(in.camera, in.camera) - 1.0;
    let disc = b * b - c;
    if disc <= 0.0 { discard; }
    let begin = max(0.0, -b - sqrt(disc));
    let end = -b + sqrt(disc);
    let step = (end - begin) / 16.0;
    var column = 0.0;
    for (var i = 0u; i < 16u; i += 1u) {
        let p = in.camera + direction * (begin + (f32(i) + 0.5) * step);
        let lobes = 0.7 + 0.3 * sin(p.x * 11.0 + optical.y) * sin(p.y * 8.0 - p.z * 7.0);
        column += exp(-3.0 * dot(p, p)) * lobes * step;
    }
    // A compact prompt flash keeps its original radius and is independent of
    // the cloud's declining optical depth. Its power is integrated per frame.
    let closest = in.camera + direction * max(0.0, -b);
    let flash_mask = select(0.0, 1.0, dot(closest, closest) <= optical.z * optical.z);
    let light = radiance.rgb * (1.0 - exp(-optical.x * column)) + flash.rgb * flash_mask;
    return vec4(light * view.exposure, 0.0);
}
