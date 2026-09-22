//! Median-metered auto exposure, after SpaceEngine: a GPU pass bins each view's
//! HDR frame into an absolute log2-luminance histogram, the histogram is read
//! back, and a CPU controller drives the camera's `Exposure` so the median lit
//! pixel lands on a target brightness. Manual +/- compensation is applied later
//! through `ColorGrading`, so it never feeds back into the metering.
use super::ViewCamera;
use bevy::{
    camera::{CameraUpdateSystems, Exposure},
    core_pipeline::{Core3dSystems, schedule::Core3d, tonemapping::tonemapping},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        gpu_readback::{Readback, ReadbackComplete},
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_sized, texture_2d, texture_depth_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, ViewQuery},
        storage::{GpuShaderBuffer, ShaderBuffer},
        view::{ColorGrading, ViewDepthTexture, ViewTarget},
    },
};
use std::num::NonZero;

const BINS: usize = 256;
/// Absolute scene luminance covered by bins 1..=255, in log2 cd/m². Darker
/// geometry (shadow with no fill light) and all sky pixels land in bin 0 and
/// are not metered.
const MIN_LOG_LUMINANCE: f32 = -10.0;
const MAX_LOG_LUMINANCE: f32 = 30.0;
const BINS_PER_STOP: f32 = (BINS - 2) as f32 / (MAX_LOG_LUMINANCE - MIN_LOG_LUMINANCE);

/// Pre-tonemap brightness the median lit pixel is exposed to (middle grey).
const TARGET_LUMINANCE: f32 = 0.18;
/// Below this weighted fraction of lit pixels the median is only partly
/// trusted, and the exposure relaxes toward `NIGHT_EV` so an empty view adapts
/// to darkness and shows the star field.
const MIN_COVERAGE: f32 = 0.02;
/// Dark-adapted exposure for a view with no lit geometry.
const NIGHT_EV: f32 = 5.0;
/// Highlight protection: the exposure never rises past the point where this
/// percentile of lit pixels exceeds `HIGHLIGHT_LUMINANCE` before tone mapping.
/// This keeps a small sunlit ship correctly exposed against black space.
const HIGHLIGHT_PERCENTILE: f64 = 0.99;
const HIGHLIGHT_LUMINANCE: f32 = 6.0;
/// Lit pixels needed before highlights constrain exposure, so a lone glint or
/// sub-pixel flare cannot pin the view dark.
const MIN_HIGHLIGHT_PIXELS: f64 = 200.0;
/// Centre-weight scale applied per pixel by the metering shader.
const PIXEL_WEIGHT: f64 = 64.0;
const MIN_EV: f32 = -4.0;
const MAX_EV: f32 = 26.0;

/// Eye-like adaptation: adjusting to a brighter scene is faster than to a
/// darker one. Beyond `EXPONENTIAL_TRANSITION` stops of error the exposure
/// moves at the full rate; closer in it eases in exponentially.
const DARKEN_STOPS_PER_S: f32 = 3.0;
const BRIGHTEN_STOPS_PER_S: f32 = 1.0;
const EXPONENTIAL_TRANSITION: f32 = 1.5;

const CENTER_FALLOFF: f32 = 0.5;
const EDGE_WEIGHT: f32 = 0.25;

/// Manual exposure compensation in stops, applied after metering.
#[derive(Component, Default)]
pub(super) struct ExposureSettings {
    stops: f32,
}

#[derive(Component)]
struct Meter {
    target_ev: Option<f32>,
    settled: bool,
}

#[derive(Component, Clone, ExtractComponent)]
struct MeterBuffer(Handle<ShaderBuffer>);

#[derive(Component, Clone, Copy, Default, ExtractComponent, ShaderType)]
struct MeterUniform {
    viewport: Vec4,
    inverse_exposure: f32,
    min_log_luminance: f32,
    bins_per_stop: f32,
    center_falloff: f32,
    edge_weight: f32,
}

