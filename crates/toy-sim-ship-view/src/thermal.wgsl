#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, get_tag},
    mesh_view_bindings::view,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

struct ThermalMaterial {
    colors: array<vec4<f32>, 256>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: ThermalMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> shell: vec4<f32>;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) temperature: f32,
    @location(1) @interpolate(flat) strength: f32,
    @location(2) @interpolate(flat) camera: vec3<f32>,
    @location(3) @interpolate(flat) inverse_x: vec4<f32>,
    @location(4) @interpolate(flat) inverse_y: vec4<f32>,
    @location(5) @interpolate(flat) inverse_z: vec4<f32>,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    let world = get_world_from_local(in.instance_index);
    let inverse = get_local_from_world(in.instance_index);
    out.inverse_x = vec4(inverse[0].x, inverse[1].x, inverse[2].x, inverse[3].x);
    out.inverse_y = vec4(inverse[0].y, inverse[1].y, inverse[2].y, inverse[3].y);
    out.inverse_z = vec4(inverse[0].z, inverse[1].z, inverse[2].z, inverse[3].z);
    out.camera = (inverse * vec4(view.world_position, 1.0)).xyz;
    let tag = get_tag(in.instance_index);
    out.temperature = f32(tag & 65535u);
    out.strength = f32(tag >> 16u) / 65535.0;

    var low = vec2(1.0);
    var high = vec2(-1.0);
    var crosses_near = false;
    for (var i = 0u; i < 8u; i += 1u) {
        let corner = vec3(f32(i & 1u), f32((i >> 1u) & 1u), f32((i >> 2u) & 1u)) * 2.0 - 1.0;
        let clip = view.clip_from_world * world * vec4(corner, 1.0);
        crosses_near = crosses_near || clip.w <= 0.0 || clip.z >= clip.w;
        low = min(low, clip.xy / max(clip.w, 0.000001));
        high = max(high, clip.xy / max(clip.w, 0.000001));
    }
    if crosses_near {
        low = vec2(-1.0);
        high = vec2(1.0);
    }
    low = clamp(low, vec2(-1.0), vec2(1.0));
    high = clamp(high, vec2(-1.0), vec2(1.0));
    out.position = vec4(mix(low, high, in.position.xy * 0.5 + 0.5), 0.0, 1.0);
    return out;
}

fn interval_length(begin: f32, end: f32, limit: f32) -> f32 {
    return max(0.0, min(end, limit) - max(begin, 0.0));
}

@fragment
fn fragment(in: Output, @builtin(sample_index) sample: u32) -> @location(0) vec4<f32> {
    let uv = (in.position.xy - view.viewport.xy) / view.viewport.zw;
    let ndc = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let view_ray = view.view_from_clip * vec4(ndc, 1.0, 1.0);
    let world_ray = (view.world_from_view * vec4(normalize(view_ray.xyz), 0.0)).xyz;
    let local_ray = vec3(dot(in.inverse_x.xyz, world_ray), dot(in.inverse_y.xyz, world_ray), dot(in.inverse_z.xyz, world_ray));
    let direction = normalize(local_ray);
    let b = dot(in.camera, direction);
    let closest = in.camera - b * direction;
    let impact_squared = dot(closest, closest);
    let disc = 1.0 - impact_squared;
    let edge_width = max(fwidth(disc), 0.000001);
    let coverage = smoothstep(0.0, edge_width, disc);
    var limit = 1e30;
#ifdef DEPTH_PREPASS
    let depth = prepass_depth(in.position, sample);
    if depth > 0.0 {
        let opaque = view.view_from_clip * vec4(ndc, depth, 1.0);
        limit = length(opaque.xyz / opaque.w) * length(local_ray);
    }
#endif
    let outer_half = sqrt(max(0.0, disc));
    let inner_radius = 1.0 - shell.x;
    let inner_half = sqrt(max(0.0, inner_radius * inner_radius - impact_squared));
    let outer = interval_length(-b - outer_half, -b + outer_half, limit);
    let inner = interval_length(-b - inner_half, -b + inner_half, limit);
    let path = max(0.0, outer - inner);
    let tau = shell.y * in.strength * path / (2.0 * shell.x);
    let absorbed = select(1.0 - exp(-tau), tau * (1.0 - 0.5 * tau), tau < 0.001);
    let alpha = absorbed * coverage;
    let entry = clamp((in.temperature - 300.0) * 255.0 / 9700.0, 0.0, 255.0);
    let low = u32(floor(entry));
    let rgb = mix(material.colors[low].rgb, material.colors[min(low + 1u, 255u)].rgb, fract(entry));
    return vec4(rgb * alpha * view.exposure, alpha);
}
