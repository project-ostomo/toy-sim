use super::*;
use osg_model::society::SocietyPresentation;
mod politics;
mod presentation;
mod tree;
use osg_model::ownership::{
    AccessGrant, AccessPolicy, AssetAffiliation, Permission, Principal, SocietyCommand, Standing,
};
use std::collections::BTreeSet;

use osg_universe::organizations::{
    self, LoreStanding, OrganizationProfile, OrganizationRole, RelationKind,
};

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Directory,
    Assets,
    ComputerGas,
    Politics,
}

#[derive(Resource, Default)]
pub struct State {
    tab: Tab,
    selected: Option<Principal>,
    selected_bloc: Option<Id>,
    members: bool,
    search: String,
    tree: tree::Tree,
    assets_after: Option<Id>,
    assets_next: Option<Id>,
    loaded_asset: Option<Id>,
    error: Option<String>,
    organization_name: String,
    asset: Option<Id>,
    draft: Option<AccessPolicy>,
    draft_owner: Option<Principal>,
    draft_base: Option<AccessPolicy>,
    grant: Option<Principal>,
    transfer: Option<Principal>,
    politics: politics::State,
}

impl State {
    pub fn new(account: AccountId) -> Self {
        Self {
            selected: Some(Principal::Player(account)),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub fn gallery_loaded(&mut self, snapshot: &SocietyData) {
        use crate::state::requests::directory::{Branch, Status};
        let branches = [Branch::Blocs, Branch::Polities, Branch::Players(None)]
            .into_iter()
            .chain(
                snapshot
                    .directory
                    .sovereignties
                    .keys()
                    .copied()
                    .map(Branch::Organizations),
            )
            .chain(
                snapshot
                    .directory
                    .organizations
                    .keys()
                    .copied()
                    .map(|id| Branch::Players(Some(id))),
            );
        self.tree.status = branches
            .map(|branch| {
                (
                    branch,
                    Status {
                        loaded: true,
                        error: None,
                    },
                )
            })
            .collect();
    }

    #[cfg(test)]
    pub fn gallery_variant(&mut self, variant: &str, selected: Principal) {
        match variant {
            "directory" => self.tab = Tab::Directory,
            "profile" | "polity" => {
                self.tab = Tab::Directory;
                self.selected = Some(selected);
            }
            "organizations" => {
                self.tab = Tab::Directory;
                self.selected = Some(selected);
                self.members = true;
            }
            "agreements" => {
                self.tab = Tab::Politics;
                self.politics.gallery_tab("agreements");
            }
            "blocs" | "bloc-public" | "bloc-inbox" => {
                self.tab = Tab::Politics;
                self.politics.gallery_tab("blocs");
            }
            "sources" => {
                self.tab = Tab::Politics;
                self.politics.gallery_tab("sources");
            }
            "history" => {
                self.tab = Tab::Politics;
                self.politics.gallery_tab("history");
            }
            _ => {
                self.tab = Tab::Politics;
                self.politics.gallery_tab("declarations");
            }
        }
        if self.tab == Tab::Politics {
            self.selected = self.politics.owner;
        }
    }

    pub fn request(
        &mut self,
        open: bool,
        session: &crate::state::SocietyUiState,
    ) -> crate::state::requests::SocietyQuery {
        self.tree.sync(session);
        self.assets_next = session.society_assets_next;
        self.loaded_asset = session.society_asset_loaded;
        self.error = session.society_error.clone();
        crate::state::requests::SocietyQuery {
            declaration_history: (open && self.tab == Tab::Politics)
                .then_some(self.politics.history)
                .flatten(),
            history_before: self.politics.history_before,
            directory: open,
            search: self.search.clone(),
            branches: {
                let mut branches = self.tree.requested.clone();
                match self.selected {
                    Some(Principal::Organization(id)) if self.members => {
                        branches
                            .insert(crate::state::requests::directory::Branch::Players(Some(id)));
                    }
                    Some(Principal::Sovereignty(id)) => {
                        branches
                            .insert(crate::state::requests::directory::Branch::Organizations(id));
                    }
                    _ => {}
                }
                branches
            },
            selected: if self.tab == Tab::Politics {
                self.politics.owner.or(self.selected)
            } else {
                self.selected
            },
            asset: (open && self.tab == Tab::Assets)
                .then_some(self.asset)
                .flatten(),
            assets_after: self.assets_after,
            assets: open && self.tab == Tab::Assets,
            profiles: open && self.tab == Tab::Assets,
            gas: open && self.tab == Tab::ComputerGas,
            advertised: Default::default(),
        }
    }

    pub fn inspect(&mut self, principal: Principal) {
        self.tree.reveal_selected = true;
        self.tab = Tab::Directory;
        self.selected = Some(principal);
        self.selected_bloc = None;
        self.members = false;
    }

    fn refresh_asset(&mut self, asset: &AssetAffiliation, selection_changed: bool) {
        let owner_changed = self.draft_owner != Some(asset.owner);
        let clean = self.draft == self.draft_base;
        if selection_changed || owner_changed || self.draft.is_none() || clean {
            self.draft = Some(asset.access.clone());
            self.draft_base = Some(asset.access.clone());
            self.draft_owner = Some(asset.owner);
        } else if self.draft.as_ref() == Some(&asset.access) {
            self.draft_base = Some(asset.access.clone());
        }
        if selection_changed || owner_changed {
            self.transfer = None;
        }
    }
}

pub fn draw(ui: &mut egui::Ui, state: &mut State, model: &FrameModel, intents: &mut Vec<Intent>) {
    if let Some(error) = &state.error {
        ui.colored_label(THREAT, error);
    }
    ui.add_enabled_ui(model.connected, |ui| {
        presentation::draw(ui, state, model, intents)
    });
}

fn role_name(role: OrganizationRole) -> &'static str {
    match role {
        OrganizationRole::Trade => "Trade",
        OrganizationRole::Industry => "Industry",
        OrganizationRole::Defense => "Defense",
        OrganizationRole::Research => "Research",
        OrganizationRole::Relief => "Relief",
        OrganizationRole::Salvage => "Salvage",
        OrganizationRole::Mining => "Mining",
        OrganizationRole::Patrol => "Patrol",
        OrganizationRole::Broadcast => "Broadcast",
    }
}

fn profile_record(ui: &mut egui::Ui, state: &mut State, profile: &OrganizationProfile) {
    ui.label(format!("Home system: {}", profile.home_system));
    ui.weak(format!(
        "Founded {} · Public record {}",
        profile.founded_year,
        organizations::REFERENCE_YEAR
    ));
    ui.label(
        profile
            .roles
            .iter()
            .map(|role| role_name(*role))
            .collect::<Vec<_>>()
            .join(" · "),
    );

    for (heading, text) in [
        ("History", &profile.history),
        ("Culture", &profile.culture),
        ("Doctrine", &profile.doctrine),
    ] {
        ui.separator();
        ui.strong(heading);
        ui.add(egui::Label::new(text).wrap());
    }

    for (heading, items) in [
        ("Public objectives", &profile.goals),
        ("Resources & capabilities", &profile.resources),
    ] {
        ui.separator();
        ui.strong(heading);
        for item in items {
            ui.add(egui::Label::new(format!("• {item}")).wrap());
        }
    }
    ui.separator();
    ui.strong("Public relationships");
    profile_relationships(ui, state, profile);
}

fn profile_relationships(ui: &mut egui::Ui, state: &mut State, profile: &OrganizationProfile) {
    for relation in &profile.relations {
        let standing = match relation.standing {
            LoreStanding::Friendly => Standing::Friendly,
            LoreStanding::Neutral => Standing::Neutral,
            LoreStanding::Hostile => Standing::Hostile,
        };
        let kind = match relation.kind {
            RelationKind::Alliance => "Alliance",
            RelationKind::Trade => "Trade",
            RelationKind::Competition => "Competition",
            RelationKind::Dispute => "Dispute",
            RelationKind::ArmedConflict => "Armed conflict",
        };
        ui.horizontal_wrapped(|ui| {
            if ui.link(&relation.organization).clicked() {
                let id = Id(organizations::organization_id(&relation.organization));
                state.inspect(Principal::Organization(id));
            }
            ui.colored_label(super::super::standing::color(Some(standing)), kind);
        });
        ui.add(egui::Label::new(&relation.reason).wrap());
        ui.add_space(4.0);
    }
}

fn gas_accounts(ui: &mut egui::Ui, snapshot: &SocietyData) {
    ui.weak("Computers with the same owner share one global gas account.");
    ui.small("Available funds new work. Reserved is committed to pending work. Spent is total billed usage.");
    ui.add_space(8.);
    if snapshot.gas_accounts.is_empty() {
        ui.weak("No gas account balances available.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("computer_gas_accounts")
        .show(ui, |ui| {
            egui::Grid::new("gas_account_balances")
                .num_columns(3)
                .spacing(egui::vec2(20., 12.))
                .striped(true)
                .show(ui, |ui| {
                    for heading in ["Owner", "Available", "Lifetime spent"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    for account in &snapshot.gas_accounts {
                        ui.label(name(&snapshot.directory, account.owner))
                            .on_hover_text(lineage(&snapshot.directory, account.owner));
                        ui.label(
                            egui::RichText::new(gas_amount(account.available))
                                .monospace()
                                .color(if account.available == 0 {
                                    THREAT
                                } else {
                                    ACCENT
                                }),
                        );
                        ui.monospace(gas_amount(account.spent));
                        ui.end_row();
                    }
                });
        });
}

fn gas_amount(amount: u64) -> String {
    let digits = amount.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

pub fn name(directory: &SocietyPresentation, principal: Principal) -> String {
    match principal {
        Principal::Sovereignty(id) => directory
            .sovereignties
            .get(&id)
            .map(|item| item.name.clone()),
        Principal::Organization(id) => directory
            .organizations
            .get(&id)
            .map(|item| item.name.clone()),
        Principal::Player(id) => directory.players.get(&id).map(|item| item.name.clone()),
    }
    .unwrap_or_else(|| {
        let id = match principal {
            Principal::Sovereignty(id) | Principal::Organization(id) | Principal::Player(id) => id,
        };
        format!("Unknown {}", short_id(id))
    })
}

fn lineage(directory: &SocietyPresentation, principal: Principal) -> String {
    directory
        .lineage(principal)
        .into_iter()
        .rev()
        .map(|item| name(directory, item))
        .collect::<Vec<_>>()
        .join(" › ")
}

pub fn principals(directory: &SocietyPresentation) -> impl Iterator<Item = Principal> + '_ {
    directory
        .players
        .keys()
        .copied()
        .map(Principal::Player)
        .chain(
            directory
                .organizations
                .keys()
                .copied()
                .map(Principal::Organization),
        )
        .chain(
            directory
                .sovereignties
                .keys()
                .copied()
                .map(Principal::Sovereignty),
        )
}

pub fn standing_card(
    ui: &mut egui::Ui,
    directory: &SocietyPresentation,
    report: &ownership::StandingReport,
) {
    use ownership::StandingSource;

    let standing = report.standing;
    let target = report.target;
    let source = report.source.clone();
    ui.colored_label(
        super::super::standing::color(Some(standing)),
        format!(
            "{} · {}",
            name(directory, target),
            super::super::standing::label(Some(standing))
        ),
    );
    match source {
        StandingSource::Declaration {
            source,
            target,
            revision,
        } => {
            ui.label(format!(
                "Declaration by {} · revision {revision}",
                name(directory, source)
            ));
            if let Some(declaration) = directory.diplomacy.declarations.get(&(
                source,
                osg_model::diplomacy::DeclarationCategory::Standing,
                target,
            )) {
                ui.weak(&declaration.note);
            }
        }
        StandingSource::Override { source, target } => {
            ui.weak(format!(
                "Standing set by {} toward {}",
                name(directory, source),
                name(directory, target)
            ));
        }
        StandingSource::MutualDefence { agreement, ally } => {
            let title = directory
                .diplomacy
                .agreements
                .get(&agreement)
                .map_or("Mutual defence", |agreement| agreement.title.as_str());
            ui.weak(format!(
                "{title} · inherited from {}",
                name(directory, ally)
            ));
        }
        StandingSource::SharedAffiliation => {
            ui.weak("Shared affiliation");
        }
        StandingSource::Default => {
            ui.weak("Neutral default · no applicable standing rule");
        }
    }
    ui.weak("Contact identity follows its IFF broadcast.");
}

pub fn contact_card(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    report: &ownership::StandingReport,
    intents: &mut Vec<Intent>,
) {
    egui::ScrollArea::vertical()
        .id_salt("contact_affiliation_body")
        .max_height(ui.available_height())
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.;
            contact_card_contents(ui, snapshot, report, intents);
        });
}

fn contact_card_contents(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    report: &ownership::StandingReport,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    let observer = Principal::Player(snapshot.account);
    presentation::section(ui, "IFF IDENTITY");
    for principal in directory.lineage(report.target) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                match principal {
                    Principal::Player(_) => Icon::User,
                    Principal::Organization(_) => Icon::Shield,
                    Principal::Sovereignty(_) => Icon::Flag,
                }
                .text(14.)
                .color(MUTED),
            );
            ui.label(name(directory, principal));
        });
    }
    presentation::section(ui, "WAR / PEACE");
    let posture = directory.political_posture(observer, report.target);
    let war = posture == Some(Standing::Hostile);
    let color = if war { THREAT } else { MUTED };
    egui::Frame::new()
        .fill(if war {
            egui::Color32::from_rgb(59, 22, 27)
        } else {
            SURFACE_RAISED
        })
        .stroke(egui::Stroke::new(1., color))
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                osg_ui::components::badge(ui, if war { "WAR" } else { "PEACE" }, color);
                let target_polity = directory
                    .lineage(report.target)
                    .into_iter()
                    .find(|p| matches!(p, Principal::Sovereignty(_)))
                    .unwrap_or(report.target);
                ui.label(name(directory, target_polity));
            });
            let polity = directory.lineage(observer).into_iter().find_map(|p| {
                if let Principal::Sovereignty(id) = p {
                    Some(id)
                } else {
                    None
                }
            });
            let bloc = polity.and_then(|id| {
                directory
                    .diplomacy
                    .blocs
                    .values()
                    .find(|bloc| bloc.members.contains(&id))
            });
            if let Some(bloc) = bloc {
                ui.weak(format!("Carried from {} posture", bloc.name));
            }
        });
    presentation::section(ui, "STANDING & LISTS");
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            super::super::standing::color(Some(report.standing)),
            super::super::standing::label(Some(report.standing)),
        );
        match report.source {
            ownership::StandingSource::Declaration {
                source, revision, ..
            } => {
                ui.weak(format!("{} · v{revision}", name(directory, source)));
            }
            ownership::StandingSource::Override { source, .. } => {
                ui.weak(format!("{} override", name(directory, source)));
            }
            ownership::StandingSource::MutualDefence { ally, .. } => {
                ui.weak(format!("Defence agreement · {}", name(directory, ally)));
            }
            ownership::StandingSource::SharedAffiliation => {
                ui.weak("Shared affiliation");
            }
            ownership::StandingSource::Default => {
                ui.weak("Default standing");
            }
        }
    });
    for category in [
        osg_model::diplomacy::DeclarationCategory::Wanted,
        osg_model::diplomacy::DeclarationCategory::Embargo,
        osg_model::diplomacy::DeclarationCategory::Licence,
    ] {
        if let Some(declaration) = directory
            .diplomacy
            .resolve(observer, category, report.target)
            .filter(|d| d.enabled)
        {
            ui.horizontal_wrapped(|ui| {
                osg_ui::components::badge(
                    ui,
                    &format!("{category:?}").to_uppercase(),
                    if category == osg_model::diplomacy::DeclarationCategory::Licence {
                        POSITIVE
                    } else {
                        WARNING
                    },
                );
                ui.weak(name(directory, declaration.source));
            });
        }
    }
    ui.add_space(6.);
    presentation::personal_standing(ui, snapshot, report.target, intents);
    if ui.small_button("Open in Directory").clicked() {
        intents.push(Intent::InspectAffiliation(report.target));
    }
}

