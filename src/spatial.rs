//! Authoritative centre-based multiresolution hashes. Occluders are additionally
//! indexed by radius class, so padding queries cannot miss large off-range bodies.
use crate::{
    GameState,
    precision::{GalacticPosition, PreciseTransform},
};
use ahash::AHashMap;
use bevy::prelude::*;

#[derive(Component, Clone, Copy)]
pub struct SpatialBody {
    pub radius_m: f64,
    pub occludes: bool,
}

#[derive(Clone, Copy)]
pub struct SpatialObject {
    pub entity: Entity,
    pub position: GalacticPosition,
    pub radius_m: f64,
    pub occludes: bool,
}

struct HashLevel {
    width_um: i128,
    cells: AHashMap<[i128; 3], Vec<usize>>,
}

struct MultiHash {
    levels: Vec<HashLevel>,
}

impl MultiHash {
    fn new() -> Self {
        Self {
            levels: (0..8)
                .map(|level| HashLevel {
                    width_um: 1_000_000_000_000 * 10_i128.pow(level), // 1,000 km to 10 billion km.
                    cells: AHashMap::default(),
                })
                .collect(),
        }
    }

    fn insert(&mut self, id: usize, position: GalacticPosition) {
        for level in &mut self.levels {
            level
                .cells
                .entry(cell_key(position, level.width_um))
                .or_default()
                .push(id);
        }
    }

    fn clear(&mut self) {
        for level in &mut self.levels {
            level.cells.clear();
        }
    }

    fn candidates(&self, centre: GalacticPosition, radius: f64, out: &mut Vec<usize>) {
        let radius_um = (radius * 1_000_000.0).ceil() as i128;
        let level = self
            .levels
            .iter()
            .find(|l| l.width_um >= radius_um)
            .unwrap_or(self.levels.last().unwrap());
        let padding = GalacticPosition::splat(radius_um);
        let lo = cell_key(centre.saturating_sub(padding), level.width_um);
        let hi = cell_key(centre.saturating_add(padding), level.width_um);
        // Unusually enormous queries scan occupied cells instead of billions of empty cells.
        let visits = (0..3).try_fold(1_i128, |n, axis| {
            n.checked_mul(hi[axis].saturating_sub(lo[axis]).saturating_add(1))
        });
        if visits.is_none_or(|n| n > 4096) {
            for (key, entries) in &level.cells {
                if (0..3).all(|k| key[k] >= lo[k] && key[k] <= hi[k]) {
                    out.extend_from_slice(entries);
                }
            }
            return;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if let Some(entries) = level.cells.get(&[x, y, z]) {
                        out.extend_from_slice(entries);
                    }
                }
            }
        }
    }
}

fn cell_key(p: GalacticPosition, width: i128) -> [i128; 3] {
    [
        p.x.div_euclid(width),
        p.y.div_euclid(width),
        p.z.div_euclid(width),
    ]
}

struct OccluderBin {
    upper_radius_m: f64,
    hash: MultiHash,
}

#[derive(Resource)]
pub struct SpatialIndex {
    pub objects: Vec<SpatialObject>,
    targets: MultiHash,
    occluders: Vec<OccluderBin>,
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self {
            objects: vec![],
            targets: MultiHash::new(),
            occluders: vec![],
        }
    }
}

impl SpatialIndex {
    fn clear(&mut self) {
        self.objects.clear();
        self.targets.clear();
        for bin in &mut self.occluders {
            bin.hash.clear();
        }
    }

    pub fn insert(&mut self, object: SpatialObject) {
        assert!(object.radius_m.is_finite() && object.radius_m >= 0.0);
        let id = self.objects.len();
        self.targets.insert(id, object.position);
        if object.occludes {
            let mut bin_index = 0;
            let mut upper = 1000.0;
            while upper < object.radius_m {
                upper *= 8.0;
                bin_index += 1;
            }
            while self.occluders.len() <= bin_index {
                let upper_radius_m = 1000.0 * 8_f64.powi(self.occluders.len() as i32);
                self.occluders.push(OccluderBin {
                    upper_radius_m,
                    hash: MultiHash::new(),
                });
            }
            self.occluders[bin_index].hash.insert(id, object.position);
        }
        self.objects.push(object);
    }

