use super::ViewCamera;
use crate::{
    state::{
        Celestial, CelestialSystem, DisplayPose, PresentationSet, RenderTime, SessionReset,
        ViewSystems,
    },
    ui::celestials::PlanetSurface,
};
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    math::DVec3,
    prelude::*,
    render::render_resource::{Extent3d, TextureDataOrder, TextureDimension, TextureFormat},
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use osg_universe::surface::{
    SurfaceGenerator, SurfaceParameters, SurfaceTextures, peak_work_bytes, texture_bytes,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Key = [u8; 32];
const MEMORY_BUDGET: usize = 128 * 1024 * 1024;
const RESIDENT_TARGET: usize = MEMORY_BUDGET * 2 / 3;
const MAX_WORKERS: usize = 2;
const FIRST_WIDTH: u32 = 256;

#[derive(Clone)]
pub(super) struct Appearance {
    pub ground: Handle<StandardMaterial>,
    pub cloud: Option<Handle<StandardMaterial>>,
}

struct Resident {
    width: u32,
    charged_bytes: usize,
    images: Vec<Handle<Image>>,
}

struct Entry {
    parameters: SurfaceParameters,
    fallback: Color,
    appearance: Appearance,
    resident: Option<Resident>,
    serial: u64,
    last_used: u64,
    failed: bool,
}

#[derive(Clone, Copy)]
struct Demand {
    key: Key,
    pixels: f64,
    requested_width: u32,
    allocated_width: u32,
    clouds: bool,
}

struct Job {
    key: Key,
    generation: u64,
    serial: u64,
    width: u32,
    reserved_bytes: usize,
    cancelled: Arc<AtomicBool>,
    task: Task<anyhow::Result<Option<SurfaceTextures>>>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Component)]
pub(super) struct CloudLayer(pub f64);

#[derive(Resource, Default)]
pub(super) struct SurfaceCache {
    entries: HashMap<Key, Entry>,
    wanted: Vec<Demand>,
    jobs: Vec<Job>,
    generation: u64,
    frame: u64,
    mesh: Option<Handle<Mesh>>,
}

impl SurfaceCache {
    pub(super) fn materials(
        &mut self,
        parameters: &SurfaceParameters,
        fallback: Color,
        materials: &mut Assets<StandardMaterial>,
    ) -> Appearance {
        self.entries
            .entry(parameters.cache_key())
            .or_insert_with(|| Entry {
                parameters: parameters.clone(),
                fallback,
                appearance: Appearance {
                    ground: materials.add(StandardMaterial {
                        base_color: fallback,
                        perceptual_roughness: 1.,
                        ..default()
                    }),
                    cloud: parameters.has_clouds().then(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::NONE,
                            perceptual_roughness: 1.,
                            alpha_mode: AlphaMode::Blend,
                            ..default()
                        })
                    }),
                },
                resident: None,
                serial: 0,
                last_used: self.frame,
                failed: false,
            })
            .appearance
            .clone()
    }

    pub(super) fn mesh(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.mesh
            .get_or_insert_with(|| {
                let mut mesh = Sphere::new(1.).mesh().uv(192, 96);
                mesh.generate_tangents()
                    .expect("planet sphere has valid UV coordinates");
                meshes.add(mesh)
            })
            .clone()
    }

    fn used_bytes(&self) -> usize {
        self.entries
            .values()
            .filter_map(|entry| entry.resident.as_ref())
            .map(|resident| resident.charged_bytes)
            .sum::<usize>()
            + self
                .jobs
                .iter()
                .map(|job| job.reserved_bytes)
                .sum::<usize>()
    }

    fn accepts(&self, job: &Job) -> bool {
        !job.cancelled.load(Ordering::Relaxed)
            && job.generation == self.generation
            && self
                .entries
                .get(&job.key)
                .is_some_and(|entry| entry.serial == job.serial)
            && self
                .wanted
                .iter()
                .any(|demand| demand.key == job.key && demand.allocated_width >= job.width)
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<SurfaceCache>()
        .add_observer(reset)
        .add_systems(
            Update,
            (gather_demand, manage_tasks, rotate_clouds)
                .chain()
                .after(super::sync_celestials)
                .in_set(PresentationSet::Render),
        );
}

