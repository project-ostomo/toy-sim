use anyhow::{Result, ensure};
use osg_model::{InhabitedDirectory, NavigationBeacon, NavigationSnapshot};
use std::collections::BTreeSet;

const ASSET_VERSION: u16 = 4;
pub const MAX_DIRECTORY_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_LIVE_BEACONS: usize = 1024;

fn valid_beacon(beacon: &NavigationBeacon) -> bool {
    !beacon.name.is_empty()
        && beacon.name.len() <= 128
        && super::pose_valid(&beacon.pose)
        && beacon.radius_m.is_finite()
        && beacon.radius_m >= 0.0
        && beacon.systems.windows(2).all(|pair| pair[0] < pair[1])
}

pub fn encode_directory(directory: &InhabitedDirectory) -> Result<Vec<u8>> {
    validate_directory(directory)?;
    ensure!(
        directory.systems.windows(2).all(|pair| pair[0] < pair[1]),
        "inhabited directory must contain sorted unique systems"
    );
    let bytes = postcard::to_allocvec(&(ASSET_VERSION, directory))?;
    ensure!(
        bytes.len() <= MAX_DIRECTORY_BYTES,
        "inhabited directory size limit"
    );
    Ok(bytes)
}

pub fn decode_directory(bytes: &[u8]) -> Result<InhabitedDirectory> {
    ensure!(
        bytes.len() <= MAX_DIRECTORY_BYTES,
        "inhabited directory size limit"
    );
    let ((version, directory), remaining): ((u16, InhabitedDirectory), _) =
        postcard::take_from_bytes(bytes)?;
    ensure!(
        version == ASSET_VERSION,
        "unsupported inhabited directory version"
    );
    ensure!(remaining.is_empty(), "trailing inhabited directory data");
    validate_directory(&directory)?;
    ensure!(
        directory.systems.windows(2).all(|pair| pair[0] < pair[1]),
        "inhabited directory must contain sorted unique systems"
    );
    Ok(directory)
}

fn validate_directory(directory: &InhabitedDirectory) -> Result<()> {
    ensure!(
        directory.sovereignties.len() <= 4096,
        "sovereignty count limit"
    );
    ensure!(
        directory.ownership.iter().all(|(system, owner)| {
            directory.systems.binary_search(system).is_ok()
                && directory.sovereignties.contains_key(owner)
        }),
        "invalid system ownership"
    );
    ensure!(
        directory.sovereignties.iter().all(|(id, sovereignty)| {
            *id == sovereignty.id && !sovereignty.name.is_empty() && sovereignty.name.len() <= 128
        }),
        "invalid public sovereignty"
    );
    Ok(())
}

pub(crate) fn validate_snapshot(snapshot: &NavigationSnapshot) -> Result<()> {
    ensure!(
        snapshot.beacons.len() <= MAX_LIVE_BEACONS,
        "live navigation limit"
    );
    let mut identities = BTreeSet::new();
    for beacon in &snapshot.beacons {
        ensure!(
            identities.insert(beacon.id) && valid_beacon(beacon),
            "invalid live navigation beacon"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::Id;

    #[test]
    fn a_million_inhabited_ids_fit_the_directory_asset() {
        let owner = Id([42; 16]);
        let directory = InhabitedDirectory {
            systems: (0..1_000_000_u128).map(|id| Id(id.to_be_bytes())).collect(),
            ownership: (0..1_000_000_u128)
                .map(|id| (Id(id.to_be_bytes()), owner))
                .collect(),
            sovereignties: [(
                owner,
                osg_model::PublicSovereignty {
                    id: owner,
                    name: "Test sovereignty".into(),
                    bloc: Default::default(),
                },
            )]
            .into(),
        };
        let bytes = encode_directory(&directory).unwrap();
        assert!(bytes.len() > crate::MAX_FRAME);
        assert!(bytes.len() < MAX_DIRECTORY_BYTES);
        assert_eq!(decode_directory(&bytes).unwrap(), directory);
    }

    #[test]
    fn invalid_membership_and_trailing_data_are_rejected() {
        let mut directory = InhabitedDirectory {
            systems: vec![Id([1; 16]), Id([1; 16])],
            ..Default::default()
        };
        assert!(encode_directory(&directory).is_err());
        directory.systems.pop();
        let mut bytes = encode_directory(&directory).unwrap();
        bytes.push(0);
        assert!(decode_directory(&bytes).is_err());
    }
}
