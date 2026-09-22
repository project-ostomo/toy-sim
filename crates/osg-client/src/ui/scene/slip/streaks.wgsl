#import osg_client::slip_settings::{Settings, gaussian_pixel}

@group(0) @binding(2) var<uniform> settings: Settings;
@group(0) @binding(3) var depth: texture_depth_2d;

struct Strip {
    @builtin(position) position: vec4<f32>,
    @location(0) transverse: f32,
    @location(1) along: f32,
    @location(2) @interpolate(flat) length: f32,
    @location(3) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Strip {
    let streak = settings.streaks[instance];
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(0.0, -1.0), vec2(1.0, 1.0), vec2(0.0, 1.0),
    );
    let corner = corners[vertex];
    let delta = (streak.end.xy - streak.start.xy) * settings.screen.xy;
    let normal = normalize(vec2(-delta.y, delta.x));
    let center = mix(streak.start.xyz, streak.end.xyz, corner.x);
    var out: Strip;
    out.position = vec4(center.xy + normal * corner.y * 3.0 / settings.screen.xy, center.z, 1.0);
    out.transverse = corner.y * 1.5;
    out.along = corner.x * streak.end.w;
    out.length = streak.end.w;
    out.color = streak.color;
    return out;
}

@fragment
fn fragment(input: Strip) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(input.position.xy);
    if textureLoad(depth, pixel, 0) > input.position.z * 1.0001 { discard; }
    // Analytic Gaussian cross-section of a thin luminous filament. Only the
    // three-pixel-wide strip is shaded, with softened caps to bound overlap.
    let profile = gaussian_pixel(input.transverse, input.color.w, fwidth(input.transverse));
    let caps = smoothstep(0.0, 3.0, input.along)
        * smoothstep(0.0, 8.0, input.length - input.along);
    return vec4(input.color.rgb * profile * caps, 0.0);
}
