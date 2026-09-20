use anyhow::{Result, ensure};
use osg_model::industry::*;
use std::collections::BTreeSet;

fn validate_upload_ack(ack: &BlueprintUploadAck) -> Result<()> {
    if let BlueprintUploadAck::Rejected { reason } = ack {
        ensure!(
            !reason.trim().is_empty() && reason.len() <= MAX_BLUEPRINT_UPLOAD_ERROR_BYTES,
            "invalid blueprint upload error"
        );
    }
    Ok(())
}

pub fn encode_blueprint_upload_ack(ack: &BlueprintUploadAck) -> Result<Vec<u8>> {
    validate_upload_ack(ack)?;
    let bytes = postcard::to_allocvec(ack)?;
    ensure!(
        bytes.len() <= MAX_BLUEPRINT_UPLOAD_ACK_BYTES,
        "blueprint upload acknowledgement limit"
    );
    Ok(bytes)
}

pub fn decode_blueprint_upload_ack(bytes: &[u8]) -> Result<BlueprintUploadAck> {
    ensure!(
        bytes.len() <= MAX_BLUEPRINT_UPLOAD_ACK_BYTES,
        "blueprint upload acknowledgement limit"
    );
    let (ack, remaining) = postcard::take_from_bytes(bytes)?;
    ensure!(
        remaining.is_empty(),
        "trailing blueprint upload acknowledgement data"
    );
    validate_upload_ack(&ack)?;
    Ok(ack)
}

fn text_valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn item_valid(item: &CargoItem) -> bool {
    match item {
        CargoItem::Resource(id) | CargoItem::Part(id) => text_valid(id),
    }
}

fn nonnegative(value: f64) -> bool {
    value.is_finite() && value >= 0.
}

fn validate_summary(entry: &FacilitySummary) -> Result<()> {
    ensure!(
        text_valid(&entry.name)
            && entry.capabilities.len() <= 4
            && entry.capabilities.iter().collect::<BTreeSet<_>>().len() == entry.capabilities.len(),
        "invalid industry directory entry"
    );
    Ok(())
}

fn validate_hangar(hangar: &HangarView) -> Result<()> {
    ensure!(
        text_valid(&hangar.host_name) && hangar.ships.len() <= MAX_DIRECTORY_ENTRIES,
        "invalid hangar directory"
    );
    if let Some(host) = &hangar.host_inventory {
        validate_summary(host)?;
        ensure!(host.entity == hangar.host, "hangar inventory host mismatch");
    }

    let mut previous = None;
    for entry in &hangar.ships {
        validate_summary(&entry.inventory)?;
        ensure!(
            entry.inventory.entity != hangar.host
                && entry.inventory.location == Some(hangar.host)
                && previous.is_none_or(|id| id < entry.inventory.entity)
                && (entry.can_focus || entry.can_open_inventory)
                && (!entry.can_control || entry.can_focus)
                && (entry.can_open_inventory
                    || (!entry.inventory.can_manage && !entry.inventory.can_transfer)),
            "invalid hangar ship"
        );
        previous = Some(entry.inventory.entity);
    }
    ensure!(
        hangar.next.is_none() || hangar.next == previous,
        "invalid hangar directory cursor"
    );
    Ok(())
}

fn validate_items(items: &[ItemStack]) -> Result<()> {
    let mut unique = BTreeSet::new();
    ensure!(items.len() <= 256, "industry item list exceeds limit");
    for item in items {
        ensure!(
            item_valid(&item.item) && item.quantity > 0 && unique.insert(&item.item),
            "invalid industry item stack"
        );
    }
    Ok(())
}

pub(super) fn validate_cargo(items: &[CargoStack]) -> Result<()> {
    let mut unique = BTreeSet::new();
    ensure!(items.len() <= MAX_CARGO_STACKS, "cargo stack limit");
    for item in items {
        ensure!(
            item_valid(&item.item)
                && text_valid(&item.name)
                && item.quantity > 0
                && item.reserved <= item.quantity
                && nonnegative(item.unit_mass_kg)
                && nonnegative(item.unit_volume_m3)
                && unique.insert(&item.item),
            "invalid cargo stack"
        );
    }
    Ok(())
}

