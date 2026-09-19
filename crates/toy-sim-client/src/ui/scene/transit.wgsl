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
@fragment
fn fragment(input: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let inside = length(input.camera) < 1.0;
    if front == inside { discard; }
    let n = normalize(input.local);
    let t = parameters.x;
    let azimuth = atan2(n.y, n.x) / 6.283185 + 0.5;
    if parameters.y > 0.5 {
        let axial = acos(clamp(n.z, -1.0, 1.0)) / 3.141593;
        let bend = 0.018 * sin(axial * 15.0 - t * 0.6);
        let cell = floor(vec2((azimuth + bend) * 210.0, axial * 8.0 + t * 2.4));
        let f = fract(vec2((azimuth + bend) * 210.0, axial * 8.0 + t * 2.4));
        let seed = hash(cell);
        let streak = pow(max(0.0, 1.0 - abs(f.x - 0.5) * 2.0), 18.0) * smoothstep(0.0, 0.1, f.y) * pow(1.0 - f.y, 2.0) * step(0.62, seed);
        let cloud = pow(0.5 + 0.5 * sin(azimuth * 36.0 + axial * 20.0 - t * 1.4 + sin(axial * 33.0 + t)), 5.0);
        let color = vec3(0.008, 0.019, 0.06) + vec3(0.025, 0.10, 0.22) * cloud + vec3(0.7, 0.85, 1.0) * streak * 4.0;
        return vec4(color * parameters.z, 1.0);
    }
    let axis = normalize(input.camera + vec3(0.0, 0.0, 0.00001));
    let reference = select(vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0), abs(axis.y) > 0.95);
    let right = normalize(cross(reference, axis));
    let up = cross(axis, right);
    let disc = vec2(dot(n, right), dot(n, up));
    let radius = length(disc);
    let angle = atan2(disc.y, disc.x);
    let turbulence = sin(radius * 27.0 - t * 0.65) * 0.4;
    let phase = angle * 5.0 - radius * 19.0 + turbulence - t * 1.3;
    let curl = 0.5 + 0.5 * sin(phase);
    let width = max(fwidth(curl), 0.025);
    let filaments = smoothstep(0.76 - width, 0.90 + width, curl);
    let veil = 0.5 + 0.5 * sin(angle * 3.0 + radius * 31.0 + t * 0.8);
    let core = pow(max(0.0, 1.0 - radius), 6.0);
    let edge = smoothstep(0.7, 0.99, radius);
    let spiral = filaments * smoothstep(0.06, 0.3, radius) * (1.0 - edge * 0.7);
    let emission = vec3(0.015, 0.08, 0.24) * (1.0 + veil)
        + vec3(0.10, 0.75, 1.7) * spiral * 2.0
        + vec3(0.3, 0.7, 1.0) * edge * 0.65
        + vec3(1.4, 1.8, 2.0) * core * 7.0;
    return vec4(emission * parameters.z, 0.80 + core * 0.19);
}
