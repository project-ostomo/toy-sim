use super::*;
use std::collections::BTreeSet;
use toy_sim_model::ownership::{
    AccessGrant, AccessPolicy, AssetAffiliation, Bloc, OwnershipDirectory, Permission, Principal,
    SocietyCommand, SocietySnapshot, Standing,
};

#[derive(Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Directory,
    Assets,
    ComputerGas,
}

#[derive(Default)]
pub(super) struct State {
    tab: Tab,
    selected: Option<Principal>,
    search: String,
    organization_name: String,
    asset: Option<Id>,
    draft: Option<AccessPolicy>,
    draft_owner: Option<Principal>,
    draft_base: Option<AccessPolicy>,
    grant: Option<Principal>,
    transfer: Option<Principal>,
}

impl State {
    pub(super) fn inspect(&mut self, principal: Principal) {
        self.tab = Tab::Directory;
        self.selected = Some(principal);
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

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &FrameModel,
    intents: &mut Vec<Intent>,
) {
    let snapshot = model.society;
    let directory = &snapshot.directory;
    let me = Principal::Player(snapshot.account);
    ui.horizontal(|ui| {
        ui.label(Icon::Shield.text(22.).color(ACCENT));
        ui.vertical(|ui| {
            ui.strong(name(directory, me));
            ui.label(
                egui::RichText::new(lineage(directory, me))
                    .size(11.)
                    .color(MUTED),
            );
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.tab, Tab::Directory, "Affiliations & standings");
        ui.selectable_value(&mut state.tab, Tab::Assets, "Asset permissions");
        ui.selectable_value(&mut state.tab, Tab::ComputerGas, "Computer gas");
    });
    if state.tab == Tab::Directory {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.organization_name)
                    .hint_text("New organization name…")
                    .desired_width(280.),
            );
            let valid = model.connected
                && !state.organization_name.trim().is_empty()
                && state.organization_name.len() <= 128;
            if ui
                .add_enabled(valid, egui::Button::new("Create organization"))
                .clicked()
            {
                intents.push(Intent::Society(
                    SocietyCommand::CreateOrganization {
                        name: state.organization_name.trim().to_owned(),
                    },
                    "Create organization",
                ));
                state.organization_name.clear();
            }
        });
        ui.weak(
            "Create an organization in your current sovereignty; you become its first officer.",
        );
    }
    ui.separator();
    ui.add_enabled_ui(model.connected, |ui| match state.tab {
        Tab::Directory => directory_panel(ui, state, snapshot, intents),
        Tab::Assets => assets_panel(ui, state, snapshot, intents),
        Tab::ComputerGas => gas_accounts(ui, snapshot),
    });
}