    /// Exact centre-distance query, with no duplicates. Physical extent is intentionally
    /// not part of sensor target range; occluders use the extent-aware query below.
    pub fn within_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        if !radius.is_finite() || radius < 0.0 {
            return vec![];
        }
        let mut ids = vec![];
        self.targets.candidates(centre, radius, &mut ids);
        ids.retain(|&id| {
            self.objects[id]
                .position
                .relative_to(centre)
                .length_squared()
                <= radius * radius
        });
        ids
    }

    /// Every opaque sphere intersecting the sensor's range sphere, even if its
    /// centre is outside range. A body appears in exactly one radius class.
    pub fn occluders_in_range(&self, centre: GalacticPosition, radius: f64) -> Vec<usize> {
        if !radius.is_finite() || radius < 0.0 {
            return vec![];
        }
        let mut ids = vec![];
        for bin in &self.occluders {
            bin.hash
                .candidates(centre, radius + bin.upper_radius_m, &mut ids);
        }
        ids.retain(|&id| {
            let object = self.objects[id];
            object.position.relative_to(centre).length_squared()
                <= (radius + object.radius_m).powi(2)
        });
        ids
    }

    pub fn occupied_cells(&self) -> usize {
        self.targets.levels.iter().map(|l| l.cells.len()).sum()
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensorSystems {
    Index,
    Scan,
}

pub struct SpatialPlugin;
impl Plugin for SpatialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpatialIndex>()
            .configure_sets(
                FixedLast,
                (SensorSystems::Index, SensorSystems::Scan).chain(),
            )
            .add_systems(
                FixedLast,
                rebuild
                    .in_set(SensorSystems::Index)
                    .run_if(in_state(GameState::Game)),
            );
    }
}

fn rebuild(
    mut index: ResMut<SpatialIndex>,
    bodies: Query<(Entity, &PreciseTransform, &SpatialBody)>,
) {
    index.clear();
    for (entity, pose, body) in &bodies {
        index.insert(SpatialObject {
            entity,
            position: pose.translation_um,
            radius_m: body.radius_m,
            occludes: body.occludes,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use rand::{RngExt, SeedableRng};

    #[test]
    fn multiresolution_queries_match_brute_force_at_negative_and_galactic_coordinates() {
        let mut world = World::new();
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(67);
        let origin = GalacticPosition::new(1_i128 << 90, -(1_i128 << 90), -1);
        let mut index = SpatialIndex::default();
        for _ in 0..1000 {
            let delta = DVec3::new(
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
                rng.random_range(-1e10..1e10),
            );
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 10_f64.powf(rng.random_range(1.0..10.0)),
                occludes: rng.random_bool(0.5),
            });
        }
        // Exact boundary and negative-cell probes.
        for delta in [
            DVec3::ZERO,
            DVec3::X * 1e6,
            -DVec3::X * 1e6,
            DVec3::splat(-0.001),
        ] {
            index.insert(SpatialObject {
                entity: world.spawn_empty().id(),
                position: origin.offset_by(delta),
                radius_m: 1.0,
                occludes: true,
            });
        }
        for radius in [0.0, 0.001, 1e6, 1e7, 1e8, 1e9, 1e10, 1e18] {
            let mut actual = index.within_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| o.position.relative_to(origin).length_squared() <= radius * radius)
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
            let mut actual = index.occluders_in_range(origin, radius);
            let expected: Vec<_> = index
                .objects
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    o.occludes
                        && o.position.relative_to(origin).length_squared()
                            <= (radius + o.radius_m).powi(2)
                })
                .map(|(i, _)| i)
                .collect();
            actual.sort_unstable();
            assert_eq!(actual, expected);
        }
        assert_eq!(
            cell_key(GalacticPosition::new(-1, -1_000_000_000, 0), 1_000_000_000),
            [-1, -1, 0]
        );
    }
}