fn membership(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    principal: Principal,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    let current_org = directory
        .players
        .get(&snapshot.account)
        .and_then(|player| player.organization);
    match principal {
        Principal::Organization(id) => {
            let Some(organization) = directory.organizations.get(&id) else {
                return;
            };
            let members = directory
                .players
                .values()
                .filter(|player| player.organization == Some(id))
                .count();
            ui.label(format!("{members} registered members"));
            if current_org == Some(id) {
                ui.colored_label(ACCENT, "Your organization");
                if ui.button("Leave organization").clicked() {
                    intents.push(Intent::Society(
                        SocietyCommand::SetMembership {
                            account: snapshot.account,
                            organization: None,
                        },
                        "Leave organization",
                    ));
                }
            } else if organization.open_membership {
                ui.weak("Open membership");
                if ui.button("Join organization").clicked() {
                    intents.push(Intent::Society(
                        SocietyCommand::SetMembership {
                            account: snapshot.account,
                            organization: Some(id),
                        },
                        "Join organization",
                    ));
                }
            } else {
                ui.weak("Membership is managed by organization officers.");
            }
            if directory.administers(snapshot.account, principal) {
                ui.colored_label(ACCENT, "You are an organization officer");
            }
        }
        Principal::Player(account) => {
            let subject_org = directory
                .players
                .get(&account)
                .and_then(|player| player.organization);
            if account == snapshot.account {
                if subject_org.is_some() && ui.button("Leave organization").clicked() {
                    intents.push(Intent::Society(
                        SocietyCommand::SetMembership {
                            account,
                            organization: None,
                        },
                        "Leave organization",
                    ));
                }
            } else {
                for organization in directory
                    .organizations
                    .values()
                    .filter(|org| org.officers.contains(&snapshot.account))
                {
                    if subject_org == Some(organization.id) {
                        let officer = organization.officers.contains(&account);
                        if ui
                            .button(if officer {
                                "Revoke officer role"
                            } else {
                                "Appoint organization officer"
                            })
                            .clicked()
                        {
                            intents.push(Intent::Society(
                                SocietyCommand::SetOfficer {
                                    organization: organization.id,
                                    account,
                                    officer: !officer,
                                },
                                "Update officer role",
                            ));
                        }
                        if ui
                            .button(format!("Remove from {}", organization.name))
                            .clicked()
                        {
                            intents.push(Intent::Society(
                                SocietyCommand::SetMembership {
                                    account,
                                    organization: None,
                                },
                                "Remove organization member",
                            ));
                        }
                    }
                }
            }
        }
        Principal::Sovereignty(_) => {}
    }
}

