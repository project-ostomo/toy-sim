//! Sixty-four BVHs partitioned by factor-of-two luminosity ranges.

use crate::luminosity_bvh::validate;
use crate::{BvhEntry, LuminosityBvh, Position};

const BUCKET_COUNT: usize = 64;

// floor(log2(value)), including subnormals, without rounding at powers of two.
fn binary_exponent(value: f64) -> i32 {
    debug_assert!(value.is_finite() && value > 0.0);
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        63 - bits.leading_zeros() as i32 - 1074
    } else {
        exponent - 1023
    }
}

/// An immutable forest of 64 BVHs, grouping objects of similar luminosity.
///
/// Bucket boundaries increase by a factor of two, starting at the power of two
/// containing the dimmest positive input luminosity. Zero luminosities belong
/// to the first bucket; the final bucket also contains brighter overflow.
/// Empty buckets are empty BVHs. Bucket boundaries are recomputed on each build.
pub struct LuminosityForest<T> {
    buckets: Box<[LuminosityBvh<T>]>,
}

impl<T> LuminosityForest<T> {
    /// Builds 64 buckets whose luminosity boundaries increase by a factor of two.
    ///
    /// If `e = floor(log2(min_positive_luminosity))`, bucket `i` covers
    /// `[2^(e+i), 2^(e+i+1))`. Zero luminosities use bucket zero. Bucket 63 also
    /// includes any brighter outliers, so a range wider than 64 binary exponents
    /// never drops entries. Empty or all-zero input uses the first bucket only.
    /// Node bounds use actual luminosities, including in the overflow bucket.
    /// Each entry is stored exactly once, without cloning its payload.
    ///
    /// # Panics
    ///
    /// Panics if any bound is invalid or a luminosity is non-finite or negative.
    pub fn build(entries: impl IntoIterator<Item = BvhEntry<T>>) -> Self {
        let entries: Vec<_> = entries.into_iter().collect();
        let mut min = f64::INFINITY;
        for entry in &entries {
            validate(entry.bounds, entry.luminosity);
            if entry.luminosity > 0.0 {
                min = min.min(entry.luminosity);
            }
        }

        let min_exponent = if min.is_finite() {
            binary_exponent(min)
        } else {
            0
        };
        let mut buckets: Vec<Vec<BvhEntry<T>>> = (0..BUCKET_COUNT).map(|_| Vec::new()).collect();
        for entry in entries {
            let bucket = if entry.luminosity == 0.0 {
                0
            } else {
                ((binary_exponent(entry.luminosity) - min_exponent) as usize).min(BUCKET_COUNT - 1)
            };
            buckets[bucket].push(entry);
        }

        Self {
            buckets: buckets
                .into_iter()
                .map(|entries| LuminosityBvh::build(entries).0)
                .collect(),
        }
    }

    /// Lazily yields candidates from every bucket using the BVH's flux bound.
    ///
    /// Coordinates are integer micrometres; `threshold` is flux in watts per
    /// square metre. Bounds containing the observer are retained, and a zero
    /// threshold returns every object. Each matching entry is yielded once,
    /// borrowed from the forest, with no guaranteed ordering. As with
    /// [`LuminosityBvh::visible_from`], callers must test actual source positions
    /// and occlusion when necessary.
    ///
    /// # Panics
    ///
    /// Panics immediately if `threshold` is negative or non-finite, even for an
    /// empty forest or if the iterator is never consumed.
    pub fn visible_from(&self, from: Position, threshold: f64) -> impl Iterator<Item = &T> {
        assert!(
            threshold.is_finite() && threshold >= 0.0,
            "visibility threshold must be finite and nonnegative"
        );
        self.buckets
            .iter()
            .flat_map(move |tree| tree.visible_from(from, threshold))
    }
}

#[cfg(test)]
#[path = "tests/forest.rs"]
mod tests;
