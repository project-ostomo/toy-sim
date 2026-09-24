use anyhow::{Result, ensure};
use osg_model::industry::*;

pub fn encode_blueprint_upload_ack(ack: &BlueprintUploadAck) -> Result<Vec<u8>> {
    Ok(postcard::to_allocvec(ack)?)
}

pub fn decode_blueprint_upload_ack(bytes: &[u8]) -> Result<BlueprintUploadAck> {
    Ok(postcard::from_bytes(bytes)?)
}

fn text_valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn item_valid(item: &CargoItem) -> bool {
    match item {
        CargoItem::Resource(id) | CargoItem::Part(id) => text_valid(id),
    }
}

pub fn validate_command(command: &IndustryCommand) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::Id;

    #[test]
    fn transfer_inputs_reject_zero_quantities() {
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
    }
}