fn assets_panel(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietyData,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.assets_after.is_some(),
                egui::Button::new("First page"),
            )
            .clicked()
        {
            state.assets_after = None;
        }
        if ui
            .add_enabled(state.assets_next.is_some(), egui::Button::new("Next page"))
            .clicked()
        {
            state.assets_after = state.assets_next;
        }
    });
    if snapshot.assets.is_empty() {
        ui.weak("No accessible assets.");
        return;
    }
    let previous = state.asset;
    if !snapshot
        .assets
        .iter()
        .any(|asset| Some(asset.entity) == state.asset)
    {
        state.asset = snapshot.assets.first().map(|asset| asset.entity);
        state.draft = None;
    }
    egui::ComboBox::from_id_salt("managed_asset")
        .width(360.)
        .selected_text(
            snapshot
                .assets
                .iter()
                .find(|asset| Some(asset.entity) == state.asset)
                .map_or("Select asset", |asset| asset.name.as_str()),
        )
        .show_ui(ui, |ui| {
            for asset in &snapshot.assets {
                ui.selectable_value(&mut state.asset, Some(asset.entity), &asset.name);
            }
        });
    if state.loaded_asset != state.asset {
        ui.weak("Loading asset permissions…");
        return;
    }
    let Some(asset) = snapshot
        .assets
        .iter()
        .find(|asset| Some(asset.entity) == state.asset)
    else {
        return;
    };
    let mut editing_asset = asset.clone();
    if let Some(binding) = directory.access_bindings.get(&asset.entity) {
        editing_asset.access = binding.overrides.clone();
        ui.colored_label(
            ACCENT,
            "Linked profile · editing additional grants for this asset",
        );
    }
    let asset = &editing_asset;
    state.refresh_asset(asset, previous != state.asset);
    ui.label(format!("Owner: {}", lineage(directory, asset.owner)));
    ui.weak(
        "Owners retain full access. Grants apply through organization and sovereignty membership.",
    );
    if !asset.can_manage {
        ui.colored_label(
            MUTED,
            "You can inspect this asset; changing its permissions requires Manage access.",
        );
    }
    let draft = state.draft.as_mut().unwrap();
    egui::ScrollArea::vertical()
        .id_salt("asset_permissions")
        .max_height(300.)
        .show(ui, |ui| {
            ui.add_enabled_ui(asset.can_manage, |ui| {
                policy_editor(ui, directory, draft, &mut state.grant);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            *draft != asset.access,
                            egui::Button::new("Apply permissions"),
                        )
                        .clicked()
                    {
                        intents.push(Intent::Society(
                            SocietyCommand::SetAssetAccess {
                                asset: asset.entity,
                                policy: draft.clone(),
                            },
                            "Apply asset permissions",
                        ));
                    }
                    if ui.button("Reload from server").clicked() {
                        *draft = asset.access.clone();
                        state.draft_base = Some(asset.access.clone());
                    }
                });
                ui.separator();
                transfer_editor(ui, snapshot, asset, &mut state.transfer, intents);
            });
        });
}