pub(super) fn install(app: &mut App) {
    bevy::asset::embedded_asset!(app, "exposure.wgsl");
    app.add_plugins((
        ExtractComponentPlugin::<MeterBuffer>::default(),
        ExtractComponentPlugin::<MeterUniform>::default(),
        UniformComponentPlugin::<MeterUniform>::default(),
    ))
    .add_observer(receive)
    .add_systems(
        osg_ui::bevy_egui::EguiPrimaryContextPass,
        shortcuts.in_set(crate::ui::input::GameplayInput::Keyboard),
    )
    .add_systems(
        PostUpdate,
        (attach, adapt).chain().after(CameraUpdateSystems),
    );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(RenderStartup, pipeline).add_systems(
            Core3d,
            // Meter the scene itself: bloom would spread faint glow over black
            // space and pull the median down.
            render
                .before(bevy::post_process::bloom::bloom)
                .before(tonemapping)
                .in_set(Core3dSystems::PostProcess),
        );
    }
}

fn attach(
    mut commands: Commands,
    mut cameras: Query<(Entity, &mut Camera3d), (With<ViewCamera>, Without<Meter>)>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    for (entity, mut camera3d) in &mut cameras {
        camera3d.depth_texture_usages =
            (TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING).into();
        let buffer = buffers.add(ShaderBuffer::with_size(
            BINS * 4,
            bevy::asset::RenderAssetUsages::default(),
        ));
        commands
            .entity(entity)
            .insert((
                Meter {
                    target_ev: None,
                    settled: false,
                },
                MeterBuffer(buffer.clone()),
                MeterUniform::default(),
                Readback::buffer(buffer),
            ))
            .insert_if_new(ColorGrading::default());
    }
}

fn receive(readback: On<ReadbackComplete>, mut meters: Query<&mut Meter>) {
    let Ok(mut meter) = meters.get_mut(readback.entity) else {
        return;
    };
    let histogram: Vec<u32> = readback
        .data
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    if let Some(reading) = Reading::from_histogram(&histogram) {
        meter.target_ev = Some(reading.target_ev());
    }
}

fn adapt(
    time: Res<Time>,
    mut cameras: Query<(&mut Meter, &mut Exposure, &Camera, &mut MeterUniform)>,
) {
    for (mut meter, mut exposure, camera, mut uniform) in &mut cameras {
        if let Some(target) = meter.target_ev {
            exposure.ev100 = if meter.settled {
                adapt_ev(exposure.ev100, target, time.delta_secs())
            } else {
                target
            };
            meter.settled = true;
        }
        let viewport = camera.physical_viewport_rect().map_or(Vec4::ZERO, |rect| {
            Vec4::new(
                rect.min.x as f32,
                rect.min.y as f32,
                rect.width() as f32,
                rect.height() as f32,
            )
        });
        *uniform = MeterUniform {
            viewport,
            inverse_exposure: 1.0 / exposure.exposure(),
            min_log_luminance: MIN_LOG_LUMINANCE,
            bins_per_stop: BINS_PER_STOP,
            center_falloff: CENTER_FALLOFF,
            edge_weight: EDGE_WEIGHT,
        };
    }
}

