#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import osg_client::slip_settings::Settings

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: Settings;
@group(0) @binding(3) var depth: texture_depth_2d;

@fragment
fn fragment(input: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let original = textureSampleLevel(source, source_sampler, uv, 0.0);
    let local = (uv - settings.viewport.xy) / settings.viewport.zw;
    if any(local < vec2(0.0)) || any(local > vec2(1.0)) { return original; }
    let dimensions = textureDimensions(depth);
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(dimensions)), vec2(0), vec2<i32>(dimensions) - 1);
    let scene_depth = textureLoad(depth, pixel, 0);
    var displacement = vec2(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let event = settings.centers[i];
        let shape = settings.shapes[i];
        let delta = (local - event.xy) * vec2(shape.z, 1.0);
        let r = length(delta) / max(event.z, 0.0001);
        if scene_depth < event.w && shape.y > 0.0 {
            let ring = exp(-pow((r - 0.45 - shape.x * 0.12) * 6.0, 2.0));
            let angle = atan2(delta.y, delta.x);
            let turbulence = sin(angle * 7.0 + shape.x * 12.0 + shape.w) * 0.4
                + sin(angle * 13.0 - shape.x * 8.0) * 0.2;
            displacement += delta * ring * shape.y * (0.06 + turbulence * 0.025) / vec2(shape.z, 1.0);
        }
    }
    if scene_depth < 0.000001 {
        displacement += vec2(sin(local.y * 16.0 + settings.time.x * 0.2), cos(local.x * 19.0 - settings.time.x * 0.3))
            * settings.time.y * 0.0015;
    }
    let sample_uv = clamp(uv + displacement * settings.viewport.zw,
        settings.viewport.xy + vec2(0.00001), settings.viewport.xy + settings.viewport.zw - vec2(0.00001));
    // Preserve foreground silhouettes when a displaced sample reaches an occluder.
    let moved_pixel = clamp(vec2<i32>(sample_uv * vec2<f32>(dimensions)), vec2(0), vec2<i32>(dimensions) - 1);
    let moved_depth = textureLoad(depth, moved_pixel, 0);
    var background = textureSampleLevel(source, source_sampler, sample_uv, 0.0);
    if moved_depth > scene_depth + 0.0001 { background = original; }
    return background;
}
