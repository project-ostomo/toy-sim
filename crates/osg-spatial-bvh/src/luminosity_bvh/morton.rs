use super::{BuildKey, BvhEntry, LuminosityBvh, build_subtree, validate};
use crate::Position;

#[derive(Clone, Copy, Default)]
struct MortonEntry {
    code: u64,
    input_index: usize,
}

impl BuildKey for MortonEntry {
    fn input_index(&self) -> usize {
        self.input_index
    }

    fn split(entries: &[Self]) -> usize {
        let first = entries[0].code;
        let differing = first ^ entries[entries.len() - 1].code;
        if differing == 0 {
            return entries.len() / 2;
        }

        let bit = 1_u64 << (63 - differing.leading_zeros());
        entries.partition_point(|entry| entry.code & bit == 0)
    }
}

fn center<T>(entry: &BvhEntry<T>) -> Position {
    std::array::from_fn(|axis| {
        let min = entry.bounds.min[axis];
        min + (min.abs_diff(entry.bounds.max[axis]) / 2) as i128
    })
}

// Spread the low 21 bits over every third bit of a 63-bit Morton key.
fn spread(mut value: u64) -> u64 {
    value &= 0x1f_ffff;
    value = (value | value << 32) & 0x001f_0000_0000_ffff;
    value = (value | value << 16) & 0x001f_0000_ff00_00ff;
    value = (value | value << 8) & 0x100f_00f0_0f00_f00f;
    value = (value | value << 4) & 0x10c3_0c30_c30c_30c3;
    (value | value << 2) & 0x1249_2492_4924_9249
}

fn radix_sort(entries: &mut Vec<MortonEntry>) {
    let mut scratch = vec![MortonEntry::default(); entries.len()];
    for shift in (0..64).step_by(8) {
        let mut offsets = [0; 256];
        for entry in entries.iter() {
            offsets[((entry.code >> shift) & 255) as usize] += 1;
        }

        let mut start = 0;
        for offset in &mut offsets {
            let count = *offset;
            *offset = start;
            start += count;
        }

        for entry in entries.iter() {
            let bucket = ((entry.code >> shift) & 255) as usize;
            scratch[offsets[bucket]] = *entry;
            offsets[bucket] += 1;
        }
        std::mem::swap(entries, &mut scratch);
    }
}

pub(super) fn build<T>(
    entries: impl IntoIterator<Item = BvhEntry<T>>,
) -> (LuminosityBvh<T>, Vec<usize>) {
    let mut payloads: Vec<_> = entries.into_iter().map(Some).collect();
    let mut min = [i128::MAX; 3];
    let mut max = [i128::MIN; 3];
    for entry in &payloads {
        let entry = entry.as_ref().unwrap();
        validate(entry.bounds, entry.luminosity);
        let center = center(entry);
        for axis in 0..3 {
            min[axis] = min[axis].min(center[axis]);
            max[axis] = max[axis].max(center[axis]);
        }
    }

    let scale: [f64; 3] = std::array::from_fn(|axis| {
        let span = min[axis].abs_diff(max[axis]);
        if span == 0 {
            0.0
        } else {
            0x1f_ffff as f64 / span as f64
        }
    });
    let mut keys: Vec<_> = payloads
        .iter()
        .enumerate()
        .map(|(input_index, entry)| {
            let center = center(entry.as_ref().unwrap());
            let mut code = 0;
            for axis in 0..3 {
                // Subtract in integer space before converting: large galactic
                // translations must not erase small local coordinate differences.
                let offset = min[axis].abs_diff(center[axis]);
                let quantized = ((offset as f64 * scale[axis]) as u64).min(0x1f_ffff);
                code |= spread(quantized) << axis;
            }
            MortonEntry { code, input_index }
        })
        .collect();
    radix_sort(&mut keys);

    let node_count = keys
        .len()
        .checked_add(keys.len().saturating_sub(1))
        .expect("too many entries for a BVH");
    let mut tree = LuminosityBvh {
        nodes: Vec::with_capacity(node_count),
        root: None,
    };
    let mut leaves = vec![0; keys.len()];
    if !keys.is_empty() {
        tree.root = Some(build_subtree(
            &mut tree,
            &mut keys,
            &mut payloads,
            &mut leaves,
        ));
    }
    (tree, leaves)
}