pub(super) fn validate_subscription(subscription: &IndustrySubscription) -> Result<()> {
    ensure!(
        subscription.inventories.len() <= MAX_SUBSCRIBED_INVENTORIES
            && subscription
                .inventories
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                == subscription.inventories.len(),
        "invalid industry inventory subscription"
    );
    ensure!(
        subscription.directory || subscription.directory_after.is_none(),
        "industry cursor requires directory subscription"
    );
    Ok(())
}

pub(super) fn validate_command(command: &IndustryCommand) -> Result<()> {
    match command {
        IndustryCommand::Refill {
            resource, quantity, ..
        }
        | IndustryCommand::UnloadProduct {
            resource, quantity, ..
        } => {
            ensure!(
                text_valid(resource) && *quantity > 0,
                "invalid resource transfer request"
            );
        }
        IndustryCommand::StartRecipe {
            recipe, batches, ..
        } => {
            ensure!(
                text_valid(recipe) && (1..=MAX_RECIPE_BATCHES).contains(batches),
                "invalid recipe request"
            );
        }
        IndustryCommand::BuildShip { .. } => {}
        IndustryCommand::Transfer {
            source,
            target,
            item,
            quantity,
        } => {
            ensure!(
                source != target && item_valid(item) && *quantity > 0,
                "invalid cargo transfer"
            );
        }
        IndustryCommand::CancelJob { .. } => {}
    }
    Ok(())
}

pub(super) fn validate_snapshot(snapshot: &IndustrySnapshot) -> Result<()> {
    validate_snapshot_content(snapshot)?;
    ensure!(
        postcard::to_allocvec(snapshot)?.len() <= MAX_SNAPSHOT_BYTES,
        "industry snapshot byte limit"
    );
    Ok(())
}