fn reset(
    _: On<SessionReset>,
    mut cache: ResMut<SurfaceCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    cache.generation += 1;
    cache.wanted.clear();
    for job in &cache.jobs {
        job.cancelled.store(true, Ordering::Relaxed);
    }
    for entry in cache.entries.values_mut() {
        evict(entry, &mut materials, &mut images);
    }
    cache.entries.clear();
}

fn projected_pixels(
    relative: DVec3,
    radius: f64,
    viewport: UVec2,
    fov: f64,
    near: f64,
) -> Option<f64> {
    if viewport.x == 0 || viewport.y == 0 || radius <= 0. || !relative.is_finite() {
        return None;
    }
    let depth = -relative.z;
    if depth + radius < near {
        return None;
    }
    let tangent_y = (fov * 0.5).tan();
    let tangent_x = tangent_y * viewport.x as f64 / viewport.y as f64;
    if relative.x.abs() > depth * tangent_x + radius * (1. + tangent_x * tangent_x).sqrt()
        || relative.y.abs() > depth * tangent_y + radius * (1. + tangent_y * tangent_y).sqrt()
    {
        return None;
    }
    let pixels = super::glints::diameter_pixels(radius, depth, viewport.y as f64, fov);
    Some(pixels.min(2. * viewport.as_dvec2().length()))
}

fn lod_width(pixels: f64, previous: u32) -> u32 {
    let mut width = previous.max(FIRST_WIDTH);
    for (threshold, level) in [(80., 512), (240., 1024), (640., 2048)] {
        if pixels > threshold * 1.15 && width < level {
            width = level;
        } else if pixels < threshold / 1.15 && width >= level {
            width = level / 2;
            break;
        }
    }
    width
}

fn resident_bytes(width: u32, clouds: bool) -> usize {
    // Keep room for both the CPU asset and the GPU texture during extraction.
    2 * texture_bytes(width, clouds)
}

fn allocate(demands: &mut [Demand]) {
    demands.sort_by(|a, b| b.pixels.total_cmp(&a.pixels).then(a.key.cmp(&b.key)));
    let mut remaining = RESIDENT_TARGET;
    for demand in demands.iter_mut() {
        let bytes = resident_bytes(FIRST_WIDTH, demand.clouds);
        demand.allocated_width = if bytes <= remaining {
            remaining -= bytes;
            FIRST_WIDTH
        } else {
            0
        };
    }
    for demand in demands {
        while demand.allocated_width > 0 && demand.allocated_width < demand.requested_width {
            let extra = resident_bytes(demand.allocated_width * 2, demand.clouds)
                - resident_bytes(demand.allocated_width, demand.clouds);
            if extra > remaining {
                break;
            }
            demand.allocated_width *= 2;
            remaining -= extra;
        }
    }
}