fn gas_accounts(ui: &mut egui::Ui, snapshot: &SocietySnapshot) {
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
                .num_columns(4)
                .spacing(egui::vec2(20., 12.))
                .striped(true)
                .show(ui, |ui| {
                    for heading in ["Owner", "Available", "Reserved", "Spent"] {
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
                        ui.monospace(gas_amount(account.reserved));
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

pub(super) fn name(directory: &OwnershipDirectory, principal: Principal) -> String {
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

fn lineage(directory: &OwnershipDirectory, principal: Principal) -> String {
    directory
        .lineage(principal)
        .into_iter()
        .rev()
        .map(|item| name(directory, item))
        .collect::<Vec<_>>()
        .join(" › ")
}

fn directory_panel(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &SocietySnapshot,
    intents: &mut Vec<Intent>,
) {
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Find a sovereignty, organization, or player…")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.);

    ui.columns(2, |columns| {
        egui::ScrollArea::vertical()
            .id_salt("affiliation_tree")
            .max_height(350.)
            .show(&mut columns[0], |ui| directory_tree(ui, state, snapshot));

        if let Some(principal) = state.selected {
            principal_details(&mut columns[1], snapshot, principal, intents);
        } else {
            columns[1].weak("Select an affiliation to inspect its hierarchy, adjust your standing, or manage membership.");
        }
    });
}

fn principals(directory: &OwnershipDirectory) -> impl Iterator<Item = Principal> + '_ {
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

fn directory_tree(ui: &mut egui::Ui, state: &mut State, snapshot: &SocietySnapshot) {
    let directory = &snapshot.directory;
    let filter = state.search.to_lowercase();
    let visible: BTreeSet<_> = principals(directory)
        .filter(|principal| name(directory, *principal).to_lowercase().contains(&filter))
        .flat_map(|principal| directory.lineage(principal))
        .collect();

    for sovereignty in directory.sovereignties.values() {
        let principal = Principal::Sovereignty(sovereignty.id);
        if !visible.contains(&principal) {
            continue;
        }

        egui::CollapsingHeader::new(&sovereignty.name)
            .id_salt(principal)
            .default_open(true)
            .show(ui, |ui| {
                principal_row(
                    ui,
                    directory,
                    snapshot.account,
                    principal,
                    &mut state.selected,
                );
                for organization in directory.organizations.values() {
                    let principal = Principal::Organization(organization.id);
                    if organization.sovereignty != sovereignty.id || !visible.contains(&principal) {
                        continue;
                    }

                    principal_row(
                        ui,
                        directory,
                        snapshot.account,
                        principal,
                        &mut state.selected,
                    );
                    ui.indent(principal, |ui| {
                        for player in directory.players.values() {
                            let principal = Principal::Player(player.account);
                            if player.organization == Some(organization.id)
                                && visible.contains(&principal)
                            {
                                principal_row(
                                    ui,
                                    directory,
                                    snapshot.account,
                                    principal,
                                    &mut state.selected,
                                );
                            }
                        }
                    });
                }
            });
    }

    for player in directory.players.values() {
        let principal = Principal::Player(player.account);
        if player.organization.is_none() && visible.contains(&principal) {
            principal_row(
                ui,
                directory,
                snapshot.account,
                principal,
                &mut state.selected,
            );
        }
    }
}

fn principal_details(
    ui: &mut egui::Ui,
    snapshot: &SocietySnapshot,
    principal: Principal,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
    ui.heading(name(directory, principal));
    ui.label(
        egui::RichText::new(lineage(directory, principal))
            .size(11.)
            .color(MUTED),
    );
    if let Principal::Sovereignty(id) = principal {
        if let Some(sovereignty) = directory.sovereignties.get(&id) {
            ui.label(match sovereignty.bloc {
                Bloc::Union => "USE",
                Bloc::League => "League of Free States member",
                Bloc::NonAligned => "Non-aligned state",
            });
        }
    }

    ui.separator();
    let observer = Principal::Player(snapshot.account);
    let standing = directory.standing(observer, principal);
    ui.colored_label(
        super::super::standing::color(Some(standing)),
        format!(
            "Effective standing: {}",
            super::super::standing::label(Some(standing))
        ),
    );
    ui.label("Your personal override");
    let current = directory.standings.get(&(observer, principal)).copied();
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [
            (None, "Inherit"),
            (Some(Standing::Friendly), "Friendly"),
            (Some(Standing::Neutral), "Neutral"),
            (Some(Standing::Hostile), "Hostile"),
        ] {
            if ui.selectable_label(current == value, label).clicked() && current != value {
                intents.push(Intent::Society(
                    SocietyCommand::SetStanding {
                        target: principal,
                        standing: value,
                    },
                    "Set personal standing",
                ));
            }
        }
    });
    ui.weak("Personal standings override organization and sovereignty defaults. IFF determines the identity shown on your Overview.");
    ui.separator();
    membership(ui, snapshot, principal, intents);
}

fn principal_row(
    ui: &mut egui::Ui,
    directory: &OwnershipDirectory,
    account: AccountId,
    principal: Principal,
    selected: &mut Option<Principal>,
) {
    let standing = Some(directory.standing(Principal::Player(account), principal));
    let text = egui::RichText::new(format!(
        "{} {}",
        super::super::standing::symbol(standing),
        name(directory, principal)
    ))
    .color(super::super::standing::color(standing));
    if ui
        .selectable_label(*selected == Some(principal), text)
        .clicked()
    {
        *selected = Some(principal);
    }
}

fn membership(
    ui: &mut egui::Ui,
    snapshot: &SocietySnapshot,
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
    snapshot: &SocietySnapshot,
    intents: &mut Vec<Intent>,
) {
    let directory = &snapshot.directory;
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
    let Some(asset) = snapshot
        .assets
        .iter()
        .find(|asset| Some(asset.entity) == state.asset)
    else {
        return;
    };
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
    snapshot: &SocietySnapshot,
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

fn transfer_recipients(snapshot: &SocietySnapshot, asset: &AssetAffiliation) -> Vec<Principal> {
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

fn policy_editor(
    ui: &mut egui::Ui,
    directory: &OwnershipDirectory,
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
    directory: &OwnershipDirectory,
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
    use toy_sim_model::ownership::PlayerAffiliation;

    #[test]
    fn gas_tab_shows_only_authorized_accounts_with_exact_integer_balances() {
        use toy_sim_model::ownership::{GasAccountSnapshot, Organization, Sovereignty};

        let ctx = egui::Context::default();
        toy_sim_ui::theme::install(&ctx);
        let mut snapshot = snapshot(true);
        let sovereignty = Id([5; 16]);
        let organization = Id([6; 16]);
        snapshot.directory.sovereignties.insert(
            sovereignty,
            Sovereignty {
                id: sovereignty,
                name: "Test sovereignty".into(),
                bloc: Bloc::NonAligned,
                officers: Default::default(),
            },
        );
        snapshot.directory.organizations.insert(
            organization,
            Organization {
                id: organization,
                name: "Shared Fleet".into(),
                sovereignty,
                open_membership: false,
                officers: BTreeSet::from([snapshot.account]),
            },
        );
        snapshot
            .directory
            .players
            .get_mut(&snapshot.account)
            .unwrap()
            .organization = Some(organization);
        snapshot.gas_accounts = vec![
            GasAccountSnapshot {
                owner: Principal::Player(snapshot.account),
                available: u64::MAX,
                reserved: 0,
                spent: 0,
            },
            GasAccountSnapshot {
                owner: Principal::Organization(organization),
                available: 9_007_199_254_740_993,
                reserved: 1234,
                spent: 56789,
            },
        ];
        assert!(snapshot.valid());
        let mut state = State::default();
        assert!(click(&ctx, &mut state, &snapshot, "Computer gas").is_empty());
        let labels = render(&ctx, &mut state, &snapshot, vec![], &mut Vec::new());
        for expected in [
            "Shared Fleet",
            "Available",
            "Reserved",
            "Spent",
            "18,446,744,073,709,551,615",
            "9,007,199,254,740,993",
            "1,234",
            "56,789",
        ] {
            assert!(
                labels.iter().any(|(text, _)| text == expected),
                "missing {expected}"
            );
        }
        assert!(!labels.iter().any(|(text, _)| text == "Contact owner"));
    }

    fn snapshot(can_manage: bool) -> SocietySnapshot {
        let account = Id([1; 16]);
        let peer = Id([2; 16]);
        let mut directory = OwnershipDirectory::default();
        for (account, name) in [(account, "Pilot"), (peer, "Contact owner")] {
            directory.players.insert(
                account,
                PlayerAffiliation {
                    account,
                    name: name.into(),
                    organization: None,
                },
            );
        }
        SocietySnapshot {
            account,
            directory,
            gas_accounts: Vec::new(),
            assets: vec![AssetAffiliation {
                entity: Id([3; 16]),
                name: "Patrol ship".into(),
                owner: Principal::Player(account),
                access: AccessPolicy::default(),
                can_manage,
            }],
        }
    }

    fn render(
        ctx: &egui::Context,
        state: &mut State,
        snapshot: &SocietySnapshot,
        events: Vec<egui::Event>,
        intents: &mut Vec<Intent>,
    ) -> Vec<(String, egui::Pos2)> {
        let navigation = NavigationCatalogue::default();
        let model = FrameModel {
            industry: empty_industry(),
            navigation_status: &NavigationStatus::Ready,
            navigation_hash: None,
            celestial_systems: Default::default(),
            society: snapshot,
            navigation: &navigation,
            ships: vec![],
            rows: vec![],
            ship: None,
            details: None,
            system: "Helion".into(),
            vicinity: String::new(),
            connected: true,
            status: "",
            time_ns: 0,
            calendar_unix_ms: None,
            diagnostics: Default::default(),
            orbits: false,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 800.),
                )),
                events,
                ..Default::default()
            },
            |ui| draw(ui, state, &model, intents),
        );
        fn collect(shape: &egui::Shape, output: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(shape) => output.push((
                    shape.galley.job.text.clone(),
                    shape.pos + shape.galley.size() / 2.,
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, output)),
                _ => {}
            }
        }
        output.textures_delta.clear();
        let mut labels = Vec::new();
        for shape in output.shapes {
            collect(&shape.shape, &mut labels);
        }
        labels
    }

    fn click(
        ctx: &egui::Context,
        state: &mut State,
        snapshot: &SocietySnapshot,
        label: &str,
    ) -> Vec<Intent> {
        let mut intents = Vec::new();
        let mut labels = Vec::new();
        for _ in 0..3 {
            labels = render(ctx, state, snapshot, vec![], &mut intents);
        }
        let position = labels
            .iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing {label}: {labels:?}"))
            .1;
        for pressed in [true, false] {
            render(
                ctx,
                state,
                snapshot,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                &mut intents,
            );
        }
        intents
    }

    #[test]
    fn personal_standing_buttons_send_explicit_override_and_inherit_actions() {
        let ctx = egui::Context::default();
        toy_sim_ui::theme::install(&ctx);
        let mut state = State {
            selected: Some(Principal::Player(Id([2; 16]))),
            ..Default::default()
        };
        let mut snapshot = snapshot(true);
        let actions = click(&ctx, &mut state, &snapshot, "Hostile");
        assert!(
            matches!(actions.as_slice(), [Intent::Society(SocietyCommand::SetStanding { target: Principal::Player(id), standing: Some(Standing::Hostile) }, _)] if *id == Id([2; 16]))
        );
        snapshot.directory.standings.insert(
            (Principal::Player(snapshot.account), state.selected.unwrap()),
            Standing::Hostile,
        );
        let actions = click(&ctx, &mut state, &snapshot, "Inherit");
        assert!(matches!(
            actions.as_slice(),
            [Intent::Society(
                SocietyCommand::SetStanding { standing: None, .. },
                _
            )]
        ));
    }

    #[test]
    fn permission_edits_require_explicit_apply_and_management_authority() {
        for can_manage in [true, false] {
            let ctx = egui::Context::default();
            toy_sim_ui::theme::install(&ctx);
            let mut state = State {
                tab: Tab::Assets,
                ..Default::default()
            };
            let snapshot = snapshot(can_manage);
            assert!(click(&ctx, &mut state, &snapshot, "Cargo").is_empty());
            let actions = click(&ctx, &mut state, &snapshot, "Apply permissions");
            if can_manage {
                assert!(
                    matches!(actions.as_slice(), [Intent::Society(SocietyCommand::SetAssetAccess { asset, policy }, _)] if *asset == Id([3; 16]) && policy.public.contains(&Permission::TransferCargo))
                );
            } else {
                assert!(actions.is_empty());
                assert!(state.draft.unwrap().public.is_empty());
            }
        }
    }
    struct PickerFixture {
        ctx: egui::Context,
        desktop: Desktop,
        selected: Option<Principal>,
        button: egui::Rect,
        frame: u64,
    }

    impl PickerFixture {
        fn new() -> Self {
            let ctx = egui::Context::default();
            toy_sim_ui::theme::install(&ctx);
            Self {
                ctx,
                desktop: Desktop::default(),
                selected: None,
                button: egui::Rect::NOTHING,
                frame: 0,
            }
        }

        fn render(
            &mut self,
            snapshot: &SocietySnapshot,
            filtered: bool,
            events: Vec<egui::Event>,
        ) -> Vec<(String, egui::Pos2)> {
            let spec = WindowSpec {
                id: "recipient_test",
                title: "ASSET PERMISSIONS",
                size: egui::vec2(650., 270.),
                min_size: egui::vec2(650., 270.),
                anchor: egui::Align2::LEFT_TOP,
                offset: egui::vec2(80., 80.),
                open: true,
            };
            self.frame += 1;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100., 700.),
                    )),
                    time: Some(self.frame as f64 / 60.),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Area::new(egui::Id::new("bottom_hud_test"))
                        .order(egui::Order::Background)
                        .fixed_pos(egui::pos2(0., 370.))
                        .movable(false)
                        .show(ui.ctx(), |ui| {
                            ui.allocate_exact_size(
                                egui::vec2(1100., 330.),
                                egui::Sense::click_and_drag(),
                            );
                        });
                    self.desktop.show(ui.ctx(), spec, |ui| {
                        ui.add_space(190.);
                        let recipients = if filtered {
                            transfer_recipients(snapshot, &snapshot.assets[0])
                        } else {
                            principals(&snapshot.directory).collect()
                        };
                        self.button = principal_picker(
                            ui,
                            "recipient_test",
                            &snapshot.directory,
                            &mut self.selected,
                            recipients.into_iter(),
                        )
                        .rect;
                    });
                },
            );
            output.textures_delta.clear();
            let mut labels = Vec::new();
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let center = text.pos + text.galley.size() / 2.;
                    if shape.clip_rect.contains(center) {
                        labels.push((text.galley.job.text.clone(), center));
                    }
                }
            }
            labels
        }

        fn click(&mut self, snapshot: &SocietySnapshot, filtered: bool, position: egui::Pos2) {
            for pressed in [true, false] {
                self.render(
                    snapshot,
                    filtered,
                    vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        }
    }

    fn crowded_snapshot() -> SocietySnapshot {
        use toy_sim_model::ownership::{Organization, Sovereignty};
        let mut snapshot = snapshot(true);
        for index in 10..90 {
            let account = Id([index; 16]);
            snapshot.directory.players.insert(
                account,
                PlayerAffiliation {
                    account,
                    name: format!("Unrelated pilot {index}"),
                    organization: None,
                },
            );
        }
        let sovereignty = Id([100; 16]);
        snapshot.directory.sovereignties.insert(
            sovereignty,
            Sovereignty {
                id: sovereignty,
                name: "Test sovereignty".into(),
                bloc: Bloc::NonAligned,
                officers: BTreeSet::new(),
            },
        );
        let organization = Id([101; 16]);
        snapshot.directory.organizations.insert(
            organization,
            Organization {
                id: organization,
                name: "Playtest Cooperative".into(),
                sovereignty,
                open_membership: true,
                officers: BTreeSet::from([snapshot.account]),
            },
        );
        snapshot
    }

    #[test]
    fn desktop_recipient_popup_scrolls_above_background_hud() {
        let snapshot = crowded_snapshot();
        let mut fixture = PickerFixture::new();
        for _ in 0..4 {
            fixture.render(&snapshot, false, vec![]);
        }
        fixture.click(&snapshot, false, fixture.button.center());
        let mut labels = fixture.render(&snapshot, false, vec![]);
        let pointer = labels
            .iter()
            .find(|(text, _)| text == "Unrelated pilot 10")
            .expect("popup opens")
            .1;
        assert!(pointer.y >= fixture.button.bottom());
        for _ in 0..20 {
            labels = fixture.render(
                &snapshot,
                false,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        phase: egui::TouchPhase::Move,
                        delta: egui::vec2(0., -10.),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let destination = labels
            .iter()
            .find(|(text, _)| text == "Playtest Cooperative")
            .unwrap_or_else(|| panic!("destination not reachable after scroll: {labels:?}"))
            .1;
        fixture.click(&snapshot, false, destination);
        assert_eq!(
            fixture.selected,
            Some(Principal::Organization(Id([101; 16])))
        );
    }

    #[test]
    fn transfer_picker_only_lists_and_selects_administrable_new_owners() {
        let snapshot = crowded_snapshot();
        let organization = Principal::Organization(Id([101; 16]));
        assert_eq!(
            transfer_recipients(&snapshot, &snapshot.assets[0]),
            vec![organization]
        );
        let mut fixture = PickerFixture::new();
        for _ in 0..4 {
            fixture.render(&snapshot, true, vec![]);
        }
        fixture.click(&snapshot, true, fixture.button.center());
        let labels = fixture.render(&snapshot, true, vec![]);
        assert!(
            !labels
                .iter()
                .any(|(text, _)| text.starts_with("Unrelated pilot"))
        );
        let destination = labels
            .iter()
            .find(|(text, _)| text == "Playtest Cooperative")
            .expect("authorized organization is selectable without scrolling")
            .1;
        fixture.click(&snapshot, true, destination);
        assert_eq!(fixture.selected, Some(organization));
    }

    #[test]
    fn authoritative_owner_change_clears_old_acl_but_updates_preserve_unsaved_edits() {
        let mut asset = snapshot(true).assets.remove(0);
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