fn transfer_editor(
    ui: &mut egui::Ui,
    snapshot: &SocietyData,
    asset: &AssetAffiliation,
    selected: &mut Option<Principal>,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    let candidates = transfer_recipients(snapshot, asset);
    if selected.is_some_and(|owner| !candidates.contains(&owner)) {
        *selected = None;
    }

    ui.strong("Transfer ownership");
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!candidates.is_empty(), |ui| {
            principal_picker(
                ui,
                "new_asset_owner",
                directory,
                selected,
                candidates.iter().copied(),
            );
        });
        let destination = *selected;
        if ui
            .add_enabled(destination.is_some(), egui::Button::new("Transfer asset"))
            .clicked()
        {
            intents.push(Intent::Society(
                SocietyCommand::TransferAsset {
                    asset: asset.entity,
                    owner: destination.unwrap(),
                },
                "Transfer ownership",
            ));
            *selected = None;
        }
    });
    ui.weak("You must administer both owners. The ship keeps its current advertised IFF after transfer.");
}

fn transfer_recipients(snapshot: &SocietyData, asset: &AssetAffiliation) -> Vec<Principal> {
    let directory = &snapshot.directory;
    if !directory.administers(snapshot.account, asset.owner) {
        return Vec::new();
    }
    principals(directory)
        .filter(|principal| {
            *principal != asset.owner && directory.administers(snapshot.account, *principal)
        })
        .collect()
}

