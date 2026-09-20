use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
use toy_sim_model::{NavigationBeacon, NavigationCatalogue, NavigationSnapshot};

const ASSET_VERSION: u16 = 2;
pub const MAX_CATALOGUE_BYTES: usize = 32 * 1024 * 1024;

fn valid_beacon(beacon: &NavigationBeacon) -> bool {
    !beacon.name.is_empty()
        && beacon.name.len() <= 128
        && super::pose_valid(&beacon.pose)
        && beacon.radius_m.is_finite()
        && beacon.radius_m >= 0.
        && beacon.gate_exit != Some(beacon.id)
}

pub fn validate_catalogue(catalogue: &NavigationCatalogue) -> Result<()> {
    ensure!(
        catalogue.systems.len() <= 65_536 && catalogue.beacons.len() <= 65_536,
        "navigation catalogue limit"
    );
    let mut systems = BTreeSet::new();
    for system in &catalogue.systems {
        ensure!(
            systems.insert(system.id)
                && !system.name.is_empty()
                && system.name.len() <= 128
                && super::position_valid(system.position),
            "invalid navigation system"
        );
    }
    let mut beacons = BTreeMap::new();
    for beacon in &catalogue.beacons {
        ensure!(
            beacons.insert(beacon.id, beacon).is_none()
                && systems.contains(&beacon.system)
                && valid_beacon(beacon),
            "invalid navigation beacon"
        );
    }
    for beacon in &catalogue.beacons {
        ensure!(
            beacon.gate_exit.is_none_or(|exit| {
                beacons
                    .get(&exit)
                    .is_some_and(|paired| paired.gate_exit == Some(beacon.id))
            }),
            "invalid reciprocal gate endpoint"
        );
    }
    Ok(())
}

pub fn encode_catalogue(catalogue: &NavigationCatalogue) -> Result<Vec<u8>> {
    validate_catalogue(catalogue)?;
    let bytes = postcard::to_allocvec(&(ASSET_VERSION, catalogue))?;
    ensure!(
        bytes.len() <= MAX_CATALOGUE_BYTES,
        "navigation asset size limit"
    );
    Ok(bytes)
}

pub fn decode_catalogue(bytes: &[u8]) -> Result<NavigationCatalogue> {
    ensure!(
        bytes.len() <= MAX_CATALOGUE_BYTES,
        "navigation asset size limit"
    );
    let ((version, catalogue), remaining): ((u16, NavigationCatalogue), _) =
        postcard::take_from_bytes(bytes)?;
    ensure!(
        version == ASSET_VERSION,
        "unsupported navigation asset version"
    );
    ensure!(remaining.is_empty(), "trailing navigation asset data");
    validate_catalogue(&catalogue)?;
    Ok(catalogue)
}

pub(crate) fn validate_snapshot(snapshot: &NavigationSnapshot) -> Result<()> {
    ensure!(
        snapshot.beacons.len() <= 65_536 && snapshot.ephemerides.len() <= 8 * 256,
        "navigation snapshot limit"
    );
    ensure!(
        snapshot.catalogue.is_some() || snapshot.beacons.is_empty(),
        "live navigation requires a catalogue"
    );
    let mut identities = BTreeSet::new();
    for beacon in &snapshot.beacons {
        ensure!(
            identities.insert(beacon.id) && valid_beacon(beacon),
            "invalid live navigation beacon"
        );
    }
    let mut systems = BTreeSet::new();
    let mut per_view = BTreeMap::<u64, usize>::new();
    for reference in &snapshot.ephemerides {
        let count = per_view.entry(reference.view).or_default();
        *count += 1;
        ensure!(
            *count <= 256
                && reference.epoch_mjd_utc.is_finite()
                && systems.insert((reference.view, reference.system)),
            "invalid or excessive navigation ephemeris"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::{GalacticPosition, Id, NavigationSystem, Pose};

    fn catalogue() -> NavigationCatalogue {
        let system = Id([1; 16]);
        NavigationCatalogue {
            topology_revision: 91,
            systems: vec![NavigationSystem {
                id: system,
                name: "Home system".into(),
                position: GalacticPosition::ZERO,
                sovereignty: Some(Id([2; 16])),
            }],
            beacons: [3, 4]
                .into_iter()
                .map(|id| NavigationBeacon {
                    id: Id([id; 16]),
                    system,
                    name: format!("Gate {id}"),
                    pose: Pose::default(),
                    radius_m: 1000.,
                    gate_exit: Some(Id([7 - id; 16])),
                    docking: false,
                })
                .collect(),
        }
    }

    #[test]
    fn catalogue_roundtrips_political_metadata_and_rejects_broken_topology() {
        let mut catalogue = catalogue();
        let bytes = encode_catalogue(&catalogue).unwrap();
        assert_eq!(decode_catalogue(&bytes).unwrap(), catalogue);
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_catalogue(&trailing).is_err());
        let mut future = bytes;
        future[0] = u16::MAX as u8;
        assert!(decode_catalogue(&future).is_err());

        catalogue.beacons[1].gate_exit = None;
        assert!(encode_catalogue(&catalogue).is_err());
        catalogue.beacons[1].gate_exit = Some(catalogue.beacons[0].id);
        catalogue.beacons[1].system = Id([9; 16]);
        assert!(encode_catalogue(&catalogue).is_err());
    }

    #[test]
    fn live_snapshot_can_omit_remote_exit_but_rejects_duplicate_or_invalid_pose() {
        let mut snapshot = NavigationSnapshot {
            catalogue: Some([1; 32]),
            beacons: vec![catalogue().beacons.remove(0)],
            ephemerides: Vec::new(),
        };
        validate_snapshot(&snapshot).unwrap();
        snapshot.beacons.push(snapshot.beacons[0].clone());
        assert!(validate_snapshot(&snapshot).is_err());
        snapshot.beacons.pop();
        snapshot.beacons[0].pose.velocity[0] = f64::NAN;
        assert!(validate_snapshot(&snapshot).is_err());
    }
}
