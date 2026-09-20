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
fn streaks(angle: f32, depth: f32, time: f32, lanes: f32, offset: f32) -> f32 {
    let lane_position = angle * lanes + offset;
    let lane = floor(lane_position);
    let seed = hash(vec2(lane, offset));
    let z = depth * 0.72 + time * (8.0 + seed * 7.0) + seed * 83.0;
    let segment = floor(z);
    let star = hash(vec2(lane + offset, segment));
    let width = mix(0.018, 0.095, star * star);
    let distance = abs(fract(lane_position) - mix(0.2, 0.8, seed));
    let aa = max(fwidth(lane_position), 0.002);
    let core = 1.0 - smoothstep(width, width + aa, distance);
    let halo = exp(-distance * distance / (width * width * 12.0 + aa * aa));
    let tail = smoothstep(0.02, 0.12, fract(z)) * (1.0 - smoothstep(0.42, 0.98, fract(z)));
    let resolved = (1.0 - smoothstep(0.25, 0.85, fwidth(z)))
        * (1.0 - smoothstep(0.5, 1.5, aa));
    return (core + halo * 0.10) * tail * mix(0.6, 3.0, star) * step(0.48, star) * resolved;
}
fn slip_space(direction: vec3<f32>, time: f32) -> vec3<f32> {
    let radial = max(length(direction.xy), 0.012);
    let depth = clamp(direction.z / radial, -65.0, 65.0);
    let angle = atan2(direction.y, direction.x) / 6.283185 + 0.5;
    let flow = depth * 0.38 + time * 1.6;
    let circle = vec2(cos(angle * 6.283185), sin(angle * 6.283185));
    let warp = noise(circle * 2.7 + vec2(flow * 0.18, -time * 0.12));
    let p = circle * (3.8 + warp * 1.8) + vec2(flow, flow * 0.37);
    let clouds = noise(p) * 0.58 + noise(p * 2.13 + vec2(19.4)) * 0.28 + noise(p * 4.37) * 0.14;
    let folds = pow(clamp(clouds * 1.5 - 0.28, 0.0, 1.0), 3.0);
    let open_throat = smoothstep(0.025, 0.18, radial);
    let haze = pow(abs(direction.z), 5.0);
    let stars = streaks(angle, depth, time, 251.0, 3.0)
        + streaks(angle, depth * 1.47, time, 397.0, 17.0) * 0.65
        + streaks(angle, depth * 2.19, time, 613.0, 41.0) * 0.32;
    return vec3(0.008, 0.025, 0.095)
        + vec3(0.045, 0.24, 0.75) * folds * (1.5 + haze * 2.0) * open_throat
        + vec3(0.12, 0.45, 0.85) * haze
        + vec3(0.65, 0.88, 1.0) * stars * 2.0 * open_throat;
}
@fragment
fn fragment(input: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let inside = length(input.camera) < 1.0;
    // The orbit camera can trail the arriving ship inside the exit aperture.
    // Its interior must not become an opaque foreground shell around the view.
    if parameters.y <= 0.5 && inside { discard; }
    if front == inside { discard; }
    let n = normalize(input.local);
    let t = parameters.x;
    if parameters.y > 0.5 {
        return vec4(slip_space(normalize(input.local - input.camera), t)
            * parameters.z * 35000.0 * view.exposure, 1.0);
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
