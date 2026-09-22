use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

pub const REFERENCE_YEAR: u16 = 2426;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    Trade,
    Industry,
    Defense,
    Research,
    Relief,
    Salvage,
    Mining,
    Patrol,
    Broadcast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoreStanding {
    Friendly,
    Neutral,
    Hostile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Alliance,
    Trade,
    Competition,
    Dispute,
    ArmedConflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationRelation {
    pub organization: String,
    pub standing: LoreStanding,
    pub kind: RelationKind,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationProfile {
    pub name: String,
    pub sovereignty: String,
    pub home_system: String,
    pub founded_year: u16,
    pub open_membership: bool,
    pub roles: Vec<OrganizationRole>,
    pub history: String,
    pub culture: String,
    pub doctrine: String,
    pub goals: Vec<String>,
    pub resources: Vec<String>,
    pub relations: Vec<OrganizationRelation>,
}

impl OrganizationProfile {
    pub fn id(&self) -> [u8; 16] {
        organization_id(&self.name)
    }
}

pub fn organization_id(name: &str) -> [u8; 16] {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame ownership identity v1");
    hash.update(b"organization\0");
    hash.update(name.as_bytes());
    hash.finalize().as_bytes()[..16].try_into().unwrap()
}

struct Catalogue {
    profiles: Vec<OrganizationProfile>,
    by_id: BTreeMap<[u8; 16], usize>,
    fingerprint: [u8; 32],
}

fn bundled() -> &'static Catalogue {
    static BUNDLED: OnceLock<Catalogue> = OnceLock::new();
    BUNDLED.get_or_init(|| {
        let mut profiles = Vec::new();
        for bytes in [
            include_str!("../data/organizations-union.json"),
            include_str!("../data/organizations-league.json"),
            include_str!("../data/organizations-independent.json"),
        ] {
            profiles.extend(
                serde_json::from_str::<Vec<OrganizationProfile>>(bytes)
                    .expect("bundled organization profiles deserialize"),
            );
        }
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        validate(&profiles).expect("bundled organization profiles are consistent");

        let by_id = profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| (profile.id(), index))
            .collect();
        let bytes = serde_json::to_vec(&(REFERENCE_YEAR, &profiles))
            .expect("public organization catalogue serializes");
        let fingerprint = *blake3::hash(&bytes).as_bytes();
        Catalogue {
            profiles,
            by_id,
            fingerprint,
        }
    })
}

pub fn catalogue() -> &'static [OrganizationProfile] {
    &bundled().profiles
}

pub fn profile(id: [u8; 16]) -> Option<&'static OrganizationProfile> {
    let catalogue = bundled();
    catalogue
        .by_id
        .get(&id)
        .map(|&index| &catalogue.profiles[index])
}

pub fn fingerprint() -> [u8; 32] {
    bundled().fingerprint
}

