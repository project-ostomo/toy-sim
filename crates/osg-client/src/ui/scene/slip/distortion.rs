use super::*;
use bevy::{
    core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d},
    render::{
        RenderApp, RenderStartup,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_resource::{
            binding_types::{sampler, texture_2d, texture_depth_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        view::{ViewDepthTexture, ViewTarget},
    },
};

pub(super) const STREAK_COUNT: usize = 192;

#[derive(Clone, Copy, Default, ShaderType)]
pub(super) struct Streak {
    pub(super) start: Vec4,
    pub(super) end: Vec4,
    pub(super) color: Vec4,
}

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub(super) struct Distortion {
    centers: [Vec4; 4],
    shapes: [Vec4; 4],
    pub(super) viewport: Vec4,
    pub(super) time: Vec4,
    pub(super) eye: Vec4,
    pub(super) right: Vec4,
    pub(super) up: Vec4,
    pub(super) forward: Vec4,
    pub(super) screen: Vec4,
    pub(super) streaks: [Streak; STREAK_COUNT],
    pub(super) wakes: [super::wakes::Ribbon; super::wakes::WAKE_COUNT],
}

impl Default for Distortion {
    fn default() -> Self {
        Self {
            centers: [Vec4::ZERO; 4],
            shapes: [Vec4::ZERO; 4],
            viewport: Vec4::ZERO,
            time: Vec4::ZERO,
            eye: Vec4::ZERO,
            right: Vec4::ZERO,
            up: Vec4::ZERO,
            forward: Vec4::ZERO,
            screen: Vec4::ZERO,
            streaks: [Streak::default(); STREAK_COUNT],
            wakes: [super::wakes::Ribbon::default(); super::wakes::WAKE_COUNT],
        }
    }
}

pub(super) fn install(app: &mut App) {
    embedded_asset!(app, "distortion.wgsl");
    bevy::shader::load_shader_library!(app, "settings.wgsl");
    embedded_asset!(app, "streaks.wgsl");
    embedded_asset!(app, "wakes.wgsl");
    app.add_plugins((
        ExtractComponentPlugin::<Distortion>::default(),
        UniformComponentPlugin::<Distortion>::default(),
    ))
    .add_systems(PostUpdate, prepare);
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .add_systems(RenderStartup, pipeline)
            .add_systems(Core3d, render.in_set(Core3dSystems::EarlyPostProcess));
    }
}

pub(super) fn prepare(
    mut commands: Commands,
    clock: Res<RenderTime>,
    history: Res<SlipEffects>,
    ships: Query<(&OwnedShip, &DisplayPose)>,
    mut views: Query<(
        Entity,
        &ViewCamera,
        &ViewObservation,
        &SlipView,
        &Transform,
        &Camera,
        &Projection,
        &mut Camera3d,
        Option<&bevy::camera::Exposure>,
    )>,
) {
    for (entity, view, observation, slip, transform, camera, projection, mut camera3d, exposure) in
        &mut views
    {
        let Projection::Perspective(projection) = projection else {
            continue;
        };
        camera3d.depth_texture_usages =
            (TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING).into();
        let target = camera
            .physical_target_size()
            .unwrap_or(UVec2::new(1280, 720))
            .as_vec2();
        let rect = camera
            .physical_viewport_rect()
            .unwrap_or(URect::from_corners(UVec2::ZERO, target.as_uvec2()));
        let min = rect.min.as_vec2() / target;
        let size = rect.size().as_vec2() / target;
        let mut settings = Distortion {
            screen: Vec4::new(target.x, target.y, 0.0, 0.0),
            viewport: Vec4::new(min.x, min.y, size.x, size.y),
            time: Vec4::new(
                (clock.display_ns as f64 * 1e-9 % 4096.0) as f32,
                slip.coverage,
                0.0,
                0.0,
            ),
            ..default()
        };
        // Camera rays and eye position in a ship-centered frame whose Z axis
        // follows the actual slip direction. Keep all arithmetic near the ship.
        if !view.private && slip.coverage > 0.0 {
            if let Some((_, pose)) = ships
                .iter()
                .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship)
            {
                let frame = Quat::from_rotation_arc(
                    Vec3::Z,
                    slip.direction.try_normalize().unwrap_or(Vec3::Z),
                )
                .inverse();
                let eye = frame
                    * (transform.translation - pose.0.position.relative_to(view.origin).as_vec3());
                let tan = (projection.fov * 0.5).tan();
                let aspect = rect.width() as f32 / rect.height().max(1) as f32;
                settings.eye = eye.extend(projection.near);
                settings.right = (frame * transform.rotation * Vec3::X * tan * aspect).extend(0.0);
                settings.up = (frame * transform.rotation * Vec3::Y * tan).extend(0.0);
                settings.forward = (frame * transform.rotation * Vec3::NEG_Z).extend(
                    exposure
                        .copied()
                        .unwrap_or(bevy::camera::Exposure::SUNLIGHT)
                        .exposure()
                        * 35000.0,
                );
                settings.time.z = slip.flow as f32;
                settings.time.w = 1.0;
                super::streaks::prepare(&mut settings, slip.flow);
            }
        }
        if !view.private {
            let own = ships
                .iter()
                .find(|(ship, _)| Some(ship.0.ship) == observation.0.focused_ship)
                .and_then(|(ship, pose)| {
                    super::wakes::own_wake(ship, pose, slip, view.view, clock.display_ns)
                });
            let own_transit = own.is_some();
            super::wakes::prepare(
                &mut settings,
                own.into_iter()
                    .chain(history.0.wakes.iter().filter(|_| !own_transit).cloned()),
                view,
                transform,
                projection,
                clock.display_ns,
                exposure
                    .copied()
                    .unwrap_or(bevy::camera::Exposure::SUNLIGHT)
                    .exposure(),
            );
            for (i, event) in history
                .0
                .transitions
                .iter()
                .filter(|e| e.view == view.view)
                .take(4)
                .enumerate()
            {
                let age = (i128::from(clock.display_ns) - i128::from(event.time_ns)) as f64 * 1e-9;
                if !(0.0..TRANSITION_LIFETIME_S).contains(&age) {
                    continue;
                }
                let center = event
                    .position
                    .offset_by(bevy::math::DVec3::from_array(event.drift_m_s) * age)
                    .relative_to(view.origin)
                    .as_vec3();
                let local = transform.rotation.inverse() * (center - transform.translation);
                let depth = -local.z;
                if depth <= projection.near {
                    continue;
                }
                let tan = (projection.fov * 0.5).tan();
                let aspect = rect.width() as f32 / rect.height().max(1) as f32;
                settings.centers[i] = Vec4::new(
                    0.5 + local.x / (2.0 * depth * tan * aspect),
                    0.5 - local.y / (2.0 * depth * tan),
                    (event.radius_m.max(8.0) as f32 * 6.0 / (2.0 * depth * tan)).min(0.45),
                    projection.near / depth,
                );
                settings.shapes[i] = Vec4::new(
                    age as f32,
                    (1.0 - age as f32 / 2.0).powi(2),
                    aspect,
                    event.seed as f32 % 1000.0,
                );
            }
        }
        commands.entity(entity).insert(settings);
    }
}