fn gather_demand(
    mut cache: ResMut<SurfaceCache>,
    cameras: Query<(&ViewCamera, &Transform, &ViewSystems, &Camera, &Projection)>,
    bodies: Query<(&Celestial, &DisplayPose, &CelestialSystem, &PlanetSurface)>,
) {
    cache.frame += 1;
    let previous: HashMap<_, _> = cache
        .wanted
        .iter()
        .map(|demand| (demand.key, demand.requested_width))
        .collect();
    let mut wanted = HashMap::<Key, Demand>::new();
    for (view, transform, systems, camera, projection) in &cameras {
        if view.private || !camera.is_active {
            continue;
        }
        let (Some(viewport), Projection::Perspective(projection)) =
            (camera.physical_viewport_size(), projection)
        else {
            continue;
        };
        let eye = view.origin.offset_by(transform.translation.as_dvec3());
        let rotation = transform.rotation.as_dquat().inverse();
        for (body, pose, system, recipe) in &bodies {
            if !systems.0.iter().any(|reference| *reference == system.0) {
                continue;
            }
            let Some(pixels) = projected_pixels(
                rotation * pose.0.position.relative_to(eye),
                body.0.radius_m,
                viewport,
                projection.fov as f64,
                projection.near as f64,
            ) else {
                continue;
            };
            let key = recipe.0.cache_key();
            if pixels < if previous.contains_key(&key) { 6. } else { 8. } {
                continue;
            }
            wanted
                .entry(key)
                .and_modify(|demand| demand.pixels = demand.pixels.max(pixels))
                .or_insert(Demand {
                    key,
                    pixels,
                    requested_width: 0,
                    allocated_width: 0,
                    clouds: recipe.0.has_clouds(),
                });
        }
    }
    let frame = cache.frame;
    let mut wanted: Vec<_> = wanted.into_values().collect();
    for demand in &mut wanted {
        demand.requested_width = lod_width(
            demand.pixels,
            previous.get(&demand.key).copied().unwrap_or(FIRST_WIDTH),
        );
        if let Some(entry) = cache.entries.get_mut(&demand.key) {
            entry.last_used = frame;
            if !previous.contains_key(&demand.key) {
                entry.serial += 1;
            }
        }
    }
    allocate(&mut wanted);
    cache.wanted = wanted;
    for job in &cache.jobs {
        if !cache.accepts(job) {
            job.cancelled.store(true, Ordering::Relaxed);
        }
    }
}

fn evict(entry: &mut Entry, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) {
    let Some(resident) = entry.resident.take() else {
        return;
    };
    if let Some(mut material) = materials.get_mut(&entry.appearance.ground) {
        material.base_color = entry.fallback;
        material.base_color_texture = None;
        material.normal_map_texture = None;
        material.metallic_roughness_texture = None;
    }
    if let Some(mut material) = entry
        .appearance
        .cloud
        .as_ref()
        .and_then(|handle| materials.get_mut(handle))
    {
        material.base_color = Color::NONE;
        material.base_color_texture = None;
    }
    for handle in resident.images {
        images.remove(handle.id());
    }
}

fn install_textures(
    entry: &mut Entry,
    textures: SurfaceTextures,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    evict(entry, materials, images);
    let width = textures.width;
    let cloud_bytes = textures.clouds.as_ref().map_or(0, Vec::len);
    let bytes =
        textures.color.len() + textures.normal.len() + textures.material.len() + cloud_bytes;
    let color = images.add(image(
        textures.color,
        width,
        textures.height,
        textures.mip_count,
        true,
    ));
    let normal = images.add(image(
        textures.normal,
        width,
        textures.height,
        textures.mip_count,
        false,
    ));
    let roughness = images.add(image(
        textures.material,
        width,
        textures.height,
        textures.mip_count,
        false,
    ));
    if let Some(mut material) = materials.get_mut(&entry.appearance.ground) {
        material.base_color = Color::WHITE;
        material.base_color_texture = Some(color.clone());
        material.normal_map_texture = Some(normal.clone());
        material.metallic_roughness_texture = Some(roughness.clone());
    }
    let mut handles = vec![color, normal, roughness];
    if let Some(clouds) = textures.clouds {
        let handle = images.add(image(
            clouds,
            width / 2,
            textures.height / 2,
            textures.mip_count - 1,
            true,
        ));
        if let Some(mut material) = entry
            .appearance
            .cloud
            .as_ref()
            .and_then(|handle| materials.get_mut(handle))
        {
            material.base_color = Color::WHITE;
            material.base_color_texture = Some(handle.clone());
        }
        handles.push(handle);
    }
    entry.resident = Some(Resident {
        width,
        charged_bytes: 2 * bytes,
        images: handles,
    });
}