pub fn policy_editor(
    ui: &mut egui::Ui,
    directory: &SocietyPresentation,
    draft: &mut AccessPolicy,
    selected: &mut Option<Principal>,
) {
    ui.strong("Public access");
    permission_checks(ui, &mut draft.public);
    ui.separator();

    let mut remove = None;
    for (index, grant) in draft.grants.iter_mut().enumerate() {
        ui.push_id(("grant", index), |ui| {
            ui.horizontal(|ui| {
                ui.strong(name(directory, grant.principal));
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
            permission_checks(ui, &mut grant.permissions);
        });
    }
    if let Some(index) = remove {
        draft.grants.remove(index);
    }

    ui.separator();
    ui.horizontal(|ui| {
        principal_picker(
            ui,
            "new_access_grant",
            directory,
            selected,
            principals(directory),
        );
        let candidate = selected.filter(|principal| {
            !draft
                .grants
                .iter()
                .any(|grant| grant.principal == *principal)
        });
        if ui
            .add_enabled(candidate.is_some(), egui::Button::new("Add grant"))
            .clicked()
        {
            draft.grants.push(AccessGrant {
                principal: candidate.unwrap(),
                permissions: BTreeSet::from([Permission::View]),
            });
        }
    });
}

fn permission_checks(ui: &mut egui::Ui, permissions: &mut BTreeSet<Permission>) {
    ui.horizontal_wrapped(|ui| {
        for (permission, label) in [
            (Permission::View, "View"),
            (Permission::Control, "Pilot"),
            (Permission::Configure, "Configure"),
            (Permission::TransferCargo, "Cargo"),
            (Permission::Industry, "Industry"),
            (Permission::Dock, "Dock"),
            (Permission::ManageAccess, "Manage access"),
        ] {
            let mut enabled = permissions.contains(&permission);
            if ui.checkbox(&mut enabled, label).changed() {
                if enabled {
                    permissions.insert(permission);
                } else {
                    permissions.remove(&permission);
                }
            }
        }
    });
}

fn principal_picker(
    ui: &mut egui::Ui,
    id: &'static str,
    directory: &SocietyPresentation,
    selected: &mut Option<Principal>,
    candidates: impl Iterator<Item = Principal>,
) -> egui::Response {
    egui::ComboBox::from_id_salt(id)
        .width(250.)
        .selected_text(selected.map_or_else(
            || "Choose recipient…".into(),
            |principal| name(directory, principal),
        ))
        .show_ui(ui, |ui| {
            for principal in candidates {
                ui.selectable_value(selected, Some(principal), name(directory, principal));
            }
        })
        .response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoritative_owner_change_clears_old_acl_but_updates_preserve_unsaved_edits() {
        let mut asset = AssetAffiliation {
            entity: Id([3; 16]),
            name: "Patrol ship".into(),
            owner: Principal::Player(Id([1; 16])),
            access: AccessPolicy::default(),
            can_manage: true,
        };
        let mut state = State::default();
        state.refresh_asset(&asset, true);
        state
            .draft
            .as_mut()
            .unwrap()
            .public
            .insert(Permission::Dock);
        state.refresh_asset(&asset, false);
        assert!(
            state
                .draft
                .as_ref()
                .unwrap()
                .public
                .contains(&Permission::Dock)
        );

        asset.owner = Principal::Organization(Id([101; 16]));
        state.refresh_asset(&asset, false);
        assert_eq!(state.draft.as_ref(), Some(&asset.access));
        assert_eq!(state.draft_base.as_ref(), Some(&asset.access));

        state
            .draft
            .as_mut()
            .unwrap()
            .public
            .insert(Permission::View);
        asset.access.public.insert(Permission::View);
        state.refresh_asset(&asset, false);
        assert_eq!(state.draft, state.draft_base);
        asset.access.public.insert(Permission::Dock);
        state.refresh_asset(&asset, false);
        assert_eq!(state.draft.as_ref(), Some(&asset.access));
    }
}
