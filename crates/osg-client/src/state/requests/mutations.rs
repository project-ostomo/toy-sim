use super::{Mutations, call};
use osg_model::{
    Id, diplomacy::DiplomacyCommand, economy::WalletCommand, industry::IndustryCommand,
    market::MarketCommand, ownership::SocietyCommand, rpc::Operation,
};
use osg_net::OsgNetClient;

macro_rules! submit {
    ($name:ident, $command:ty, $perform:ident) => {
        pub(crate) fn $name(
            requests: &mut Mutations,
            client: &OsgNetClient,
            world: Id,
            generation: u64,
            command: $command,
        ) -> Id {
            let operation = Operation {
                world,
                id: Id::new(),
            };
            let client = client.clone();
            requests.submit(world, generation, operation.id, async move {
                $perform(&client, operation, command).await
            });
            operation.id
        }
    };
}

submit!(wallet, WalletCommand, wallet_call);
submit!(market, MarketCommand, market_call);
submit!(industry, IndustryCommand, industry_call);
submit!(society, SocietyCommand, society_call);

async fn wallet_call(
    client: &OsgNetClient,
    operation: Operation,
    command: WalletCommand,
) -> Result<(), String> {
    match command {
        WalletCommand::Transfer {
            from,
            to,
            currency,
            amount,
        } => call(client.transfer_money(operation, from, to, currency, amount)).await,
        WalletCommand::TransferGas { from, to, amount } => {
            call(client.transfer_gas(operation, from, to, amount)).await
        }
        WalletCommand::SetTurnoverTax {
            sovereignty,
            basis_points,
        } => call(client.set_turnover_tax(operation, sovereignty, basis_points)).await,
    }
}

async fn market_call(
    client: &OsgNetClient,
    operation: Operation,
    command: MarketCommand,
) -> Result<(), String> {
    match command {
        MarketCommand::Limit {
            instrument,
            owner,
            side,
            quantity,
            price,
        } => {
            call(client.place_limit_order(operation, owner, instrument, side, quantity, price))
                .await
        }
        MarketCommand::Immediate {
            instrument,
            owner,
            side,
            quantity,
            price,
        } => {
            call(client.execute_market_order(operation, owner, instrument, side, quantity, price))
                .await
        }
        MarketCommand::Cancel { order } => call(client.cancel_order(operation, order)).await,
        MarketCommand::MoveStorage {
            owner,
            station,
            ship,
            item,
            quantity,
            deposit,
        } => {
            if deposit {
                call(client.deposit_storage(operation, owner, station, ship, item, quantity)).await
            } else {
                call(client.withdraw_storage(operation, owner, station, ship, item, quantity)).await
            }
        }
    }
}

pub(crate) async fn industry_call(
    client: &OsgNetClient,
    operation: Operation,
    command: IndustryCommand,
) -> Result<(), String> {
    match command {
        IndustryCommand::Transfer {
            source,
            target,
            item,
            quantity,
        } => call(client.transfer_cargo(operation, source, target, item, quantity)).await,
        IndustryCommand::UnloadProduct {
            source,
            target,
            resource,
            quantity,
        } => call(client.unload_product(operation, source, target, resource, quantity)).await,
        IndustryCommand::Refill {
            source,
            ship,
            resource,
            quantity,
        } => call(client.refill_ship(operation, source, ship, resource, quantity)).await,
        IndustryCommand::StartRecipe {
            facility,
            recipe,
            batches,
        } => call(client.start_recipe(operation, facility, recipe, batches)).await,
        IndustryCommand::BuildShip {
            facility,
            owner,
            blueprint_hash,
        } => call(client.build_ship(operation, facility, owner, blueprint_hash)).await,
        IndustryCommand::CancelJob { facility, job } => {
            call(client.cancel_industry_job(operation, facility, job)).await
        }
    }
}

