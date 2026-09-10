use crate::{GalacticPosition, Star, StarId, min_brightness};
use anyhow::{Result, ensure};
use kdtree::{KdTree, distance::squared_euclidean};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap, HashMap},
};

struct Bucket {
    tree: KdTree<f64, usize, [f64; 3]>,
    max_luminosity: f64,
    extent: f64,
}
/// Immutable after construction; share with Arc for lock-free read-only queries.
pub struct StarCatalogue {
    stars: Vec<Star>,
    identities: HashMap<StarId, usize>,
    anchor: GalacticPosition,
    buckets: Vec<Bucket>,
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
        let anchor = stars.first().map_or(GalacticPosition::ZERO, |s| s.position);
        let mut identities = HashMap::with_capacity(stars.len());
        let mut buckets = BTreeMap::<i32, Bucket>::new();
        for (index, star) in stars.iter().enumerate() {
            star.validate()?;
            ensure!(
                identities.insert(star.id, index).is_none(),
                "duplicate star identity {:?}",
                star.id
            );
            // Adjacent buckets span a factor of two in intrinsic luminosity.
            let key = star.luminosity.log2().floor() as i32;
            let bucket = buckets.entry(key).or_insert_with(|| Bucket {
                tree: KdTree::with_capacity(3, 32),
                max_luminosity: 0.,
                extent: 0.,
            });
            let point = star.position.relative_to(anchor).to_array();
            bucket.max_luminosity = bucket.max_luminosity.max(star.luminosity);
            bucket.extent = bucket
                .extent
                .max(point.iter().map(|v| v.abs()).fold(0., f64::max));
            bucket.tree.add(point, index)?;
        }
        Ok(Self {
            stars,
            identities,
            anchor,
            buckets: buckets.into_values().collect(),
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
        self.buckets.len()
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
        let point = origin.relative_to(self.anchor).to_array();
        let mut found = Vec::new();
        for bucket in &self.buckets {
            for (_, &id) in bucket.tree.within(
                &point,
                padded_radius_squared(bucket, point, radius),
                &squared_euclidean,
            )? {
                if self.stars[id].position.relative_to(origin).length_squared() <= radius * radius {
                    found.push(id);
                }
            }
        }
        Ok(found)
    }
    pub fn nearest(&self, origin: GalacticPosition) -> Result<Option<(usize, f64)>> {
        let point = origin.relative_to(self.anchor).to_array();
        let mut best: Option<(usize, f64)> = None;
        for bucket in &self.buckets {
            for (_, &id) in bucket.tree.nearest(&point, 1, &squared_euclidean)? {
                let d = self.stars[id].position.relative_to(origin).length();
                if best.is_none_or(|(_, old)| d < old) {
                    best = Some((id, d));
                }
            }
        }
        // The KD-tree uses approximate f64 points. Refine in a conservatively
        // padded radius to handle coincident rounded points and tiny separations.
        if let Some((_, radius)) = best {
            for id in self.within_radius(origin, radius)? {
                let d = self.stars[id].position.relative_to(origin).length();
                if best.is_none_or(|(old_id, old)| d < old || (d == old && id < old_id)) {
                    best = Some((id, d));
                }
            }
        }
        Ok(best)
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
        let point = origin.relative_to(self.anchor).to_array();
        let mut result = VisibleStars::default();
        let mut best = BinaryHeap::new();
        for bucket in &self.buckets {
            result.buckets_searched += 1;
            let radius = (bucket.max_luminosity / query.min_brightness).sqrt();
            for (_, &id) in bucket.tree.within(
                &point,
                padded_radius_squared(bucket, point, radius),
                &squared_euclidean,
            )? {
                result.candidates += 1;
                let star = &self.stars[id];
                if query.excluded.contains(&star.id) {
                    continue;
                }
                let d2 = star.position.relative_to(origin).length_squared();
                if d2 <= 0. {
                    continue;
                }
                let brightness = star.luminosity / d2;
                if brightness < query.min_brightness {
                    continue;
                }
                result.matched += 1;
                if query.max_stars == 0 {
                    continue;
                }
                best.push(Ranked {
                    brightness,
                    index: id,
                });
                if best.len() > query.max_stars {
                    best.pop();
                }
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
fn padded_radius_squared(bucket: &Bucket, point: [f64; 3], radius: f64) -> f64 {
    // KD points and the observer have independently rounded coordinates. Expand
    // broad-phase radii, then filter using precise relative positions. Padding
    // is negligible astronomically but prevents false negatives near boundaries.
    let extent = point.iter().map(|v| v.abs()).fold(bucket.extent, f64::max);
    let padding = 32. * f64::EPSILON * (extent + radius) + 1e-6;
    (radius + padding).powi(2)
}
