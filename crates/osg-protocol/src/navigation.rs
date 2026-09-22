use anyhow::Result;
use osg_model::InhabitedDirectory;

pub const MAX_DIRECTORY_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_LIVE_BEACONS: usize = 1024;

pub fn encode_directory(directory: &InhabitedDirectory) -> Result<Vec<u8>> {
    Ok(postcard::to_allocvec(directory)?)
}

pub fn decode_directory(bytes: &[u8]) -> Result<InhabitedDirectory> {
    Ok(postcard::from_bytes(bytes)?)
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
}
