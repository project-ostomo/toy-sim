// Absolute log2-luminance histogram of a view's HDR frame, read back to the
// CPU exposure controller. Only geometry is metered: sky pixels (nothing in
// the depth buffer) count as unlit, so the star field can neither set the
// exposure nor feed back into it. Bin 0 holds unlit pixels; bins 1..255 cover
// the metered range evenly in stops.

const BINS: u32 = 256u;
const RGB_TO_LUMINANCE = vec3<f32>(0.2125, 0.7154, 0.0721);

struct Meter {
    // Viewport origin and size, in physical pixels of the view target.
    viewport: vec4<f32>,
    // Undoes the camera exposure the frame was rendered with.
    inverse_exposure: f32,
    min_log_luminance: f32,
    bins_per_stop: f32,
    // Centre weighting: max(sqrt(1 - falloff * r^2), edge_weight), r = 1 at
    // the top and bottom edges.
    center_falloff: f32,
    edge_weight: f32,
}

@group(0) @binding(0) var color: texture_2d<f32>;
@group(0) @binding(1) var<uniform> meter: Meter;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>, BINS>;
@group(0) @binding(3) var depth: texture_depth_2d;

var<workgroup> local_histogram: array<atomic<u32>, BINS>;

fn bin(luminance: f32) -> u32 {
    // Also catches NaN.
    if !(luminance > 0.0) {
        return 0u;
    }
    let position = (log2(luminance) - meter.min_log_luminance) * meter.bins_per_stop + 1.0;
    return u32(clamp(position, 0.0, f32(BINS - 1u)));
}

fn weight(pixel: vec2<u32>, size: vec2<u32>) -> u32 {
    let extent = vec2<f32>(size);
    var p = (vec2<f32>(pixel) + 0.5) / extent * 2.0 - 1.0;
    p.x *= extent.x / extent.y;
    let falloff = sqrt(saturate(1.0 - meter.center_falloff * dot(p, p)));
    // 64 levels (`PIXEL_WEIGHT` on the CPU) keeps a 4K frame's weighted total
    // well inside u32.
    return u32(round(max(falloff, meter.edge_weight) * 64.0));
}

@compute @workgroup_size(16, 16, 1)
fn histogram_pass(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    atomicStore(&local_histogram[index], 0u);
    workgroupBarrier();

    let size = vec2<u32>(meter.viewport.zw);
    if all(id.xy < size) {
        let pixel = vec2<u32>(meter.viewport.xy) + id.xy;
        let rgb = textureLoad(color, pixel, 0).rgb;
        // Reverse-Z: the sky is left at 0.
        let geometry = textureLoad(depth, pixel, 0) > 0.0;
        let luminance = select(0.0, dot(rgb, RGB_TO_LUMINANCE) * meter.inverse_exposure, geometry);
        atomicAdd(&local_histogram[bin(luminance)], weight(id.xy, size));
    }

    workgroupBarrier();
    let count = atomicLoad(&local_histogram[index]);
    if count > 0u {
        atomicAdd(&histogram[index], count);
    }
}
