use crate::{GalacticPosition, Star, StarId, min_brightness};
use anyhow::{Result, ensure};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
};
use toy_sim_spatial::{Entry, SpatialHash};

/// Immutable after construction; share with Arc for lock-free read-only queries.
pub struct StarCatalogue {
    stars: Vec<Star>,
    identities: HashMap<StarId, usize>,
    index: SpatialHash,
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
    pub buckets_searched: usize,
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
        ensure!(stars.len() <= u32::MAX as usize, "too many stars");
        let mut identities = HashMap::with_capacity(stars.len());
        let mut spatial = SpatialHash::default();
        for (index, star) in stars.iter().enumerate() {
            star.validate()?;
            ensure!(
                identities.insert(star.id, index).is_none(),
                "duplicate star identity {:?}",
                star.id
            );
            spatial.insert(
                index as u32,
                Entry {
                    position: star.position,
                    radius_m: 0.0,
                    luminosity: star.luminosity,
                },
            );
        }
        Ok(Self {
            stars,
            identities,
            index: spatial,
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
    pub fn bucket_count(&self) -> usize {
        self.index.bucket_count()
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
        Ok(self
            .index
            .within_radius(origin, radius)
            .ids
            .into_iter()
            .map(|id| id as usize)
            .collect())
    }

    pub fn nearest(&self, origin: GalacticPosition) -> Result<Option<(usize, f64)>> {
        Ok(self
            .index
            .nearest(origin, f64::INFINITY, 1)
            .first()
            .map(|&id| {
                let index = id as usize;
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
        let visible = self.index.visible(origin, query.min_brightness);
        let mut result = VisibleStars {
            candidates: visible.stats.candidates,
            buckets_searched: visible.stats.buckets_searched,
            ..Default::default()
        };
        let mut best = BinaryHeap::new();
        for id in visible.ids {
            let index = id as usize;
            let star = &self.stars[index];
            if query.excluded.contains(&star.id) {
                continue;
            }
            let d2 = star.position.relative_to(origin).length_squared();
            if d2 <= 0.0 {
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
