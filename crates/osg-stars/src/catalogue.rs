use crate::{GalacticPosition, Star, StarId, min_brightness};
use anyhow::{Result, ensure};
use osg_spatial_bvh::{
    Aabb, BvhEntry, OPTICAL_LUMENS_PER_WATT, QueryBudget, RecordKind, SpatialQuery, SpatialRecord,
    SpatialService,
};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
};
use std::{f64::consts::PI, sync::Arc};

/// Immutable after construction; share with Arc for lock-free read-only queries.
pub struct StarCatalogue {
    stars: Vec<Star>,
    identities: HashMap<StarId, usize>,
    index: Arc<SpatialService<SpatialRecord>>,
}
#[derive(Clone, Copy)]
pub struct VisibilityQuery<'a> {
    pub min_brightness: f64,
    pub max_stars: usize,
    pub excluded: &'a [StarId],
}
impl VisibilityQuery<'_> {
    pub fn magnitude(limit: f64) -> Self {
        Self {
            min_brightness: min_brightness(limit),
            max_stars: usize::MAX,
            excluded: &[],
        }
    }
}
#[derive(Debug, Default)]
pub struct VisibleStars {
    /// Compact indices into StarCatalogue::stars(), brightest first.
    pub indices: Vec<usize>,
    pub matched: usize,
    pub candidates: usize,
    pub nodes_visited: usize,
}
#[derive(PartialEq)]
struct Ranked {
    brightness: f64,
    index: usize,
}
impl Eq for Ranked {}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .brightness
            .total_cmp(&self.brightness)
            .then_with(|| self.index.cmp(&other.index))
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl StarCatalogue {
    pub fn from_stars(stars: Vec<Star>) -> Result<Self> {
        for star in &stars {
            star.validate()?;
        }
        let index = Arc::new(SpatialService::new(stars.iter().enumerate().map(
            |(id, star)| BvhEntry {
                object: SpatialRecord {
                    kind: RecordKind::Source,
                    id: id as u64,
                    position: star.position.to_array(),
                    radius_m: 0.0,
                },
                bounds: Aabb::sphere(star.position.to_array(), 0.0),
                luminosity: star.luminosity / OPTICAL_LUMENS_PER_WATT,
            },
        )));
        Self::from_shared(stars, index)
    }

    /// Attach catalogue metadata to the universe's existing source records.
    /// Source IDs must match the supplied star order.
    pub fn from_shared(
        stars: Vec<Star>,
        index: Arc<SpatialService<SpatialRecord>>,
    ) -> Result<Self> {
        ensure!(stars.len() <= u32::MAX as usize, "too many stars");
        let mut identities = HashMap::with_capacity(stars.len());
        for (index, star) in stars.iter().enumerate() {
            star.validate()?;
            ensure!(
                identities.insert(star.id, index).is_none(),
                "duplicate star identity {:?}",
                star.id
            );
        }
        Ok(Self {
            stars,
            identities,
            index,
        })
    }
    pub fn stars(&self) -> &[Star] {
        &self.stars
    }
    pub fn len(&self) -> usize {
        self.stars.len()
    }
    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }
    pub fn node_count(&self) -> usize {
        self.index.node_count()
    }
    pub fn star(&self, id: StarId) -> Option<&Star> {
        self.identities.get(&id).map(|&i| &self.stars[i])
    }
    /// All spatial math at leaves uses exact integer subtraction before f64 conversion.
    pub fn within_radius(&self, origin: GalacticPosition, radius: f64) -> Result<Vec<usize>> {
        ensure!(
            radius.is_finite() && radius >= 0.,
            "radius must be finite and nonnegative"
        );
        let mut found: Vec<_> = self
            .index
            .sphere_candidates(origin.to_array(), radius)
            .into_iter()
            .filter(|r| r.kind == RecordKind::Source && r.distance(origin.to_array()) <= radius)
            .map(|r| r.id as usize)
            .collect();
        found.sort_unstable();
        Ok(found)
    }

    pub fn nearest(&self, origin: GalacticPosition) -> Result<Option<(usize, f64)>> {
        Ok(self
            .index
            .nearest(origin.to_array(), f64::INFINITY, 1, |r| {
                (r.kind == RecordKind::Source).then(|| r.distance(origin.to_array()))
            })
            .first()
            .map(|record| {
                let index = record.id as usize;
                (
                    index,
                    self.stars[index].position.relative_to(origin).length(),
                )
            }))
    }

    pub fn visible(
        &self,
        origin: GalacticPosition,
        query: VisibilityQuery<'_>,
    ) -> Result<VisibleStars> {
        ensure!(
            query.min_brightness.is_finite() && query.min_brightness >= 0.,
            "brightness threshold must be finite and nonnegative"
        );
        let mut cursor = self.index.query(SpatialQuery::Visibility {
            observer: origin.to_array(),
            observer_radius_m: 0.0,
            min_flux_w_m2: query.min_brightness / (4.0 * PI * OPTICAL_LUMENS_PER_WATT),
        });
        let visible = cursor.advance(QueryBudget {
            max_work: usize::MAX,
            max_results: usize::MAX,
        });
        let mut result = VisibleStars {
            candidates: visible.stats.objects_tested,
            nodes_visited: visible.stats.nodes_visited,
            ..Default::default()
        };
        let mut best = BinaryHeap::new();
        for record in visible.objects {
            if record.kind != RecordKind::Source {
                continue;
            }
            let index = record.id as usize;
            let star = &self.stars[index];
            if query.excluded.contains(&star.id) {
                continue;
            }
            let d2 = star.position.relative_to(origin).length_squared();
            if d2 <= 0.0 || star.luminosity / d2 < query.min_brightness {
                continue;
            }
            result.matched += 1;
            if query.max_stars == 0 {
                continue;
            }
            best.push(Ranked {
                brightness: star.luminosity / d2,
                index,
            });
            if best.len() > query.max_stars {
                best.pop();
            }
        }
        result.indices = best
            .into_sorted_vec()
            .into_iter()
            .map(|r| r.index)
            .collect();
        Ok(result)
    }
}