#[derive(Resource)]
struct Pipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    pipeline: CachedRenderPipelineId,
    streaks: CachedRenderPipelineId,
    wakes: CachedRenderPipelineId,
}

fn pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "slip distortion",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<Distortion>(true),
                texture_depth_2d(),
            ),
        ),
    );
    let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("slip distortion".into()),
        layout: vec![layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: assets.load("embedded://osg_client/ui/scene/slip/distortion.wgsl"),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });
    let streak_shader = assets.load("embedded://osg_client/ui/scene/slip/streaks.wgsl");
    let streak_descriptor = RenderPipelineDescriptor {
        label: Some("slip streak strips".into()),
        layout: vec![layout.clone()],
        vertex: VertexState {
            shader: streak_shader.clone(),
            entry_point: Some("vertex".into()),
            ..default()
        },
        fragment: Some(FragmentState {
            shader: streak_shader,
            entry_point: Some("fragment".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::One,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                }),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        primitive: PrimitiveState {
            cull_mode: None,
            ..default()
        },
        ..default()
    };
    let streaks = cache.queue_render_pipeline(streak_descriptor.clone());
    let wake_shader = assets.load("embedded://osg_client/ui/scene/slip/wakes.wgsl");
    let mut wake_descriptor = streak_descriptor;
    wake_descriptor.label = Some("slip wake ribbons".into());
    wake_descriptor.vertex.shader = wake_shader.clone();
    wake_descriptor.fragment.as_mut().unwrap().shader = wake_shader;
    let wakes = cache.queue_render_pipeline(wake_descriptor);
    commands.insert_resource(Pipeline {
        layout,
        sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        pipeline,
        streaks,
        wakes,
    });
}

fn render(
    view: ViewQuery<(
        &ViewTarget,
        &ViewDepthTexture,
        &Distortion,
        &DynamicUniformIndex<Distortion>,
    )>,
    pipeline: Option<Res<Pipeline>>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<Distortion>>,
    mut context: RenderContext,
) {
    let Some(pipeline) = pipeline else { return };
    let Some(compiled) = cache.get_render_pipeline(pipeline.pipeline) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let (target, depth, settings, index) = view.into_inner();
    if settings.time.y <= 0.0
        && settings.screen.w == 0.0
        && settings.shapes.iter().all(|shape| shape.y <= 0.0)
    {
        return;
    }
    let active = settings.time.w > 0.0 && settings.time.y > 0.0;
    let streak_pipeline = cache.get_render_pipeline(pipeline.streaks);
    let wake_pipeline = cache.get_render_pipeline(pipeline.wakes);
    if settings.screen.w > 0.0 && wake_pipeline.is_none() {
        return;
    }
    if active && streak_pipeline.is_none() {
        return;
    }
    let output = target.post_process_write();
    let bind = context.render_device().create_bind_group(
        "slip distortion",
        &cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((output.source, &pipeline.sampler, binding, depth.view())),
    );
    let mut pass = context
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("slip distortion"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: output.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(compiled);
    pass.set_bind_group(0, &bind, &[index.index()]);
    pass.draw(0..3, 0..1);
    if active || settings.screen.w > 0.0 {
        let size = target.main_texture().size();
        let x = (settings.viewport.x * size.width as f32).round() as u32;
        let y = (settings.viewport.y * size.height as f32).round() as u32;
        let width = (settings.viewport.z * size.width as f32).round() as u32;
        let height = (settings.viewport.w * size.height as f32).round() as u32;
        pass.set_scissor_rect(
            x,
            y,
            width.max(1).min(size.width - x),
            height.max(1).min(size.height - y),
        );
        if active && let Some(streak_pipeline) = streak_pipeline {
            pass.set_pipeline(streak_pipeline);
            pass.draw(0..6, 0..settings.screen.z as u32);
        }
        if settings.screen.w > 0.0
            && let Some(wake_pipeline) = wake_pipeline
        {
            pass.set_pipeline(wake_pipeline);
            pass.draw(0..18, 0..settings.screen.w as u32);
        }
    }
}