pub fn validate_snapshot_content(snapshot: &IndustrySnapshot) -> Result<()> {
    ensure!(
        snapshot.directory.len() <= MAX_DIRECTORY_ENTRIES
            && snapshot.facilities.len() + snapshot.omitted_inventories.len()
                <= MAX_SUBSCRIBED_INVENTORIES
            && snapshot
                .error
                .as_ref()
                .is_none_or(|message| !message.is_empty() && message.len() <= 512),
        "industry snapshot limit"
    );
    let mut directory = BTreeSet::new();
    for entry in &snapshot.directory {
        validate_summary(entry)?;
        ensure!(
            directory.insert(entry.entity),
            "duplicate industry directory entry"
        );
    }
    ensure!(
        snapshot.directory_next.is_none()
            || snapshot.directory.last().map(|entry| entry.entity) == snapshot.directory_next,
        "invalid industry directory cursor"
    );
    if let Some(hangar) = &snapshot.hangar {
        validate_hangar(hangar)?;
    }
    let mut facilities = BTreeSet::new();
    for omitted in &snapshot.omitted_inventories {
        ensure!(facilities.insert(*omitted), "duplicate omitted inventory");
    }
    for facility in &snapshot.facilities {
        ensure!(
            facilities.insert(facility.entity)
                && text_valid(&facility.name)
                && nonnegative(facility.cargo_capacity_m3)
                && nonnegative(facility.cargo_used_m3)
                && facility.jobs.len() <= MAX_FACILITY_JOBS
                && facility.capabilities.len() <= 4096,
            "invalid industry facility"
        );
        validate_cargo(&facility.items)?;
        validate_cargo(&facility.products)?;
        ensure!(
            facility.products.iter().all(|product| {
                matches!(product.item, CargoItem::Resource(_)) && product.reserved == 0
            }),
            "invalid product reservoir stock"
        );
        let mut jobs = BTreeSet::new();
        for job in &facility.jobs {
            ensure!(
                jobs.insert(job.id)
                    && text_valid(&job.name)
                    && job.duration_ticks > 0
                    && job.progress_ticks <= job.duration_ticks
                    && job.supplied_power_w <= job.requested_power_w,
                "invalid industry job"
            );
        }
        let mut parts = BTreeSet::new();
        for capability in &facility.capabilities {
            ensure!(
                parts.insert(capability.part)
                    && capability.lanes > 0
                    && capability.power_per_lane_w > 0
                    && capability
                        .max_radius_m
                        .is_none_or(|radius| radius.is_finite() && radius > 0.),
                "invalid industry module"
            );
        }
    }
    if let Some(catalogue) = &snapshot.catalogue {
        ensure!(
            catalogue.recipes.len() <= MAX_CATALOGUE_RECIPES
                && catalogue.blueprints.len() <= MAX_CATALOGUE_BLUEPRINTS,
            "industry catalogue limit"
        );
        let mut recipes = BTreeSet::new();
        for recipe in &catalogue.recipes {
            ensure!(
                text_valid(&recipe.id)
                    && text_valid(&recipe.name)
                    && recipes.insert(&recipe.id)
                    && recipe.duration_ticks > 0
                    && !recipe.inputs.is_empty()
                    && !recipe.outputs.is_empty(),
                "invalid industry recipe"
            );
            validate_items(&recipe.inputs)?;
            validate_items(&recipe.outputs)?;
        }
        for blueprint in &catalogue.blueprints {
            ensure!(
                text_valid(&blueprint.name)
                    && !blueprint.blueprint.is_empty()
                    && blueprint.blueprint.len() <= 48 * 1024
                    && blueprint.duration_ticks > 0,
                "invalid industry blueprint"
            );
            validate_items(&blueprint.inputs)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::{Id, ownership::Principal};

    #[test]
    fn upload_acknowledgements_are_bounded_and_reject_trailing_data() {
        for ack in [
            BlueprintUploadAck::Ready { hash: [7; 32] },
            BlueprintUploadAck::Rejected {
                reason: "界".repeat(85),
            },
        ] {
            let mut bytes = encode_blueprint_upload_ack(&ack).unwrap();
            assert_eq!(decode_blueprint_upload_ack(&bytes).unwrap(), ack);
            bytes.push(0);
            assert!(decode_blueprint_upload_ack(&bytes).is_err());
        }
        for reason in [String::new(), " ".into(), "界".repeat(86)] {
            let ack = BlueprintUploadAck::Rejected { reason };
            assert!(encode_blueprint_upload_ack(&ack).is_err());
            assert!(decode_blueprint_upload_ack(&postcard::to_allocvec(&ack).unwrap()).is_err());
        }
        assert!(decode_blueprint_upload_ack(&vec![0; MAX_BLUEPRINT_UPLOAD_ACK_BYTES + 1]).is_err());
    }

    fn snapshot() -> IndustrySnapshot {
        IndustrySnapshot {
            subscription_revision: 4,
            facilities: vec![FacilityView {
                entity: Id([1; 16]),
                owner: Principal::Organization(Id([2; 16])),
                name: "Anchorage fabrication".into(),
                can_manage: true,
                can_transfer: true,
                cargo_capacity_m3: 1000.,
                cargo_used_m3: 1.,
                items: vec![CargoStack {
                    item: CargoItem::Part("fuselage".into()),
                    quantity: 7,
                    reserved: 3,
                    name: "Fuselage kit".into(),
                    unit_mass_kg: 1200.,
                    unit_volume_m3: 1.,
                }],
                products: Vec::new(),
                jobs: Vec::new(),
                capabilities: Vec::new(),
                location: Some(Id([3; 16])),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn cargo_reservations_and_duplicate_views_are_validated() {
        let original = snapshot();
        validate_snapshot(&original).unwrap();
        let encoded = postcard::to_allocvec(&original).unwrap();
        assert_eq!(
            postcard::from_bytes::<IndustrySnapshot>(&encoded).unwrap(),
            original
        );

        let mut invalid = original.clone();
        invalid.facilities[0].items[0].reserved = 8;
        assert!(validate_snapshot(&invalid).is_err());
        invalid = original.clone();
        invalid.facilities[0].items[0].unit_volume_m3 = f64::NAN;
        assert!(validate_snapshot(&invalid).is_err());
        invalid = original.clone();
        invalid.facilities.push(invalid.facilities[0].clone());
        assert!(validate_snapshot(&invalid).is_err());
        invalid = original;
        invalid
            .omitted_inventories
            .push(invalid.facilities[0].entity);
        assert!(validate_snapshot(&invalid).is_err());
    }

    #[test]
    fn hangar_pages_validate_local_membership_permissions_and_cursor() {
        let mut original = snapshot();
        let summary = FacilitySummary {
            entity: Id([2; 16]),
            owner: Principal::Player(Id([8; 16])),
            name: "Docked hull".into(),
            location: Some(Id([3; 16])),
            capabilities: Vec::new(),
            can_manage: false,
            can_transfer: true,
        };
        original.hangar = Some(HangarView {
            ship: summary.entity,
            host: Id([3; 16]),
            host_name: "Local station".into(),
            host_inventory: None,
            ships: vec![HangarEntry {
                inventory: summary,
                can_focus: false,
                can_open_inventory: true,
                can_control: false,
            }],
            next: Some(Id([2; 16])),
        });
        validate_snapshot(&original).unwrap();
        let bytes = postcard::to_allocvec(&original).unwrap();
        assert_eq!(
            postcard::from_bytes::<IndustrySnapshot>(&bytes).unwrap(),
            original
        );

        for case in 0..7 {
            let mut invalid = original.clone();
            let hangar = invalid.hangar.as_mut().unwrap();
            match case {
                0 => hangar.next = Some(Id([9; 16])),
                1 => hangar.ships[0].inventory.location = Some(Id([4; 16])),
                2 => hangar.ships[0].can_control = true,
                3 => hangar.ships[0].can_open_inventory = false,
                4 => hangar.host_inventory = Some(hangar.ships[0].inventory.clone()),
                5 => hangar.ships.push(hangar.ships[0].clone()),
                6 => hangar.host_name = "Private\nstation".into(),
                _ => unreachable!(),
            }
            assert!(validate_snapshot(&invalid).is_err(), "case {case}");
        }
    }

    #[test]
    fn individually_valid_large_inventories_cannot_exceed_the_wire_byte_budget() {
        let mut snapshot = snapshot();
        snapshot.facilities[0].items = (0..MAX_CARGO_STACKS)
            .map(|index| CargoStack {
                item: CargoItem::Part(format!("{index:0128}")),
                quantity: u64::MAX,
                reserved: 0,
                name: "N".repeat(128),
                unit_mass_kg: 1.,
                unit_volume_m3: 1.,
            })
            .collect();
        validate_snapshot(&snapshot).unwrap();
        let mut second = snapshot.facilities[0].clone();
        second.entity = Id([4; 16]);
        snapshot.facilities.push(second);
        assert!(validate_snapshot(&snapshot).is_err());
    }

    #[test]
    fn reservoir_products_are_resources_without_cargo_reservations() {
        let mut snapshot = snapshot();
        snapshot.facilities[0].products.push(CargoStack {
            item: CargoItem::Resource("spent_fuel".into()),
            quantity: 10,
            reserved: 0,
            name: "Spent reactor fuel".into(),
            unit_mass_kg: 1.,
            unit_volume_m3: 0.001,
        });
        validate_snapshot(&snapshot).unwrap();
        let bytes = postcard::to_allocvec(&snapshot).unwrap();
        assert_eq!(
            postcard::from_bytes::<IndustrySnapshot>(&bytes).unwrap(),
            snapshot
        );

        let original = snapshot.clone();
        snapshot.facilities[0].products[0].reserved = 1;
        assert!(validate_snapshot(&snapshot).is_err());
        snapshot = original.clone();
        snapshot.facilities[0].products[0].item = CargoItem::Part("engine".into());
        assert!(validate_snapshot(&snapshot).is_err());
        snapshot = original;
        let duplicate = snapshot.facilities[0].products[0].clone();
        snapshot.facilities[0].products.push(duplicate);
        assert!(validate_snapshot(&snapshot).is_err());

        for (resource, quantity) in [("", 1), ("spent_fuel", 0)] {
            assert!(
                validate_command(&IndustryCommand::UnloadProduct {
                    source: Id([1; 16]),
                    target: Id([1; 16]),
                    resource: resource.into(),
                    quantity,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn transfer_and_subscription_inputs_reject_invalid_quantities_and_duplicate_interest() {
        let mut command = IndustryCommand::Transfer {
            source: Id([1; 16]),
            target: Id([2; 16]),
            item: CargoItem::Resource("water".into()),
            quantity: 1,
        };
        validate_command(&command).unwrap();
        let IndustryCommand::Transfer { quantity, .. } = &mut command else {
            unreachable!()
        };
        *quantity = 0;
        assert!(validate_command(&command).is_err());
        assert!(
            validate_subscription(&IndustrySubscription {
                inventories: vec![Id([1; 16]); 2],
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_subscription(&IndustrySubscription {
                directory_after: Some(Id([1; 16])),
                ..Default::default()
            })
            .is_err()
        );
    }
}
