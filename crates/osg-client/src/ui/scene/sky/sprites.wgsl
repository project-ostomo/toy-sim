// Screen-space star sprites with per-frame parallax.
//
// Each star is one quad. The vertex shader recomputes its direction and
// illuminance from the camera's offset every frame, then sizes the quad in
// pixels. The fragment shader spreads exactly the star's illuminance over the
// pixel grid: a normalized Gaussian core, plus a wider Gaussian halo (after
// SpaceEngine's point flares) that takes a growing share of the energy as the
// core overexposes, so brighter stars read larger instead of all clipping to
// the same white pixel. Sprites sit at infinite depth: geometry occludes them
// and exposure metering treats them as sky.

#import bevy_pbr::mesh_view_bindings::view

struct StarField {
    // xyz: camera offset from the snapshot origin, in sprite units (1e12 m).
    // w: illuminance (lux) at which sprites have faded out.
    camera: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> field: StarField;

struct Vertex {
    // Star offset from the snapshot origin, in sprite units.
    @location(0) position: vec3<f32>,
    // rgb: colour-weighted L / (4π unit²); a: the same without colour.
    // Divided by the squared distance in sprite units this is lux.
    @location(1) flux: vec4<f32>,
    @location(2) corner: vec2<f32>,
}

struct Output {
    @builtin(position) position: vec4<f32>,
    // Offset from the star's centre, in pixels.
    @location(0) offset: vec2<f32>,
    // Exposed luminance if all the star's light fell in one pixel.
    @location(1) @interpolate(flat) radiance: vec3<f32>,
    // Core sigma, halo sigma, halo energy fraction (pixels, pixels, 0..1).
    @location(2) @interpolate(flat) psf: vec3<f32>,
}

const PI: f32 = 3.14159265;
const LOG10_2: f32 = 0.30103;
const CORE_SIGMA: f32 = 0.8;
// The halo starts at this width once the core clips and widens per stop of
// further overexposure, taking a growing (capped) share of the energy.
const HALO_BASE_SIGMA: f32 = 2.0;
const HALO_SIGMA_PER_STOP: f32 = 0.9;
const HALO_MAX_SIGMA: f32 = 24.0;
const HALO_ENERGY_PER_STOP: f32 = 0.03;
const HALO_MAX_ENERGY: f32 = 0.35;
// Sprites whose brightest pixel stays below this after exposure are dropped.
const CULL_LUMINANCE: f32 = 1e-4;
// Keep additive output inside Rgba16Float.
const MAX_OUTPUT: f32 = 60000.0;
// Anchor to the dark-adapted camera (EV 5), while retaining a visible response
// to exposure in daylight. Use the same gain for sizing and culling.
const NIGHT_EXPOSURE: f32 = 0.026041667;

fn gaussian(r2: f32, sigma: f32) -> f32 {
    return exp(-r2 / (2.0 * sigma * sigma)) / (2.0 * PI * sigma * sigma);
}

@vertex
fn vertex(in: Vertex) -> Output {
    var out: Output;
    let relative = in.position - field.camera.xyz;
    let distance2 = max(dot(relative, relative), 1e-30);
    let illuminance = in.flux.a / distance2;
    // Same soft cutoff the baked sky used: fully visible half a magnitude in.
    let fade = saturate(5.0 * LOG10_2 * log2(illuminance / field.camera.w));

    // Solid angle of one pixel at the screen centre.
    let pixel = 2.0 / (view.clip_from_view[1][1] * view.viewport.w);
    let star_exposure = 2.0 * sqrt(max(view.exposure, 0.0) * NIGHT_EXPOSURE);
    let exposed = illuminance * fade * star_exposure / (pixel * pixel);
    let core_peak = exposed * gaussian(0.0, CORE_SIGMA);

    // A direction, not a point: w = 0 drops the camera translation.
    let clip = view.clip_from_world * vec4(relative, 0.0);
    if clip.w <= 0.0 || core_peak < CULL_LUMINANCE {
        // Degenerate quad: all four corners coincide and rasterize nothing.
        out.position = vec4(0.0, 0.0, 0.0, 1.0);
        return out;
    }

    let over = max(log2(core_peak), 0.0);
    let halo_fraction = min(HALO_ENERGY_PER_STOP * over, HALO_MAX_ENERGY);
    let halo_sigma = min(HALO_BASE_SIGMA + HALO_SIGMA_PER_STOP * over, HALO_MAX_SIGMA);
    let radius = 3.0 * select(CORE_SIGMA, halo_sigma, halo_fraction > 0.0);

    let ndc = clip.xy / clip.w;
    // Depth 0 is infinitely far under Bevy's reverse-Z.
    out.position = vec4(ndc + in.corner * radius * 2.0 / view.viewport.zw, 0.0, 1.0);
    out.offset = in.corner * radius;
    out.radiance = in.flux.rgb / distance2 * fade * star_exposure / (pixel * pixel);
    out.psf = vec3(CORE_SIGMA, halo_sigma, halo_fraction);
    return out;
}

@fragment
fn fragment(in: Output) -> @location(0) vec4<f32> {
    let r2 = dot(in.offset, in.offset);
    let weight = mix(gaussian(r2, in.psf.x), gaussian(r2, in.psf.y), in.psf.z);
    return vec4(min(in.radiance * weight, vec3(MAX_OUTPUT)), 0.0);
}