async fn society_call(
    client: &OsgNetClient,
    operation: Operation,
    command: SocietyCommand,
) -> Result<(), String> {
    match command {
        SocietyCommand::UnlinkAccessProfile { asset } => {
            call(client.unlink_access_profile(operation, asset)).await
        }
        SocietyCommand::SetAccessDenied { asset, denied } => {
            call(client.set_access_denied(operation, asset, denied)).await
        }
        SocietyCommand::SaveAccessProfile(profile) => {
            call(client.save_access_profile(operation, profile)).await
        }
        SocietyCommand::DeleteAccessProfile { id } => {
            call(client.delete_access_profile(operation, id)).await
        }
        SocietyCommand::ApplyAccessProfile { asset, profile } => {
            call(client.apply_access_profile(operation, asset, profile)).await
        }
        SocietyCommand::CreateOrganization { name } => {
            call(client.create_organization(operation, name)).await
        }
        SocietyCommand::SetOfficer {
            organization,
            account,
            officer,
        } => call(client.set_organization_officer(operation, organization, account, officer)).await,
        SocietyCommand::SetStanding { target, standing } => {
            call(client.set_personal_standing(operation, target, standing)).await
        }
        SocietyCommand::SetMembership {
            account,
            organization,
        } => call(client.set_membership(operation, account, organization)).await,
        SocietyCommand::SetAssetAccess { asset, policy } => {
            call(client.set_asset_access(operation, asset, policy)).await
        }
        SocietyCommand::TransferAsset { asset, owner } => {
            call(client.transfer_asset(operation, asset, owner)).await
        }
        SocietyCommand::Diplomacy(command) => diplomacy_call(client, operation, command).await,
    }
}

async fn diplomacy_call(
    client: &OsgNetClient,
    operation: Operation,
    command: DiplomacyCommand,
) -> Result<(), String> {
    match command {
        DiplomacyCommand::Publish(declaration) => {
            call(client.publish_declaration(operation, declaration)).await
        }
        DiplomacyCommand::SetTrust {
            owner,
            category,
            sources,
        } => call(client.set_trust(operation, owner, category, sources)).await,
        DiplomacyCommand::ProposeAgreement {
            from,
            to,
            title,
            terms,
            note,
        } => call(client.propose_agreement(operation, from, to, title, terms, note)).await,
        DiplomacyCommand::ChangeAgreement {
            id,
            expected_revision,
            status,
        } => call(client.change_agreement(operation, id, expected_revision, status)).await,
        DiplomacyCommand::CreateBloc { name, founder } => {
            call(client.create_bloc(operation, name, founder)).await
        }
        DiplomacyCommand::ApplyToBloc {
            bloc,
            polity,
            apply,
        } => call(client.apply_to_bloc(operation, bloc, polity, apply)).await,
        DiplomacyCommand::DecideApplication {
            bloc,
            polity,
            admit,
        } => call(client.decide_bloc_application(operation, bloc, polity, admit)).await,
        DiplomacyCommand::RequestBlocWithdrawal {
            bloc,
            polity,
            request,
        } => call(client.request_bloc_withdrawal(operation, bloc, polity, request)).await,
        DiplomacyCommand::DecideBlocWithdrawal {
            bloc,
            polity,
            grant,
        } => call(client.decide_bloc_withdrawal(operation, bloc, polity, grant)).await,
        DiplomacyCommand::RemoveBlocMember { bloc, polity } => {
            call(client.remove_bloc_member(operation, bloc, polity)).await
        }
        DiplomacyCommand::SetBlocOfficer {
            bloc,
            account,
            officer,
        } => call(client.set_bloc_officer(operation, bloc, account, officer)).await,
        DiplomacyCommand::SetPosture {
            polity,
            target,
            standing,
        } => call(client.set_political_posture(operation, polity, target, standing)).await,
        DiplomacyCommand::SetBlocPosture {
            bloc,
            target,
            standing,
        } => call(client.set_bloc_posture(operation, bloc, target, standing)).await,
    }
}
