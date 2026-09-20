#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, get_tag, mesh_position_local_to_clip},
    mesh_view_bindings::{view, globals},
}

struct PlumeMaterial {
    shape: vec4<f32>,
    emission: vec4<f32>,
    falloff: vec4<f32>,
    animation: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> plume: PlumeMaterial;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) local_position: vec3<f32>,
    @location(1) @interpolate(flat) camera: vec3<f32>,
    @location(2) @interpolate(flat) thrust: f32,
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    out.position = mesh_position_local_to_clip(get_world_from_local(in.instance_index), vec4(in.position, 1.0));
    out.local_position = in.position;
    out.camera = (get_local_from_world(in.instance_index) * vec4(view.world_position, 1.0)).xyz;
    out.thrust = clamp(bitcast<f32>(get_tag(in.instance_index)), 0.0, 1.0);
    return out;
}

@fragment
fn fragment(in: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let bounds = vec3(plume.shape.w, plume.shape.w, plume.shape.x * 0.5);
    let inside = all(abs(in.camera) < bounds);
    if front == inside { discard; }
    let direction = normalize(in.local_position - in.camera);
    // Slab intersection in part-local metres, including camera-inside views.
    let safe_direction = select(vec3(-1.0), vec3(1.0), direction >= vec3(0.0)) * max(abs(direction), vec3(0.000001));
    let a = (-bounds - in.camera) / safe_direction;
    let b = (bounds - in.camera) / safe_direction;
    let near = min(a, b);
    let far = max(a, b);
    let start = max(0.0, max(near.x, max(near.y, near.z)));
    let end = min(far.x, min(far.y, far.z));
    if end <= start || in.thrust <= 0.0 { discard; }

    // An optically thin emitting volume, sampled only within a small proxy box.
    // Shorter and dimmer at low thrust; no rigid cone surface or particles.
    let length_m = plume.shape.x * (0.25 + 0.75 * sqrt(in.thrust));
    let step_m = (end - start) / 32.0;
    var emission = 0.0;
    for (var i = 0u; i < 32u; i += 1u) {
        let p = in.camera + direction * (start + (f32(i) + 0.5) * step_m) + vec3(0.0, 0.0, bounds.z);
        let radius = plume.shape.y + max(p.z, 0.0) * plume.shape.z;
        let axial = clamp(1.0 - p.z / length_m, 0.0, 1.0);
        let radial = clamp(1.0 - length(p.xy) / radius, 0.0, 1.0);
        let flow = (p.z - globals.time * plume.animation.x) / plume.falloff.w;
        let noise = 1.0 + plume.falloff.z * sin(flow * 6.283185) * sin(dot(p.xy / plume.falloff.w, vec2(2.7, 3.1)) + flow * 1.7);
        emission += pow(axial, plume.falloff.x) * pow(radial, plume.falloff.y) * noise * step_m;
    }
    // Normalize the path integral to nozzle diameter. Additive HDR emission
    // passes through Bevy's existing bloom and tonemapping pipeline.
    let rgb = plume.emission.rgb * plume.emission.a * view.exposure * in.thrust * emission / (2.0 * plume.shape.y);
    // Bevy implements Add with premultiplied-alpha blending: RGB + (1-A)*dst.
    return vec4(rgb, 0.0);
}