fn manage_tasks(
    mut cache: ResMut<SurfaceCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    sources: Query<&PlanetSurface>,
) {
    let allocation: HashMap<_, _> = cache
        .wanted
        .iter()
        .map(|demand| (demand.key, demand.allocated_width))
        .collect();
    for (key, entry) in &mut cache.entries {
        if entry.resident.as_ref().is_some_and(|resident| {
            allocation
                .get(key)
                .is_some_and(|width| resident.width > *width)
        }) {
            evict(entry, &mut materials, &mut images);
        }
    }

    // Poll at most one completion/upload per frame. Canceled tasks retain their slots until done.
    let completed = cache
        .jobs
        .iter_mut()
        .enumerate()
        .find_map(|(index, job)| check_ready(&mut job.task).map(|result| (index, result)));
    if let Some((index, result)) = completed {
        let job = cache.jobs.swap_remove(index);
        if cache.accepts(&job) {
            match result {
                Ok(Some(textures)) => {
                    let texture_bytes = textures.color.len()
                        + textures.normal.len()
                        + textures.material.len()
                        + textures.clouds.as_ref().map_or(0, Vec::len);
                    assert!(2 * texture_bytes <= job.reserved_bytes);
                    let entry = cache.entries.get_mut(&job.key).unwrap();
                    install_textures(entry, textures, &mut materials, &mut images);
                    tracing::debug!(
                        width = job.width,
                        used_mib = cache.used_bytes() / (1024 * 1024),
                        "planet surface ready"
                    );
                }
                Err(error) => {
                    cache.entries.get_mut(&job.key).unwrap().failed = true;
                    tracing::error!(%error, "planet surface generation failed");
                }
                Ok(None) => {}
            }
        }
    }

    let mut candidates = cache.wanted.clone();
    // Every visible body gets a coarse surface before spending work on refinements.
    candidates.sort_by_key(|demand| {
        cache
            .entries
            .get(&demand.key)
            .is_some_and(|entry| entry.resident.is_some())
    });
    for demand in candidates {
        if cache.jobs.len() >= MAX_WORKERS {
            break;
        }
        let Some(entry) = cache.entries.get(&demand.key) else {
            continue;
        };
        let current = entry.resident.as_ref().map_or(0, |resident| resident.width);
        if entry.failed
            || demand.allocated_width <= current
            || cache.jobs.iter().any(|job| job.key == demand.key)
        {
            continue;
        }
        let width = if current == 0 {
            FIRST_WIDTH
        } else {
            demand.allocated_width
        };
        let reserved_bytes =
            peak_work_bytes(width, demand.clouds).max(resident_bytes(width, demand.clouds));
        let mut unused: Vec<_> = cache
            .entries
            .iter()
            .filter(|(key, entry)| entry.resident.is_some() && !allocation.contains_key(*key))
            .map(|(key, entry)| (*key, entry.last_used))
            .collect();
        unused.sort_by_key(|(key, frame)| (*frame, *key));
        for (key, _) in unused {
            if cache.used_bytes() + reserved_bytes <= MEMORY_BUDGET {
                break;
            }
            evict(
                cache.entries.get_mut(&key).unwrap(),
                &mut materials,
                &mut images,
            );
        }
        if cache.used_bytes() + reserved_bytes > MEMORY_BUDGET {
            continue;
        }
        let entry = &cache.entries[&demand.key];
        let parameters = entry.parameters.clone();
        let serial = entry.serial;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            SurfaceGenerator::new(parameters)?
                .bake_while(width, || cancellation.load(Ordering::Relaxed))
        });
        let generation = cache.generation;
        cache.jobs.push(Job {
            key: demand.key,
            generation,
            serial,
            width,
            reserved_bytes,
            cancelled,
            task,
        });
    }

    if cache.entries.len() > 64 {
        let live: HashSet<_> = sources.iter().map(|recipe| recipe.0.cache_key()).collect();
        let busy: HashSet<_> = cache.jobs.iter().map(|job| job.key).collect();
        let stale: Vec<_> = cache
            .entries
            .keys()
            .filter(|key| !live.contains(*key) && !busy.contains(*key))
            .copied()
            .collect();
        for key in stale {
            if let Some(mut entry) = cache.entries.remove(&key) {
                evict(&mut entry, &mut materials, &mut images);
            }
        }
    }
    debug_assert!(cache.used_bytes() <= MEMORY_BUDGET);
}