fn shortcuts(
    mut contexts: osg_ui::bevy_egui::EguiContexts,
    selection: Res<crate::ui::Selection>,
    mut cameras: Query<(&ViewCamera, &mut ExposureSettings, &mut ColorGrading)>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    for (view, mut settings, mut grading) in &mut cameras {
        if Some(view.view) != selection.view {
            continue;
        }
        ctx.input(|input| {
            if !input.modifiers.command && !input.modifiers.ctrl && !input.modifiers.alt {
                if input.key_pressed(osg_ui::egui::Key::Plus)
                    || input.key_pressed(osg_ui::egui::Key::Equals)
                {
                    settings.stops += 0.5;
                }
                if input.key_pressed(osg_ui::egui::Key::Minus) {
                    settings.stops -= 0.5;
                }
            }
        });
        settings.stops = settings.stops.clamp(-10., 10.);
        grading.global.exposure = settings.stops;
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Reading {
    /// Median log2 luminance of the metered (lit) pixels, if any.
    median_log_luminance: Option<f32>,
    /// `HIGHLIGHT_PERCENTILE` log2 luminance of the lit pixels, if enough are lit.
    highlight_log_luminance: Option<f32>,
    /// Weighted fraction of the view that is lit.
    coverage: f32,
}

impl Reading {
    /// `None` until the GPU pass has produced a histogram.
    fn from_histogram(histogram: &[u32]) -> Option<Self> {
        let total: u64 = histogram.iter().map(|&count| count as u64).sum();
        if total == 0 {
            return None;
        }
        let lit: u64 = histogram[1..].iter().map(|&count| count as u64).sum();
        Some(Self {
            median_log_luminance: lit_percentile(histogram, lit, 0.5),
            highlight_log_luminance: (lit as f64 >= MIN_HIGHLIGHT_PIXELS * PIXEL_WEIGHT)
                .then(|| lit_percentile(histogram, lit, HIGHLIGHT_PERCENTILE))
                .flatten(),
            coverage: (lit as f64 / total as f64) as f32,
        })
    }

    /// Bevy renders at `exposure = 1 / (1.2 * 2^ev100)`, so a median scene
    /// luminance `L` reaches `TARGET_LUMINANCE` at `ev100 = log2(L / (1.2 * target))`.
    ///
    /// Higher EV is darker, so taking the larger of the scene and highlight
    /// targets lets highlights cap how far the view may brighten.
    fn target_ev(&self) -> f32 {
        let scene = self.median_log_luminance.map_or(NIGHT_EV, |median| {
            let metered = ev_for(median, TARGET_LUMINANCE);
            let trust = (self.coverage / MIN_COVERAGE).min(1.0);
            NIGHT_EV + (metered - NIGHT_EV) * trust
        });
        let highlight = self
            .highlight_log_luminance
            .map_or(f32::NEG_INFINITY, |highlight| {
                ev_for(highlight, HIGHLIGHT_LUMINANCE)
            });
        scene.max(highlight).clamp(MIN_EV, MAX_EV)
    }
}

fn ev_for(log_luminance: f32, exposed: f32) -> f32 {
    log_luminance - (1.2 * exposed).log2()
}

/// Log2 luminance below which `fraction` of the lit (bin 1+) samples fall,
/// interpolated within its bin.
fn lit_percentile(histogram: &[u32], lit: u64, fraction: f64) -> Option<f32> {
    let rank = lit as f64 * fraction;
    let mut below = 0.0;
    for (index, &count) in histogram.iter().enumerate().skip(1) {
        let count = count as f64;
        if count > 0.0 && below + count >= rank {
            let within = ((rank - below) / count) as f32;
            return Some(MIN_LOG_LUMINANCE + (index as f32 - 1.0 + within) / BINS_PER_STOP);
        }
        below += count;
    }
    None
}

fn adapt_ev(current: f32, target: f32, dt: f32) -> f32 {
    let delta = target - current;
    let rate = if delta > 0.0 {
        DARKEN_STOPS_PER_S
    } else {
        BRIGHTEN_STOPS_PER_S
    } * dt;
    let step = (delta.abs() * rate / EXPONENTIAL_TRANSITION).min(rate);
    current + step.copysign(delta)
}

#[derive(Resource)]
struct Pipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn pipeline(mut commands: Commands, assets: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = BindGroupLayoutDescriptor::new(
        "exposure meter",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                uniform_buffer::<MeterUniform>(true),
                storage_buffer_sized(false, NonZero::new(BINS as u64 * 4)),
                texture_depth_2d(),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("exposure meter".into()),
        layout: vec![layout.clone()],
        shader: assets.load("embedded://osg_client/ui/scene/exposure.wgsl"),
        entry_point: Some("histogram_pass".into()),
        ..default()
    });
    commands.insert_resource(Pipeline { layout, pipeline });
}

fn render(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthTexture,
        &MeterBuffer,
        &MeterUniform,
        &DynamicUniformIndex<MeterUniform>,
    )>,
    pipeline: Option<Res<Pipeline>>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<MeterUniform>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut context: RenderContext,
) {
    let Some(pipeline) = pipeline else { return };
    let Some(compiled) = cache.get_compute_pipeline(pipeline.pipeline) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let (target, depth, buffer, settings, index) = view.into_inner();
    let Some(histogram) = buffers.get(&buffer.0) else {
        return;
    };
    let size = settings.viewport.zw().as_uvec2();
    if size.x == 0 || size.y == 0 {
        return;
    }
    let bind = context.render_device().create_bind_group(
        "exposure meter",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((
            target.main_texture_view(),
            binding,
            histogram.buffer.as_entire_buffer_binding(),
            depth.view(),
        )),
    );
    let encoder = context.command_encoder();
    encoder.clear_buffer(&histogram.buffer, 0, None);
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("exposure meter"),
        timestamp_writes: None,
    });
    pass.set_pipeline(compiled);
    pass.set_bind_group(0, &bind, &[index.index()]);
    pass.dispatch_workgroups(size.x.div_ceil(16), size.y.div_ceil(16), 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_for(log_luminance: f32) -> usize {
        ((log_luminance - MIN_LOG_LUMINANCE) * BINS_PER_STOP + 1.0) as usize
    }

    #[test]
    fn empty_histogram_waits_for_the_gpu() {
        assert_eq!(Reading::from_histogram(&[0; BINS]), None);
    }

    #[test]
    fn median_ignores_unlit_pixels_and_interpolates_within_its_bin() {
        let mut histogram = [0; BINS];
        histogram[0] = 1_000_000;
        let bin = bin_for(10.0);
        histogram[bin] = 100;
        let reading = Reading::from_histogram(&histogram).unwrap();
        let median = reading.median_log_luminance.unwrap();
        let low = MIN_LOG_LUMINANCE + (bin as f32 - 1.0) / BINS_PER_STOP;
        assert!((median - (low + 0.5 / BINS_PER_STOP)).abs() < 1e-4);
        assert!(reading.coverage < 1e-3);
    }

    #[test]
    fn well_covered_view_exposes_its_median_to_the_target() {
        let mut histogram = [0; BINS];
        histogram[bin_for(12.0)] = 500;
        histogram[bin_for(4.0)] = 200;
        histogram[bin_for(20.0)] = 200;
        let reading = Reading::from_histogram(&histogram).unwrap();
        let median = reading.median_log_luminance.unwrap();
        assert!((median - 12.0).abs() < 1.0 / BINS_PER_STOP);
        let ev = reading.target_ev();
        let exposure = Exposure { ev100: ev }.exposure();
        let exposed = median.exp2() * exposure;
        assert!((exposed - TARGET_LUMINANCE).abs() < 1e-3, "{exposed}");
    }

    #[test]
    fn empty_view_adapts_to_night() {
        let dark = Reading {
            median_log_luminance: None,
            highlight_log_luminance: None,
            coverage: 0.0,
        };
        assert_eq!(dark.target_ev(), NIGHT_EV);
    }

    #[test]
    fn sparse_lit_pixels_relax_toward_night_unless_highlights_cap_them() {
        let full = Reading {
            median_log_luminance: Some(12.0),
            highlight_log_luminance: None,
            coverage: 1.0,
        };
        let sparse = Reading {
            median_log_luminance: Some(12.0),
            highlight_log_luminance: None,
            coverage: MIN_COVERAGE * 0.25,
        };
        let expected = NIGHT_EV + (full.target_ev() - NIGHT_EV) * 0.25;
        assert!((sparse.target_ev() - expected).abs() < 1e-4);

        // A small sunlit ship: its highlights stop the view opening up for stars.
        let capped = Reading {
            highlight_log_luminance: Some(13.0),
            ..sparse
        };
        let ev = capped.target_ev();
        let exposed = 13f32.exp2() * Exposure { ev100: ev }.exposure();
        assert!((exposed - HIGHLIGHT_LUMINANCE).abs() < 1e-2, "{exposed}");
    }

    #[test]
    fn highlights_need_enough_lit_pixels() {
        let mut histogram = [0; BINS];
        histogram[0] = 10_000_000;
        histogram[bin_for(20.0)] = (MIN_HIGHLIGHT_PIXELS * PIXEL_WEIGHT) as u32 - 1;
        let glint = Reading::from_histogram(&histogram).unwrap();
        assert_eq!(glint.highlight_log_luminance, None);
        histogram[bin_for(20.0)] += 1;
        let ship = Reading::from_histogram(&histogram).unwrap();
        assert!((ship.highlight_log_luminance.unwrap() - 20.0).abs() < 1.0 / BINS_PER_STOP);
    }

    #[test]
    fn adaptation_darkens_faster_than_it_brightens_and_eases_in() {
        let darkened = adapt_ev(10.0, 20.0, 0.1);
        let brightened = adapt_ev(20.0, 10.0, 0.1);
        assert!((darkened - 10.3).abs() < 1e-5);
        assert!((brightened - 19.9).abs() < 1e-5);
        let near = adapt_ev(10.0, 10.5, 0.1);
        assert!((near - 10.1).abs() < 1e-5);
    }
}