fn validate(profiles: &[OrganizationProfile]) -> Result<()> {
    ensure!(
        profiles.len() >= 100,
        "at least one hundred public organizations are required"
    );
    let systems: BTreeMap<_, _> = crate::civilization::map()
        .systems
        .iter()
        .map(|system| (system.name.as_str(), system))
        .collect();
    let mut names = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut histories = BTreeSet::new();
    let mut cultures = BTreeSet::new();
    for profile in profiles {
        ensure!(
            !profile.name.trim().is_empty()
                && profile.name.len() <= 128
                && !profile.name.chars().any(char::is_control),
            "invalid organization name"
        );
        ensure!(
            names.insert(profile.name.as_str(), profile).is_none(),
            "duplicate organization name: {}",
            profile.name
        );
        ensure!(ids.insert(profile.id()), "duplicate organization identity");
        ensure!(
            histories.insert(&profile.history) && cultures.insert(&profile.culture),
            "organization history and culture must be distinct: {}",
            profile.name
        );
        let home = systems
            .get(profile.home_system.as_str())
            .ok_or_else(|| anyhow::anyhow!("unknown home system for {}", profile.name))?;
        ensure!(
            home.sovereignty == profile.sovereignty,
            "home system and sovereignty disagree: {}",
            profile.name
        );
        ensure!(
            profile.founded_year > 0 && profile.founded_year <= REFERENCE_YEAR,
            "organization founding lies outside the historical calendar: {}",
            profile.name
        );
        ensure!(
            !profile.roles.is_empty()
                && profile.roles.len() <= 6
                && profile.roles.iter().collect::<BTreeSet<_>>().len() == profile.roles.len(),
            "invalid organization roles: {}",
            profile.name
        );
        ensure!(
            profile.history.split_whitespace().count() >= 70
                && profile.culture.split_whitespace().count() >= 45
                && profile.doctrine.split_whitespace().count() >= 35,
            "organization needs substantive history, culture and doctrine: {}",
            profile.name
        );
        ensure!(
            (3..=8).contains(&profile.goals.len()) && (3..=8).contains(&profile.resources.len()),
            "organization needs concrete goals and resources: {}",
            profile.name
        );
        ensure!(
            profile
                .goals
                .iter()
                .chain(&profile.resources)
                .all(|text| !text.trim().is_empty()),
            "empty organization objective or resource: {}",
            profile.name
        );
        ensure!(
            (2..=16).contains(&profile.relations.len()),
            "organization needs named institutional relationships: {}",
            profile.name
        );
        ensure!(
            profile
                .relations
                .iter()
                .any(|relation| relation.standing == LoreStanding::Friendly)
                && profile
                    .relations
                    .iter()
                    .any(|relation| relation.standing != LoreStanding::Friendly),
            "organization needs both partners and institutional tensions: {}",
            profile.name
        );
    }
    for profile in profiles {
        let mut targets = BTreeSet::new();
        for relation in &profile.relations {
            ensure!(
                relation.organization != profile.name && targets.insert(&relation.organization),
                "duplicate or self relationship: {}",
                profile.name
            );
            let other = names.get(relation.organization.as_str()).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown organization relationship {} -> {}",
                    profile.name,
                    relation.organization
                )
            })?;
            ensure!(
                !relation.reason.trim().is_empty(),
                "relationship needs a public rationale"
            );
            ensure!(
                match relation.kind {
                    RelationKind::Alliance | RelationKind::Trade =>
                        relation.standing == LoreStanding::Friendly,
                    RelationKind::Competition | RelationKind::Dispute =>
                        relation.standing == LoreStanding::Neutral,
                    RelationKind::ArmedConflict => relation.standing == LoreStanding::Hostile,
                },
                "relationship label and standing disagree: {} -> {}",
                profile.name,
                relation.organization
            );
            if let Some(reverse) = other
                .relations
                .iter()
                .find(|value| value.organization == profile.name)
            {
                ensure!(
                    !matches!(
                        (relation.standing, reverse.standing),
                        (LoreStanding::Friendly, LoreStanding::Hostile)
                            | (LoreStanding::Hostile, LoreStanding::Friendly)
                    ),
                    "incompatible reciprocal relationship: {} <-> {}",
                    profile.name,
                    relation.organization
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_catalogue_has_distinct_profiles_for_every_inhabited_polity() {
        let profiles = catalogue();
        validate(profiles).unwrap();
        assert!(profiles.len() >= 100);
        let polities: BTreeSet<_> = crate::civilization::map()
            .systems
            .iter()
            .map(|system| system.sovereignty.as_str())
            .collect();
        let represented: BTreeSet<_> = profiles
            .iter()
            .map(|profile| profile.sovereignty.as_str())
            .collect();
        assert_eq!(represented, polities);
        let roles: BTreeSet<_> = profiles
            .iter()
            .flat_map(|profile| profile.roles.iter().copied())
            .collect();
        assert_eq!(roles.len(), 9);
        let encoded = serde_json::to_vec(profiles).unwrap();
        let decoded: Vec<OrganizationProfile> = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, profiles);
        for expected in profiles {
            assert_eq!(profile(expected.id()), Some(expected));
        }
    }

    #[test]
    fn founding_dates_home_jurisdictions_and_relationships_are_checked() {
        let mut profiles = catalogue().to_vec();
        profiles[0].founded_year = REFERENCE_YEAR + 1;
        assert!(validate(&profiles).is_err());
        profiles[0] = catalogue()[0].clone();
        profiles[0].home_system = "Unknown system".into();
        assert!(validate(&profiles).is_err());
        profiles[0] = catalogue()[0].clone();
        profiles[0].relations[0].organization = "Nonexistent organization".into();
        assert!(validate(&profiles).is_err());
    }

    #[test]
    fn reciprocal_friend_and_enemy_claims_are_rejected() {
        let mut profiles = catalogue().to_vec();
        let a = profiles[0].name.clone();
        let b = profiles[1].name.clone();
        let c = profiles[2].name.clone();
        profiles[0].relations = vec![
            OrganizationRelation {
                organization: b,
                standing: LoreStanding::Friendly,
                kind: RelationKind::Trade,
                reason: "A mutually recognized supply contract.".into(),
            },
            OrganizationRelation {
                organization: c.clone(),
                standing: LoreStanding::Neutral,
                kind: RelationKind::Competition,
                reason: "Competing bids for limited repair capacity.".into(),
            },
        ];
        profiles[1].relations = vec![
            OrganizationRelation {
                organization: a,
                standing: LoreStanding::Hostile,
                kind: RelationKind::ArmedConflict,
                reason: "An incompatible account claiming an ongoing armed conflict.".into(),
            },
            OrganizationRelation {
                organization: c,
                standing: LoreStanding::Friendly,
                kind: RelationKind::Trade,
                reason: "A routine component supply relationship.".into(),
            },
        ];
        let error = validate(&profiles).unwrap_err().to_string();
        assert!(
            error.contains("incompatible reciprocal relationship"),
            "{error}"
        );
    }

    #[test]
    fn lookup_preserves_existing_identities_and_explicit_privateer_hostility() {
        let privateers = profile(organization_id("Terminus Privateers")).unwrap();
        for name in [
            "Helion Flight Cooperative",
            "Unifleet Station Services",
            "St Raphael Trade Confraternity",
        ] {
            let organization = profile(organization_id(name)).unwrap();
            assert_eq!(organization.name, name);
            assert!(
                organization
                    .relations
                    .iter()
                    .any(|relation| relation.organization == privateers.name
                        && relation.standing == LoreStanding::Hostile)
            );
            assert!(
                privateers
                    .relations
                    .iter()
                    .any(|relation| relation.organization == name
                        && relation.standing == LoreStanding::Hostile)
            );
        }
        assert!(profile(organization_id("Unifleet Defense")).is_some());
    }
}
