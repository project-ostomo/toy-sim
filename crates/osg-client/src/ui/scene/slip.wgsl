#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, mesh_position_local_to_clip},
    mesh_view_bindings::view,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> parameters: vec4<f32>;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec3<f32>,
    @location(1) @interpolate(flat) camera: vec3<f32>,
}
@vertex
fn vertex(input: Vertex) -> Output {
    var out: Output;
    out.position = mesh_position_local_to_clip(get_world_from_local(input.instance_index), vec4(input.position, 1.0));
    out.local = input.position;
    out.camera = (get_local_from_world(input.instance_index) * vec4(view.world_position, 1.0)).xyz;
    return out;
}
fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}
fn noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(cell), hash(cell + vec2(1.0, 0.0)), u.x),
        mix(hash(cell + vec2(0.0, 1.0)), hash(cell + vec2(1.0)), u.x), u.y);
}
fn streak(angle: f32, depth: f32, phase: f32, lanes: f32) -> f32 {
    let lane = angle * lanes;
    let seed = hash(vec2(floor(lane), lanes));
    let z = depth + phase * 10.0 + seed * 71.0;
    let width = max(fwidth(lane), 0.035);
    let core = exp(-pow((fract(lane) - 0.5) / width, 2.0));
    let tail = smoothstep(0.02, 0.12, fract(z)) * (1.0 - smoothstep(0.3, 0.95, fract(z)));
    let resolved = 1.0 - smoothstep(0.3, 0.9, fwidth(z));
    // At high speed integrate unresolved motion into a continuous streak.
    let motion_blur = smoothstep(0.2, 1.0, parameters.w * 10.0);
    return core * mix(tail, 0.55, motion_blur) * resolved * step(0.25, seed);
}
@fragment
fn fragment(input: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let mode = parameters.z;
    let t = parameters.x;
    let q = parameters.y;
    let inside = length(input.camera) < 1.0;
    if front == inside { discard; }
    let n = normalize(input.local);
    let angle = atan2(n.y, n.x) / 6.283185 + 0.5;
    let circle = vec2(cos(angle * 6.283185), sin(angle * 6.283185));
    var color = vec3(0.0);
    var alpha = 0.0;
    if mode < 1.5 {
        let ray = normalize(input.local - input.camera);
        let radial = max(length(ray.xy), 0.015);
        let depth = clamp(ray.z / radial, -65.0, 65.0);
        let a = atan2(ray.y, ray.x) / 6.283185 + 0.5;
        let flow = depth * 0.38 + t * 1.6;
        let cloud = noise(circle * 4.0 + vec2(flow, flow * 0.37));
        let filaments = streak(a, depth * 0.72, t, 251.0)
            + 0.6 * streak(a, depth * 1.47, t, 397.0);
        let folds = pow(clamp(cloud * 1.5 - 0.28, 0.0, 1.0), 3.0);
        color = vec3(0.008, 0.025, 0.095) + vec3(0.05, 0.25, 0.8) * folds * 3.0
            + vec3(0.65, 0.88, 1.0) * filaments * 3.0;
        let coordinate = n.z * 0.5 + 0.5 + (noise(circle * 7.0 + vec2(t)) - 0.5) * 0.12;
        alpha = smoothstep(coordinate - 0.08, coordinate + 0.08, q * 1.3 - 0.15);
        if mode > 0.5 {
            let front_band = exp(-pow((coordinate - (q * 1.3 - 0.15)) * 8.0, 2.0));
            let threads = pow(noise(circle * 24.0 + vec2(n.z * 6.0 + t * 5.0, t)), 5.0);
            alpha = (front_band * 0.65 + threads * 0.8) * sin(clamp(q, 0.0, 1.0) * 3.141593);
            color = vec3(0.25, 0.7, 1.0) * (2.0 + front_band * 3.0 + threads * 6.0);
        }
    } else if mode < 2.5 {
        let longitudinal = 1.0 - abs(input.local.z);
        let stripes = pow(0.5 + 0.5 * sin(angle * 87.9646 + input.local.z * 10.0 - t * 15.0), 8.0);
        alpha = pow(max(0.0, 1.0 - q / 3.0), 2.0) * longitudinal * (0.3 + stripes * 0.7);
        color = vec3(0.1, 0.6, 1.0) * (2.0 + stripes * 6.0);
    } else {
        let facing = abs(dot(n, normalize(input.camera - input.local)));
        alpha = pow(max(0.0, 1.0 - q / 0.35), 2.0) * facing;
        color = vec3(0.6, 0.85, 1.0) * 20.0;
    }
    return vec4(color * 35000.0 * view.exposure, clamp(alpha, 0.0, 1.0));
}
