#import osg_client::slip_settings::{Settings, gaussian_pixel, gaussian_erf}

@group(0) @binding(2) var<uniform> settings: Settings;
@group(0) @binding(3) var depth: texture_depth_2d;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) start: vec4<f32>,
    @location(1) @interpolate(flat) end: vec4<f32>,
    @location(2) @interpolate(flat) color_start: vec4<f32>,
    @location(3) @interpolate(flat) color_end: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Output {
    let ribbon = settings.wakes[instance];
    let corners = array<vec2<f32>, 6>(vec2(0.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(0.0, -1.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    let corner = corners[vertex % 6u];
    let section = vertex / 6u;
    let delta = ribbon.end.xy - ribbon.start.xy;
    let length = length(delta);
    let axis = select(vec2(1.0, 0.0), delta / max(length, 0.00001), length > 0.00001);
    let normal = vec2(-axis.y, axis.x);
    var endpoint = mix(ribbon.start, ribbon.end, corner.x);
    if section == 1u { endpoint = ribbon.start; }
    if section == 2u { endpoint = ribbon.end; }
    let radius = select(endpoint.w, max(ribbon.start.w, ribbon.end.w), length < 0.5);
    let extent = max(radius * 3.0, 1.5);
    // Keep the tapered body and its two caps separate. Extending the tapered
    // quad itself moves its edges off the projected width near the camera.
    var pixel = endpoint.xy + normal * corner.y * extent;
    if section == 1u { pixel += axis * (corner.x - 1.0) * extent; }
    if section == 2u { pixel += axis * corner.x * extent; }
    var out: Output;
    out.position = vec4(pixel / settings.screen.xy * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
    out.start = ribbon.start;
    out.end = ribbon.end;
    out.color_start = ribbon.color_start;
    out.color_end = ribbon.color_end;
    return out;
}

@fragment
fn fragment(input: Output) -> @location(0) vec4<f32> {
    let delta = input.end.xy - input.start.xy;
    let length = length(delta);
    let axis = select(vec2(1.0, 0.0), delta / max(length, 0.00001), length > 0.00001);
    let normal = vec2(-axis.y, axis.x);
    let offset = input.position.xy - input.start.xy;
    let along = dot(offset, axis);
    let fraction = clamp(along / max(length, 0.00001), 0.0, 1.0);
    var reverse_depth = mix(input.start.z, input.end.z, fraction);
    var sigma = mix(input.start.w, input.end.w, fraction);
    var color = mix(input.color_start.rgb, input.color_end.rgb, fraction);
    if length < 0.5 {
        reverse_depth = max(input.start.z, input.end.z);
        sigma = max(input.start.w, input.end.w);
        color = select(input.color_end.rgb, input.color_start.rgb, input.start.z >= input.end.z);
    }
    let transverse = dot(offset, normal);
    let profile = gaussian_pixel(transverse, sigma, abs(normal.x) + abs(normal.y));
    // Half intensity at each endpoint lets adjoining spans meet without bright
    // doubled joints. Rounded caps also remain stable for short projected spans.
    let cap_width = max(sigma, 0.5);
    var caps = 0.5 * (gaussian_erf(along / cap_width) + gaussian_erf((length - along) / cap_width));
    if length < 0.5 {
        caps = gaussian_pixel(along, max(input.start.w, input.end.w), abs(axis.x) + abs(axis.y));
    }
    if textureLoad(depth, vec2<i32>(input.position.xy), 0) > reverse_depth * 1.0001 { discard; }
    return vec4(color * profile * caps, 0.0);
}
