#import bevy_pbr::{
    forward_io::Vertex,
    mesh_functions::{get_world_from_local, get_local_from_world, mesh_position_local_to_clip},
    mesh_view_bindings::view,
}
struct Settings {
    parameters: vec4<f32>,
    detail: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: Settings;
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
fn hash(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}
fn noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(mix(hash(i), hash(i + vec3(1.0, 0.0, 0.0)), u.x),
            mix(hash(i + vec3(0.0, 1.0, 0.0)), hash(i + vec3(1.0, 1.0, 0.0)), u.x), u.y),
        mix(mix(hash(i + vec3(0.0, 0.0, 1.0)), hash(i + vec3(1.0, 0.0, 1.0)), u.x),
            mix(hash(i + vec3(0.0, 1.0, 1.0)), hash(i + vec3(1.0)), u.x), u.y), u.z);
}
fn fbm(p: vec3<f32>) -> f32 {
    return noise(p) * 0.57 + noise(p * 2.07 + 17.3) * 0.28 + noise(p * 4.13 - 11.7) * 0.15;
}
fn palette(x: f32) -> vec3<f32> {
    return mix(vec3(0.27, 0.075, 0.6), vec3(0.035, 0.52, 0.85), smoothstep(0.2, 0.8, x));
}
@fragment
fn fragment(input: Output, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let p = settings.parameters;
    let d = settings.detail;
    let mode = p.z;
    let t = p.x;
    var color = vec3(0.0);
    var alpha = 0.0;
    if mode < 1.5 {
        let inside = length(input.camera) < 1.0;
        if front == inside { discard; }
        let ray = normalize(input.local - input.camera);
        let angle = atan2(ray.y, ray.x);
        let coverage = smoothstep(0.0, 1.0, p.y);
        let n = normalize(input.local);
        let rim = pow(1.0 - abs(n.z), 8.0);
        let lines = pow(max(0.0, sin(angle * 73.0 + n.z * 2.0 - t * 4.0)), 38.0);
        alpha = rim * lines * sin(coverage * 3.141593) * 0.3;
        color = vec3(0.8, 0.76, 0.67) * 5.0;
    } else if mode < 3.5 {
        let inside = length(input.camera) < 1.0;
        if front == inside { discard; }
        let rd = normalize(input.local - input.camera);
        let ro = input.camera;
        if d.y < 0.5 {
            // Integrate an unresolved rupture as a small luminous point instead
            // of undersampling the individual aperture filaments.
            let impact = length(cross(ro, rd));
            let life = pow(max(0.0, 1.0 - p.y / 2.0), 2.0);
            let opacity = exp(-impact * impact * 8.0) * life * min(1.0, d.y * d.y * 20.0);
            let emission = vec3(0.85, 0.82, 1.0) * (400.0 * exp(-p.y * 20.0) + 6.0);
            return vec4(emission * 35000.0 * view.exposure, opacity);
        }
        let b = dot(ro, rd);
        let c = dot(ro, ro) - 1.0;
        let discriminant = b * b - c;
        if discriminant <= 0.0 { discard; }
        let enter = max(0.0, -b - sqrt(discriminant));
        let exit = -b + sqrt(discriminant);
        let step_size = (exit - enter) / 24.0;
        var light = vec3(0.0);
        var opacity = 0.0;
        let age = p.y;
        let life = pow(max(0.0, 1.0 - age / 2.0), 2.0);
        let aperture = 0.09 + 0.25 * (1.0 - exp(-age * 6.0));
        for (var i = 0u; i < 24u; i += 1u) {
            let q = ro + rd * (enter + (f32(i) + 0.5) * step_size);
            let field = q * vec3(6.0, 6.0, 3.0) + vec3(d.x * 0.01, age * 0.7, age * p.w);
            let turbulence = fbm(field);
            let radial = length(q.xy);
            let ring = exp(-pow((radial - aperture - (turbulence - 0.5) * 0.18) * 22.0, 2.0))
                * exp(-abs(q.z) * 9.0);
            let cracks = pow(1.0 - abs(2.0 * turbulence - 1.0), 20.0);
            let shroud = cracks * exp(-radial * 4.0) * exp(-abs(q.z) * 3.0);
            let core = exp(-dot(q.xy, q.xy) * 900.0 - abs(q.z) * 35.0) * exp(-age * 18.0);
            let boundary = 1.0 - smoothstep(0.7, 1.0, length(q));
            let density = (ring * 4.0 + shroud * 2.2 + core * 25.0) * life * boundary;
            let absorption = 1.0 - exp(-density * step_size * 3.0);
            let emission = palette(turbulence) * (0.5 + ring * 1.7 + cracks)
                + vec3(0.65, 0.82, 1.0) * core * 10.0;
            light += (1.0 - opacity) * absorption * emission;
            opacity += (1.0 - opacity) * absorption;
        }
        alpha = opacity;
        color = light / max(opacity, 0.001);
        color *= (400.0 * exp(-age * 20.0) + 6.0) * d.y * d.y;
    }
    if mode > 3.5 {
        let n = normalize(input.local);
        let angle = atan2(n.y, n.x);
        let field = vec3(cos(angle) * 9.0, sin(angle) * 9.0, t * 0.5);
        let turbulence = fbm(field);
        let cracks = pow(1.0 - abs(2.0 * turbulence - 1.0), 14.0);
        alpha = cracks * exp(-n.z * n.z * 70.0) * p.y * p.y * 0.8;
        color = palette(turbulence) * (1.0 + p.y * 3.0);
    }
    return vec4(color * 35000.0 * view.exposure, clamp(alpha, 0.0, 0.96));
}
