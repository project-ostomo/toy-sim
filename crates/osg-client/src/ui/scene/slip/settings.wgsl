#define_import_path osg_client::slip_settings

struct Streak {
    start: vec4<f32>,
    end: vec4<f32>,
    color: vec4<f32>,
}

struct Settings {
    centers: array<vec4<f32>, 4>,
    shapes: array<vec4<f32>, 4>,
    viewport: vec4<f32>,
    time: vec4<f32>,
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    screen: vec4<f32>,
    streaks: array<Streak, 192>,
    wakes: array<Ribbon, 96>,
}

struct Ribbon {
    start: vec4<f32>,
    end: vec4<f32>,
    color_start: vec4<f32>,
    color_end: vec4<f32>,
}

// Gaussian integrated over a pixel footprint, rather than sampled at its
// center. This preserves subpixel energy as the line crosses the pixel grid.
fn gaussian_erf(x: f32) -> f32 {
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let polynomial = (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t
        - 0.284496736) * t + 0.254829592) * t;
    return sign(x) * (1.0 - polynomial * exp(-a * a));
}

fn gaussian_pixel(distance: f32, sigma: f32, footprint: f32) -> f32 {
    let width = max(sigma, 0.000001);
    let pixel = max(footprint, 0.0001);
    let lo = (distance - pixel * 0.5) / width;
    let hi = (distance + pixel * 0.5) / width;
    return max(0.0, 0.886226925 * width / pixel * (gaussian_erf(hi) - gaussian_erf(lo)));
}
