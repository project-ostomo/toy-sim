use crate::{GalacticPosition, Star, StarCatalogue, StarId};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};
pub const HEADER_BYTES: u64 = 40;
pub const RECORD_BYTES: u64 = 76;
const MAGIC: &[u8; 8] = b"TOYSTAR\0";
impl StarCatalogue {
    /// Sequentially load fixed-size records, then build the in-memory index.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path.as_ref())
            .with_context(|| format!("opening {}", path.as_ref().display()))?;
        let size = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(1024 * 1024, file);
        let mut h = [0; HEADER_BYTES as usize];
        reader.read_exact(&mut h)?;
        ensure!(&h[..8] == MAGIC, "not a TOYSTAR catalogue");
        let u32_at = |n| u32::from_le_bytes(h[n..n + 4].try_into().unwrap());
        let u64_at = |n| u64::from_le_bytes(h[n..n + 8].try_into().unwrap());
        ensure!(u32_at(8) == 1, "unsupported star format version");
        ensure!(
            u32_at(12) == RECORD_BYTES as u32,
            "unsupported star record size"
        );
        let count = u64_at(16);
        ensure!(
            u32_at(24) == 1,
            "unsupported coordinates: expected integer micrometres"
        );
        ensure!(
            u32_at(28) == 1,
            "unsupported coordinate frame: expected ICRS J2016.0"
        );
        let namespace = u64_at(32);
        ensure!(
            count
                .checked_mul(RECORD_BYTES)
                .and_then(|v| v.checked_add(HEADER_BYTES))
                == Some(size),
            "star count does not match file size"
        );
        let count = usize::try_from(count)?;
        let mut stars = Vec::new();
        stars.try_reserve_exact(count)?;
        for _ in 0..count {
            let mut r = [0; RECORD_BYTES as usize];
            reader.read_exact(&mut r)?;
            let axis = |n| i128::from_le_bytes(r[n..n + 16].try_into().unwrap());
            let star = Star {
                id: StarId {
                    namespace,
                    value: u64::from_le_bytes(r[..8].try_into().unwrap()),
                },
                position: GalacticPosition::new(axis(8), axis(24), axis(40)),
                luminosity: f64::from_le_bytes(r[56..64].try_into().unwrap()),
                colour: std::array::from_fn(|i| {
                    f32::from_le_bytes(r[64 + i * 4..68 + i * 4].try_into().unwrap())
                }),
            };
            stars.push(star);
        }
        Self::from_stars(stars)
    }
    /// Write one namespace in the portable flat format. Indexes are never serialized.
    pub fn save(&self, path: impl AsRef<Path>, namespace: u64) -> Result<()> {
        ensure!(
            self.stars().iter().all(|s| s.id.namespace == namespace),
            "file contains mixed namespaces"
        );
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(MAGIC)?;
        out.write_all(&1u32.to_le_bytes())?;
        out.write_all(&(RECORD_BYTES as u32).to_le_bytes())?;
        out.write_all(&(self.len() as u64).to_le_bytes())?;
        out.write_all(&1u32.to_le_bytes())?; // Micrometres.
        out.write_all(&1u32.to_le_bytes())?; // ICRS, J2016.0.
        out.write_all(&namespace.to_le_bytes())?;
        for s in self.stars() {
            out.write_all(&s.id.value.to_le_bytes())?;
            for x in [s.position.x, s.position.y, s.position.z] {
                out.write_all(&x.to_le_bytes())?;
            }
            out.write_all(&s.luminosity.to_le_bytes())?;
            for c in s.colour {
                out.write_all(&c.to_le_bytes())?;
            }
        }
        out.flush()?;
        Ok(())
    }
}