fn rotate_clouds(clock: Res<RenderTime>, mut clouds: Query<(&CloudLayer, &mut Transform)>) {
    for (cloud, mut transform) in &mut clouds {
        let angle = (cloud.0 * clock.display_ns as f64 * 1e-9).rem_euclid(std::f64::consts::TAU);
        transform.rotation = Quat::from_rotation_z(angle as f32);
    }
}

fn image(data: Vec<u8>, width: u32, height: u32, mips: u32, srgb: bool) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.data_order = TextureDataOrder::MipMajor;
    image.texture_descriptor.mip_level_count = mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use osg_universe::orrery_cfg::{Body, PlanetKind, PlanetParameters};

    fn parameters() -> SurfaceParameters {
        let mut parameters = SurfaceParameters::from_body(&Body {
            radius: 6e6,
            mass: 5e24,
            planet: Some(PlanetParameters {
                seed: [7; 32],
                kind: PlanetKind::Ocean,
                equilibrium_temperature_k: 255.,
                temperature_k: 288.,
                bond_albedo: 0.3,
                ocean_fraction: 0.7,
                relief_m: 6_000.,
                cloud_fraction: 0.5,
                cloud_altitude_m: 8_000.,
                cloud_rotation_period_s: 400_000.,
                biosphere: true,
            }),
            ..default()
        })
        .unwrap();
        parameters.atmospheric_pressure_pa = 100_000.;
        parameters
    }

    fn demand(key: Key, pixels: f64) -> Demand {
        Demand {
            key,
            pixels,
            requested_width: 2048,
            allocated_width: 2048,
            clouds: true,
        }
    }

    fn pending_job(key: Key, generation: u64, serial: u64) -> Job {
        let pool = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        Job {
            key,
            generation,
            serial,
            width: FIRST_WIDTH,
            reserved_bytes: 1_000_000,
            cancelled: Arc::new(AtomicBool::new(false)),
            task: pool.spawn(std::future::pending()),
        }
    }

    #[test]
    fn view_demand_shares_appearances_and_ignores_private_inactive_or_unsubscribed_views() {
        use crate::state::ViewObservation;
        use osg_model::{
            GalacticPosition, Id, Pose, ViewState, presentation::CelestialPresentation,
        };

        let mut world = World::new();
        world.init_resource::<SurfaceCache>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let system = Id([3; 16]);
        let mut views = Vec::new();
        for id in 1..=2 {
            views.push(
                world
                    .spawn((
                        ViewObservation(ViewState {
                            id,
                            revision: 1,
                            focused_ship: None,
                            origin: GalacticPosition::ZERO,
                        }),
                        ViewSystems(vec![system]),
                    ))
                    .id(),
            );
        }
        let params = parameters();
        let pose = Pose {
            position: GalacticPosition::from_meters(DVec3::new(0., 0., -6e7)),
            ..default()
        };
        world.spawn((
            Celestial(CelestialPresentation {
                reference: osg_model::travel::CelestialRef {
                    system: osg_model::Id::default(),
                    body: osg_model::Id::default(),
                },
                entity: Id([5; 16]),
                name: "Test planet".into(),
                pose: pose.clone(),
                radius_m: params.radius_m,
                gravitational_parameter: 1.,
                luminosity_lumens: 0.,
                temperature_k: params.temperature_k,
                color: [0.3; 3],
                atmosphere: None,
            }),
            DisplayPose(pose),
            CelestialSystem(system),
            PlanetSurface(params),
        ));
        world
            .run_system_once(super::super::camera::setup_views)
            .unwrap();
        for (index, entity) in views.iter().enumerate() {
            *world.get_mut::<Transform>(*entity).unwrap() = Transform::IDENTITY;
            world.get_mut::<Camera>(*entity).unwrap().viewport = Some(bevy::camera::Viewport {
                physical_size: UVec2::new(960, 540) * (index as u32 * 3 + 1),
                ..default()
            });
        }
        world
            .run_system_once(super::super::sync_celestials)
            .unwrap();
        world.run_system_once(gather_demand).unwrap();
        let cache = world.resource::<SurfaceCache>();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.wanted.len(), 1);
        let high_pixels = cache.wanted[0].pixels;
        assert_eq!(world.resource::<Assets<Mesh>>().len(), 1);
        assert_eq!(world.resource::<Assets<StandardMaterial>>().len(), 2);

        world.get_mut::<ViewCamera>(views[1]).unwrap().private = true;
        world.run_system_once(gather_demand).unwrap();
        let low_pixels = world.resource::<SurfaceCache>().wanted[0].pixels;
        assert!((high_pixels - 4. * low_pixels).abs() < 1e-6);
        world.get_mut::<Camera>(views[0]).unwrap().is_active = false;
        world.run_system_once(gather_demand).unwrap();
        assert!(world.resource::<SurfaceCache>().wanted.is_empty());
        world.get_mut::<Camera>(views[0]).unwrap().is_active = true;
        world.get_mut::<ViewSystems>(views[0]).unwrap().0.clear();
        world.run_system_once(gather_demand).unwrap();
        assert!(world.resource::<SurfaceCache>().wanted.is_empty());
    }

    #[test]
    fn demand_uses_physical_pixels_and_conservatively_keeps_partial_discs() {
        let size = UVec2::new(1920, 1080);
        let fov = std::f64::consts::FRAC_PI_2;
        let center = DVec3::new(0., 0., -100.);
        let pixels = projected_pixels(center, 1., size, fov, 0.1).unwrap();
        let high_dpi = projected_pixels(center, 1., size * 2, fov, 0.1).unwrap();
        assert!((high_dpi - pixels * 2.).abs() < 1e-9);
        assert!(projected_pixels(DVec3::new(0., 0., 100.), 1., size, fov, 0.1).is_none());
        assert!(projected_pixels(DVec3::new(1000., 0., -100.), 1., size, fov, 0.1).is_none());
        assert!(projected_pixels(DVec3::new(180., 0., -100.), 5., size, fov, 0.1).is_some());
        assert!(
            projected_pixels(DVec3::ZERO, 100., size, fov, 0.1)
                .unwrap()
                .is_finite()
        );
    }

    #[test]
    fn lod_hysteresis_does_not_rebuild_at_a_threshold() {
        assert_eq!(lod_width(79., 256), 256);
        assert_eq!(lod_width(81., 256), 256);
        assert_eq!(lod_width(79., 512), 512);
        assert_eq!(lod_width(81., 512), 512);
        assert_eq!(lod_width(100., 256), 512);
        assert_eq!(lod_width(10., 2048), 256);
    }

    #[test]
    fn resolution_allocation_is_stable_and_bounded_for_many_visible_planets() {
        let mut demands: Vec<_> = (0_u64..400)
            .map(|index| {
                let mut key = [0; 32];
                key[..8].copy_from_slice(&index.to_be_bytes());
                demand(key, 1000.)
            })
            .collect();
        let mut reversed = demands.clone();
        reversed.reverse();
        allocate(&mut demands);
        allocate(&mut reversed);
        let layouts = |demands: &[Demand]| {
            demands
                .iter()
                .map(|demand| (demand.key, demand.allocated_width))
                .collect::<Vec<_>>()
        };
        assert_eq!(layouts(&demands), layouts(&reversed));
        assert!(demands.iter().any(|demand| demand.allocated_width == 0));
        let bytes: usize = demands
            .iter()
            .filter(|demand| demand.allocated_width > 0)
            .map(|demand| resident_bytes(demand.allocated_width, demand.clouds))
            .sum();
        assert!(bytes <= RESIDENT_TARGET);
    }

    #[test]
    fn shared_material_eviction_detaches_textures_even_when_views_keep_handles() {
        let params = parameters();
        let key = params.cache_key();
        let mut cache = SurfaceCache::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut images = Assets::<Image>::default();
        let first_view = cache.materials(&params, Color::srgb(0.2, 0.3, 0.4), &mut materials);
        let second_view = cache.materials(&params, Color::BLACK, &mut materials);
        assert_eq!(first_view.ground.id(), second_view.ground.id());
        assert_eq!(
            first_view.cloud.as_ref().unwrap().id(),
            second_view.cloud.as_ref().unwrap().id()
        );

        let textures = SurfaceGenerator::new(params).unwrap().bake(16).unwrap();
        let entry = cache.entries.get_mut(&key).unwrap();
        install_textures(entry, textures, &mut materials, &mut images);
        assert!(
            materials
                .get(&first_view.ground)
                .unwrap()
                .base_color_texture
                .is_some()
        );
        assert_eq!(images.len(), 4);
        evict(entry, &mut materials, &mut images);
        for view in [&first_view, &second_view] {
            let ground = materials.get(&view.ground).unwrap();
            assert!(ground.base_color_texture.is_none());
            assert!(ground.normal_map_texture.is_none());
            assert!(ground.metallic_roughness_texture.is_none());
            assert!(
                materials
                    .get(view.cloud.as_ref().unwrap())
                    .unwrap()
                    .base_color_texture
                    .is_none()
            );
        }
        assert!(images.is_empty());
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn unsubscribe_resubscribe_and_session_reset_reject_old_completions() {
        let params = parameters();
        let key = params.cache_key();
        let mut cache = SurfaceCache::default();
        let mut materials = Assets::<StandardMaterial>::default();
        cache.materials(&params, Color::BLACK, &mut materials);
        cache.wanted.push(demand(key, 1000.));
        let job = pending_job(key, cache.generation, 0);
        assert!(cache.accepts(&job));
        cache.wanted.clear();
        assert!(!cache.accepts(&job));
        cache.wanted.push(demand(key, 1000.));
        cache.entries.get_mut(&key).unwrap().serial += 1;
        assert!(!cache.accepts(&job));
        let new_job = pending_job(key, cache.generation, 1);
        assert!(cache.accepts(&new_job));
        cache.generation += 1;
        assert!(!cache.accepts(&new_job));
    }

    #[test]
    fn canceled_unfinished_jobs_keep_worker_slots_and_memory_reservations() {
        let params = parameters();
        let key = params.cache_key();
        let mut app = App::new();
        app.init_resource::<SurfaceCache>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<Image>>()
            .add_systems(Update, manage_tasks);
        app.world_mut()
            .resource_scope(|world, mut cache: Mut<SurfaceCache>| {
                cache.materials(
                    &params,
                    Color::BLACK,
                    &mut world.resource_mut::<Assets<StandardMaterial>>(),
                );
                cache.wanted.push(demand(key, 1000.));
                for index in 0..MAX_WORKERS {
                    let job = pending_job([index as u8; 32], 0, 0);
                    job.cancelled.store(true, Ordering::Relaxed);
                    cache.jobs.push(job);
                }
            });
        app.update();
        let cache = app.world().resource::<SurfaceCache>();
        assert_eq!(cache.jobs.len(), MAX_WORKERS);
        assert!(
            cache
                .jobs
                .iter()
                .all(|job| job.cancelled.load(Ordering::Relaxed))
        );
        assert_eq!(cache.used_bytes(), 2_000_000);
        assert!(cache.entries[&key].resident.is_none());
    }
}
