use super::*;
use bevy::ecs::query::QueryData;
use osg_model::travel::Presence;

/// Read-only vessel identity, authority and physical context. Inventory and
/// production state are borrowed separately by the systems that change them.
#[derive(QueryData)]
pub struct Asset {
    pub entity: Entity,
    pub identity: &'static identity::Identity,
    pub owner: &'static ownership::AssetOwner,
    pub access: Option<&'static ownership::AssetAccess>,
    pub hull: &'static hardware::Hull,
    pub presence: &'static travel::PresenceState,
    pub dormant: Has<travel::Dormant>,
    pub design: &'static vessel::ShipDesign,
    pub parts: &'static hardware::PartDevices,
    pub vessel: Option<&'static vessel::Vessel>,
    pub bays: Option<&'static travel::DockingBays>,
    pub stored: Option<&'static travel::StoredShips>,
    pub landmark: Option<&'static super::super::infrastructure::Landmark>,
    pub power: Option<&'static hardware::PowerFlow>,
    pub control: Option<&'static identity::Control>,
}

impl AssetItem<'_, '_> {
    pub fn available(&self) -> bool {
        self.hull.0 > 0. && self.location().is_some()
    }

    pub fn active(&self) -> bool {
        self.available() && !self.dormant
    }

    pub fn location(&self) -> Option<Id> {
        match self.presence.0 {
            Presence::Space => Some(self.identity.0),
            Presence::Docked { host, .. } => Some(host),
            _ => None,
        }
    }

    pub fn permits(
        &self,
        directory: &OwnershipDirectory,
        subject: Principal,
        permission: Permission,
    ) -> bool {
        ownership::treaty_permits(directory, self.owner.0, subject, permission)
            || ownership::permits_principal(
                directory,
                self.owner.0,
                self.access.map(|access| &access.0),
                subject,
                permission,
            )
    }

    pub fn authorize(
        &self,
        directory: &OwnershipDirectory,
        account: AccountId,
        permission: Permission,
    ) -> Result<()> {
        ensure!(
            self.permits(directory, Principal::Player(account), permission),
            "{permission:?} access denied"
        );
        Ok(())
    }

    pub fn operator(&self, directory: &OwnershipDirectory, account: AccountId) -> bool {
        directory.administers(account, self.owner.0)
    }

    pub fn inventory_access(
        &self,
        directory: &OwnershipDirectory,
        account: AccountId,
    ) -> Option<(bool, bool)> {
        if !self.available() {
            return None;
        }
        let subject = Principal::Player(account);
        let manage = self.permits(directory, subject, Permission::Industry);
        let transfer = self.permits(directory, subject, Permission::TransferCargo);
        let view = self.permits(directory, subject, Permission::View);
        (manage || transfer || view).then_some((manage, transfer))
    }

    pub fn name(&self) -> String {
        let name = self
            .vessel
            .map_or("Inventory", |vessel| vessel.vessel_name.as_str());
        let mut bounded = String::new();
        for character in name.chars().filter(|character| !character.is_control()) {
            if bounded.len() + character.len_utf8() > 128 {
                break;
            }
            bounded.push(character);
        }
        if bounded.trim().is_empty() {
            "Inventory".into()
        } else {
            bounded
        }
    }

    pub fn operational(
        &self,
        facility: &IndustrialFacility,
        parts: &Query<&hardware::Device>,
    ) -> Vec<bool> {
        facility
            .modules
            .iter()
            .map(|module| {
                self.active()
                    && self
                        .parts
                        .0
                        .get(module.part_index)
                        .and_then(|entity| parts.get(*entity).ok())
                        .is_some_and(|device| device.0.operational)
            })
            .collect()
    }
}

pub fn lookup(index: &identity::IdentityIndex, id: Id) -> Result<Entity> {
    index
        .entries()
        .get(&id)
        .copied()
        .context("Entity unavailable")
}

pub fn colocated(source: &AssetItem<'_, '_>, target: &AssetItem<'_, '_>) -> Result<()> {
    ensure!(
        source.available() && target.available(),
        "inventory unavailable"
    );
    let from = source.location().context("source inventory unavailable")?;
    let to = target.location().context("target inventory unavailable")?;
    ensure!(
        from == to || from == target.identity.0 || to == source.identity.0,
        "cargo transfers require a shared dock"
    );
    Ok(())
}

pub fn construction_bay(
    assets: &Query<Asset>,
    host: &AssetItem<'_, '_>,
    directory: &OwnershipDirectory,
    owner: Principal,
    radius: f64,
    mass: f64,
) -> Result<u32> {
    ensure!(
        host.active() && host.presence.0 == Presence::Space,
        "shipyard unavailable"
    );
    let mut pending = vec![(host.entity, 0)];
    let mut seen = BTreeSet::new();
    while let Some((entity, depth)) = pending.pop() {
        ensure!(
            seen.insert(entity) && seen.len() <= 8192 && depth < 8,
            "containment limit or cycle"
        );
        if let Some(stored) = assets.get(entity)?.stored {
            pending.extend(stored.iter().map(|child| (child, depth + 1)));
        }
    }

    let bays = host.bays.context("shipyard has no docking aperture")?;
    let bay = bays
        .0
        .iter()
        .position(|bay| {
            radius <= bay.radius_m
                && mass <= bay.mass_capacity_kg
                && (bay.public
                    || matches!(owner, Principal::Player(account) if bay.allowed.contains(&account))
                    || host.permits(directory, owner, Permission::Dock))
        })
        .context("no available authorized docking aperture")?;
    Ok(bay as u32)
}
